//! Bakes each built-in world's place for the default seed in a release build, so the game
//! decodes it at load instead of generating it (`src/places.rs`): into the binary natively,
//! as `OUT_DIR/<scenario id>.place` files for the web build to ship beside the page. A
//! debug build bakes nothing (and generates at load), so `cargo check`, `clippy` and `test`
//! stay quick.

use std::fmt::Write as _;
use std::path::Path;

#[allow(dead_code)]
#[path = "src/builtin.rs"]
mod builtin;

/// The fields of a scenario the place depends on; the rest are the app's.
#[derive(serde::Deserialize)]
struct Scenario {
    region: String,
    #[serde(default)]
    layout: Option<String>,
    grammar: String,
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/builtin.rs");
    println!("cargo:rerun-if-changed=../../assets");
    println!("cargo:rerun-if-env-changed=PROFILE");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let bake = std::env::var("PROFILE").as_deref() == Ok("release");
    let mut table = String::from("&[\n");
    // The web build fetches each world's file when it is picked (`tools/web-build.sh`
    // copies them next to the page), so the wasm download stays small.
    let embed = std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("wasm32");
    for (id, text) in builtin::SCENARIOS.iter().filter(|_| bake) {
        let s: Scenario = ron::from_str(text).unwrap_or_else(|e| panic!("scenario {id}: {e}"));
        let text = builtin::grammar_text(&s.grammar).unwrap_or_else(|| panic!("{id}: grammar"));
        let grammar = builtin::load_builtin_grammar(text).unwrap_or_else(|e| panic!("{id}: {e}"));
        let place = builtin::generate(
            builtin::BAKED_SEED,
            &s.region,
            s.layout.as_deref(),
            &grammar,
        )
        .unwrap_or_else(|e| panic!("{id}: {e}"));
        let file = Path::new(&out).join(format!("{id}.place"));
        std::fs::write(&file, place::encode(&place)).expect("write the baked place");
        if embed {
            let file = file.display().to_string();
            writeln!(table, "    ({id:?}, include_bytes!({file:?})),").unwrap();
        }
    }
    table.push(']');
    std::fs::write(Path::new(&out).join("baked.rs"), table).expect("write baked.rs");
}
