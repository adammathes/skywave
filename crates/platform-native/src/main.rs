//! `skywave`: the desktop shell. Opens a window, sets up wgpu, feeds keyboard, mouse and
//! gamepad input to the input mapper and drives the platform-independent `App` once per
//! redraw.
//!
//! Mouse look: while the right button is held the cursor is hidden and locked (or confined
//! to the window where the platform cannot lock it, e.g. X11 and Windows); it is released
//! with the button or when the window loses focus. Platforms that refuse both just keep a
//! free cursor; the raw motion still drives the look.
//!
//! Usage: `skywave [--seed N] [--stats]`. `--stats` also prints the detected gamepads at
//! startup and as they connect; otherwise that goes to the info log (`RUST_LOG=info`).

mod pad;

use std::fmt::Write as _;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use app::{Action, App, InputMapper, Key, RawInput, MOUSE_RIGHT};
use pad::Pads;
use render::Renderer;
use skywave_core::Seed;
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
    input: InputMapper,
    pads: Pads,
    /// Window focus: pads and mouse motion only count while focused.
    focused: bool,
    /// The cursor is grabbed for mouse look.
    grabbed: bool,
    gpu: Option<Gpu>,
    last_frame: Option<Instant>,
    stats: Stats,
    error: Option<String>,
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
                let input = self.input.sample(dt.as_secs_f32());
                // No pause menu yet: Pause (Esc) quits, as before.
                if input.just_pressed(Action::Pause) {
                    event_loop.exit();
                    return;
                }
                let steps = self.app.update(input, dt.as_secs_f64());
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
    let pads = Pads::new(opts.stats);
    if opts.stats {
        eprintln!("{}", pads.summary());
    } else {
        log::info!("{}", pads.summary());
    }
    let mut shell = Shell {
        app: App::new(Seed(opts.seed)),
        opts,
        input: InputMapper::default(),
        pads,
        focused: true,
        grabbed: false,
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
