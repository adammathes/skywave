//! `skywave`: the desktop shell. Opens a window, sets up wgpu, feeds keyboard, mouse and
//! gamepad input to the input mapper and drives the platform-independent `App` once per
//! redraw.
//!
//! Mouse look: while the right button is held the cursor is hidden and locked (or confined
//! to the window where the platform cannot lock it, e.g. X11 and Windows); it is released
//! with the button or when the window loses focus. Platforms that refuse both just keep a
//! free cursor; the raw motion still drives the look.
//!
//! Usage: `skywave [--seed N] [--car ID] [--scene KEY=VALUE]... [--stats] [--frames N]
//! [--exit] [--no-vsync] [--quality NAME] [--no-cull] [--version]`.
//! `--scene` sets one key of the starting scene (`app::scene`: `t=22:30`, `cam=cockpit`,
//! `pos=-92,4.9`, `yaw=90`, `lights=1`, ...); `--seed N` and `--car ID` are
//! `--scene seed=N` and `--scene car=ID`. `--stats` prints the adapter, the surface and a
//! line of frame statistics every second to stderr (format in `docs/PERFORMANCE.md`), with
//! GPU time per pass where the adapter has timestamp queries; it also prints the detected
//! gamepads at startup and as they connect (otherwise the info log has them,
//! `RUST_LOG=info`). `--frames N` prints a summary after N frames, and with `--exit` quits
//! there (scripted benchmarks). `--no-vsync` presents without waiting for the display
//! (`AutoNoVsync`; the default is `AutoVsync`). `--quality low|deck|medium|high|ultra`
//! forces a graphics preset for the session (not saved); `--no-cull` turns instance culling
//! off (for comparisons). `--version` prints the build's `git describe` (see `build.rs`)
//! and exits.
//!
//! The town streams round the camera within `app::stream::StreamBudget::NATIVE` (13×13
//! chunks, 4 ms a sim step); `--scene stream=0` builds it all at load instead.
//!
//! Esc / Start opens the pause menu; settings and bindings are saved to
//! `$XDG_CONFIG_HOME/skywave/settings.json` (see `settings`).
//!
//! Sound plays through the default output device at 48 kHz (`sfx`); without one the game
//! runs silent.

mod pad;
mod settings;
mod sfx;

use std::fmt::Write as _;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use app::{App, GpuInfo, InputMapper, Key, Quality, RawInput, SceneSetup, MOUSE_RIGHT};
use pad::Pads;
use render::{RenderStats, Renderer, GPU_PASS_NAMES};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// Pixel-precise scrolling (touchpads) counts this many pixels as one wheel notch, the
/// same figure the web build uses.
const PIXELS_PER_NOTCH: f64 = 100.0;

/// Frames left out of the `--frames` summary while the town streams in and caches warm.
const SUMMARY_WARMUP_FRAMES: usize = 60;

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
                    format!("--quality is one of low, deck, medium, high, ultra, not `{v}`")
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
    if r.gpu_timing {
        let total: u32 = r.gpu_us.iter().sum();
        let _ = write!(s, " | gpu_ms total={:.2}", total as f32 / 1000.0);
        for (name, us) in GPU_PASS_NAMES.iter().zip(r.gpu_us) {
            let _ = write!(s, " {name}={:.2}", us as f32 / 1000.0);
        }
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
}

impl Stats {
    fn new(frames: Option<u32>) -> Stats {
        Stats {
            frame_ms: Vec::with_capacity(1024),
            cpu_ms: Vec::with_capacity(1024),
            run: Vec::with_capacity(frames.map_or(0, |n| n as usize)),
            ..Stats::default()
        }
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
        // GPU time per pass, for `--stats` only.
        let timestamps = wgpu::Features::TIMESTAMP_QUERY;
        let features = if opts.stats && adapter.features().contains(timestamps) {
            timestamps
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
                "quality: {} {:?} culling {}",
                app.ui().quality().name(),
                options.quality,
                if options.culling { "on" } else { "off" },
            );
        }
        let mut renderer = Renderer::with_options(
            &device,
            &queue,
            config.format,
            (config.width, config.height),
            options,
        );
        for mesh in app.meshes() {
            renderer.add_mesh(mesh);
        }
        renderer.reserve_instances(app.max_instances());
        let map = app.map_image();
        renderer.upload_map(map.w, map.h, &map.rgba);
        if opts.stats {
            let on = renderer.enable_gpu_timing();
            eprintln!(
                "gpu timing: {}",
                if on {
                    "timestamp queries per pass"
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
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.renderer.resize((width, height));
    }

    /// Draws a frame; returns the CPU time spent building and submitting it (the scene
    /// view and the renderer, not waiting for the swapchain).
    fn draw(&mut self, app: &mut App, alpha: f32) -> Duration {
        app.sync_chunks(&mut self.renderer);
        // A car driven for the first time this session (and the saved one, at startup).
        for mesh in app.take_new_meshes() {
            self.renderer.add_mesh(&mesh);
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Duration::ZERO;
            }
            other => {
                log::warn!("skipping frame: {other:?}");
                return Duration::ZERO;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let aspect = self.config.width as f32 / self.config.height as f32;
        let start = Instant::now();
        self.renderer.render(&view, &app.scene_view(alpha, aspect));
        let cpu = start.elapsed();
        self.window.pre_present_notify();
        self.queue.present(frame);
        cpu
    }

    fn stats(&self) -> render::RenderStats {
        self.renderer.stats()
    }
}

struct Shell {
    opts: Options,
    app: App,
    input: InputMapper,
    pads: Pads,
    /// Window focus: pads and mouse motion only count while focused.
    focused: bool,
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
}

impl Shell {
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
            }
            WindowEvent::Focused(true) => {
                self.focused = true;
                self.pads.resync(&mut self.input);
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
                let now = Instant::now();
                let dt = self.last_frame.map_or(Duration::ZERO, |t| now - t);
                self.last_frame = Some(now);
                self.pads.poll(&mut self.input, self.focused);
                let clock = || self.started.elapsed().as_secs_f64();
                let steps = self.app.frame(&mut self.input, dt.as_secs_f64(), &clock);
                if let Some(sfx) = &self.sfx {
                    sfx.set_params(self.app.audio_params());
                }
                if self.app.quit_requested() {
                    event_loop.exit();
                    return;
                }
                let Some(gpu) = &mut self.gpu else {
                    return;
                };
                let cpu = gpu.draw(&mut self.app, steps.alpha);
                let render = gpu.stats();
                self.app.set_render_stats(render);
                self.app.set_cpu_submit_ms(cpu.as_secs_f32() * 1000.0);
                self.frames_drawn += 1;
                let keep = self.opts.frames.is_some();
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

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(gpu) = &self.gpu {
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
                "usage: skywave [--seed N] [--car ID] [--scene KEY=VALUE]... [--stats] \
                 [--frames N [--exit]] [--no-vsync] [--quality low|deck|medium|high|ultra] \
                 [--no-cull] [--version]"
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
    let mut app = match App::from_setup(&opts.scene) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("skywave: {e}");
            return ExitCode::from(2);
        }
    };
    app.set_can_quit(true);
    app.set_stream_budget(app::stream::StreamBudget::NATIVE);
    app.set_settings_store(Box::new(settings::FileStore::from_env()));
    let opts_frames = opts.frames;
    let mut shell = Shell {
        app,
        opts,
        input: InputMapper::default(),
        pads,
        focused: true,
        grabbed: false,
        gpu: None,
        last_frame: None,
        started: Instant::now(),
        stats: Stats::new(opts_frames),
        frames_drawn: 0,
        error: None,
        sfx: sfx::Sfx::start(),
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

    fn args(list: &[&str]) -> Result<Options, String> {
        parse_args(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_arguments() {
        assert_eq!(args(&[]), Ok(Options::default()));
        let mut scene = SceneSetup::from_query("seed=42&car=palisade_wagon").unwrap();
        assert_eq!(
            args(&["--stats", "--seed", "42", "--car", "palisade_wagon"]),
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
                "car=palisade_wagon",
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
            "--scene",
            "place=coast",
        ])
        .unwrap();
        assert!(o.stats && o.exit && o.no_vsync && o.no_cull);
        assert_eq!(o.frames, Some(600));
        assert_eq!(o.quality, Some(Quality::Deck));
        assert!(args(&["--frames"]).is_err());
        assert!(args(&["--frames", "0"]).is_err());
        assert!(args(&["--frames", "x"]).is_err());
        assert!(args(&["--exit"]).is_err(), "--exit needs --frames");
        assert!(args(&["--quality", "potato"]).is_err());
        assert!(!args(&["--frames", "3"]).unwrap().exit);
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

    /// Prints what culling and each preset leave the GPU on the streamed coast at the
    /// native budget (instances and triangles per pass, from the renderer's stats on
    /// wgpu's no-op device) and what culling costs the CPU, without traffic and with busy
    /// traffic. Run with
    /// `cargo test --release -p platform-native -- --ignored --nocapture culling_on_the_coast`;
    /// `docs/PERFORMANCE.md` quotes it. Under `SKYWAVE_ENFORCE_BUDGETS` it checks the Deck
    /// and Medium presets with busy traffic against the budget there ([`COAST_BUDGET`]).
    #[test]
    #[ignore = "benchmark; prints numbers"]
    fn culling_on_the_coast() {
        for traffic in ["off", "busy"] {
            culling_on_the_coast_with(traffic);
        }
    }

    /// Deck and Medium on the coast with busy traffic: at most this many draws, forward
    /// and shadow triangles a frame (`docs/PERFORMANCE.md`).
    const COAST_BUDGET: [(Quality, u32, u64, u64); 2] = [
        (Quality::Deck, 500, 880_000, 800_000),
        (Quality::Medium, 700, 880_000, 1_400_000),
    ];

    fn culling_on_the_coast_with(traffic: &str) {
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
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: limits,
            ..Default::default()
        }))
        .unwrap();
        let query = format!("?seed=1&place=coast&t=17:40&traffic={traffic}");
        let mut app = App::from_setup(&SceneSetup::from_query(&query).unwrap()).unwrap();
        app.set_stream_budget(app::stream::StreamBudget::NATIVE);
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let size = (1280, 800);
        let mut r =
            render::Renderer::with_options(&device, &queue, format, size, app.renderer_options());
        for m in app.meshes() {
            r.add_mesh(m);
        }
        r.reserve_instances(app.max_instances());
        let mut mapper = InputMapper::default();
        let started = Instant::now();
        let clock = || started.elapsed().as_secs_f64();
        while !app.stream_idle() {
            app.frame(&mut mapper, 1.0 / 60.0, &clock);
            app.sync_chunks(&mut r);
            for m in app.take_new_meshes() {
                r.add_mesh(&m);
            }
        }
        eprintln!("traffic {traffic}: {} cars", app.traffic_count());
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
        let view = target.create_view(&Default::default());
        let runs = [
            ("high, no culling", Quality::High, false),
            ("high", Quality::High, true),
            ("medium", Quality::Medium, true),
            ("deck", Quality::Deck, true),
            ("low", Quality::Low, true),
        ];
        for (label, q, culling) in runs {
            app.force_quality(Some(q));
            r.set_culling(culling);
            let mut best = f64::INFINITY;
            let mut total = 0.0;
            // Shadow work alternates when the far cascade is kept: the most per cascade
            // over the run, and the mean triangles a frame.
            let mut shadow_max = [0; 3];
            let mut shadow_tris = 0u64;
            const FRAMES: u32 = 40;
            for _ in 0..FRAMES {
                let t = Instant::now();
                r.render(&view, &app.scene_view(1.0, 1.6));
                let dt = t.elapsed().as_secs_f64();
                best = best.min(dt);
                total += dt;
                let s = r.stats();
                for (m, n) in shadow_max.iter_mut().zip(s.shadow_instances) {
                    *m = (*m).max(n);
                }
                shadow_tris += s.shadow_triangles;
            }
            let s = r.stats();
            let budget = COAST_BUDGET.iter().find(|b| b.0 == q && culling);
            if let (Some(&(_, draws, fwd, shadow)), "busy") = (budget, traffic) {
                if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
                    let sh = shadow_tris / u64::from(FRAMES);
                    assert!(s.draw_calls <= draws, "{label}: {} draws", s.draw_calls);
                    assert!(s.triangles <= fwd, "{label}: {} triangles", s.triangles);
                    assert!(sh <= shadow, "{label}: {sh} shadow triangles");
                }
            }
            eprintln!(
                "{label:>16}: instances {} fwd {} shadow {:?} ({} cascades) | tris fwd {}k \
                 shadow {}k a frame | draws {} | cpu render() avg {:.2} ms best {:.2} ms",
                s.instances,
                s.instances_drawn,
                shadow_max,
                s.cascades,
                s.triangles / 1000,
                shadow_tris / u64::from(FRAMES) / 1000,
                s.draw_calls,
                total / f64::from(FRAMES) * 1000.0,
                best * 1000.0,
            );
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
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
