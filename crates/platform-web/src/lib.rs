//! Browser shell for Skywave: owns the canvas, picks WebGPU or falls back to WebGL2,
//! feeds keyboard events to the input mapper and drives the frame loop with
//! `requestAnimationFrame`.
//!
//! JavaScript entry point (see `web/index.html`):
//! `await run(canvas, backend => { window.skywaveReady = true; })`.

#[cfg(target_arch = "wasm32")]
mod web;

/// Reads `seed=<u64>` from a URL query string such as `?seed=123&x=1`.
pub fn seed_from_query(query: &str) -> Option<u64> {
    query
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix("seed="))
        .and_then(|v| v.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::seed_from_query;

    #[test]
    fn parses_seed_parameter() {
        assert_eq!(seed_from_query("?seed=123"), Some(123));
        assert_eq!(
            seed_from_query("?a=1&seed=18446744073709551615"),
            Some(u64::MAX)
        );
        assert_eq!(seed_from_query("seed=7&seed=8"), Some(7));
        assert_eq!(seed_from_query(""), None);
        assert_eq!(seed_from_query("?seed=abc"), None);
        assert_eq!(seed_from_query("?myseed=4"), None);
    }
}
