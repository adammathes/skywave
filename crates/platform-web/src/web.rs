use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use app::{
    pad_table, App, InputMapper, Key, RawInput, SceneSetup, SettingsStore, MOUSE_BUTTONS,
    MOUSE_RIGHT,
};
use audio::RadioCommand;
use render::{HdrMode, MeshId, MeshInstance, Probe, Renderer, RendererOptions, SceneView};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    console, AddEventListenerOptions, Gamepad, GamepadButton, GamepadEvent, GamepadMappingType,
    HtmlCanvasElement, KeyboardEvent, MouseEvent, WheelEvent,
};

use crate::{BackendChoice, StartFailure, WebBackend};

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
    /// The last animation frame's time, processed or skipped, ms.
    last_tick_ms: Option<f64>,
    /// The time between animation frames, smoothed, ms: the display's refresh.
    refresh_ms: f64,
    /// Animation frames skipped since the last one processed (the frame-rate cap).
    skipped: u32,
    /// The streaming clock (`performance.now()`), when the page has one.
    performance: Option<web_sys::Performance>,
    /// What `window.skywaveStreamIdle` was last set to.
    stream_idle: Option<bool>,
    backend: WebBackend,
    on_first_frame: Option<js_sys::Function>,
    /// The sound sink, when the page has its glue.
    sfx: Option<crate::sfx::Sfx>,
    page: Page,
    /// What the device reported outside error scopes.
    errors: ErrorLog,
    /// The corona check's outcome has been reported.
    corona_reported: bool,
    /// The page's radio player glue, when it has one.
    radio: Option<RadioGlue>,
    /// The radio's commands of this frame (reused).
    radio_cmds: Vec<RadioCommand>,
    /// What the glue was last told the radio's state is: kind, station, live, blocked,
    /// and the deck's: source, track changes, mode, playing, live, blocked.
    radio_reported: (u8, Option<usize>, bool, bool, DeckKey),
    /// The page's music folder glue, when it has one.
    music: Option<MusicGlue>,
    /// Whether the glue last said the folders must be picked again.
    music_again: bool,
}

/// What the page is told changed about the deck: source, track changes, mode, playing,
/// live, blocked.
type DeckKey = (
    audio::deck::Source,
    u32,
    audio::deck::Mode,
    bool,
    bool,
    bool,
);

/// `window.skywaveMusic` (web/index.html): the user's music folders, picked or dropped.
/// The files stay in the page; the game only sees each file's key, path, size and first
/// bytes (for its tags), and plays a file by its key through the radio player.
struct MusicGlue {
    this: JsValue,
    pick: js_sys::Function,
    take_folder: js_sys::Function,
    forget: js_sys::Function,
}

impl MusicGlue {
    fn find(window: &web_sys::Window) -> Option<MusicGlue> {
        let this = js_sys::Reflect::get(window, &JsValue::from_str("skywaveMusic")).ok()?;
        let func = |name: &str| {
            js_sys::Reflect::get(&this, &JsValue::from_str(name))
                .ok()?
                .dyn_into::<js_sys::Function>()
                .ok()
        };
        Some(MusicGlue {
            pick: func("pick")?,
            take_folder: func("takeFolder")?,
            forget: func("forget")?,
            this,
        })
    }

    fn get(v: &JsValue, key: &str) -> JsValue {
        js_sys::Reflect::get(v, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    /// Folders picked last session could not be kept (the browser cannot store their
    /// handles, or has not let us read them again yet).
    fn again(&self) -> bool {
        Self::get(&self.this, "again").as_bool().unwrap_or(false)
    }

    /// The next folder read, whether to announce it and whether the page will remember
    /// it.
    fn take_folder(&self) -> Option<(audio::deck::Folder, bool, bool)> {
        let f = self.take_folder.call0(&self.this).ok()?;
        if f.is_null() || f.is_undefined() {
            return None;
        }
        let id = Self::get(&f, "path").as_string().unwrap_or_default();
        let name = Self::get(&f, "name").as_string().unwrap_or_default();
        let announce = Self::get(&f, "announce").as_bool().unwrap_or(false);
        let kept = Self::get(&f, "kept").as_bool().unwrap_or(false);
        let mixtape = Self::get(&f, "mixtape").as_bool().unwrap_or(false);
        let files = js_sys::Array::from(&Self::get(&f, "files"));
        let files = files
            .iter()
            .map(|file| {
                let key = Self::get(&file, "key").as_string().unwrap_or_default();
                let path = Self::get(&file, "rel").as_string().unwrap_or_default();
                let size = Self::get(&file, "size").as_f64().unwrap_or(0.0) as u64;
                let bytes = |k| js_sys::Uint8Array::new(&Self::get(&file, k)).to_vec();
                let probe = audio::music::probe(&bytes("head"), &bytes("audio"), size);
                audio::deck::FileInfo { key, path, probe }
            })
            .collect();
        let folder = audio::deck::Folder {
            id,
            name,
            mixtape,
            files,
        };
        Some((folder, announce, kept))
    }
}

/// `window.skywaveRadio` (web/index.html): plays what the radio asks for, reports back.
struct RadioGlue {
    this: JsValue,
    command: js_sys::Function,
    take_event: js_sys::Function,
    report: js_sys::Function,
}

impl RadioGlue {
    fn find(window: &web_sys::Window) -> Option<RadioGlue> {
        let this = js_sys::Reflect::get(window, &JsValue::from_str("skywaveRadio")).ok()?;
        let func = |name: &str| {
            js_sys::Reflect::get(&this, &JsValue::from_str(name))
                .ok()?
                .dyn_into::<js_sys::Function>()
                .ok()
        };
        Some(RadioGlue {
            command: func("command")?,
            take_event: func("takeEvent")?,
            report: func("report")?,
            this,
        })
    }

    /// Hands one command to the player; `name` is the station's, for the page's media
    /// metadata.
    fn send(&self, cmd: &RadioCommand, name: &str) {
        let (kind, url, token, value) = match cmd {
            RadioCommand::Play { url, token } => ("play", url.as_str(), *token, 0.0),
            RadioCommand::PlayFile {
                key,
                start_s,
                token,
            } => ("file", key.as_str(), *token, *start_s),
            RadioCommand::Stop => ("stop", "", 0, 0.0),
            RadioCommand::Volume(v) => ("volume", "", 0, *v),
        };
        let args = js_sys::Array::of5(
            &JsValue::from_str(kind),
            &JsValue::from_str(url),
            &JsValue::from_f64(f64::from(token)),
            &JsValue::from_f64(f64::from(value)),
            &JsValue::from_str(name),
        );
        if let Err(e) = self.command.apply(&self.this, &args) {
            log::warn!("skywave: the radio player refused a command: {e:?}");
        }
    }

    /// The player's next report, if any.
    fn take_event(&self) -> Option<audio::PlayerEvent> {
        let v = self.take_event.call0(&self.this).ok()?;
        crate::parse_player_event(&v.as_string()?)
    }
}

/// Settings in the page's `localStorage`. Browsers that refuse storage (private modes,
/// blocked cookies) just keep the defaults.
struct LocalStore;

const STORAGE_KEY: &str = "skywave.settings";

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

impl SettingsStore for LocalStore {
    fn load(&self) -> Option<String> {
        local_storage()?.get_item(STORAGE_KEY).ok()?
    }

    fn save(&mut self, json: &str) {
        if let Some(storage) = local_storage() {
            if storage.set_item(STORAGE_KEY, json).is_err() {
                log::warn!("skywave: could not save settings to localStorage");
            }
        }
    }
}

/// The page's hooks, from `run`'s `options`: `onNote(kind, text)` gets "attempt" (the
/// backend being started), "backend" (the one that started), "fallback" (why an attempt
/// failed), "warning" (something non-fatal turned off or reported), "lost" (the device is
/// gone) and "diag" (the `?diag=1` report).
#[derive(Clone)]
pub(crate) struct Page {
    on_note: Option<js_sys::Function>,
}

impl Page {
    fn from_options(options: &JsValue) -> Page {
        let on_note = js_sys::Reflect::get(options, &JsValue::from_str("onNote"))
            .ok()
            .and_then(|f| f.dyn_into::<js_sys::Function>().ok());
        Page { on_note }
    }

    pub(crate) fn note(&self, kind: &str, text: &str) {
        if let Some(f) = &self.on_note {
            let _ = f.call2(
                &JsValue::NULL,
                &JsValue::from_str(kind),
                &JsValue::from_str(text),
            );
        }
    }
}

/// A string member of `run`'s `options` object.
fn option_str(options: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(options, &JsValue::from_str(key))
        .ok()?
        .as_string()
}

fn warn(text: &str) {
    console::warn_1(&JsValue::from_str(&format!("skywave: {text}")));
}

/// Errors the device reported outside any error scope (`on_uncaptured_error`, which also
/// keeps wgpu from panicking on them) and a device loss, for the frame loop to report.
#[derive(Clone, Default)]
pub(crate) struct ErrorLog(Arc<Mutex<Errors>>);

#[derive(Default)]
struct Errors {
    /// The first few messages since the last `take`.
    messages: Vec<String>,
    lost: Option<String>,
}

/// Messages kept between two `take`s; the rest are only counted in the log line.
const ERRORS_KEPT: usize = 8;

impl ErrorLog {
    fn install(device: &wgpu::Device) -> ErrorLog {
        let log = ErrorLog::default();
        let sink = log.clone();
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            if let Ok(mut errors) = sink.0.lock() {
                if errors.messages.len() < ERRORS_KEPT {
                    errors.messages.push(e.to_string());
                }
            }
        }));
        let sink = log.clone();
        device.set_device_lost_callback(move |reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                if let Ok(mut errors) = sink.0.lock() {
                    errors.lost = Some(message);
                }
            }
        });
        log
    }

    /// The messages and the loss gathered since the last call, if any (no allocation
    /// when there are none).
    pub(crate) fn take(&self) -> Option<(Vec<String>, Option<String>)> {
        let mut errors = self.0.lock().ok()?;
        if errors.messages.is_empty() && errors.lost.is_none() {
            return None;
        }
        Some((std::mem::take(&mut errors.messages), errors.lost.take()))
    }
}

/// A start-up failure: what kind, and the browser's or wgpu's words.
pub(crate) struct Failure {
    pub kind: StartFailure,
    pub detail: String,
}

impl Failure {
    fn new(kind: StartFailure, detail: impl std::fmt::Display) -> Failure {
        Failure {
            kind,
            detail: detail.to_string(),
        }
    }
}

/// An adapter and device on one canvas.
pub(crate) struct Gpu {
    pub surface: wgpu::Surface<'static>,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub errors: ErrorLog,
}

/// Opens `backend` on `canvas`: instance, surface, adapter and device, with the error log
/// installed. The canvas is bound to that kind of context afterwards, whatever happens.
pub(crate) async fn open_gpu(
    canvas: &HtmlCanvasElement,
    backend: WebBackend,
) -> Result<Gpu, Failure> {
    let backends = match backend {
        WebBackend::WebGpu => {
            if !wgpu::util::is_browser_webgpu_supported().await {
                return Err(Failure::new(
                    StartFailure::NoWebGpu,
                    "navigator.gpu is missing or requestAdapter() gave no adapter",
                ));
            }
            wgpu::Backends::BROWSER_WEBGPU
        }
        WebBackend::WebGl2 => wgpu::Backends::GL,
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
        .map_err(|e| Failure::new(StartFailure::Surface, e))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
        .map_err(|e| Failure::new(StartFailure::NoAdapter, e))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("skywave"),
            // No optional features: the renderer needs none, and asking for one a browser
            // lacks rejects the whole request.
            required_features: wgpu::Features::empty(),
            required_limits: required_limits(backend, &adapter.limits()),
            ..Default::default()
        })
        .await
        .map_err(|e| Failure::new(StartFailure::Device, e))?;
    let errors = ErrorLog::install(&device);
    Ok(Gpu {
        surface,
        adapter,
        device,
        queue,
        errors,
    })
}

/// The limits to request. The renderer keeps to WebGL2's limits on every backend. On
/// WebGL2 those are requested as such; on WebGPU the downlevel set (every value at or
/// below WebGPU's defaults, none zeroed), so no request asks for more than a minimal
/// implementation offers. Either way the texture size is the adapter's, and nothing
/// exceeds what the adapter reports (a request above it is rejected outright).
fn required_limits(backend: WebBackend, adapter: &wgpu::Limits) -> wgpu::Limits {
    let base = match backend {
        WebBackend::WebGl2 => wgpu::Limits::downlevel_webgl2_defaults(),
        WebBackend::WebGpu => wgpu::Limits::downlevel_defaults(),
    };
    base.using_resolution(adapter.clone())
        .or_worse_values_from(adapter)
}

/// The surface configuration for `size`. WebGPU: the canvas's preferred 8-bit format
/// (`getPreferredCanvasFormat`, listed first; the shaders encode sRGB themselves), never a
/// float canvas, FIFO, alpha `Auto` (opaque). WebGL2: an sRGB format where offered.
pub(crate) fn surface_config(
    gpu: &Gpu,
    backend: WebBackend,
    size: (u32, u32),
) -> Option<wgpu::SurfaceConfiguration> {
    let mut config = gpu
        .surface
        .get_default_config(&gpu.adapter, size.0, size.1)?;
    let caps = gpu.surface.get_capabilities(&gpu.adapter);
    config.format = match backend {
        WebBackend::WebGpu => caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
            .unwrap_or(config.format),
        WebBackend::WebGl2 => caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(config.format),
    };
    config.present_mode = wgpu::PresentMode::AutoVsync;
    config.alpha_mode = wgpu::CompositeAlphaMode::Auto;
    config.view_formats.clear();
    Some(config)
}

/// The HDR mode for the adapter: the float path only where `Rgba16Float` renders, blends
/// and filters (WebGPU always; WebGL2 with its float extensions).
pub(crate) fn hdr_mode(adapter: &wgpu::Adapter) -> HdrMode {
    let features = adapter.get_texture_format_features(render::HDR_TEXTURE_FORMAT);
    if render::hdr_format_supported(features) {
        HdrMode::Auto
    } else {
        HdrMode::Ldr
    }
}

/// Validation, out-of-memory and internal error scopes pushed together. Popping returns a
/// future of the first error any of them caught. Never `?` out while one is open: scopes
/// must be popped in order (dropping pops them, innermost first).
pub(crate) struct Scopes(Vec<wgpu::ErrorScopeGuard>);

impl Scopes {
    pub(crate) fn push(device: &wgpu::Device) -> Scopes {
        Scopes(vec![
            device.push_error_scope(wgpu::ErrorFilter::Validation),
            device.push_error_scope(wgpu::ErrorFilter::OutOfMemory),
            device.push_error_scope(wgpu::ErrorFilter::Internal),
        ])
    }

    pub(crate) fn pop(mut self) -> impl Future<Output = Option<String>> {
        // Innermost first; each pop takes effect now, the answers come later.
        let answers: Vec<_> = std::iter::from_fn(|| self.0.pop())
            .map(wgpu::ErrorScopeGuard::pop)
            .collect();
        async move {
            for answer in answers {
                if let Some(error) = answer.await {
                    return Some(error.to_string());
                }
            }
            None
        }
    }
}

impl Drop for Scopes {
    fn drop(&mut self) {
        while let Some(scope) = self.0.pop() {
            drop(scope);
        }
    }
}

/// Draws [`SceneView::probe`] (with one instance of the first mesh, if there is one) to
/// the canvas and presents it, reconfiguring once if the surface is outdated or lost.
pub(crate) fn probe_frame(
    gpu: &Gpu,
    config: &wgpu::SurfaceConfiguration,
    renderer: &mut Renderer,
    with_mesh: bool,
) -> Result<(), String> {
    const IDENTITY: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let instance = [MeshInstance::new(MeshId(0), IDENTITY)];
    let instances: &[MeshInstance] = if with_mesh { &instance } else { &[] };
    for _ in 0..2 {
        match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                let view = frame.texture.create_view(&Default::default());
                // At the renderer's own quality, so the first real frame rebuilds nothing.
                let scene = SceneView {
                    quality: renderer.quality(),
                    ..SceneView::probe(instances, &[])
                };
                renderer.render(&view, &scene);
                gpu.queue.present(frame);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gpu.surface.configure(&gpu.device, config);
            }
            other => return Err(format!("the canvas gave no texture: {other:?}")),
        }
    }
    Err("the canvas gave no texture, even after reconfiguring (surface lost)".to_string())
}

/// The canvas's drawing-buffer size now ([`crate::canvas_size`]).
fn client_size(
    window: &web_sys::Window,
    canvas: &HtmlCanvasElement,
    max: u32,
) -> Option<(u32, u32)> {
    crate::canvas_size(
        (canvas.client_width(), canvas.client_height()),
        window.device_pixel_ratio(),
        max,
    )
}

/// Records a panic's message in `window.skywavePanic` (for the page's fallback note) after
/// the usual console report.
fn install_panic_hook() {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            console_error_panic_hook::hook(info);
            if let Some(window) = web_sys::window() {
                let _ = js_sys::Reflect::set(
                    &window,
                    &JsValue::from_str("skywavePanic"),
                    &JsValue::from_str(&info.to_string()),
                );
            }
        }));
    });
}

/// Swaps `old` for a fresh canvas with the same id, class, style and tab index: a canvas
/// that gave out a WebGPU context never gives a WebGL2 one.
fn replace_canvas(old: &HtmlCanvasElement) -> Result<HtmlCanvasElement, JsValue> {
    let document = old.owner_document().ok_or("no document")?;
    let canvas: HtmlCanvasElement = document.create_element("canvas")?.dyn_into()?;
    for name in ["id", "class", "style", "tabindex"] {
        if let Some(value) = old.get_attribute(name) {
            canvas.set_attribute(name, &value)?;
        }
    }
    old.replace_with_with_node_1(&canvas)?;
    Ok(canvas)
}

fn has_webgpu(window: &web_sys::Window) -> bool {
    js_sys::Reflect::has(&window.navigator(), &JsValue::from_str("gpu")).unwrap_or(false)
}

fn build_app(setup: &SceneSetup, query: &str) -> App {
    let mut app = App::from_setup(setup).unwrap_or_else(|e| {
        log::warn!("skywave: {e}; driving the {}", app::DEFAULT_VEHICLE);
        let default_car = SceneSetup {
            car: app::DEFAULT_VEHICLE.to_string(),
            ..setup.clone()
        };
        App::from_setup(&default_car).expect("the default car is built in")
    });
    app.set_settings_store(Box::new(LocalStore));
    // The page can ask for a stream URL (window.prompt).
    app.set_text_entry(true);
    if crate::menu_from_query(query) {
        app.open_settings();
    } else if crate::vehicle_menu_from_query(query) {
        app.open_menu(app::ui::Page::Vehicle);
    }
    app
}

/// Starts the game on `canvas` in the scene the URL query describes (`app::scene`).
/// Resolves once the first frame is scheduled; rejects with a readable message when no
/// backend could start. `on_first_frame` is called with the backend name ("WebGPU" or
/// "WebGL2") after the first frame is presented.
///
/// `options` (may be `undefined`): `onNote(kind, text)` (see [`Page`]); `backend`, "gl"
/// or "gpu", overriding the URL's `gl=1` / `gpu=1`; `failure`, why the page restarted us
/// (a crash during an earlier WebGPU start), for the fallback note.
///
/// With `?selftest=1` nothing is drawn: the determinism self-test (`app::selftest`) runs,
/// prints its one JSON line to the console, and `on_first_frame` is called with
/// "selftest". With `?diag=1` the start-up checks run on each backend without the game,
/// the report goes to the console and to `onNote("diag", report)`, and `on_first_frame`
/// is called with "diag".
#[wasm_bindgen]
pub async fn run(
    canvas: HtmlCanvasElement,
    on_first_frame: js_sys::Function,
    options: JsValue,
) -> Result<(), JsValue> {
    install_panic_hook();
    // Ignore the error if a logger is already installed (run called twice).
    let _ = console_log::init_with_level(log::Level::Warn);

    let window = web_sys::window().ok_or("no window")?;
    let query = window.location().search()?;
    let page = Page::from_options(&options);
    if crate::query_param(&query, "selftest") == Some("1") {
        let line = app::selftest::SelfTest::run().console_line();
        console::log_1(&JsValue::from_str(&line));
        let _ = on_first_frame.call1(&JsValue::NULL, &JsValue::from_str("selftest"));
        return Ok(());
    }
    let choice = BackendChoice::from_query(&query, option_str(&options, "backend").as_deref());
    if crate::query_param(&query, "diag") == Some("1") {
        let report = crate::diag::report(&window, choice).await;
        console::log_1(&JsValue::from_str(&report));
        page.note("diag", &report);
        let _ = on_first_frame.call1(&JsValue::NULL, &JsValue::from_str("diag"));
        return Ok(());
    }
    let setup = SceneSetup::from_query(&query).unwrap_or_else(|e| {
        log::warn!("skywave: {e}; starting the default scene");
        SceneSetup::default()
    });
    if let Some(detail) = option_str(&options, "failure") {
        let note = crate::fallback(
            BackendChoice::Auto,
            WebBackend::WebGpu,
            StartFailure::Panic,
            &detail,
        )
        .note;
        warn(&note);
        page.note("fallback", &note);
    }

    let mut backend = choice.first(has_webgpu(&window));
    let mut canvas = canvas;
    let mut app = None;
    let mut first_attempt = true;
    let (gpu, config, renderer) = loop {
        if !first_attempt {
            canvas = replace_canvas(&canvas)?;
        }
        first_attempt = false;
        page.note("attempt", backend.label());
        let failure = match open_gpu(&canvas, backend).await {
            Ok(gpu) => match start(&gpu, &window, &canvas, backend, &setup, &query, &mut app).await
            {
                Ok((config, renderer)) => break (gpu, config, renderer),
                Err(failure) => {
                    // Frees the attempt's buffers now (dropping a browser device does not).
                    gpu.device.destroy();
                    failure
                }
            },
            Err(failure) => failure,
        };
        let fallback = crate::fallback(choice, backend, failure.kind, &failure.detail);
        if failure.kind == StartFailure::NoWebGpu && fallback.retry.is_some() {
            // The everyday case (a browser listing WebGPU with no adapter behind it).
            console::info_1(&JsValue::from_str(&format!("skywave: {}", fallback.note)));
        } else {
            warn(&fallback.note);
        }
        page.note("fallback", &fallback.note);
        match fallback.retry {
            Some(next) => backend = next,
            None => return Err(JsValue::from_str(&fallback.note)),
        }
    };
    let mut app = app.expect("a started attempt built the app");
    log::info!(
        "skywave: {}, {:?}, {setup:?}",
        backend.label(),
        config.format
    );
    page.note("backend", backend.label());
    // Found per started instance: a restart on WebGL2 in a fresh copy of the glue finds the
    // page's one player again (its listeners live in the page, registered once).
    let radio = RadioGlue::find(&window);
    if radio.is_none() {
        log::info!("skywave: no radio player on this page; the radio stays silent");
    }
    let music = MusicGlue::find(&window);
    app.set_music_picker(music.is_some(), false);

    let state = Rc::new(RefCell::new(State {
        canvas,
        surface: gpu.surface,
        device: gpu.device,
        queue: gpu.queue,
        config,
        renderer,
        app,
        input: InputMapper::default(),
        pad: None,
        pads_ok: true,
        last_ms: None,
        last_tick_ms: None,
        refresh_ms: 1000.0 / 60.0,
        skipped: 0,
        performance: window.performance(),
        stream_idle: None,
        backend,
        on_first_frame: Some(on_first_frame),
        sfx: crate::sfx::Sfx::new(&window),
        page,
        errors: gpu.errors,
        corona_reported: false,
        radio,
        radio_cmds: Vec::with_capacity(8),
        radio_reported: (
            0,
            None,
            false,
            false,
            (
                audio::deck::Source::Radio,
                0,
                audio::deck::Mode::Normal,
                false,
                false,
                false,
            ),
        ),
        music,
        music_again: false,
    }));
    listen_keys(&window, &state)?;
    listen_mouse(&window, &state)?;
    listen_gamepads(&window)?;
    start_frame_loop(&window, state)?;
    Ok(())
}

/// Configures the surface, builds the app (once, kept across attempts) and the renderer,
/// and draws a probe frame, all inside error scopes; any error, a surface that gives no
/// texture or a lost device fails the attempt.
async fn start(
    gpu: &Gpu,
    window: &web_sys::Window,
    canvas: &HtmlCanvasElement,
    backend: WebBackend,
    setup: &SceneSetup,
    query: &str,
    app: &mut Option<App>,
) -> Result<(wgpu::SurfaceConfiguration, Renderer), Failure> {
    let max = gpu.device.limits().max_texture_dimension_2d;
    // Not laid out yet: a 16:9 stand-in until the frame loop sees the real size.
    let size = client_size(window, canvas, max).unwrap_or((max.min(1280), max.min(720)));
    let config = surface_config(gpu, backend, size).ok_or_else(|| {
        Failure::new(
            StartFailure::Surface,
            "the canvas supports no configuration for this adapter",
        )
    })?;

    let build = Scopes::push(&gpu.device);
    gpu.surface.configure(&gpu.device, &config);
    let app = app.get_or_insert_with(|| build_app(setup, query));
    app.set_target_size(size.0, size.1);
    // Per attempt: a fallback to WebGL2 may land on another adapter.
    let info = gpu.adapter.get_info();
    app.set_gpu_info(&app::GpuInfo {
        name: info.name.clone(),
        backend: backend.label().to_owned(),
        integrated: info.device_type == wgpu::DeviceType::IntegratedGpu,
        linux: false,
        surface: size,
    });
    let options = RendererOptions {
        hdr: hdr_mode(&gpu.adapter),
        ..app.renderer_options()
    };
    let mut renderer =
        Renderer::with_options(&gpu.device, &gpu.queue, config.format, size, options);
    // The app outlives a failed attempt but its `take_new_meshes` queue (the saved car)
    // is only drained by the frame loop, after an attempt succeeds: nothing is lost here.
    for mesh in app.meshes() {
        renderer.add_mesh(mesh);
    }
    renderer.reserve_instances(app.max_instances());
    let map = app.map_image();
    renderer.upload_map(map.w, map.h, &map.rgba);
    let build = build.pop();

    let frame = Scopes::push(&gpu.device);
    let presented = probe_frame(gpu, &config, &mut renderer, !app.meshes().is_empty());
    let frame = frame.pop();

    if let Some(error) = build.await {
        return Err(Failure::new(StartFailure::Pipeline, error));
    }
    if let Some(error) = frame.await {
        return Err(Failure::new(StartFailure::FirstFrame, error));
    }
    presented.map_err(|e| Failure::new(StartFailure::Surface, e))?;
    if let Some((messages, lost)) = gpu.errors.take() {
        if let Some(lost) = lost {
            return Err(Failure::new(StartFailure::DeviceLost, lost));
        }
        return Err(Failure::new(StartFailure::FirstFrame, messages.join("; ")));
    }
    Ok((config, renderer))
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

    /// The radio's side of the frame: the player's reports in, this frame's commands out,
    /// the state to the page when it changed, and the stream URL prompt when asked for.
    fn pump_radio(&mut self) {
        if self.app.take_stream_url_request() {
            if let Some(window) = web_sys::window() {
                let answer = window.prompt_with_message("Stream URL (https://...)");
                if let Ok(Some(url)) = answer {
                    self.app.add_stream_url(&url);
                }
            }
            // Key releases during the modal prompt never arrived.
            self.input.release_all();
        }
        if let Some(music) = &self.music {
            if self.app.take_music_request() {
                // The menu key's or click's activation lets the page open its folder
                // picker; a gamepad press gives none, and the page says so.
                let opened = music.pick.call0(&music.this).ok().and_then(|v| v.as_bool());
                if opened == Some(false) {
                    self.app.music_picker_refused();
                }
            }
            for path in self.app.take_forgotten_music() {
                let _ = music.forget.call1(&music.this, &JsValue::from_str(&path));
            }
            while let Some((folder, announce, kept)) = music.take_folder() {
                self.app.add_music_folder(folder, announce, kept);
            }
            let again = music.again();
            if again != self.music_again {
                self.music_again = again;
                self.app.set_music_picker(true, again);
            }
        }
        let Some(glue) = &self.radio else {
            // Nobody plays: drop the commands.
            self.app.radio_commands().for_each(drop);
            return;
        };
        while let Some(event) = glue.take_event() {
            self.app.radio_event(event);
        }
        self.radio_cmds.extend(self.app.radio_commands());
        let name = match self.app.source() {
            audio::deck::Source::Radio => {
                self.app.radio().channel().map_or("", |c| c.name.as_str())
            }
            audio::deck::Source::Deck => self.app.deck().track().map_or("", |t| t.label.as_str()),
        };
        for cmd in &self.radio_cmds {
            glue.send(cmd, name);
        }
        self.radio_cmds.clear();
        let r = self.app.radio();
        let kind = match r.state() {
            audio::RadioState::Off => 0,
            audio::RadioState::Tuning { .. } => 1,
            audio::RadioState::Playing { .. } => 2,
            audio::RadioState::Error { .. } => 3,
        };
        let d = self.app.deck();
        let deck = (
            self.app.source(),
            d.changes(),
            d.mode(),
            d.is_playing(),
            d.is_live(),
            d.is_blocked(),
        );
        let key = (kind, r.station(), r.is_live(), r.is_blocked(), deck);
        if key != self.radio_reported {
            let text = crate::radio_state_text(r);
            let readout = self.app.readout_text();
            let _ = glue.report.call2(
                &glue.this,
                &JsValue::from_str(&text),
                &JsValue::from_str(&readout),
            );
            self.radio_reported = key;
        }
    }

    /// One animation frame: skipped unless it is the Nth since the last one processed
    /// (the frame-rate cap: every Nth refresh); else the game's frame, drawing only what
    /// changed (nothing at all when nothing did: the canvas keeps the last frame). The
    /// game pauses while the page is hidden (a tab switch; the browser stops animation
    /// frames then too).
    fn frame(&mut self, now_ms: f64) {
        if let Some(last) = self.last_tick_ms {
            // Refreshes run 4 to 50 ms apart; longer gaps are the page stalling.
            let tick = now_ms - last;
            if (4.0..50.0).contains(&tick) {
                self.refresh_ms += 0.05 * (tick - self.refresh_ms);
                self.app.set_panel_hz((1000.0 / self.refresh_ms).round());
            }
        }
        self.last_tick_ms = Some(now_ms);
        let hidden = web_sys::window()
            .and_then(|w| w.document())
            .is_some_and(|d| d.hidden());
        self.app.set_focused(!hidden);
        if self.skipped + 1 < self.app.frame_rate().divisor() {
            self.skipped += 1;
            return;
        }
        self.skipped = 0;
        let dt = self.last_ms.map_or(0.0, |last| (now_ms - last) / 1000.0);
        self.last_ms = Some(now_ms);

        if let Some(window) = web_sys::window() {
            let max = self.device.limits().max_texture_dimension_2d;
            // A canvas without a layout size (hidden) keeps its last configuration.
            if let Some((w, h)) = client_size(&window, &self.canvas, max) {
                if (w, h) != (self.config.width, self.config.height) {
                    self.canvas.set_width(w);
                    self.canvas.set_height(h);
                    self.config.width = w;
                    self.config.height = h;
                    self.surface.configure(&self.device, &self.config);
                    self.renderer.resize((w, h));
                    self.app.set_target_size(w, h);
                }
            }
            self.poll_gamepads(&window);
        }
        self.report_device_messages();

        let performance = &self.performance;
        let now = || performance.as_ref().map_or(0.0, |p| p.now() / 1000.0);
        let steps = self.app.frame(&mut self.input, dt, &now);
        if let Some(sfx) = &mut self.sfx {
            sfx.frame(self.app.audio_params(), dt);
        }
        self.pump_radio();
        self.app.sync_chunks(&mut self.renderer);
        // A car driven for the first time this session (and the saved one, at startup).
        for mesh in self.app.take_new_meshes() {
            self.renderer.add_mesh(&mesh);
        }
        let idle = self.app.stream_idle();
        if self.stream_idle != Some(idle) {
            // For screenshot tools: nothing left to stream (always true with `stream=0`).
            if let Some(window) = web_sys::window() {
                let key = JsValue::from_str("skywaveStreamIdle");
                let _ = js_sys::Reflect::set(&window, &key, &JsValue::from_bool(idle));
                if idle {
                    if let Some(s) = self.app.stream_stats() {
                        log::info!("skywave: streaming idle: {s:?}");
                        let stats = JsValue::from_str(&format!("{s:?}"));
                        let key = JsValue::from_str("skywaveStreamStats");
                        let _ = js_sys::Reflect::set(&window, &key, &stats);
                    }
                }
            }
            self.stream_idle = Some(idle);
        }
        let mut reconfigured = false;
        let (surface, device, config) = (&self.surface, &self.device, &self.config);
        let acquire = || match surface.get_current_texture() {
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
        };
        let aspect = self.config.width as f32 / self.config.height as f32;
        let started = self.performance.as_ref().map(|p| p.now());
        let scene = self.app.scene_view(steps.alpha, aspect);
        let (_, frame) = self.renderer.draw(&scene, acquire);
        if reconfigured {
            // The new surface shows nothing yet.
            self.renderer.screen_lost();
        }
        let Some(frame) = frame else {
            return;
        };
        if let (Some(p), Some(t)) = (&self.performance, started) {
            self.app.set_cpu_submit_ms((p.now() - t) as f32);
        }
        self.queue.present(frame);
        self.app.set_render_stats(self.renderer.stats());

        if let Some(cb) = self.on_first_frame.take() {
            let _ = cb.call1(&JsValue::NULL, &JsValue::from_str(self.backend.label()));
        }
    }

    /// Reports, once each, how the corona check came out and whatever the device said
    /// outside error scopes since the last frame; a lost device ends the game.
    fn report_device_messages(&mut self) {
        if !self.corona_reported {
            match self.renderer.corona_probe() {
                Probe::Pending => {}
                Probe::Passed => self.corona_reported = true,
                Probe::Failed(error) => {
                    let note = format!(
                        "{}: the corona pass was rejected, coronas are off ({error})",
                        self.backend.label()
                    );
                    warn(&note);
                    self.page.note("warning", &note);
                    self.corona_reported = true;
                }
            }
        }
        let Some((messages, lost)) = self.errors.take() else {
            return;
        };
        for message in &messages {
            console::error_1(&JsValue::from_str(&format!(
                "skywave: {} error: {message}",
                self.backend.label()
            )));
        }
        if let Some(first) = messages.first() {
            self.page.note(
                "warning",
                &format!("{} error: {first}", self.backend.label()),
            );
        }
        if let Some(lost) = lost {
            let note = format!("the {} device was lost: {lost}", self.backend.label());
            console::error_1(&JsValue::from_str(&format!("skywave: {note}")));
            self.page.note("lost", &note);
        }
    }
}
