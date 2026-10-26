use std::cell::{Cell, RefCell};
use std::rc::Rc;

use app::{pad_table, App, InputMapper, Key, RawInput, MOUSE_BUTTONS, MOUSE_RIGHT};
use render::Renderer;
use skywave_core::Seed;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    console, AddEventListenerOptions, Gamepad, GamepadButton, GamepadEvent, GamepadMappingType,
    HtmlCanvasElement, KeyboardEvent, MouseEvent, WheelEvent,
};

struct State {
    canvas: HtmlCanvasElement,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    app: App,
    input: InputMapper,
    /// `Gamepad.index` of "the" pad (see `crate::choose_pad`).
    pad: Option<u32>,
    /// False once `getGamepads()` has thrown (no Gamepad API, insecure context, or a
    /// permissions policy): polling stops.
    pads_ok: bool,
    last_ms: Option<f64>,
    backend: &'static str,
    on_first_frame: Option<js_sys::Function>,
}

fn js_err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// Starts the game on `canvas`. Resolves once the first frame is scheduled; rejects with a
/// readable message if no WebGPU or WebGL2 adapter is available. `on_first_frame` is called
/// with the backend name ("WebGPU" or "WebGL2") after the first frame is presented.
#[wasm_bindgen]
pub async fn run(
    canvas: HtmlCanvasElement,
    on_first_frame: js_sys::Function,
) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    // Ignore the error if a logger is already installed (run called twice).
    let _ = console_log::init_with_level(log::Level::Warn);

    let window = web_sys::window().ok_or("no window")?;
    let query = window.location().search()?;
    let seed = crate::seed_from_query(&query).unwrap_or(1);

    let instance = wgpu::util::new_instance_with_webgpu_detection(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    })
    .await;
    let (width, height) = canvas_pixel_size(&window, &canvas, 8192);
    canvas.set_width(width);
    canvas.set_height(height);
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
        .map_err(js_err)?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
        .map_err(|e| js_err(format!("no WebGPU or WebGL2 adapter: {e}")))?;
    let backend = match adapter.get_info().backend {
        wgpu::Backend::BrowserWebGpu => "WebGPU",
        wgpu::Backend::Gl => "WebGL2",
        _ => "unknown",
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("skywave"),
            // Never exceed WebGL2 limits, whichever backend we got.
            required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                .using_resolution(adapter.limits()),
            ..Default::default()
        })
        .await
        .map_err(js_err)?;

    let max_dim = device.limits().max_texture_dimension_2d;
    let (width, height) = canvas_pixel_size(&window, &canvas, max_dim);
    let mut config = surface
        .get_default_config(&adapter, width, height)
        .ok_or("surface is not supported by the adapter")?;
    let caps = surface.get_capabilities(&adapter);
    if let Some(srgb) = caps.formats.iter().find(|f| f.is_srgb()) {
        config.format = *srgb;
    }
    surface.configure(&device, &config);

    let car = crate::query_param(&query, "car").unwrap_or(app::DEFAULT_VEHICLE);
    let mut app = App::new_with(Seed(seed), car).unwrap_or_else(|e| {
        log::warn!("skywave: {e}; driving the {}", app::DEFAULT_VEHICLE);
        App::new(Seed(seed))
    });
    if let Some(mode) = crate::query_param(&query, "cam").and_then(app::CameraMode::from_name) {
        app.set_camera_mode(mode);
    }
    app.set_fly(crate::query_param(&query, "fly") == Some("1"));
    let mut renderer = Renderer::new(&device, &queue, config.format, (width, height));
    for mesh in app.meshes() {
        renderer.add_mesh(mesh);
    }
    log::info!("skywave: {backend}, {:?}, seed {seed}", config.format);

    let state = Rc::new(RefCell::new(State {
        canvas,
        surface,
        device,
        queue,
        config,
        renderer,
        app,
        input: InputMapper::default(),
        pad: None,
        pads_ok: true,
        last_ms: None,
        backend,
        on_first_frame: Some(on_first_frame),
    }));
    listen_keys(&window, &state)?;
    listen_mouse(&window, &state)?;
    listen_gamepads(&window)?;
    start_frame_loop(&window, state)?;
    Ok(())
}

fn canvas_pixel_size(window: &web_sys::Window, canvas: &HtmlCanvasElement, max: u32) -> (u32, u32) {
    let dpr = window.device_pixel_ratio();
    let w = (f64::from(canvas.client_width()) * dpr).round() as u32;
    let h = (f64::from(canvas.client_height()) * dpr).round() as u32;
    (w.clamp(1, max), h.clamp(1, max))
}

fn listen_keys(window: &web_sys::Window, state: &Rc<RefCell<State>>) -> Result<(), JsValue> {
    for (event, down) in [("keydown", true), ("keyup", false)] {
        let state = state.clone();
        let handler = Closure::<dyn FnMut(KeyboardEvent)>::new(move |e: KeyboardEvent| {
            if let Some(key) = Key::from_code(&e.code()) {
                let raw = RawInput::Key(key);
                let input = &mut state.borrow_mut().input;
                // Keep browser shortcuts (Ctrl+R, F5, ...) unless the game uses the key.
                if input.bindings().is_bound(raw) && !e.ctrl_key() && !e.meta_key() {
                    e.prevent_default();
                }
                if down {
                    input.press(raw);
                } else {
                    input.release(raw);
                }
            }
        });
        window.add_event_listener_with_callback(event, handler.as_ref().unchecked_ref())?;
        // The listener lives as long as the page.
        handler.forget();
    }
    let state = state.clone();
    let blur = Closure::<dyn FnMut()>::new(move || state.borrow_mut().input.release_all());
    window.add_event_listener_with_callback("blur", blur.as_ref().unchecked_ref())?;
    blur.forget();
    Ok(())
}

/// A DOM `MouseEvent.button` the mapper tracks.
fn dom_button(button: i16) -> Option<u8> {
    u8::try_from(button).ok().filter(|&b| b < MOUSE_BUTTONS)
}

/// Asks for pointer lock on `canvas`. Newer browsers return a promise that rejects when
/// they refuse; `ignore` swallows that so a refusal is not an uncaught error. Browsers
/// without pointer lock are skipped.
fn request_pointer_lock(canvas: &HtmlCanvasElement, ignore: &Closure<dyn FnMut(JsValue)>) {
    let Ok(request) = js_sys::Reflect::get(canvas, &JsValue::from_str("requestPointerLock")) else {
        return;
    };
    let Ok(request) = request.dyn_into::<js_sys::Function>() else {
        return;
    };
    if let Ok(result) = request.call0(canvas) {
        if let Ok(promise) = result.dyn_into::<js_sys::Promise>() {
            let _ = promise.catch(ignore);
        }
    }
}

fn exit_pointer_lock() {
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        if document.pointer_lock_element().is_some() {
            document.exit_pointer_lock();
        }
    }
}

/// Mouse buttons, motion and wheel. The right button on the canvas holds pointer lock for
/// mouse look; it is released with the button. Motion and button-up are read on the
/// window, so a drag that leaves the canvas still ends.
fn listen_mouse(window: &web_sys::Window, state: &Rc<RefCell<State>>) -> Result<(), JsValue> {
    let canvas = state.borrow().canvas.clone();
    // Every listener below lives as long as the page.
    let ignore = Closure::<dyn FnMut(JsValue)>::new(|_: JsValue| {});

    let down = {
        let state = state.clone();
        let canvas = canvas.clone();
        Closure::<dyn FnMut(MouseEvent)>::new(move |e: MouseEvent| {
            let Some(b) = dom_button(e.button()) else {
                return;
            };
            if b != 0 {
                // No middle-click autoscroll or other default actions over the game.
                e.prevent_default();
            }
            state.borrow_mut().input.press(RawInput::MouseButton(b));
            if b == MOUSE_RIGHT {
                request_pointer_lock(&canvas, &ignore);
            }
        })
    };
    canvas.add_event_listener_with_callback("mousedown", down.as_ref().unchecked_ref())?;
    down.forget();

    let up = {
        let state = state.clone();
        Closure::<dyn FnMut(MouseEvent)>::new(move |e: MouseEvent| {
            let Some(b) = dom_button(e.button()) else {
                return;
            };
            state.borrow_mut().input.release(RawInput::MouseButton(b));
            if b == MOUSE_RIGHT {
                exit_pointer_lock();
            }
        })
    };
    window.add_event_listener_with_callback("mouseup", up.as_ref().unchecked_ref())?;
    up.forget();

    let motion = {
        let state = state.clone();
        Closure::<dyn FnMut(MouseEvent)>::new(move |e: MouseEvent| {
            let (dx, dy) = (e.movement_x() as f32, e.movement_y() as f32);
            state.borrow_mut().input.mouse_delta(dx, dy);
        })
    };
    window.add_event_listener_with_callback("mousemove", motion.as_ref().unchecked_ref())?;
    motion.forget();

    let wheel = {
        let state = state.clone();
        Closure::<dyn FnMut(WheelEvent)>::new(move |e: WheelEvent| {
            let notches = crate::wheel_notches(e.delta_y(), e.delta_mode());
            state.borrow_mut().input.axis(RawInput::MouseWheel, notches);
        })
    };
    let passive = AddEventListenerOptions::new();
    passive.set_passive(true);
    canvas.add_event_listener_with_callback_and_add_event_listener_options(
        "wheel",
        wheel.as_ref().unchecked_ref(),
        &passive,
    )?;
    wheel.forget();

    // The right button is mouse look, not a context menu.
    let menu = Closure::<dyn FnMut(web_sys::Event)>::new(|e: web_sys::Event| e.prevent_default());
    canvas.add_event_listener_with_callback("contextmenu", menu.as_ref().unchecked_ref())?;
    menu.forget();

    let logged = Cell::new(false);
    let refused = Closure::<dyn FnMut()>::new(move || {
        if !logged.replace(true) {
            console::info_1(&JsValue::from_str(
                "skywave: pointer lock refused; mouse look keeps a free cursor",
            ));
        }
    });
    if let Some(document) = window.document() {
        document.add_event_listener_with_callback(
            "pointerlockerror",
            refused.as_ref().unchecked_ref(),
        )?;
    }
    refused.forget();
    Ok(())
}

/// Logs gamepads as they come and go; the state itself is polled every frame.
fn listen_gamepads(window: &web_sys::Window) -> Result<(), JsValue> {
    for (event, what) in [
        ("gamepadconnected", "connected"),
        ("gamepaddisconnected", "disconnected"),
    ] {
        let handler = Closure::<dyn FnMut(GamepadEvent)>::new(move |e: GamepadEvent| {
            let Some(pad) = e.gamepad() else {
                return;
            };
            let mapping = if pad.mapping() == GamepadMappingType::Standard {
                "standard mapping"
            } else {
                "non-standard mapping, used only without a standard pad"
            };
            console::info_1(&JsValue::from_str(&format!(
                "skywave: gamepad {} {what}: {:?} ({mapping})",
                pad.index(),
                pad.id()
            )));
        });
        window.add_event_listener_with_callback(event, handler.as_ref().unchecked_ref())?;
        handler.forget();
    }
    Ok(())
}

type FrameCallback = Closure<dyn FnMut(f64)>;

fn start_frame_loop(window: &web_sys::Window, state: Rc<RefCell<State>>) -> Result<(), JsValue> {
    let callback: Rc<RefCell<Option<FrameCallback>>> = Rc::new(RefCell::new(None));
    let next = callback.clone();
    *callback.borrow_mut() = Some(Closure::new(move |now_ms: f64| {
        state.borrow_mut().frame(now_ms);
        if let (Some(w), Some(cb)) = (web_sys::window(), next.borrow().as_ref()) {
            let _ = w.request_animation_frame(cb.as_ref().unchecked_ref());
        }
    }));
    let first = callback.borrow();
    window.request_animation_frame(
        first
            .as_ref()
            .ok_or("no frame callback")?
            .as_ref()
            .unchecked_ref(),
    )?;
    Ok(())
}

impl State {
    /// Reads "the" gamepad into the mapper: every standard-mapping button (with the
    /// triggers' analog values) and stick axis, as absolute state. Nothing is fed while the
    /// page is not focused (the blur handler released everything).
    fn poll_gamepads(&mut self, window: &web_sys::Window) {
        if !self.pads_ok {
            return;
        }
        let pads = match window.navigator().get_gamepads() {
            Ok(pads) => pads,
            Err(_) => {
                self.pads_ok = false;
                console::info_1(&JsValue::from_str(
                    "skywave: gamepads are unavailable in this browser context",
                ));
                return;
            }
        };
        let connected = || {
            pads.iter()
                .filter_map(|v| v.dyn_into::<Gamepad>().ok())
                .filter(|p| p.connected())
        };
        let chosen = crate::choose_pad(
            self.pad,
            connected().map(|p| (p.index(), p.mapping() == GamepadMappingType::Standard)),
        );
        if chosen != self.pad {
            pad_table::release_gamepad(&mut self.input);
            self.pad = chosen;
        }
        let Some(index) = chosen else {
            return;
        };
        let focused = window
            .document()
            .and_then(|d| d.has_focus().ok())
            .unwrap_or(true);
        if !focused {
            return;
        }
        let Some(pad) = connected().find(|p| p.index() == index) else {
            return;
        };
        for (i, b) in pad.buttons().iter().enumerate() {
            if let Ok(b) = b.dyn_into::<GamepadButton>() {
                pad_table::feed_button(&mut self.input, i, b.value() as f32, b.pressed());
            }
        }
        for (i, v) in pad.axes().iter().enumerate() {
            pad_table::feed_axis(&mut self.input, i, v.as_f64().unwrap_or(0.0) as f32);
        }
    }

    fn frame(&mut self, now_ms: f64) {
        let dt = self.last_ms.map_or(0.0, |last| (now_ms - last) / 1000.0);
        self.last_ms = Some(now_ms);

        if let Some(window) = web_sys::window() {
            let max = self.device.limits().max_texture_dimension_2d;
            let (w, h) = canvas_pixel_size(&window, &self.canvas, max);
            if (w, h) != (self.config.width, self.config.height) {
                self.canvas.set_width(w);
                self.canvas.set_height(h);
                self.config.width = w;
                self.config.height = h;
                self.surface.configure(&self.device, &self.config);
                self.renderer.resize((w, h));
            }
            self.poll_gamepads(&window);
        }

        let steps = self.app.update(self.input.sample(dt as f32), dt);
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
        self.renderer
            .render(&view, &self.app.scene_view(steps.alpha, aspect));
        self.queue.present(frame);

        if let Some(cb) = self.on_first_frame.take() {
            let _ = cb.call1(&JsValue::NULL, &JsValue::from_str(self.backend));
        }
    }
}
