use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct LinkCandidate {
    pub url: String,
    pub text: String,
}

/// Extract (deduped) absolute links from HTML.
///
/// - Resolves relative links against `base_url` when provided.
/// - Drops fragments.
/// - Returns at most `max_links`.
pub fn extract_links(html: &str, base_url: Option<&str>, max_links: usize) -> Vec<String> {
    let max_links = max_links.min(500);
    if max_links == 0 {
        return Vec::new();
    }

    let base = base_url.and_then(|u| url::Url::parse(u).ok());
    let doc = html_scraper::Html::parse_document(html);
    let sel = match html_scraper::Selector::parse("a[href]") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut out = BTreeSet::new();
    for el in doc.select(&sel) {
        if out.len() >= max_links {
            break;
        }
        let href = match el.value().attr("href") {
            Some(h) => h.trim(),
            None => continue,
        };
        if href.is_empty() {
            continue;
        }
        // Skip javascript/mailto-like URLs early.
        let href_lc = href.to_ascii_lowercase();
        if href_lc.starts_with("javascript:") || href_lc.starts_with("mailto:") {
            continue;
        }

        let abs = if let Ok(u) = url::Url::parse(href) {
            u
        } else if let Some(b) = &base {
            match b.join(href) {
                Ok(u) => u,
                Err(_) => continue,
            }
        } else {
            continue;
        };

        // Drop fragments for stability.
        let mut u = abs;
        u.set_fragment(None);
        out.insert(u.to_string());
    }

    out.into_iter().take(max_links).collect()
}

/// Extract (deduped) absolute links from HTML with anchor text.
///
/// - Resolves relative links against `base_url` when provided.
/// - Drops fragments.
/// - Returns at most `max_links`.
///
/// This is intended for agentic discovery loops: anchor text often carries the
/// semantic cue (“Cursor Docs”, “MCP config”) that the URL string does not.
pub fn extract_link_candidates(
    html: &str,
    base_url: Option<&str>,
    max_links: usize,
) -> Vec<LinkCandidate> {
    let max_links = max_links.min(500);
    if max_links == 0 {
        return Vec::new();
    }

    let base = base_url.and_then(|u| url::Url::parse(u).ok());
    let doc = html_scraper::Html::parse_document(html);
    let sel = match html_scraper::Selector::parse("a[href]") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut seen = BTreeSet::<String>::new();
    let mut out: Vec<LinkCandidate> = Vec::new();
    for el in doc.select(&sel) {
        if out.len() >= max_links {
            break;
        }
        let href = match el.value().attr("href") {
            Some(h) => h.trim(),
            None => continue,
        };
        if href.is_empty() {
            continue;
        }
        let href_lc = href.to_ascii_lowercase();
        if href_lc.starts_with("javascript:") || href_lc.starts_with("mailto:") {
            continue;
        }

        let abs = if let Ok(u) = url::Url::parse(href) {
            u
        } else if let Some(b) = &base {
            match b.join(href) {
                Ok(u) => u,
                Err(_) => continue,
            }
        } else {
            continue;
        };

        let mut u = abs;
        u.set_fragment(None);
        let url = u.to_string();
        if !seen.insert(url.clone()) {
            continue;
        }

        let text = el
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        out.push(LinkCandidate { url, text });
    }

    out
}

/// Extract (deduped) absolute links from Markdown with link text.
///
/// Supports the common inline pattern: `[text](url)`.
/// - Resolves relative links against `base_url` when provided.
/// - Drops fragments.
/// - Returns at most `max_links`.
///
/// This is intentionally simple (no full Markdown parsing) to keep it fast and
/// dependency-light, but it restores agentic discovery when we fetch via
/// Firecrawl (markdown output).
pub fn extract_markdown_link_candidates(
    markdown: &str,
    base_url: Option<&str>,
    max_links: usize,
) -> Vec<LinkCandidate> {
    let max_links = max_links.min(500);
    if max_links == 0 {
        return Vec::new();
    }

    let base = base_url.and_then(|u| url::Url::parse(u).ok());
    let mut seen = BTreeSet::<String>::new();
    let mut out: Vec<LinkCandidate> = Vec::new();

    // Very small inline parser for `[text](url)`.
    let bytes = markdown.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && out.len() < max_links {
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }
        let close_bracket = match markdown[i + 1..].find(']') {
            Some(d) => i + 1 + d,
            None => break,
        };
        // Require `](` right after.
        if close_bracket + 1 >= bytes.len() || bytes[close_bracket + 1] != b'(' {
            i = close_bracket + 1;
            continue;
        }
        let close_paren = match markdown[close_bracket + 2..].find(')') {
            Some(d) => close_bracket + 2 + d,
            None => break,
        };

        let raw_text = markdown[i + 1..close_bracket].trim();
        let raw_url = markdown[close_bracket + 2..close_paren].trim();
        i = close_paren + 1;

        if raw_url.is_empty() {
            continue;
        }
        let url_lc = raw_url.to_ascii_lowercase();
        if url_lc.starts_with("javascript:") || url_lc.starts_with("mailto:") {
            continue;
        }

        let abs = if let Ok(u) = url::Url::parse(raw_url) {
            u
        } else if let Some(b) = &base {
            match b.join(raw_url) {
                Ok(u) => u,
                Err(_) => continue,
            }
        } else {
            continue;
        };
        let mut u = abs;
        u.set_fragment(None);
        let url = u.to_string();
        if !seen.insert(url.clone()) {
            continue;
        }

        let text = raw_text.split_whitespace().collect::<Vec<_>>().join(" ");
        out.push(LinkCandidate { url, text });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_and_resolves_links() {
        let html = r#"
        <html><body>
          <a href="/a#x">A</a>
          <a href="https://example.com/b">B</a>
          <a href="mailto:test@example.com">mail</a>
        </body></html>
        "#;
        let links = extract_links(html, Some("https://example.com/root"), 10);
        assert!(links.contains(&"https://example.com/a".to_string()));
        assert!(links.contains(&"https://example.com/b".to_string()));
        assert!(!links.iter().any(|u| u.starts_with("mailto:")));
    }

    #[test]
    fn extracts_link_candidates_with_text() {
        let html = r#"
        <html><body>
          <a href="/a#x">Hello Docs</a>
          <a href="https://example.com/b">B</a>
        </body></html>
        "#;
        let links = extract_link_candidates(html, Some("https://example.com/root"), 10);
        assert!(links.iter().any(|c| c.url == "https://example.com/a"));
        assert!(links.iter().any(|c| c.text.to_lowercase().contains("docs")));
    }

    #[test]
    fn extracts_markdown_link_candidates_with_text() {
        let md = r#"
See the [Cursor MCP docs](/context/model-context-protocol) and [Example](https://example.com/x#frag).
"#;
        let links = extract_markdown_link_candidates(md, Some("https://docs.example.com/"), 10);
        assert!(links
            .iter()
            .any(|c| c.url == "https://docs.example.com/context/model-context-protocol"));
        assert!(links.iter().any(|c| c.url == "https://example.com/x"));
        assert!(links
            .iter()
            .any(|c| c.text.to_lowercase().contains("cursor")));
    }
}
