use std::cell::RefCell;
use std::rc::Rc;

use app::{App, Key, Keyboard};
use render::Renderer;
use skywave_core::Seed;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, KeyboardEvent};

struct State {
    canvas: HtmlCanvasElement,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    app: App,
    keys: Keyboard,
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
    let seed = crate::seed_from_query(&window.location().search()?).unwrap_or(1);

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

    let app = App::new(Seed(seed));
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
        keys: Keyboard::default(),
        last_ms: None,
        backend,
        on_first_frame: Some(on_first_frame),
    }));
    listen_keys(&window, &state)?;
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
                e.prevent_default();
                state.borrow_mut().keys.set(key, down);
            }
        });
        window.add_event_listener_with_callback(event, handler.as_ref().unchecked_ref())?;
        // The listener lives as long as the page.
        handler.forget();
    }
    let state = state.clone();
    let blur = Closure::<dyn FnMut()>::new(move || state.borrow_mut().keys.release_all());
    window.add_event_listener_with_callback("blur", blur.as_ref().unchecked_ref())?;
    blur.forget();
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
        }

        let steps = self.app.update(&self.keys.input(), dt);
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
