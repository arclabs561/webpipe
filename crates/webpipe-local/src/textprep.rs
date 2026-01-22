//! Minimal, deterministic text normalization helpers.

/// Conservative “scrub” used for matching/search keys.
///
/// - ASCII lowercase
/// - collapse whitespace/punct into single spaces
/// - drop other punctuation/control
pub fn scrub(s: &str) -> String {
    let mut out = String::new();
    let mut last_space = true;
    for ch in s.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_space = false;
        } else if c.is_whitespace() || c == '-' || c == '_' || c == '/' {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            // drop
        }
    }
    out.trim().to_string()
}
