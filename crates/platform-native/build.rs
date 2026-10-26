//! Sets `SKYWAVE_VERSION` for `skywave --version`: `git describe --tags --always --dirty`
//! when the source is a git checkout, else the `SKYWAVE_VERSION` environment variable (for
//! builds from a source archive), else the crate version.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (out.status.success() && !text.is_empty()).then(|| text.to_string())
}

/// Reruns this script when one of `files` in `dir` changes. Missing files are skipped: cargo
/// would rerun on every build for them.
fn watch(dir: &Path, files: &[&str]) {
    for path in files.iter().map(|f| dir.join(f)).filter(|p| p.exists()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=SKYWAVE_VERSION");
    // Rebuild when the commit, the tags or the staged state change. Edits that are not yet
    // staged do not rerun this, so `-dirty` can lag behind during an edit session.
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        watch(&PathBuf::from(dir), &["HEAD", "index"]);
    }
    if let Some(common) = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
        watch(&PathBuf::from(common), &["refs/tags", "packed-refs"]);
    }
    let version = git(&["describe", "--tags", "--always", "--dirty"])
        .or_else(|| {
            std::env::var("SKYWAVE_VERSION")
                .ok()
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    println!("cargo:rustc-env=SKYWAVE_VERSION={version}");
}
