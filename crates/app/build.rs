//! Bakes each built-in world's place for the default seed into the release build, so the
//! game decodes it at load instead of generating it (`src/places.rs`). A debug build bakes
//! nothing (and generates at load), so `cargo check`, `clippy` and `test` stay quick.

use std::fmt::Write as _;
use std::path::Path;

#[allow(dead_code)]
#[path = "src/builtin.rs"]
mod builtin;

/// The two fields of a scenario the place depends on; the rest are the app's.
#[derive(serde::Deserialize)]
struct Scenario {
    region: String,
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
    let mut done: Vec<(String, String)> = Vec::new();
    for (id, text) in builtin::SCENARIOS.iter().filter(|_| bake) {
        let s: Scenario = ron::from_str(text).unwrap_or_else(|e| panic!("scenario {id}: {e}"));
        if done.contains(&(s.region.clone(), s.grammar.clone())) {
            continue;
        }
        let text = builtin::grammar_text(&s.grammar).unwrap_or_else(|| panic!("{id}: grammar"));
        let grammar = builtin::load_builtin_grammar(text).unwrap_or_else(|e| panic!("{id}: {e}"));
        let place = builtin::generate(builtin::BAKED_SEED, &s.region, &grammar)
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        let file = Path::new(&out).join(format!("{id}.place"));
        std::fs::write(&file, place::encode(&place)).expect("write the baked place");
        writeln!(
            table,
            "    ({:?}, {:?}, include_bytes!({:?})),",
            s.region,
            s.grammar,
            file.display().to_string()
        )
        .unwrap();
        done.push((s.region, s.grammar));
    }
    table.push(']');
    std::fs::write(Path::new(&out).join("baked.rs"), table).expect("write baked.rs");
}
