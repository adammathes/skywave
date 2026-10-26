//! `?diag=1`: the start-up checks without the game, on each backend in turn (WebGPU, then
//! WebGL2; only the one `gl=1` / `gpu=1` names), each on its own detached canvas, written
//! up as plain text for pasting into a bug report. The format is `key: value` lines under a
//! `[backend]` header per backend, ending in a `verdict:` line (see docs/RENDERING.md).

use std::fmt::Write;

use render::{CoronaDepth, Probe, Renderer, RendererOptions};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, Window};

use crate::web::{hdr_mode, open_gpu, probe_frame, surface_config, Scopes};
use crate::{BackendChoice, WebBackend};

/// The diagnostics canvas: small, 16:9.
const SIZE: (u32, u32) = (320, 180);

/// Runs the checks and returns the report.
pub(crate) async fn report(window: &Window, choice: BackendChoice) -> String {
    let mut out = String::new();
    let navigator = window.navigator();
    let has_gpu = js_sys::Reflect::has(&navigator, &JsValue::from_str("gpu")).unwrap_or(false);
    let _ = writeln!(
        out,
        "Skywave start-up diagnostics (platform-web {})",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(
        out,
        "user agent: {}",
        navigator.user_agent().unwrap_or_default()
    );
    let inner = |v: Result<JsValue, JsValue>| v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
    let _ = writeln!(
        out,
        "window: {}x{} CSS px, device pixel ratio {}",
        inner(window.inner_width()),
        inner(window.inner_height()),
        window.device_pixel_ratio()
    );
    let _ = writeln!(
        out,
        "navigator.gpu: {}",
        if has_gpu { "present" } else { "missing" }
    );
    let _ = writeln!(out, "backend choice: {choice:?}");
    let backends: &[WebBackend] = match choice {
        BackendChoice::Auto => &[WebBackend::WebGpu, WebBackend::WebGl2],
        BackendChoice::WebGpu => &[WebBackend::WebGpu],
        BackendChoice::WebGl2 => &[WebBackend::WebGl2],
    };
    let mut verdicts = Vec::new();
    for &backend in backends {
        let _ = writeln!(out, "\n[{}]", backend.label());
        let ok = match detached_canvas(window) {
            Ok(canvas) => section(&mut out, window, &canvas, backend).await,
            Err(e) => {
                let _ = writeln!(out, "canvas: FAILED: {e:?}");
                false
            }
        };
        let _ = writeln!(out, "verdict: {}", if ok { "OK" } else { "FAILED" });
        verdicts.push((backend, ok));
    }
    let game = match verdicts.iter().find(|(_, ok)| *ok) {
        Some((backend, _)) => backend.label(),
        None => "none (the game cannot start)",
    };
    let _ = writeln!(out, "\nthe game would start on: {game}");
    out
}

fn detached_canvas(window: &Window) -> Result<HtmlCanvasElement, JsValue> {
    let document = window.document().ok_or("no document")?;
    let canvas: HtmlCanvasElement = document.create_element("canvas")?.dyn_into()?;
    canvas.set_width(SIZE.0);
    canvas.set_height(SIZE.1);
    Ok(canvas)
}

/// Checks one backend; true when it could run the game.
async fn section(
    out: &mut String,
    window: &Window,
    canvas: &HtmlCanvasElement,
    backend: WebBackend,
) -> bool {
    let gpu = match open_gpu(canvas, backend).await {
        Ok(gpu) => gpu,
        Err(f) => {
            let _ = writeln!(out, "start: FAILED: {}: {}", f.kind.label(), f.detail);
            return false;
        }
    };
    let info = gpu.adapter.get_info();
    let _ = writeln!(
        out,
        "adapter: {:?} (vendor {:#06x}, device {:#06x}, type {:?}, driver {:?} {:?})",
        info.name, info.vendor, info.device, info.device_type, info.driver, info.driver_info
    );
    let _ = writeln!(out, "adapter features: {:?}", gpu.adapter.features());
    let a = gpu.adapter.limits();
    let d = gpu.device.limits();
    for (name, adapter, device) in [
        (
            "max_texture_dimension_2d",
            u64::from(a.max_texture_dimension_2d),
            u64::from(d.max_texture_dimension_2d),
        ),
        (
            "max_uniform_buffer_binding_size",
            a.max_uniform_buffer_binding_size,
            d.max_uniform_buffer_binding_size,
        ),
        ("max_buffer_size", a.max_buffer_size, d.max_buffer_size),
        (
            "max_vertex_attributes",
            u64::from(a.max_vertex_attributes),
            u64::from(d.max_vertex_attributes),
        ),
        (
            "max_inter_stage_shader_variables",
            u64::from(a.max_inter_stage_shader_variables),
            u64::from(d.max_inter_stage_shader_variables),
        ),
        (
            "max_sampled_textures_per_shader_stage",
            u64::from(a.max_sampled_textures_per_shader_stage),
            u64::from(d.max_sampled_textures_per_shader_stage),
        ),
        (
            "max_color_attachment_bytes_per_sample",
            u64::from(a.max_color_attachment_bytes_per_sample),
            u64::from(d.max_color_attachment_bytes_per_sample),
        ),
    ] {
        let _ = writeln!(out, "limit {name}: adapter {adapter}, device {device}");
    }
    let caps = gpu.surface.get_capabilities(&gpu.adapter);
    let _ = writeln!(
        out,
        "surface formats: {:?}; present modes: {:?}; alpha modes: {:?}",
        caps.formats, caps.present_modes, caps.alpha_modes
    );
    let hdr_features = gpu
        .adapter
        .get_texture_format_features(render::HDR_TEXTURE_FORMAT);
    let _ = writeln!(
        out,
        "Rgba16Float: usages {:?}, flags {:?} -> {}",
        hdr_features.allowed_usages,
        hdr_features.flags,
        if render::hdr_format_supported(hdr_features) {
            "float (HDR) path"
        } else {
            "LDR path"
        }
    );
    let Some(config) = surface_config(&gpu, backend, SIZE) else {
        let _ = writeln!(out, "surface config: FAILED: no supported configuration");
        gpu.device.destroy();
        return false;
    };
    let _ = writeln!(
        out,
        "surface config: {:?} {}x{}, {:?}, alpha {:?}",
        config.format, config.width, config.height, config.present_mode, config.alpha_mode
    );

    let build = Scopes::push(&gpu.device);
    gpu.surface.configure(&gpu.device, &config);
    let options = RendererOptions {
        hdr: hdr_mode(&gpu.adapter),
        ..RendererOptions::default()
    };
    let mut renderer =
        Renderer::with_options(&gpu.device, &gpu.queue, config.format, SIZE, options);
    let build = build.pop();
    let frame = Scopes::push(&gpu.device);
    let presented = probe_frame(&gpu, &config, &mut renderer, false);
    let frame = frame.pop();
    let mut ok = true;
    let mut line = |out: &mut String, what: &str, result: Result<(), String>| {
        let _ = match result {
            Ok(()) => writeln!(out, "{what}: ok"),
            Err(e) => {
                ok = false;
                writeln!(out, "{what}: FAILED: {e}")
            }
        };
    };
    line(out, "pipelines", build.await.map_or(Ok(()), Err));
    line(
        out,
        "probe frame (validation)",
        frame.await.map_or(Ok(()), Err),
    );
    line(out, "probe frame (present)", presented);
    let _ = writeln!(
        out,
        "renderer: {} path, shadow cascades {} px",
        if renderer.is_hdr() { "float" } else { "LDR" },
        renderer.shadow_cascade_size()
    );

    // The browser answers the corona check a little later; give it a moment.
    let mut waited = 0;
    while renderer.corona_probe() == Probe::Pending && waited < 40 {
        sleep_ms(window, 25).await;
        waited += 1;
    }
    let depth = match renderer.corona_depth() {
        CoronaDepth::Loaded => "textureLoad",
        _ => "nearest sampler",
    };
    let _ = match renderer.corona_probe() {
        Probe::Passed => writeln!(out, "coronas (depth via {depth}): ok"),
        Probe::Pending => writeln!(out, "coronas (depth via {depth}): no answer yet"),
        // Optional: the game runs without them.
        Probe::Failed(e) => writeln!(out, "coronas (depth via {depth}): off: {e}"),
    };
    match gpu.errors.take() {
        None => {
            let _ = writeln!(out, "uncaptured errors: none");
        }
        Some((messages, lost)) => {
            for m in &messages {
                let _ = writeln!(out, "uncaptured error: {m}");
            }
            if let Some(lost) = lost {
                let _ = writeln!(out, "device lost: {lost}");
            }
            ok = false;
        }
    }
    drop(renderer);
    gpu.device.destroy();
    ok
}

async fn sleep_ms(window: &Window, ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}
