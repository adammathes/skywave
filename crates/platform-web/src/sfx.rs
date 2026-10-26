//! Sound for the web build: the `audio::mixer::Mixer` runs on the main thread, in the
//! animation frame, and hands its 48 kHz stereo blocks to the page's `window.skywaveSfx`
//! glue (the `sfx` script in `web/index.html`), whose AudioWorklet plays them from a ring.
//!
//! Each frame `skywaveSfx.wanted(dt)` says how many frames to render (0 until the first
//! gesture has started the AudioContext): enough to keep two blocks plus one animation
//! frame queued. The blocks go to `skywaveSfx.push` as a view into wasm memory, which the
//! glue copies at once. Without the glue (another page) the game is silent.

use audio::mixer::{Mixer, BLOCK_FRAMES};
use audio::synth::AudioParams;
use js_sys::{Float32Array, Function, Reflect};
use wasm_bindgen::{JsCast, JsValue};

/// Most frames rendered in one animation frame (~170 ms; a longer stall is not caught up).
const MAX_FRAMES: usize = BLOCK_FRAMES * 32;

pub struct Sfx {
    mixer: Box<Mixer>,
    glue: JsValue,
    wanted: Function,
    push: Function,
    buf: Vec<f32>,
}

fn method(obj: &JsValue, name: &str) -> Option<Function> {
    Reflect::get(obj, &JsValue::from_str(name))
        .ok()?
        .dyn_into::<Function>()
        .ok()
}

impl Sfx {
    /// The sink, if the page has the glue.
    pub fn new(window: &web_sys::Window) -> Option<Sfx> {
        let glue = Reflect::get(window, &JsValue::from_str("skywaveSfx")).ok()?;
        if !glue.is_object() {
            return None;
        }
        Some(Sfx {
            mixer: Box::new(Mixer::new()),
            wanted: method(&glue, "wanted")?,
            push: method(&glue, "push")?,
            glue,
            buf: vec![0.0; MAX_FRAMES * 2],
        })
    }

    /// Once per animation frame, after `App::frame`: the new parameters, and as many
    /// blocks as the worklet wants.
    pub fn frame(&mut self, params: &AudioParams, dt: f64) {
        self.mixer.set_params(params);
        let Ok(n) = self.wanted.call1(&self.glue, &JsValue::from_f64(dt)) else {
            return;
        };
        let frames = (n.as_f64().unwrap_or(0.0).max(0.0) as usize).min(MAX_FRAMES);
        let frames = frames / BLOCK_FRAMES * BLOCK_FRAMES;
        if frames == 0 {
            return;
        }
        let out = &mut self.buf[..frames * 2];
        self.mixer.render(out);
        // SAFETY: the view lives only for this call; the glue copies it before returning,
        // and nothing on our side allocates (or grows wasm memory) while it exists.
        let view = unsafe { Float32Array::view(out) };
        let _ = self.push.call1(&self.glue, &view);
    }
}
