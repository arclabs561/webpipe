//! Minimal, deterministic text normalization helpers.
//!
//! Note: this module is webpipe-local specific glue that composes the workspace `textprep` crate
//! with a more aggressive “punctuation-as-separator” policy to improve retrieval/matching.

/// Conservative “scrub” used for matching/search keys.
///
/// - Unicode normalization + lowercase + diacritics stripping (via `textprep`)
/// - treat non-alphanumeric as separators (collapse to single spaces)
pub fn scrub(s: &str) -> String {
    // Start with a robust search-key normalization. This is intentionally lossy:
    // it is used only for matching/scoring, not for display.
    let s0 = textprep_crate::scrub_with(s, &textprep_crate::ScrubConfig::search_key());

    // Then apply a strict token separator policy: anything non-alphanumeric becomes a space.
    // This keeps behavior close to the old scrub (hyphens/underscores/slashes split tokens),
    // but also fixes cases like "v1.2" or "foo.bar" which were previously merged.
    let mut out = String::with_capacity(s0.len());
    let mut last_space = true;
    for ch in s0.chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim().to_string()
}
