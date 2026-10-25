//! `skywave`: the desktop shell. Opens a window, sets up wgpu, maps keys to game actions
//! and drives the platform-independent `App` once per redraw.
//!
//! Usage: `skywave [--seed N] [--stats]`

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use app::{App, Key, Keyboard};
use render::Renderer;
use skywave_core::Seed;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

#[derive(Debug, PartialEq)]
struct Options {
    seed: u64,
    stats: bool,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut opts = Options {
        seed: 1,
        stats: false,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--stats" => opts.stats = true,
            "--seed" => {
                let v = args.next().ok_or("--seed needs a value")?;
                opts.seed = v.parse().map_err(|_| format!("bad seed: {v}"))?;
            }
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(opts)
}

fn map_key(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::KeyW => Key::KeyW,
        KeyCode::KeyA => Key::KeyA,
        KeyCode::KeyS => Key::KeyS,
        KeyCode::KeyD => Key::KeyD,
        KeyCode::KeyE => Key::KeyE,
        KeyCode::KeyQ => Key::KeyQ,
        KeyCode::Space => Key::Space,
        KeyCode::ShiftLeft => Key::ShiftLeft,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        _ => return None,
    })
}

/// Frame-time statistics, printed to stderr once per second with `--stats`.
#[derive(Default)]
struct Stats {
    frames: u32,
    total: Duration,
    min: Option<Duration>,
    max: Duration,
}

impl Stats {
    fn record(&mut self, dt: Duration) -> Option<String> {
        self.frames += 1;
        self.total += dt;
        self.min = Some(self.min.map_or(dt, |m| m.min(dt)));
        self.max = self.max.max(dt);
        if self.total < Duration::from_secs(1) {
            return None;
        }
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let line = format!(
            "frames {:4}  avg {:6.2} ms  min {:6.2} ms  max {:6.2} ms",
            self.frames,
            ms(self.total) / f64::from(self.frames),
            ms(self.min.unwrap_or_default()),
            ms(self.max),
        );
        *self = Stats::default();
        Some(line)
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
    fn new(event_loop: &ActiveEventLoop, app: &App) -> Result<Gpu, String> {
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
        log::info!("adapter: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("skywave"),
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
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        let mut renderer = Renderer::new(
            &device,
            &queue,
            config.format,
            (config.width, config.height),
        );
        for mesh in app.meshes() {
            renderer.add_mesh(mesh);
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

    fn draw(&mut self, app: &App, alpha: f32) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            other => {
                log::warn!("skipping frame: {other:?}");
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let aspect = self.config.width as f32 / self.config.height as f32;
        self.renderer.render(&view, &app.scene_view(alpha, aspect));
        self.window.pre_present_notify();
        self.queue.present(frame);
    }
}

struct Shell {
    opts: Options,
    app: App,
    keys: Keyboard,
    gpu: Option<Gpu>,
    last_frame: Option<Instant>,
    stats: Stats,
    error: Option<String>,
}

impl ApplicationHandler for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        match Gpu::new(event_loop, &self.app) {
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
            WindowEvent::Focused(false) => self.keys.release_all(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if code == KeyCode::Escape {
                        event_loop.exit();
                    } else if let Some(key) = map_key(code) {
                        self.keys.set(key, event.state == ElementState::Pressed);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = self.last_frame.map_or(Duration::ZERO, |t| now - t);
                self.last_frame = Some(now);
                let steps = self.app.update(&self.keys.input(), dt.as_secs_f64());
                if let Some(gpu) = &mut self.gpu {
                    gpu.draw(&self.app, steps.alpha);
                }
                if self.opts.stats && !dt.is_zero() {
                    if let Some(line) = self.stats.record(dt) {
                        eprintln!("{line}");
                    }
                }
            }
            _ => {}
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
            eprintln!("usage: skywave [--seed N] [--stats]");
            return if e.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            };
        }
    };
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("skywave: cannot open a window: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut shell = Shell {
        app: App::new(Seed(opts.seed)),
        opts,
        keys: Keyboard::default(),
        gpu: None,
        last_frame: None,
        stats: Stats::default(),
        error: None,
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
        assert_eq!(
            args(&[]),
            Ok(Options {
                seed: 1,
                stats: false
            })
        );
        assert_eq!(
            args(&["--stats", "--seed", "42"]),
            Ok(Options {
                seed: 42,
                stats: true
            })
        );
        assert!(args(&["--seed"]).is_err());
        assert!(args(&["--seed", "x"]).is_err());
        assert!(args(&["--fast"]).is_err());
        assert_eq!(args(&["--help"]), Err(String::new()));
    }

    #[test]
    fn every_game_key_has_a_winit_code() {
        let codes = [
            KeyCode::KeyW,
            KeyCode::KeyA,
            KeyCode::KeyS,
            KeyCode::KeyD,
            KeyCode::KeyE,
            KeyCode::KeyQ,
            KeyCode::Space,
            KeyCode::ShiftLeft,
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
        ];
        assert_eq!(codes.len(), Key::COUNT);
        for code in codes {
            // winit's KeyCode names are the DOM `code` names the web build uses.
            assert_eq!(map_key(code), Key::from_code(&format!("{code:?}")));
            assert!(map_key(code).is_some());
        }
        assert_eq!(map_key(KeyCode::KeyZ), None);
    }

    #[test]
    fn stats_report_once_per_second() {
        let mut s = Stats::default();
        let mut lines = 0;
        for _ in 0..240 {
            if s.record(Duration::from_millis(10)).is_some() {
                lines += 1;
            }
        }
        assert_eq!(lines, 2);
    }
}
