use std::collections::BTreeSet;

fn canonicalize_url_for_compare(url: &str) -> String {
    // Best-effort canonicalization for overlap metrics:
    // - normalize scheme/host casing
    // - drop fragments
    // - preserve query (can matter for some docs sites)
    if let Ok(mut u) = url::Url::parse(url) {
        u.set_fragment(None);
        // url::Url normalizes host casing; scheme is already lowercased.
        return u.to_string();
    }
    url.trim().to_string()
}

pub fn url_set(urls: &[String], k: usize) -> BTreeSet<String> {
    urls.iter()
        .take(k)
        .map(|u| canonicalize_url_for_compare(u))
        .collect()
}

pub fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let uni = a.union(b).count() as f64;
    if uni == 0.0 {
        0.0
    } else {
        inter / uni
    }
}

pub fn diff(a: &BTreeSet<String>, b: &BTreeSet<String>) -> (Vec<String>, Vec<String>) {
    let a_only = a.difference(b).cloned().collect::<Vec<_>>();
    let b_only = b.difference(a).cloned().collect::<Vec<_>>();
    (a_only, b_only)
}

fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                cur.push(lc);
            }
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn shingles(tokens: &[String], k: usize) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if k == 0 || tokens.len() < k {
        return out;
    }
    for w in tokens.windows(k) {
        out.insert(w.join(" "));
    }
    out
}

pub fn text_jaccard(a: &str, b: &str, k: usize) -> f64 {
    let ta = tokenize(a);
    let tb = tokenize(b);
    let sa = shingles(&ta, k);
    let sb = shingles(&tb, k);
    jaccard(&sa, &sb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_drops_fragment() {
        let u = "https://example.com/path#section";
        let c = canonicalize_url_for_compare(u);
        assert_eq!(c, "https://example.com/path");
    }

    #[test]
    fn jaccard_basic() {
        let a = BTreeSet::from(["a".to_string(), "b".to_string()]);
        let b = BTreeSet::from(["b".to_string(), "c".to_string()]);
        let j = jaccard(&a, &b);
        assert!((j - (1.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn text_jaccard_is_high_for_similar_text() {
        let a = "Hello world. This is a test.";
        let b = "Hello world! This is a test";
        let j = text_jaccard(a, b, 2);
        assert!(j > 0.5, "j={}", j);
    }
}
