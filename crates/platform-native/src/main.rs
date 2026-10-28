//! `skywave`: the desktop shell. Opens a window, sets up wgpu, feeds keyboard, mouse and
//! gamepad input to the input mapper and drives the platform-independent `App` once per
//! redraw.
//!
//! Mouse look: while the right button is held the cursor is hidden and locked (or confined
//! to the window where the platform cannot lock it, e.g. X11 and Windows); it is released
//! with the button or when the window loses focus. Platforms that refuse both just keep a
//! free cursor; the raw motion still drives the look.
//!
//! Usage: `skywave [--seed N] [--car ID] [--scenario ID] [--scene KEY=VALUE]... [--stats]
//! [--frames N] [--exit] [--no-vsync] [--quality NAME] [--no-cull] [--no-prepass] [--radio-off]
//! [--music DIR] [--version]`.
//! `--scene` sets one key of the starting scene (`app::scene`: `t=22:30`, `cam=cockpit`,
//! `pos=-92,4.9`, `yaw=90`, `lights=1`, ...); `--seed N`, `--car ID` and `--scenario ID`
//! are `--scene seed=N`, `--scene car=ID` and `--scene scenario=ID` (a world,
//! `assets/scenarios/`; without one or a place the game starts in the world last picked on
//! the World page, and picking another there reloads the game in it). `--stats` prints the adapter, the surface and a
//! line of frame statistics every second to stderr (format in `docs/PERFORMANCE.md`), with
//! GPU time per pass where the adapter has timestamp queries; it also prints the detected
//! gamepads at startup and as they connect (otherwise the info log has them,
//! `RUST_LOG=info`). `--frames N` prints a summary after N frames, and with `--exit` quits
//! there (scripted benchmarks). `--no-vsync` presents without waiting for the display
//! (`AutoNoVsync`; the default is `AutoVsync`) and, for benchmarks, draws the whole world
//! every frame with no frame-rate cap. `--quality low|deck|medium|high|ultra`
//! forces a graphics preset for the session (not saved); `--no-cull` turns instance culling
//! off and `--no-prepass` the depth pre-pass (for comparisons). `--radio-off` keeps the car
//! radio silent and off the network (CI and benchmarks). `--version` prints the build's
//! `git describe` (see `build.rs`)
//! and exits.
//!
//! Frames are drawn no more often than Settings › Video › Frame rate allows (the panel's
//! rate, or every Nth refresh: the loop sleeps to deadlines N refreshes apart and
//! presents with vsync). Unfocused, the game pauses and the loop runs at most
//! [`app::pacing::BACKGROUND_FPS`] times a second; hidden (where the system says so), it
//! draws nothing. Each frame
//! draws only what changed (`render::Renderer::draw`): nothing when nothing did, the
//! tonemap and HUD over the kept world when only the HUD did.
//!
//! The town streams round the camera within `app::stream::StreamBudget::NATIVE` (13×13
//! chunks, 4 ms a sim step); `--scene stream=0` builds it all at load instead.
//!
//! Esc / Start opens the pause menu; settings and bindings are saved to
//! `$XDG_CONFIG_HOME/skywave/settings.json` (see `settings`).
//!
//! Sound plays through the default output device at 48 kHz (`sfx`); without one the game
//! runs silent. The car radio's internet stations play through it too (`radio`); the
//! window title shows the station and what it says is playing. The player's own MP3s are
//! the deck's library: the music folder (`--music DIR`, else `music` beside the executable,
//! else `~/Music`) and the folders picked on the Audio page or dropped on the window,
//! scanned at start (`music`). `--radio-off` leaves out both.

mod music;
mod pad;
mod radio;
mod settings;
mod sfx;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use app::pacing::{frame_interval, Pacer};
use app::{
    App, GpuInfo, InputMapper, Key, Quality, RawInput, SceneSetup, SettingsStore, MOUSE_RIGHT,
};
use pad::Pads;
use render::{RenderStats, Renderer, GPU_PARTS, GPU_PART_NAMES, GPU_PASSES, GPU_PASS_NAMES};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// Pixel-precise scrolling (touchpads) counts this many pixels as one wheel notch, the
/// same figure the web build uses.
const PIXELS_PER_NOTCH: f64 = 100.0;

/// Frames left out of the `--frames` summary while the town streams in and caches warm.
const SUMMARY_WARMUP_FRAMES: usize = 60;

/// A frame due within this many seconds is slept to in the redraw itself (precise); one
/// due later is left to the event loop's timer (which may wake a millisecond off).
const SLEEP_SLACK: f64 = 0.002;

/// The panel's refresh rate when the monitor does not say, Hz.
const DEFAULT_PANEL_HZ: f64 = 60.0;

/// Sleeps until `pacer` has a frame due at `interval` seconds by `now` (seconds, any
/// origin), then starts it.
fn sleep_until_due(pacer: &mut Pacer, interval: f64, now: impl Fn() -> f64) {
    let wait = pacer.wait(now(), interval);
    if wait > 0.0 {
        std::thread::sleep(Duration::from_secs_f64(wait));
    }
    // A sleep never ends early; the slack only absorbs rounding.
    pacer.due(now(), interval, 1e-4);
}

/// `--stats` prints the GPU table over this many seconds.
const GPU_TABLE_SECONDS: u64 = 5;

/// GPU time of one frame read back: each pass, then each part ([`GPU_PART_NAMES`]), µs.
type GpuSample = [u32; GPU_PASSES + GPU_PARTS];

#[derive(Debug, Default, PartialEq)]
struct Options {
    scene: SceneSetup,
    stats: bool,
    version: bool,
    /// Print a summary after this many frames.
    frames: Option<u32>,
    /// Quit after `frames`.
    exit: bool,
    no_vsync: bool,
    quality: Option<Quality>,
    no_cull: bool,
    no_prepass: bool,
    /// The radio stays silent and fetches nothing.
    radio_off: bool,
    /// The music folder (`--music`).
    music: Option<std::path::PathBuf>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut opts = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--stats" => opts.stats = true,
            "--exit" => opts.exit = true,
            "--no-vsync" => opts.no_vsync = true,
            "--no-cull" => opts.no_cull = true,
            "--no-prepass" => opts.no_prepass = true,
            "--radio-off" => opts.radio_off = true,
            "--music" => {
                let v = args.next().ok_or("--music needs a folder")?;
                opts.music = Some(v.into());
            }
            "--frames" => {
                let v = args.next().ok_or("--frames needs a count")?;
                let n = v
                    .parse::<u32>()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or_else(|| format!("--frames needs a positive count, not `{v}`"))?;
                opts.frames = Some(n);
            }
            "--quality" => {
                let v = args.next().ok_or("--quality needs a preset")?;
                let q = Quality::from_name(&v).ok_or_else(|| {
                    format!(
                        "--quality is one of low, battery, deck, medium, high, ultra, not `{v}`"
                    )
                })?;
                opts.quality = Some(q);
            }
            "-V" | "--version" => opts.version = true,
            "--seed" => {
                let v = args.next().ok_or("--seed needs a value")?;
                opts.scene.set("seed", &v)?;
            }
            "--car" => {
                let v = args.next().ok_or("--car needs a vehicle id")?;
                opts.scene.set("car", &v)?;
            }
            "--scenario" => {
                let v = args.next().ok_or("--scenario needs a world id")?;
                opts.scene.set("scenario", &v)?;
            }
            "--scene" => {
                let kv = args.next().ok_or("--scene needs key=value")?;
                let (key, value) = kv
                    .split_once('=')
                    .ok_or_else(|| format!("--scene needs key=value, not `{kv}`"))?;
                opts.scene.set(key, value)?;
            }
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if opts.exit && opts.frames.is_none() {
        return Err("--exit needs --frames N".into());
    }
    Ok(opts)
}

/// Maps a winit key to a game key. winit's `KeyCode` names are the DOM `code` names
/// [`Key::from_code`] knows; the name is formatted on the stack, so this never allocates.
fn map_key(code: KeyCode) -> Option<Key> {
    struct Name {
        buf: [u8; 24],
        len: usize,
    }
    impl std::fmt::Write for Name {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            let end = self.len + s.len();
            self.buf
                .get_mut(self.len..end)
                .ok_or(std::fmt::Error)?
                .copy_from_slice(s.as_bytes());
            self.len = end;
            Ok(())
        }
    }
    let mut name = Name {
        buf: [0; 24],
        len: 0,
    };
    write!(name, "{code:?}").ok()?;
    Key::from_code(std::str::from_utf8(&name.buf[..name.len]).ok()?)
}

/// A winit mouse button as a DOM `MouseEvent.button` number (0 left, 1 middle, 2 right,
/// 3 back, 4 forward). Platform-specific extra buttons are ignored.
fn mouse_button(b: MouseButton) -> Option<u8> {
    match b {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Back => Some(3),
        MouseButton::Forward => Some(4),
        MouseButton::Other(_) => None,
    }
}

/// Vertical scroll in wheel notches, + away from the user. winit's line deltas are notches
/// with + meaning "scroll up", which is the same direction.
fn wheel_notches(delta: MouseScrollDelta) -> f32 {
    match delta {
        MouseScrollDelta::LineDelta(_, y) => y,
        MouseScrollDelta::PixelDelta(p) => (p.y / PIXELS_PER_NOTCH) as f32,
    }
}

/// Hides and captures the cursor for mouse look, or gives it back. Locked where the
/// platform can, else confined to the window; refusals are ignored.
fn grab_cursor(window: &Window, on: bool) {
    if on {
        if let Err(e) = window
            .set_cursor_grab(CursorGrabMode::Locked)
            .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
        {
            log::debug!("cursor grab refused: {e}");
        }
    } else if let Err(e) = window.set_cursor_grab(CursorGrabMode::None) {
        log::debug!("cursor release refused: {e}");
    }
    window.set_cursor_visible(!on);
}

/// Mean, 99th percentile and maximum of `ms` (sorted in place), ms.
fn summarize(ms: &mut [f32]) -> (f32, f32, f32) {
    if ms.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    ms.sort_by(f32::total_cmp);
    let mean = ms.iter().sum::<f32>() / ms.len() as f32;
    let p99 = ms[((ms.len() as f32 * 0.99).ceil() as usize).clamp(1, ms.len()) - 1];
    (mean, p99, ms[ms.len() - 1])
}

/// What the renderer did in the last frame, as `--stats` prints it.
fn render_line(r: &RenderStats) -> String {
    let mut s = format!(
        "inst={} fwd={} sh={}/{}/{} draws={} tris_fwd={}k tris_sh={}k shadow={}x{} drawn={} \
         view={}x{} surface={}x{} bloom={}",
        r.instances,
        r.instances_drawn,
        r.shadow_instances[0],
        r.shadow_instances[1],
        r.shadow_instances[2],
        r.draw_calls,
        r.triangles / 1000,
        r.shadow_triangles / 1000,
        r.shadow_size,
        r.cascades,
        r.cascades_drawn,
        r.render_size.0,
        r.render_size.1,
        r.surface_size.0,
        r.surface_size.1,
        r.bloom_levels,
    );
    // A frame that kept the world (Ui) or drew nothing (Skip) leaves the counts above as
    // the last world frame's.
    let _ = write!(
        s,
        " | last={:?} world={} ui={} skip={}",
        r.last_work, r.frames, r.ui_frames, r.skipped_frames
    );
    if r.gpu_timing {
        let total: u32 = r.gpu_us.iter().sum();
        let _ = write!(s, " | gpu_ms total={:.2}", total as f32 / 1000.0);
        for (name, us) in GPU_PASS_NAMES.iter().zip(r.gpu_us) {
            let _ = write!(s, " {name}={:.2}", us as f32 / 1000.0);
        }
    }
    s
}

/// Minimum, median and 95th percentile of `us` (sorted in place), ms.
fn spread(us: &mut [u32]) -> (f32, f32, f32) {
    if us.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    us.sort_unstable();
    let p95 = us[((us.len() as f32 * 0.95).ceil() as usize).clamp(1, us.len()) - 1];
    let ms = |v: u32| v as f32 / 1000.0;
    (ms(us[0]), ms(us[us.len() / 2]), ms(p95))
}

/// The `--stats` GPU table: min, median and p95 of each pass (and, with `parts`, each
/// part under its pass) and of their total, over `samples`.
fn gpu_table(samples: &[GpuSample], parts: bool) -> String {
    let mut s = format!(
        "{:<14}{:>8}{:>8}{:>8}   ({} frames read)",
        "gpu ms",
        "min",
        "median",
        "p95",
        samples.len()
    );
    let mut column: Vec<u32> = Vec::with_capacity(samples.len());
    let mut row = |s: &mut String, name: &str, f: &dyn Fn(&GpuSample) -> u32| {
        column.clear();
        column.extend(samples.iter().map(f));
        let (min, med, p95) = spread(&mut column);
        let _ = write!(s, "\n  {name:<12}{min:>8.2}{med:>8.2}{p95:>8.2}");
    };
    // Which parts belong under which pass: the cascades under shadow, the rest under
    // forward.
    let under = |p: usize| match p {
        0 => 0..render::CASCADES,
        1 => render::CASCADES..GPU_PARTS,
        _ => 0..0,
    };
    for (p, name) in GPU_PASS_NAMES.iter().enumerate() {
        row(&mut s, name, &|g| g[p]);
        if parts {
            for k in under(p) {
                row(&mut s, &format!("  {}", GPU_PART_NAMES[k]), &|g| {
                    g[GPU_PASSES + k]
                });
            }
        }
    }
    row(&mut s, "total", &|g| g[..GPU_PASSES].iter().sum());
    s
}

/// The renderer's work in its last frame: draws, triangles, and (with GPU timing) the
/// fragments the forward pass shaded and the depth texels each cascade wrote.
fn work_line(r: &RenderStats) -> String {
    let px = u64::from(r.render_size.0) * u64::from(r.render_size.1);
    let mut s = format!(
        "draws {} (shadow {}, bloom {}) | tris fwd {}k shadow {}k | lights: {} instances with \
         a full set, {} in all sets",
        r.draw_calls,
        r.shadow_draw_calls,
        r.bloom_draw_calls,
        r.triangles / 1000,
        r.shadow_triangles / 1000,
        r.full_light_sets,
        r.light_slots,
    );
    if r.gpu_timing {
        let c = &r.gpu_counts;
        let covered = px.saturating_sub(c.sky).max(1);
        let _ = write!(
            s,
            " | px {}k: sky {}k, meshes {}k (overdraw {:.2}{}), light evals {}k, pcf {}k \
             lookups x{} taps | depth texels {}k/{}k/{}k",
            px / 1000,
            c.sky / 1000,
            c.meshes / 1000,
            c.meshes as f64 / covered as f64,
            if c.complete {
                ""
            } else {
                ", some draws uncounted"
            },
            c.light_evals / 1000,
            c.shadowed / 1000,
            render::PCF_TAPS,
            c.depth_texels[0] / 1000,
            c.depth_texels[1] / 1000,
            c.depth_texels[2] / 1000,
        );
    }
    s
}

/// Frame-time statistics, printed to stderr once per second with `--stats`, and over the
/// whole run (after warmup) with `--frames`. Sample buffers are reused; the run's grows
/// only with `--frames`.
#[derive(Default)]
struct Stats {
    frames: u32,
    total: Duration,
    /// This second's frame times and CPU submit times, ms.
    frame_ms: Vec<f32>,
    cpu_ms: Vec<f32>,
    /// Every frame of a `--frames` run, ms (frame, CPU).
    run: Vec<(f32, f32)>,
    /// GPU readings over the table's window, the newest reading's frame and when the
    /// window started.
    gpu: Vec<GpuSample>,
    gpu_frame: u64,
    gpu_since: Option<Instant>,
}

impl Stats {
    fn new(frames: Option<u32>) -> Stats {
        Stats {
            frame_ms: Vec::with_capacity(1024),
            cpu_ms: Vec::with_capacity(1024),
            run: Vec::with_capacity(frames.map_or(0, |n| n as usize)),
            gpu: Vec::with_capacity(1024),
            ..Stats::default()
        }
    }

    /// Records the GPU reading in `r` if it is new; every [`GPU_TABLE_SECONDS`] (of `now`)
    /// returns the table and the work line to print.
    fn record_gpu(&mut self, r: &RenderStats, now: Instant) -> Option<String> {
        if !r.gpu_timing {
            return None;
        }
        if r.gpu_frame != self.gpu_frame {
            self.gpu_frame = r.gpu_frame;
            let mut g: GpuSample = [0; GPU_PASSES + GPU_PARTS];
            g[..GPU_PASSES].copy_from_slice(&r.gpu_us);
            g[GPU_PASSES..].copy_from_slice(&r.gpu_part_us);
            self.gpu.push(g);
        }
        let since = *self.gpu_since.get_or_insert(now);
        if now - since < Duration::from_secs(GPU_TABLE_SECONDS) || self.gpu.is_empty() {
            return None;
        }
        let out = format!("{}\n  {}", gpu_table(&self.gpu, r.gpu_parts), work_line(r));
        self.gpu.clear();
        self.gpu_since = Some(now);
        Some(out)
    }

    /// Records a frame of `dt` with `cpu` spent recording and submitting it; once a
    /// second returns the line to print.
    fn record(&mut self, dt: Duration, cpu: Duration, keep_run: bool) -> Option<String> {
        let ms = |d: Duration| (d.as_secs_f64() * 1000.0) as f32;
        self.frames += 1;
        self.total += dt;
        self.frame_ms.push(ms(dt));
        self.cpu_ms.push(ms(cpu));
        if keep_run {
            self.run.push((ms(dt), ms(cpu)));
        }
        if self.total < Duration::from_secs(1) {
            return None;
        }
        let (avg, p99, max) = summarize(&mut self.frame_ms);
        let (cpu_avg, cpu_p99, _) = summarize(&mut self.cpu_ms);
        let line = format!(
            "frames={} fps={:.1} frame_ms avg={avg:.2} p99={p99:.2} max={max:.2} \
             cpu_ms avg={cpu_avg:.2} p99={cpu_p99:.2}",
            self.frames,
            f64::from(self.frames) / self.total.as_secs_f64(),
        );
        self.frames = 0;
        self.total = Duration::ZERO;
        self.frame_ms.clear();
        self.cpu_ms.clear();
        Some(line)
    }

    /// The whole run after [`SUMMARY_WARMUP_FRAMES`].
    fn summary(&mut self) -> String {
        let skip = SUMMARY_WARMUP_FRAMES.min(self.run.len().saturating_sub(1));
        self.frame_ms.clear();
        self.cpu_ms.clear();
        for &(f, c) in &self.run[skip..] {
            self.frame_ms.push(f);
            self.cpu_ms.push(c);
        }
        let n = self.frame_ms.len();
        let (avg, p99, max) = summarize(&mut self.frame_ms);
        let (cpu_avg, cpu_p99, _) = summarize(&mut self.cpu_ms);
        self.frame_ms.clear();
        self.cpu_ms.clear();
        format!(
            "frames={n} (after {skip} warmup) fps={:.1} frame_ms avg={avg:.2} p99={p99:.2} \
             max={max:.2} cpu_ms avg={cpu_avg:.2} p99={cpu_p99:.2}",
            if avg > 0.0 { 1000.0 / avg } else { 0.0 },
        )
    }
}

/// The adapter as `--stats` prints it and the overlay shows it.
fn gpu_info(info: &wgpu::AdapterInfo, surface: (u32, u32)) -> GpuInfo {
    GpuInfo {
        name: info.name.clone(),
        backend: format!("{:?}, {} {}", info.backend, info.driver, info.driver_info)
            .trim_end()
            .to_owned(),
        integrated: info.device_type == wgpu::DeviceType::IntegratedGpu,
        linux: cfg!(target_os = "linux"),
        surface,
    }
}

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    /// The monitor's refresh rate, Hz.
    panel_hz: f64,
}

impl Gpu {
    fn new(event_loop: &ActiveEventLoop, app: &mut App, opts: &Options) -> Result<Gpu, String> {
        let attrs = Window::default_attributes()
            .with_title("Skywave")
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            display: Some(Box::new(event_loop.owned_display_handle())),
            ..wgpu::InstanceDescriptor::new_without_display_handle_from_env()
        });
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        let info = adapter.get_info();
        log::info!("adapter: {info:?}");
        // GPU time per pass (and per part of a pass, where timestamps can be written
        // inside passes), for `--stats` only.
        let timestamps = wgpu::Features::TIMESTAMP_QUERY;
        let inside = wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
            | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES;
        let features = if opts.stats && adapter.features().contains(timestamps) {
            timestamps | (adapter.features() & inside)
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("skywave"),
            required_features: features,
            // Same budget as the web build, so desktop never relies on more than WebGL2 has.
            required_limits:
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface is not supported by the adapter")?;
        if let Some(srgb) = surface
            .get_capabilities(&adapter)
            .formats
            .into_iter()
            .find(|f| f.is_srgb())
        {
            config.format = srgb;
        }
        config.present_mode = if opts.no_vsync {
            wgpu::PresentMode::AutoNoVsync
        } else {
            wgpu::PresentMode::AutoVsync
        };
        surface.configure(&device, &config);

        let gpu = gpu_info(&info, (config.width, config.height));
        app.set_gpu_info(&gpu);
        let panel_hz = window
            .current_monitor()
            .and_then(|m| m.refresh_rate_millihertz())
            .map_or(DEFAULT_PANEL_HZ, |mhz| f64::from(mhz) / 1000.0);
        app.set_panel_hz(panel_hz);
        app.force_quality(opts.quality);
        let options = render::RendererOptions {
            culling: !opts.no_cull,
            ..app.renderer_options()
        };
        if opts.stats {
            let logical = size.to_logical::<f64>(window.scale_factor());
            eprintln!(
                "adapter: {} | {} | {:?}",
                gpu.name, gpu.backend, info.device_type
            );
            eprintln!(
                "surface: {}x{} px (scale factor {}, {}x{} logical) {:?} present {:?} \
                 (supported {:?})",
                config.width,
                config.height,
                window.scale_factor(),
                logical.width.round(),
                logical.height.round(),
                config.format,
                config.present_mode,
                surface.get_capabilities(&adapter).present_modes,
            );
            eprintln!(
                "quality: {} {:?} culling {}{}",
                app.ui().quality().name(),
                options.quality,
                if options.culling { "on" } else { "off" },
                if opts.no_prepass {
                    ", depth pre-pass forced off"
                } else {
                    ""
                },
            );
            eprintln!(
                "frame rate: {} = {:.0} fps (panel {panel_hz:.0} Hz){}",
                app.frame_rate().name(),
                app.frame_rate().fps(panel_hz),
                if opts.no_vsync {
                    "; --no-vsync: uncapped, the world drawn every frame"
                } else {
                    ""
                },
            );
        }
        let (renderer, timing) = build_renderer(&device, &queue, &config, app, opts);
        if opts.stats {
            let on = timing;
            eprintln!(
                "gpu timing: {}",
                if on && renderer.stats().gpu_parts {
                    "timestamp queries per pass and inside passes; occlusion queries"
                } else if on {
                    "timestamp queries per pass (none inside passes); occlusion queries"
                } else {
                    "unavailable (no TIMESTAMP_QUERY on this adapter)"
                }
            );
        }
        Ok(Gpu {
            window,
            surface,
            device,
            queue,
            config,
            renderer,
            panel_hz,
        })
    }

    /// A new renderer for `app`, which just moved to another world.
    fn reload(&mut self, app: &App, opts: &Options) {
        self.renderer = build_renderer(&self.device, &self.queue, &self.config, app, opts).0;
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.renderer.resize((width, height));
        // A new swapchain shows nothing yet, even at the same size (Windows sends one
        // restoring from minimized).
        self.renderer.screen_lost();
    }

    /// Draws what changed since the last frame (the whole world every frame when
    /// `always`) and presents it, if anything did; `no_prepass` turns the depth pre-pass
    /// off. Returns the CPU time spent building and submitting it (the scene view and the
    /// renderer, not waiting for the swapchain) and whether a frame was presented.
    fn draw(
        &mut self,
        app: &mut App,
        alpha: f32,
        always: bool,
        no_prepass: bool,
    ) -> (Duration, bool) {
        app.sync_chunks(&mut self.renderer);
        // A car driven for the first time this session (and the saved one, at startup).
        for mesh in app.take_new_meshes() {
            self.renderer.add_mesh(&mesh);
        }
        if always {
            self.renderer.screen_lost();
        }
        let aspect = self.config.width as f32 / self.config.height as f32;
        let start = Instant::now();
        let mut waited = Duration::ZERO;
        let mut reconfigured = false;
        let (surface, device, config) = (&self.surface, &self.device, &self.config);
        let mut acquire = || {
            let t = Instant::now();
            let texture = surface.get_current_texture();
            waited = t.elapsed();
            match texture {
                wgpu::CurrentSurfaceTexture::Success(t)
                | wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                    let view = t.texture.create_view(&Default::default());
                    Some((t, view))
                }
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    surface.configure(device, config);
                    reconfigured = true;
                    None
                }
                other => {
                    log::warn!("skipping frame: {other:?}");
                    None
                }
            }
        };
        let mut scene = app.scene_view(alpha, aspect);
        scene.quality.depth_prepass &= !no_prepass;
        let frame = if always {
            let frame = acquire();
            if let Some((_, view)) = &frame {
                self.renderer.render(view, &scene);
            }
            frame.map(|(t, _)| t)
        } else {
            self.renderer.draw(&scene, acquire).1
        };
        let cpu = start.elapsed().saturating_sub(waited);
        if reconfigured {
            self.renderer.screen_lost();
        }
        let Some(frame) = frame else {
            return (cpu, false);
        };
        self.window.pre_present_notify();
        self.queue.present(frame);
        (cpu, true)
    }

    fn stats(&self) -> render::RenderStats {
        self.renderer.stats()
    }
}

/// A renderer for `app` with what it draws from uploaded (its meshes, instance room, map
/// and stills), and with `--stats` GPU timing on where the adapter has it (true then).
fn build_renderer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    config: &wgpu::SurfaceConfiguration,
    app: &App,
    opts: &Options,
) -> (Renderer, bool) {
    let options = render::RendererOptions {
        culling: !opts.no_cull,
        ..app.renderer_options()
    };
    let size = (config.width, config.height);
    let mut renderer = Renderer::with_options(device, queue, config.format, size, options);
    for mesh in app.meshes() {
        renderer.add_mesh(mesh);
    }
    renderer.reserve_instances(app.max_instances());
    let map = app.map_image();
    renderer.upload_map(map.w, map.h, &map.rgba);
    let (w, h, rgba) = app.stills();
    renderer.upload_stills(w, h, rgba);
    let timing = opts.stats && renderer.enable_gpu_timing();
    (renderer, timing)
}

struct Shell {
    opts: Options,
    app: App,
    input: InputMapper,
    pads: Pads,
    /// Window focus: pads and mouse motion only count while focused.
    focused: bool,
    /// The window is hidden (minimized, or covered where the system says so): nothing is
    /// drawn.
    occluded: bool,
    /// When the next frame may start (the frame-rate cap, the background rate).
    pacer: Pacer,
    /// The last frame was presented (the display paces the next at the panel's rate).
    presented: bool,
    /// The cursor is grabbed for mouse look.
    grabbed: bool,
    gpu: Option<Gpu>,
    last_frame: Option<Instant>,
    /// Origin of the streaming clock.
    started: Instant,
    stats: Stats,
    /// Frames drawn so far (for `--frames`).
    frames_drawn: u32,
    error: Option<String>,
    /// The sound sink, if a device opened.
    sfx: Option<sfx::Sfx>,
    /// The car radio's player, when there is a sink to play it and it is not turned off.
    radio: Option<radio::Player>,
    /// Music folder scans under way.
    music: Vec<MusicScan>,
    /// The folders scanned whole: the music folder, then the picked ones.
    music_roots: Vec<PathBuf>,
    /// The folders put in the library this session.
    music_paths: Vec<String>,
    /// The folder dialog, while it is open.
    picking: Option<Receiver<music::Picked>>,
    /// Paths dropped on the window since the last frame.
    dropped: Vec<PathBuf>,
    /// What the window title last showed: the station playing and the player's title count.
    title: (Option<usize>, u32),
}

/// A scan of music folders on a background thread, and what it is for.
struct MusicScan {
    found: Receiver<Vec<audio::deck::Folder>>,
    of: ScanOf,
}

enum ScanOf {
    /// The folders at start: kept quiet.
    Start,
    /// A folder the player just picked or dropped: announced.
    Picked(PathBuf),
    /// Loose files dropped on the window: a mixtape.
    Drop,
}

impl Shell {
    /// The music library's side of the frame: the folder dialog, drops, finished scans, and
    /// removals written down. Nothing here waits on a dialog or a disk.
    fn pump_music(&mut self) {
        if self.app.take_music_request() && self.picking.is_none() {
            self.picking = Some(music::pick_in_background());
        }
        if let Some(answer) = &self.picking {
            match answer.try_recv() {
                Ok(music::Picked::Folder(dir)) => {
                    self.picking = None;
                    self.add_music_dir(dir);
                }
                Ok(music::Picked::NoDialog) => {
                    self.picking = None;
                    self.app.music_dialog_missing();
                }
                Ok(music::Picked::Cancelled) | Err(TryRecvError::Disconnected) => {
                    self.picking = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        // Folders dropped join as if picked; loose MP3s make one mixtape a drop.
        let (dirs, files): (Vec<PathBuf>, Vec<PathBuf>) = std::mem::take(&mut self.dropped)
            .into_iter()
            .partition(|p| p.is_dir());
        for dir in dirs {
            self.add_music_dir(dir);
        }
        if !files.is_empty() {
            self.music.push(MusicScan {
                found: music::mixtape_in_background(files),
                of: ScanOf::Drop,
            });
        }
        let mut scans = std::mem::take(&mut self.music);
        scans.retain(|scan| match scan.found.try_recv() {
            Ok(folders) => {
                self.music_paths
                    .extend(folders.iter().filter(|f| !f.mixtape).map(|f| f.id.clone()));
                match &scan.of {
                    ScanOf::Start => self.app.add_music_folders(folders),
                    ScanOf::Drop => {
                        for tape in folders {
                            self.app.add_music_folder(tape, true, false);
                        }
                    }
                    ScanOf::Picked(dir) => {
                        let tracks = folders
                            .iter()
                            .flat_map(|f| &f.files)
                            .filter(|f| Path::new(&f.key).starts_with(dir))
                            .count();
                        let name = dir.file_name().unwrap_or(dir.as_os_str());
                        self.app
                            .add_picked_music(&name.to_string_lossy(), tracks, folders);
                    }
                }
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
        });
        scans.append(&mut self.music);
        self.music = scans;
        let forgotten = self.app.take_forgotten_music();
        if !forgotten.is_empty() {
            // Removed on the Audio page: the scan leaves them out from now on, and "Remove
            // music folders" forgets the picked folders too.
            let all = forgotten.iter().any(|p| p == "*");
            let paths: Vec<String> = if all {
                self.music_paths.clone()
            } else {
                forgotten
            };
            let paths: Vec<String> = paths
                .into_iter()
                .filter(|p| !p.starts_with(music::MIXTAPE_ID))
                .collect();
            if let Some(file) = music::ignore_file() {
                music::add_to_list(&file, &paths);
            }
            if let (true, Some(file)) = (all, music::folders_file()) {
                music::remove_from_list(&file, None);
            }
        }
    }

    /// Puts the folder the player picked or dropped in the library: off the ignore list
    /// (with everything in it), remembered for next session, and scanned, as part of the
    /// folder it is in if that one is scanned already (so its folders keep their ids).
    fn add_music_dir(&mut self, dir: PathBuf) {
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        if let Some(file) = music::ignore_file() {
            music::remove_from_list(&file, Some(&dir));
        }
        if let Some(file) = music::folders_file() {
            music::add_to_list(&file, &[dir.to_string_lossy().into_owned()]);
        }
        let root = match self.music_roots.iter().find(|r| dir.starts_with(r)) {
            Some(root) => root.clone(),
            None => {
                self.music_roots.push(dir.clone());
                dir.clone()
            }
        };
        let ignored = music::ignore_file().map_or(Vec::new(), |f| music::load_list(&f));
        self.music.push(MusicScan {
            found: music::scan_in_background(vec![root], ignored),
            of: ScanOf::Picked(dir),
        });
    }

    /// The radio's side of the frame: the player's reports in, this frame's commands out,
    /// and the station with its now-playing title in the window title.
    fn pump_radio(&mut self) {
        if self.radio.is_some() {
            self.pump_music();
        }
        let Some(player) = &mut self.radio else {
            // Nobody plays: drop the commands.
            self.app.radio_commands().for_each(drop);
            return;
        };
        while let Some(event) = player.poll() {
            self.app.radio_event(event);
        }
        if player.titles() != self.title.1 {
            self.app.set_now_playing(player.title());
        }
        for cmd in self.app.radio_commands() {
            player.command(&cmd);
        }
        let r = self.app.radio();
        let playing = match r.state() {
            audio::RadioState::Playing { station } => Some(station),
            _ => None,
        };
        let key = (playing, player.titles());
        let Some(gpu) = self.gpu.as_ref().filter(|_| key != self.title) else {
            return;
        };
        let mut title = String::from("Skywave");
        if let Some(c) = playing.and_then(|i| r.channels().get(i)) {
            let _ = write!(title, " \u{b7} {}", c.name);
            if let Some(now) = player.title() {
                let _ = write!(title, " \u{b7} {now}");
            }
        }
        gpu.window.set_title(&title);
        self.title = key;
    }

    /// Seconds since the start: the pacing clock.
    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Seconds between frames: the frame-rate cap, or the background rate; 0 when the
    /// display paces them (vsync at the panel's rate) or nothing does (`--no-vsync`).
    fn interval(&self) -> f64 {
        if self.opts.no_vsync {
            return 0.0;
        }
        let focused = self.focused && !self.occluded;
        let hz = self.gpu.as_ref().map_or(DEFAULT_PANEL_HZ, |g| g.panel_hz);
        match frame_interval(self.app.frame_rate(), hz, focused) {
            Some(s) => s,
            None if self.presented => 0.0,
            // Nothing presented to wait on: poll at the panel's rate.
            None => 1.0 / hz,
        }
    }

    fn note_focus(&mut self) {
        self.app.set_focused(self.focused && !self.occluded);
    }

    fn set_grab(&mut self, on: bool) {
        if self.grabbed == on {
            return;
        }
        self.grabbed = on;
        if let Some(gpu) = &self.gpu {
            grab_cursor(&gpu.window, on);
        }
    }
}

impl ApplicationHandler for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        match Gpu::new(event_loop, &mut self.app, &self.opts) {
            Ok(gpu) => self.gpu = Some(gpu),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
            }
            WindowEvent::Focused(false) => {
                self.focused = false;
                self.input.release_all();
                self.set_grab(false);
                self.note_focus();
            }
            WindowEvent::Focused(true) => {
                self.focused = true;
                self.pads.resync(&mut self.input);
                self.note_focus();
            }
            WindowEvent::Occluded(hidden) => {
                self.occluded = hidden;
                if let (false, Some(gpu)) = (hidden, &mut self.gpu) {
                    gpu.renderer.screen_lost();
                }
                self.note_focus();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(b) = mouse_button(button) {
                    let down = state == ElementState::Pressed;
                    if down {
                        self.input.press(RawInput::MouseButton(b));
                    } else {
                        self.input.release(RawInput::MouseButton(b));
                    }
                    if b == MOUSE_RIGHT {
                        self.set_grab(down);
                    }
                }
            }
            WindowEvent::DroppedFile(path) => {
                if self.radio.is_some() {
                    self.dropped.push(path);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.input.axis(RawInput::MouseWheel, wheel_notches(delta));
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(key) = match event.physical_key {
                    PhysicalKey::Code(code) => map_key(code),
                    PhysicalKey::Unidentified(_) => None,
                } {
                    match event.state {
                        ElementState::Pressed => self.input.press(RawInput::Key(key)),
                        ElementState::Released => self.input.release(RawInput::Key(key)),
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let interval = self.interval();
                if self.pacer.wait(self.now(), interval) > SLEEP_SLACK {
                    // Early for the cap: `about_to_wait` asks again when it is due.
                    return;
                }
                let started = self.started;
                sleep_until_due(&mut self.pacer, interval, || {
                    started.elapsed().as_secs_f64()
                });
                let now = Instant::now();
                let dt = self.last_frame.map_or(Duration::ZERO, |t| now - t);
                self.last_frame = Some(now);
                self.pads.poll(&mut self.input, self.focused);
                let clock = || self.started.elapsed().as_secs_f64();
                let steps = self.app.frame(&mut self.input, dt.as_secs_f64(), &clock);
                if let Some(sfx) = &self.sfx {
                    sfx.set_params(self.app.audio_params());
                }
                self.pump_radio();
                if self.app.quit_requested() {
                    event_loop.exit();
                    return;
                }
                if let Some(world) = self.app.take_world_pick() {
                    match self.app.switch_world(&self.opts.scene, world) {
                        Ok(()) => {
                            // The new world's radio starts off: silence the old one's.
                            if let Some(player) = &mut self.radio {
                                player.command(&audio::RadioCommand::Stop);
                            }
                            if let Some(gpu) = &mut self.gpu {
                                gpu.reload(&self.app, &self.opts);
                            }
                        }
                        Err(e) => log::warn!("{e}; staying in this world"),
                    }
                }
                let Some(gpu) = &mut self.gpu else {
                    return;
                };
                if self.occluded {
                    self.presented = false;
                    return;
                }
                let (no_vsync, no_prepass) = (self.opts.no_vsync, self.opts.no_prepass);
                let (cpu, presented) = gpu.draw(&mut self.app, steps.alpha, no_vsync, no_prepass);
                self.presented = presented;
                let render = gpu.stats();
                self.app.set_render_stats(render);
                self.app.set_cpu_submit_ms(cpu.as_secs_f32() * 1000.0);
                self.frames_drawn += 1;
                let keep = self.opts.frames.is_some();
                if self.opts.stats {
                    if let Some(table) = self.stats.record_gpu(&render, now) {
                        eprintln!(
                            "[gpu] t={:.0}s over {GPU_TABLE_SECONDS} s, quality={}\n{table}",
                            self.started.elapsed().as_secs_f64(),
                            self.app.ui().quality().name(),
                        );
                    }
                }
                if (self.opts.stats || keep) && !dt.is_zero() {
                    if let Some(line) = self.stats.record(dt, cpu, keep) {
                        if self.opts.stats {
                            eprintln!(
                                "[stats] t={:.0}s {line} | {} | present={:?} quality={}",
                                self.started.elapsed().as_secs_f64(),
                                render_line(&render),
                                gpu.config.present_mode,
                                self.app.ui().quality().name(),
                            );
                        }
                    }
                }
                if Some(self.frames_drawn) == self.opts.frames {
                    eprintln!(
                        "[summary] {} | {}",
                        self.stats.summary(),
                        render_line(&render)
                    );
                    if self.opts.exit {
                        event_loop.exit();
                    }
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        // Raw, unaccelerated motion: keeps coming while the cursor is locked.
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if self.focused {
                self.input.mouse_delta(dx as f32, dy as f32);
            }
        }
    }

    /// Asks for the next frame when it is due, else sleeps until then (input events wake
    /// the loop in between and are sampled by that frame).
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gpu) = &self.gpu else {
            return;
        };
        let wait = self.pacer.wait(self.now(), self.interval());
        if wait > SLEEP_SLACK {
            let early = Duration::from_secs_f64(wait - 0.5 * SLEEP_SLACK);
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + early));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
            gpu.window.request_redraw();
        }
    }
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let opts = match parse_args(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("skywave: {e}");
            }
            eprintln!(
                "usage: skywave [--seed N] [--car ID] [--scenario ID] [--scene KEY=VALUE]... [--stats] \
                 [--frames N [--exit]] [--no-vsync] [--quality low|battery|deck|medium|high|ultra] \
                 [--no-cull] [--no-prepass] [--radio-off] [--music DIR] [--version]"
            );
            return if e.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            };
        }
    };
    if opts.version {
        println!("skywave {}", env!("SKYWAVE_VERSION"));
        return ExitCode::SUCCESS;
    }
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("skywave: cannot open a window: {e}");
            return ExitCode::FAILURE;
        }
    };
    let pads = Pads::new(opts.stats);
    if opts.stats {
        eprintln!("{}", pads.summary());
    } else {
        log::info!("{}", pads.summary());
    }
    let store = settings::FileStore::from_env();
    let scene = opts.scene.clone().with_saved_world(store.load().as_deref());
    let mut app = match App::from_setup(&scene) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("skywave: {e}");
            return ExitCode::from(2);
        }
    };
    app.set_can_quit(true);
    app.set_stream_budget(app::stream::StreamBudget::NATIVE);
    app.set_settings_store(Box::new(store));
    let opts_frames = opts.frames;
    // The radio plays through the sound sink: without a device (or with --radio-off) its
    // commands are dropped, as on a web page without the player glue.
    let radio_out = (!opts.radio_off).then(|| Arc::new(radio::Output::new()));
    let sfx = sfx::Sfx::start(radio_out.clone());
    let radio = radio_out.filter(|_| sfx.is_some()).map(radio::Player::new);
    // The music folder and the folders picked in earlier sessions, scanned at once.
    let music_roots = radio.as_ref().map_or(Vec::new(), |_| {
        let picked = music::folders_file().map_or(Vec::new(), |f| music::load_list(&f));
        let roots = music::default_dir(opts.music.as_deref())
            .into_iter()
            .chain(picked.into_iter().map(PathBuf::from))
            .collect();
        music::roots(roots)
    });
    let music: Vec<MusicScan> = (!music_roots.is_empty())
        .then(|| {
            let ignored = music::ignore_file().map_or(Vec::new(), |f| music::load_list(&f));
            MusicScan {
                found: music::scan_in_background(music_roots.clone(), ignored),
                of: ScanOf::Start,
            }
        })
        .into_iter()
        .collect();
    // The Audio page offers "Music library" wherever music plays.
    app.set_music_picker(radio.is_some(), false);
    let mut shell = Shell {
        app,
        opts,
        input: InputMapper::default(),
        pads,
        focused: true,
        occluded: false,
        pacer: Pacer::default(),
        presented: false,
        grabbed: false,
        gpu: None,
        last_frame: None,
        started: Instant::now(),
        stats: Stats::new(opts_frames),
        frames_drawn: 0,
        error: None,
        sfx,
        radio,
        music,
        music_roots,
        music_paths: Vec::new(),
        picking: None,
        dropped: Vec::new(),
        title: (None, 0),
    };
    if let Err(e) = event_loop.run_app(&mut shell) {
        eprintln!("skywave: event loop failed: {e}");
        return ExitCode::FAILURE;
    }
    match shell.error {
        Some(e) => {
            eprintln!("skywave: {e}");
            ExitCode::FAILURE
        }
        None => ExitCode::SUCCESS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::FrameRate;
    use render::FrameWork;

    fn args(list: &[&str]) -> Result<Options, String> {
        parse_args(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_arguments() {
        assert_eq!(args(&[]), Ok(Options::default()));
        let mut scene = SceneSetup::from_query("seed=42&car=homestead_wagon").unwrap();
        assert_eq!(
            args(&["--stats", "--seed", "42", "--car", "homestead_wagon"]),
            Ok(Options {
                scene: scene.clone(),
                stats: true,
                ..Options::default()
            })
        );
        scene.set("t", "22:30").unwrap();
        scene.set("cam", "cockpit").unwrap();
        assert_eq!(
            args(&[
                "--seed",
                "42",
                "--scene",
                "car=homestead_wagon",
                "--scene",
                "t=22:30",
                "--scene",
                "cam=cockpit"
            ]),
            Ok(Options {
                scene,
                ..Options::default()
            })
        );
        assert_eq!(args(&["--version"]).map(|o| o.version), Ok(true));
        assert_eq!(args(&["-V"]).map(|o| o.version), Ok(true));
        assert!(args(&["--scene"]).is_err());
        assert!(args(&["--scene", "t"]).is_err());
        assert!(args(&["--scene", "t=25:00"]).is_err());
        assert!(args(&["--scene", "zoom=2"]).is_err());
        assert!(args(&["--seed"]).is_err());
        assert!(args(&["--car"]).is_err());
        assert!(args(&["--scenario"]).is_err());
        let world = args(&["--scenario", "mirage_valley_2001"]).unwrap().scene;
        assert_eq!((world.scenario, world.place), (1, app::PlaceKey::Coast));
        let old = args(&["--scenario", "inland_basin_2001"]).unwrap().scene;
        assert_eq!((old.scenario, old.unknown_scenario), (1, false));
        let unknown = args(&["--scenario", "atlantis"]).unwrap().scene;
        assert!(unknown.unknown_scenario && unknown.scenario == 0);
        assert!(args(&["--seed", "x"]).is_err());
        assert!(args(&["--fast"]).is_err());
        assert_eq!(args(&["--help"]), Err(String::new()));
    }

    #[test]
    fn parses_benchmark_flags() {
        let o = args(&[
            "--stats",
            "--frames",
            "600",
            "--exit",
            "--no-vsync",
            "--quality",
            "deck",
            "--no-cull",
            "--no-prepass",
            "--scene",
            "place=coast",
        ])
        .unwrap();
        assert!(o.stats && o.exit && o.no_vsync && o.no_cull && o.no_prepass);
        assert_eq!(o.frames, Some(600));
        assert_eq!(o.quality, Some(Quality::Deck));
        assert!(args(&["--frames"]).is_err());
        assert!(args(&["--frames", "0"]).is_err());
        assert!(args(&["--frames", "x"]).is_err());
        assert!(args(&["--exit"]).is_err(), "--exit needs --frames");
        assert!(args(&["--quality", "potato"]).is_err());
        assert!(!args(&["--frames", "3"]).unwrap().exit);
        assert!(args(&["--radio-off"]).unwrap().radio_off);
        assert_eq!(
            args(&["--music", "/tunes"]).unwrap().music,
            Some("/tunes".into())
        );
        assert!(args(&["--music"]).is_err());
    }

    #[test]
    fn summaries_take_mean_p99_and_max() {
        let mut ms: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        assert_eq!(summarize(&mut ms), (50.5, 99.0, 100.0));
        assert_eq!(summarize(&mut []), (0.0, 0.0, 0.0));
        let mut s = Stats::new(Some(200));
        for i in 0..200 {
            let dt = Duration::from_millis(if i < 60 { 50 } else { 10 });
            s.record(dt, Duration::from_millis(1), true);
        }
        let line = s.summary();
        assert!(
            line.starts_with("frames=140 (after 60 warmup) fps=100.0"),
            "{line}"
        );
        assert!(line.contains("frame_ms avg=10.00 p99=10.00"), "{line}");
    }

    #[test]
    fn every_game_key_has_a_winit_code() {
        let codes = [
            KeyCode::KeyA,
            KeyCode::KeyB,
            KeyCode::KeyC,
            KeyCode::KeyD,
            KeyCode::KeyE,
            KeyCode::KeyF,
            KeyCode::KeyG,
            KeyCode::KeyH,
            KeyCode::KeyI,
            KeyCode::KeyJ,
            KeyCode::KeyK,
            KeyCode::KeyL,
            KeyCode::KeyM,
            KeyCode::KeyN,
            KeyCode::KeyO,
            KeyCode::KeyP,
            KeyCode::KeyQ,
            KeyCode::KeyR,
            KeyCode::KeyS,
            KeyCode::KeyT,
            KeyCode::KeyU,
            KeyCode::KeyV,
            KeyCode::KeyW,
            KeyCode::KeyX,
            KeyCode::KeyY,
            KeyCode::KeyZ,
            KeyCode::Digit0,
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
            KeyCode::F1,
            KeyCode::F2,
            KeyCode::F3,
            KeyCode::F4,
            KeyCode::F5,
            KeyCode::F6,
            KeyCode::F7,
            KeyCode::F8,
            KeyCode::F9,
            KeyCode::F10,
            KeyCode::F11,
            KeyCode::F12,
            KeyCode::Escape,
            KeyCode::Enter,
            KeyCode::Space,
            KeyCode::Tab,
            KeyCode::Backspace,
            KeyCode::ShiftLeft,
            KeyCode::ShiftRight,
            KeyCode::ControlLeft,
            KeyCode::ControlRight,
            KeyCode::AltLeft,
            KeyCode::AltRight,
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
            KeyCode::BracketLeft,
            KeyCode::BracketRight,
            KeyCode::Backslash,
            KeyCode::Comma,
            KeyCode::Period,
            KeyCode::Slash,
            KeyCode::Semicolon,
            KeyCode::Quote,
            KeyCode::Minus,
            KeyCode::Equal,
            KeyCode::Backquote,
        ];
        assert_eq!(codes.len(), Key::COUNT);
        for (code, key) in codes.into_iter().zip(Key::all()) {
            assert_eq!(map_key(code), Some(key), "{code:?}");
        }
        assert_eq!(map_key(KeyCode::NumpadEnter), None);
        assert_eq!(map_key(KeyCode::MediaPlayPause), None);
    }

    #[test]
    fn mouse_buttons_and_wheel_follow_the_dom_numbering() {
        let table = [
            (MouseButton::Left, Some(0)),
            (MouseButton::Middle, Some(1)),
            (MouseButton::Right, Some(MOUSE_RIGHT)),
            (MouseButton::Back, Some(3)),
            (MouseButton::Forward, Some(4)),
            (MouseButton::Other(9), None),
        ];
        for (b, want) in table {
            assert_eq!(mouse_button(b), want, "{b:?}");
        }
        assert_eq!(wheel_notches(MouseScrollDelta::LineDelta(0.0, 1.0)), 1.0);
        assert_eq!(wheel_notches(MouseScrollDelta::LineDelta(3.0, -2.0)), -2.0);
        let px = winit::dpi::PhysicalPosition::new(0.0, -250.0);
        assert_eq!(wheel_notches(MouseScrollDelta::PixelDelta(px)), -2.5);
    }

    #[test]
    fn stats_report_once_per_second() {
        let mut s = Stats::new(None);
        let mut lines = Vec::new();
        for _ in 0..240 {
            if let Some(l) = s.record(Duration::from_millis(10), Duration::from_millis(2), false) {
                lines.push(l);
            }
        }
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("fps=100.0"), "{}", lines[0]);
        assert!(lines[0].contains("cpu_ms avg=2.00"), "{}", lines[0]);
        assert!(s.run.is_empty());
        let r = RenderStats {
            instances: 10,
            instances_drawn: 4,
            gpu_timing: true,
            gpu_us: [1000, 2000, 0, 500, 250, 50],
            ..RenderStats::default()
        };
        let line = render_line(&r);
        assert!(line.starts_with("inst=10 fwd=4 "), "{line}");
        assert!(
            line.contains("gpu_ms total=3.80 shadow=1.00 forward=2.00"),
            "{line}"
        );
    }

    #[test]
    fn gpu_table_reports_min_median_and_p95_per_pass_and_part() {
        let samples: Vec<GpuSample> = (1..=100)
            .map(|i| std::array::from_fn(|k| if k == 1 { 2000 * i } else { 100 * i }))
            .collect();
        let table = gpu_table(&samples, true);
        let lines: Vec<&str> = table.lines().collect();
        assert!(lines[0].contains("(100 frames read)"), "{table}");
        assert!(
            table.contains("\n  shadow          0.10    5.10    9.50"),
            "{table}"
        );
        assert!(
            table.contains("\n    cascade1      0.10    5.10    9.50"),
            "{table}"
        );
        assert!(
            table.contains("\n  forward         2.00  102.00  190.00"),
            "{table}"
        );
        assert!(table.contains("\n    depth "), "{table}");
        // Six passes (each 100 µs × i, forward 2000 µs × i) summed.
        assert!(
            table.contains("\n  total           2.50  127.50  237.50"),
            "{table}"
        );
        assert_eq!(lines.len(), 1 + GPU_PASSES + GPU_PARTS + 1);
        assert_eq!(
            gpu_table(&samples, false).lines().count(),
            1 + GPU_PASSES + 1
        );

        // A reading is kept once (by its frame) and the table comes every few seconds.
        let mut st = Stats::new(None);
        let t0 = Instant::now();
        let mut r = RenderStats {
            gpu_timing: true,
            gpu_frame: 1,
            ..RenderStats::default()
        };
        assert!(st.record_gpu(&r, t0).is_none());
        assert!(st.record_gpu(&r, t0 + Duration::from_secs(1)).is_none());
        r.gpu_frame = 2;
        let out = st
            .record_gpu(&r, t0 + Duration::from_secs(GPU_TABLE_SECONDS))
            .unwrap();
        assert!(out.contains("(2 frames read)"), "{out}");
        assert!(out.contains("draws 0 (shadow 0, bloom 0)"), "{out}");
        assert!(st.gpu.is_empty());
        r.gpu_timing = false;
        assert!(st.record_gpu(&r, t0 + Duration::from_secs(60)).is_none());
    }

    /// Frames of the coast's `coast-strip-golden` scene at Deck on a Vulkan device
    /// (lavapipe on CI), counted with occlusion queries: the sky shades only the pixels the
    /// meshes leave, the depth pre-pass shades each covered pixel about once (half the
    /// fragments of drawing without it), and a still scene keeps both cascades. Skipped
    /// without a Vulkan device, except on CI.
    #[test]
    fn fragments_on_the_coast_at_deck() {
        let Some((device, queue, name)) = vulkan_device() else {
            assert!(std::env::var_os("CI").is_none(), "no Vulkan device on CI");
            eprintln!("skipped: no Vulkan device (install mesa-vulkan-drivers)");
            return;
        };
        eprintln!("device: {name}");
        let size = (1280, 800);
        let query = "?seed=1&place=coast&car=gannet_gt&still=1&cam=chase-far&t=17:40&traffic=off";
        let (mut app, mut r) = streamed(query, None, &device, &queue, size);
        let (texture, _) = target(&device, size);
        assert!(r.enable_gpu_timing());
        app.force_quality(Some(Quality::Deck));
        let (without, before) = deck_frames(&mut app, &mut r, &device, &queue, &texture, false);
        let (with, after) = deck_frames(&mut app, &mut r, &device, &queue, &texture, true);
        // The same image ([`assert_same_image`]).
        assert_same_image(&before, &after, "coast-strip-golden");
        let px = 1280 * 800;
        let (a, b) = (&without.gpu_counts, &with.gpu_counts);
        assert!(a.complete && b.complete);
        // Measured: 104k of 1024k pixels are sky.
        assert!(b.sky > 0 && b.sky < px / 5, "{b:?}");
        assert_eq!(a.sky, b.sky);
        // Measured: 1958k mesh fragments without the pre-pass, 920k (the 920k pixels
        // covered) with it.
        let covered = px - b.sky;
        assert!(b.meshes <= covered + covered / 50, "{b:?}");
        assert!(b.meshes * 100 <= a.meshes * 55, "{a:?} {b:?}");
        // Bloom from half the 1280×800 target, four levels: four passes down, three up.
        assert_eq!(with.bloom_draw_calls, 7);
        // Nothing moves: after the first frames both cascades are kept.
        assert_eq!(with.cascades_drawn, 0, "{with:?}");
        assert_eq!(with.shadow_draw_calls, 0);
    }

    /// The town's wagon (`town-noon-wagon`: trim and wood panels over its body) looks the
    /// same at Deck with the depth pre-pass as without it ([`assert_same_image`]). Skipped without a Vulkan device,
    /// except on CI.
    #[test]
    fn depth_prepass_keeps_the_image() {
        let Some((device, queue, _)) = vulkan_device() else {
            assert!(std::env::var_os("CI").is_none(), "no Vulkan device on CI");
            return;
        };
        let size = (1280, 800);
        let query = "?seed=1&car=homestead_wagon&cam=chase-far&t=12:00&pos=-197.5,-150&yaw=0\
                     &still=1&stream=0&traffic=off";
        let (mut app, mut r) = streamed(query, None, &device, &queue, size);
        let (texture, _) = target(&device, size);
        app.force_quality(Some(Quality::Deck));
        let (_, before) = deck_frames(&mut app, &mut r, &device, &queue, &texture, false);
        let (_, after) = deck_frames(&mut app, &mut r, &device, &queue, &texture, true);
        assert_same_image(&before, &after, "town-noon-wagon");
    }

    /// Three frames of `app` at Deck with the pre-pass on or off into `texture`: the last
    /// frame's stats and its pixels (RGBA8, rows packed).
    fn deck_frames(
        app: &mut App,
        r: &mut render::Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        prepass: bool,
    ) -> (RenderStats, Vec<u8>) {
        let view = texture.create_view(&Default::default());
        for _ in 0..3 {
            let mut scene = app.scene_view(1.0, 1.6);
            assert!(scene.quality.depth_prepass, "the Deck preset has it");
            scene.quality.depth_prepass = prepass;
            r.render(&view, &scene);
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        }
        let s = r.stats();
        eprintln!("{}", work_line(&s));
        (s, pixels(device, queue, texture))
    }

    /// `texture`'s RGBA8 pixels, rows packed.
    fn pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
        let (w, h) = (texture.width(), texture.height());
        let row = (4 * w).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            texture.size(),
        );
        queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let data = buffer.get_mapped_range(..).unwrap();
        data.chunks(row as usize)
            .flat_map(|r| &r[..4 * w as usize])
            .copied()
            .collect()
    }

    /// Fails unless `a` and `b` are the same image but for a few pixels in 10 000: where two
    /// triangles of one mesh tie in depth the pre-pass's less-or-equal test lets the later
    /// one win (measured: 130 isolated pixels of 1 024 000 on the coast, 5 for the wagon,
    /// on distant silhouettes).
    fn assert_same_image(a: &[u8], b: &[u8], scene: &str) {
        let differ = a.chunks(4).zip(b.chunks(4)).filter(|(p, q)| p != q).count();
        let limit = a.len() / 4 / 5000;
        assert!(
            differ <= limit,
            "{scene}: {differ} pixels differ with the depth pre-pass (at most {limit})"
        );
    }

    /// Prints what culling and each preset leave the GPU on the streamed coast at the
    /// native budget (instances and triangles per pass, from the renderer's stats on
    /// wgpu's no-op device) and what culling costs the CPU, without traffic and with busy
    /// traffic. Run with
    /// `cargo test --release -p platform-native -- --ignored --nocapture culling_on_the_coast`;
    /// `docs/PERFORMANCE.md` quotes it. [`traffic_at_a_junction_within_budget`] is the
    /// enforced check.
    #[test]
    #[ignore = "benchmark; prints numbers"]
    fn culling_on_the_coast() {
        let runs = [
            ("high, no culling", Quality::High, false),
            ("high", Quality::High, true),
            ("medium", Quality::Medium, true),
            ("deck", Quality::Deck, true),
            ("low", Quality::Low, true),
        ];
        for traffic in ["off", "busy"] {
            let query = format!("?seed=1&place=coast&t=17:40&traffic={traffic}");
            let stream = app::stream::StreamBudget::NATIVE;
            eprintln!("traffic {traffic}:");
            render_counts(&query, Some(stream), &runs, 40);
        }
    }

    /// The coast junction of the `coast-traffic-noon` golden, busy traffic and pedestrians
    /// warmed up (queued, oncoming and crossing cars and people on the walks in view). Not
    /// `still`: the world runs, so cars and people move between frames.
    const JUNCTION: &str = "?seed=1&place=coast&car=gannet_gt&cam=chase-mid&t=12:00\
                            &pos=747.7,493.9&yaw=-20.5";

    /// Deck and Medium at [`JUNCTION`] with busy traffic and pedestrians: at most this many
    /// draws in the worst frame and in the median frame after the first, forward and shadow
    /// triangles a frame (`docs/PERFORMANCE.md`): the measured numbers plus about 10%.
    const JUNCTION_BUDGET: [(Quality, u32, u32, u64, u64); 2] = [
        (Quality::Deck, 911, 529, 259_000, 817_000),
        (Quality::Medium, 939, 477, 259_000, 1_345_000),
    ];
    // Measured over 8 frames driving at 100 km/h with the world running after it streamed
    // held still (the same in release and debug; 96 cars and 25 people live, 190 car and
    // figure-part instances more in the forward pass than with traffic off): the most draws
    // in a frame (the first draws every cascade; at Deck the depth pre-pass draws the
    // forward pass's meshes twice), the median frame's after it (no far cascade), forward
    // triangles and shadow triangles a frame (moving casters redraw the far cascade on its
    // interval): Deck 828, 481, 235k, 743k; Medium 854, 434, 235k, 1223k. Main's junction
    // before the map edges (another layout): Deck 1134, 681, 428k, 672k; Medium 1121, 503,
    // 428k, 1206k.
    /// Instances one pedestrian adds: a figure's parts.
    const FIGURE_PARTS: u32 = app::FIGURE_PARTS as u32;

    /// What busy traffic and pedestrians in view cost the GPU at a coast junction on the
    /// no-op device, streamed at the web budget: the cars and people are drawn (at least
    /// three cars and three figures' parts more in the forward pass than without them), and
    /// under `SKYWAVE_ENFORCE_BUDGETS` Deck and Medium stay within [`JUNCTION_BUDGET`].
    #[test]
    fn traffic_at_a_junction_within_budget() {
        let runs = [
            ("medium", Quality::Medium, true),
            ("deck", Quality::Deck, true),
        ];
        let started = Instant::now();
        let off = render_counts(&format!("{JUNCTION}&traffic=off"), None, &runs, 8);
        let busy = render_counts(&format!("{JUNCTION}&traffic=busy"), None, &runs, 8);
        eprintln!(
            "junction benchmark took {:.1} s",
            started.elapsed().as_secs_f64()
        );
        for ((q, a), (_, b)) in off.iter().zip(&busy) {
            let npcs = b.instances_drawn - a.instances_drawn;
            eprintln!("{q:?}: {npcs} NPC car and figure-part instances in the forward pass");
            assert!(npcs >= 3 + 3 * FIGURE_PARTS, "{q:?}: {npcs} in view");
            let &(_, draws, steady, fwd, shadow) =
                JUNCTION_BUDGET.iter().find(|j| j.0 == *q).unwrap();
            if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
                assert!(b.draws <= draws, "{q:?}: {} draws", b.draws);
                assert!(
                    b.steady_draws <= steady,
                    "{q:?}: {} draws in the median frame",
                    b.steady_draws
                );
                assert!(b.triangles <= fwd, "{q:?}: {} triangles", b.triangles);
                assert!(
                    b.shadow_triangles <= shadow,
                    "{q:?}: {} shadow",
                    b.shadow_triangles
                );
            }
        }
    }

    /// One preset's counts over a run of frames.
    struct Counts {
        instances_drawn: u32,
        /// The most in a frame, and the median frame's after the first (which draws every
        /// cascade from cold): a frame without the far cascade.
        draws: u32,
        steady_draws: u32,
        triangles: u64,
        /// Mean a frame (a kept far cascade draws none on alternate frames).
        shadow_triangles: u64,
    }

    /// Streams the scene `query` until idle (at `stream`, else the web budget), then
    /// renders `frames` frames per run on the no-op device at 1280×800 with the camera
    /// driving, printing and returning each run's counts.
    fn render_counts(
        query: &str,
        stream: Option<app::stream::StreamBudget>,
        runs: &[(&str, Quality, bool)],
        frames: u32,
    ) -> Vec<(Quality, Counts)> {
        let (device, queue) = noop_device();
        let size = (1280, 800);
        let (mut app, mut r) = streamed(query, stream, &device, &queue, size);
        eprintln!(
            "{query}: {} cars, {} people",
            app.traffic_count(),
            app.pedestrian_count()
        );
        let (_, view) = target(&device, size);
        let mut mapper = InputMapper::default();
        let started = Instant::now();
        let clock = || started.elapsed().as_secs_f64();
        let mut out = Vec::new();
        for &(label, q, culling) in runs {
            app.force_quality(Some(q));
            r.set_culling(culling);
            let mut best = f64::INFINITY;
            let mut total = 0.0;
            // Shadow work alternates when the far cascade is kept: the most per cascade
            // over the run, and the mean triangles a frame.
            let mut shadow_max = [0; 3];
            let mut shadow_tris = 0u64;
            let mut draws = 0;
            let mut later = Vec::with_capacity(frames as usize);
            for f in 0..frames {
                // The world moves on (traffic, people, the clock) a 90 Hz frame at a time.
                app.frame(&mut mapper, 1.0 / 90.0, &clock);
                // Driving: the camera moves 0.3 m a frame along the world's x (100 km/h
                // at 90 fps), so the cascades cannot be kept as they are for a still view.
                let mut scene = app.scene_view(1.0, 1.6);
                let d = 0.3 * f as f32;
                for k in 0..3 {
                    scene.camera.view[3][k] -= d * scene.camera.view[0][k];
                }
                let t = Instant::now();
                r.render(&view, &scene);
                let dt = t.elapsed().as_secs_f64();
                best = best.min(dt);
                total += dt;
                let s = r.stats();
                for (m, n) in shadow_max.iter_mut().zip(s.shadow_instances) {
                    *m = (*m).max(n);
                }
                shadow_tris += s.shadow_triangles;
                draws = draws.max(s.draw_calls);
                if f > 0 {
                    later.push(s.draw_calls);
                }
            }
            later.sort_unstable();
            let steady = later.get(later.len() / 2).copied().unwrap_or(draws);
            let s = r.stats();
            eprintln!(
                "{label:>16}: instances {} fwd {} shadow {:?} ({} cascades) | tris fwd {}k \
                 shadow {}k a frame | draws {} at most, median {} after the first | cpu render() \
                 avg {:.2} ms best {:.2} ms",
                s.instances,
                s.instances_drawn,
                shadow_max,
                s.cascades,
                s.triangles / 1000,
                shadow_tris / u64::from(frames) / 1000,
                draws,
                steady,
                total / f64::from(frames) * 1000.0,
                best * 1000.0,
            );
            out.push((
                q,
                Counts {
                    instances_drawn: s.instances_drawn,
                    draws,
                    steady_draws: steady,
                    triangles: s.triangles,
                    shadow_triangles: shadow_tris / u64::from(frames),
                },
            ));
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        }
        out
    }

    /// A Vulkan device (lavapipe in a container, or a real GPU) with timestamp queries,
    /// inside passes where it has them; `None` without one.
    fn vulkan_device() -> Option<(wgpu::Device, wgpu::Queue, String)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        let wanted = wgpu::Features::TIMESTAMP_QUERY
            | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
            | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: adapter.features() & wanted,
            required_limits: render_limits().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .ok()?;
        let info = adapter.get_info();
        Some((
            device,
            queue,
            format!("{} ({})", info.name, info.driver_info),
        ))
    }

    /// Renders `frames` frames of `app` at `q`, waiting for each, and returns the last
    /// frame's stats (GPU medians over the run, counts of the last frame read).
    fn measure(
        app: &mut App,
        r: &mut render::Renderer,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
        q: Quality,
        frames: u32,
    ) -> RenderStats {
        app.force_quality(Some(q));
        for _ in 0..frames {
            r.render(view, &app.scene_view(1.0, 1.6));
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        }
        // One more so the last frame's readings are in.
        r.render(view, &app.scene_view(1.0, 1.6));
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        r.stats()
    }

    /// The renderer's work and its GPU medians, for a preset.
    fn gpu_report(label: &str, s: &RenderStats) -> String {
        let mut line = format!("{label:>8}: {} | gpu median", work_line(s));
        for (name, us) in GPU_PASS_NAMES.iter().zip(s.gpu_median_us) {
            let _ = write!(line, " {name}={:.2}", us as f32 / 1000.0);
        }
        line
    }

    /// The coast's golden scenes on a Vulkan device (lavapipe here): fragments shaded per
    /// pass from occlusion queries and GPU time per pass, at Deck and Medium. On a CPU
    /// rasterizer the times rank the passes, not the Deck's milliseconds; the fragment
    /// counts are exact. Run with
    /// `cargo test --release -p platform-native -- --ignored --nocapture gpu_passes_on_the_coast`.
    #[test]
    #[ignore = "benchmark; needs a Vulkan device (lavapipe: mesa-vulkan-drivers)"]
    fn gpu_passes_on_the_coast() {
        let Some((device, queue, name)) = vulkan_device() else {
            eprintln!("no Vulkan device");
            return;
        };
        eprintln!("device: {name}");
        let scenes = [
            ("coast-strip-golden", "cam=chase-far&t=17:40"),
            (
                "coast-traffic-noon",
                "cam=chase-mid&t=12:00&pos=747.7,493.9&yaw=-20.5&traffic=busy&phase=1440",
            ),
            ("coast-night-golden", "cam=chase-near&t=22:30&lights=1"),
        ];
        let size = (1280, 800);
        let (_, view) = target(&device, size);
        let frames = std::env::var("SKYWAVE_BENCH_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6);
        for (scene, params) in scenes {
            let query = format!("?seed=1&place=coast&car=gannet_gt&still=1&{params}");
            let (mut app, mut r) = streamed(&query, None, &device, &queue, size);
            assert!(r.enable_gpu_timing(), "no timestamp queries");
            eprintln!("{scene}:");
            for q in [Quality::Deck, Quality::Medium] {
                let s = measure(&mut app, &mut r, &device, &view, q, frames);
                eprintln!("{}", gpu_report(q.name(), &s));
            }
        }
    }

    const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

    /// The scene `query` loaded and streamed until idle (at `stream`, else the web budget)
    /// into a renderer for a `size` target.
    fn streamed(
        query: &str,
        stream: Option<app::stream::StreamBudget>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
    ) -> (App, render::Renderer) {
        let mut app = App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap();
        if let Some(budget) = stream {
            app.set_stream_budget(budget);
        }
        let mut r = render::Renderer::with_options(
            device,
            queue,
            TARGET_FORMAT,
            size,
            app.renderer_options(),
        );
        for m in app.meshes() {
            r.add_mesh(m);
        }
        r.reserve_instances(app.max_instances());
        let mut mapper = InputMapper::default();
        let started = Instant::now();
        let clock = || started.elapsed().as_secs_f64();
        // The world holds still while the place streams: how many frames that takes depends
        // on the machine, and the traffic measured after must not.
        let still = app.still();
        app.set_still(true);
        while !app.stream_idle() {
            app.frame(&mut mapper, 1.0 / 60.0, &clock);
            app.sync_chunks(&mut r);
            for m in app.take_new_meshes() {
                r.add_mesh(&m);
            }
        }
        app.set_still(still);
        (app, r)
    }

    /// A render target of `size` the renderer can draw into as it would the swapchain, and
    /// its view.
    fn target(device: &wgpu::Device, size: (u32, u32)) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        (texture, view)
    }

    /// wgpu's no-op device with [`render_limits`]: records every pass, draws nothing.
    fn noop_device() -> (wgpu::Device, wgpu::Queue) {
        let limits = render_limits();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::NOOP,
            backend_options: wgpu::BackendOptions {
                noop: wgpu::NoopBackendOptions {
                    enable: true,
                    limits: Some(limits.clone()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: limits,
            ..Default::default()
        }))
        .unwrap()
    }

    /// Pixels a frame of `work` writes at least once, from the renderer's counters: the
    /// shadow cascades drawn, the scene's target (before overdraw), the bloom chain, the
    /// tonemap; HUD quads not counted. A frame that keeps the world writes the tonemap.
    fn pixels_written(s: &RenderStats, work: FrameWork) -> u64 {
        let px = |(w, h): (u32, u32)| u64::from(w) * u64::from(h);
        let tonemap = px(s.surface_size);
        match work {
            FrameWork::Skip => 0,
            FrameWork::Ui => tonemap,
            FrameWork::World => {
                let shadow = u64::from(s.cascades_drawn) * px((s.shadow_size, s.shadow_size));
                // Level k is the target over 2^(k+1), drawn down then up the chain.
                let bloom: u64 = (0..s.bloom_levels)
                    .map(|k| 2 * px((s.render_size.0 >> (k + 1), s.render_size.1 >> (k + 1))))
                    .sum();
                shadow + px(s.render_size) + bloom + tonemap
            }
        }
    }

    /// What one phase of [`idle_frames_draw_the_world_once`] drew.
    #[derive(Debug, Default, PartialEq)]
    struct Drawn {
        world: u32,
        ui: u32,
        skip: u32,
        /// Pixels written over the run ([`pixels_written`]).
        pixels: u64,
    }

    /// `frames` frames at 90 Hz of the scene `query` the way the shells draw them (App,
    /// then `Renderer::draw` on the no-op device at 1280×800), after at least `settle`
    /// frames run but not drawn (and until nothing is left to stream); `setup` runs on the
    /// app first and `hold` is held throughout.
    /// Returns what each frame drew and the mean CPU time of `Renderer::draw`'s plan on the
    /// frames that drew nothing.
    fn draw_frames(
        query: &str,
        setup: impl FnOnce(&mut App),
        hold: Option<Key>,
        settle: u32,
        frames: u32,
    ) -> (Drawn, Duration) {
        let (device, queue) = noop_device();
        let mut app = App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let size = (1280, 800);
        let mut r =
            render::Renderer::with_options(&device, &queue, format, size, app.renderer_options());
        for m in app.meshes() {
            r.add_mesh(m);
        }
        r.reserve_instances(app.max_instances());
        setup(&mut app);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let mut mapper = InputMapper::default();
        if let Some(key) = hold {
            mapper.press(RawInput::Key(key));
        }
        let mut t = 0.0;
        let mut drawn = Drawn::default();
        let mut skip_time = Duration::ZERO;
        let mut settled = 0;
        while settled < settle || !app.stream_idle() {
            t += 1.0 / 90.0;
            app.frame(&mut mapper, 1.0 / 90.0, &|| t);
            app.sync_chunks(&mut r);
            for m in app.take_new_meshes() {
                r.add_mesh(&m);
            }
            settled += 1;
        }
        for _ in 0..frames {
            t += 1.0 / 90.0;
            let steps = app.frame(&mut mapper, 1.0 / 90.0, &|| t);
            app.sync_chunks(&mut r);
            for m in app.take_new_meshes() {
                r.add_mesh(&m);
            }
            let scene = app.scene_view(steps.alpha, 1.6);
            let started = Instant::now();
            let acquire = || Some(((), target.create_view(&Default::default())));
            let work = r.draw(&scene, acquire).0;
            drawn.pixels += pixels_written(&r.stats(), work);
            match work {
                FrameWork::World => drawn.world += 1,
                FrameWork::Ui => drawn.ui += 1,
                FrameWork::Skip => {
                    drawn.skip += 1;
                    skip_time += started.elapsed();
                }
            }
            let _ = device.poll(wgpu::PollType::Poll);
        }
        assert_eq!(
            u64::from(drawn.world),
            r.stats().frames,
            "world passes recorded"
        );
        let plan = skip_time / drawn.skip.max(1);
        (drawn, plan)
    }

    /// The idle path, through the app and the renderer's recording: the world is drawn
    /// once over 120 paused frames and once over 120 frames of a still scene (the car at
    /// rest, the clock stopped, no traffic), and on every frame while driving.
    #[test]
    fn idle_frames_draw_the_world_once() {
        // The test town, built at load; the clock fixed at noon; no traffic.
        const STILL: &str = "?stream=0&t=12:00&traffic=off";
        // Three seconds for the car to settle on its springs, not drawn.
        const SETTLE: u32 = 270;
        let pause = |app: &mut App| app.open_menu(app::ui::Page::Pause);
        let (paused, _) = draw_frames(STILL, pause, None, SETTLE, 120);
        eprintln!("paused: {paused:?}");
        assert_eq!(paused.world, 1, "{paused:?}");
        let (still, plan) = draw_frames(STILL, |_| {}, None, SETTLE, 120);
        eprintln!("still: {still:?}, plan {plan:?} a frame drawn nothing");
        assert_eq!(still.world, 1, "{still:?}");
        let (driving, _) = draw_frames(STILL, |_| {}, Some(Key::KeyW), 0, 120);
        eprintln!("driving: {driving:?}");
        assert_eq!(driving.world, 120, "{driving:?}");
    }

    /// What deciding to draw nothing costs the CPU on the streamed coast (seed 1, the
    /// start, clock and traffic stopped): `Renderer::plan` comparing the scene's instances
    /// and lights with the last world frame's and building the HUD's quads. Run with
    /// `cargo test --release -p platform-native -- --ignored --nocapture idle_cost`;
    /// `docs/PERFORMANCE.md` quotes it.
    #[test]
    #[ignore = "benchmark; prints numbers"]
    fn idle_cost_on_the_coast() {
        let query = "?seed=1&place=coast&t=17:40&traffic=off";
        let (drawn, plan) = draw_frames(query, |_| {}, None, 270, 600);
        eprintln!("coast, still: {drawn:?}; plan {plan:?} a frame drawn nothing");
        let pause = |app: &mut App| app.open_menu(app::ui::Page::Pause);
        let query = "?seed=1&place=coast&t=17:40&traffic=busy";
        let (drawn, plan) = draw_frames(query, pause, None, 270, 600);
        eprintln!("coast, busy traffic, paused: {drawn:?}; plan {plan:?} a frame drawn nothing");
    }

    /// The frame-rate cap as the shell sleeps to it, for two seconds at each cap of a
    /// 90 Hz and a 60 Hz panel: the median time between frames within 2% of the cap's
    /// interval and about as many frames as it allows. Frames more than 10% off are
    /// printed, not failed: on a loaded test machine a scheduler stall makes one late and
    /// the grid's catch-up the next one early.
    #[test]
    fn capped_frames_are_evenly_paced() {
        for (hz, rate) in [
            (90.0, FrameRate::Half),
            (90.0, FrameRate::Third),
            (60.0, FrameRate::Half),
        ] {
            let interval = app::pacing::frame_interval(rate, hz, true).unwrap();
            let start = Instant::now();
            let now = || start.elapsed().as_secs_f64();
            let mut pacer = Pacer::default();
            let mut times = Vec::with_capacity(128);
            while now() < 2.0 {
                sleep_until_due(&mut pacer, interval, now);
                times.push(now());
                // Some work, as a frame would do.
                std::thread::sleep(Duration::from_micros(500));
            }
            let mut gaps: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
            let off: Vec<&f64> = gaps
                .iter()
                .filter(|dt| (*dt - interval).abs() > 0.1 * interval)
                .collect();
            eprintln!(
                "{hz} Hz {rate:?}: {} frames, more than 10% off: {off:?}",
                times.len()
            );
            gaps.sort_by(f64::total_cmp);
            let median = gaps[gaps.len() / 2];
            assert!(
                (median - interval).abs() <= 0.02 * interval,
                "{hz} Hz {rate:?}: median {median}"
            );
            let expected = 2.0 / interval;
            assert!(
                (times.len() as f64 - expected).abs() <= 0.1 * expected,
                "{hz} Hz {rate:?}: {} frames",
                times.len()
            );
        }
    }

    /// The native shell's device limits, with room for full-size cascades.
    fn render_limits() -> wgpu::Limits {
        wgpu::Limits {
            max_texture_dimension_2d: 16384,
            ..wgpu::Limits::downlevel_webgl2_defaults()
        }
    }
}
