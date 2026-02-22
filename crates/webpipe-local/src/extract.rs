use crate::shellout;
use crate::textprep;
use serde::Serialize;
use slabs_crate::Chunker;
use std::io::Cursor;
use std::process::Command;

/// Convert HTML to readable plain text.
///
/// Notes:
/// - This is intentionally "good enough" and deterministic, not a full readability engine.
/// - Callers should apply their own output bounds (chars) if needed.
pub fn html_to_text(html: &str, width: usize) -> String {
    fn strip_block(html: &str, tag: &str) -> String {
        let lower = html.to_ascii_lowercase();
        let open_pat = format!("<{tag}");
        let close_pat = format!("</{tag}");
        let mut out = String::with_capacity(html.len());
        let mut i = 0usize;
        while i < html.len() {
            let Some(start_rel) = lower[i..].find(&open_pat) else {
                out.push_str(&html[i..]);
                break;
            };
            let start = i + start_rel;
            out.push_str(&html[i..start]);

            // Find the end of the closing tag: </tag ...>
            let Some(close_rel) = lower[start..].find(&close_pat) else {
                // Unterminated block; drop tail.
                break;
            };
            let close_start = start + close_rel;
            let Some(gt_rel) = lower[close_start..].find('>') else {
                break;
            };
            i = close_start + gt_rel + 1;
        }
        out
    }

    // Keep script/style content out of extracted text (deterministic, safety-first).
    let s = strip_block(html, "script");
    let s = strip_block(&s, "style");

    // html2text expects bytes.
    let out = html2text::from_read(Cursor::new(s.as_bytes()), width).unwrap_or(s);
    // html2text may emit whitespace/newlines even for “empty” pages; normalize that to empty
    // so downstream empty-extraction detection is meaningful.
    if !has_any_text(&out) {
        String::new()
    } else {
        out
    }
}

fn norm_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_any_text(s: &str) -> bool {
    s.chars().any(|c| !c.is_whitespace())
}

fn truncate_len_utf8_boundary(bytes: &[u8], max_len: usize) -> usize {
    // Choose a byte length <= max_len that is safe to decode as UTF-8.
    //
    // This is a best-effort quality improvement: truncating HTML mid-codepoint can introduce
    // replacement chars and can confuse HTML parsing (especially near tag boundaries).
    //
    // Bound work: we only backtrack a few bytes (UTF-8 codepoints are at most 4 bytes).
    let n = max_len.min(bytes.len());
    if n == bytes.len() {
        return n;
    }
    if std::str::from_utf8(&bytes[..n]).is_ok() {
        return n;
    }
    for back in 1..=4 {
        if n >= back && std::str::from_utf8(&bytes[..(n - back)]).is_ok() {
            return n - back;
        }
    }
    // Worst case: give up and return 0 to avoid panics / invalid slicing.
    0
}

#[derive(Debug, Clone, Serialize)]
pub struct StructuredBlock {
    pub kind: &'static str, // "heading" | "paragraph" | "list_item" | "code" | "other"
    /// Character offset into `structure_text`.
    pub start_char: usize,
    /// Character offset into `structure_text`.
    pub end_char: usize,
    /// Bounded block text.
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtractedStructure {
    pub engine: &'static str,
    pub title: Option<String>,
    pub outline: Vec<String>,
    /// Bounded concatenation of block texts (blocks joined with blank lines).
    pub structure_text: String,
    pub text_chars: usize,
    pub blocks: Vec<StructuredBlock>,
    pub warnings: Vec<&'static str>,
}

/// Extract text from a PDF body (in-memory bytes).
///
/// This is used for endpoints like arXiv `/pdf/...` and other PDF-first sources.
///
/// Notes:
/// - Callers should apply their own output bounds (chars) if needed.
/// - Extraction quality varies by PDF (text layer vs scanned images).
pub fn pdf_to_text(bytes: &[u8]) -> Result<String, String> {
    // `pdf-extract` is pure-Rust and works from memory; keep errors as strings
    // so callers can surface them as warnings without adding new error enums.
    //
    // Important: `pdf-extract` has had panics on some malformed PDFs. Since webpipe
    // treats “offline cache corpus scanning” as a best-effort workflow, we must
    // not let one bad cached PDF take down the whole scan.
    use std::cell::Cell;
    use std::sync::OnceLock;

    thread_local! {
        static SUPPRESS_PDF_PANIC_HOOK: Cell<bool> = const { Cell::new(false) };
    }
    static HOOK_INSTALLED: OnceLock<()> = OnceLock::new();
    HOOK_INSTALLED.get_or_init(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let suppressed = SUPPRESS_PDF_PANIC_HOOK.with(|c| c.get());
            if suppressed {
                return;
            }
            prev(info);
        }));
    });

    struct SuppressGuard;
    impl Drop for SuppressGuard {
        fn drop(&mut self) {
            SUPPRESS_PDF_PANIC_HOOK.with(|c| c.set(false));
        }
    }

    let r = std::panic::catch_unwind(|| {
        // Ensure any panic from the PDF parser does not spam stderr.
        SUPPRESS_PDF_PANIC_HOOK.with(|c| c.set(true));
        let _g = SuppressGuard;

        // Deterministic test hook: simulate a pdf-extract panic without relying on
        // crate-internal failure modes (kept under cfg(test)).
        #[cfg(test)]
        {
            if bytes.starts_with(b"WEBPIPE_TEST_PDF_EXTRACT_PANIC") {
                panic!("simulated pdf-extract panic");
            }
        }
        pdf_extract::extract_text_from_mem(bytes)
    });
    match r {
        Ok(inner) => inner.map_err(|e| e.to_string()),
        Err(_) => Err("pdf_extract_panicked".to_string()),
    }
}

/// Very low-fidelity PDF fallback: scan raw PDF bytes for plausible ASCII “word” runs.
///
/// This is meant as a last resort when PDF text extraction fails (or panics) and no shellout
/// tools are available. It will not recover text from compressed streams reliably, but it often
/// recovers metadata / uncompressed fragments that can still help downstream matching.
fn pdf_strings_fallback(bytes: &[u8], max_chars: usize) -> Option<String> {
    // Keep this deterministic and cheap (O(n) scan).
    const MIN_LEN: usize = 4;
    const MAX_LEN: usize = 48;

    fn keep_token(tok: &[u8]) -> bool {
        if tok.len() < MIN_LEN || tok.len() > MAX_LEN {
            return false;
        }
        if tok.iter().all(|b| b.is_ascii_digit()) {
            return false;
        }
        match tok {
            // Common PDF syntax / dictionary keys (very low signal).
            b"endobj"
            | b"stream"
            | b"endstream"
            | b"trailer"
            | b"startxref"
            | b"flatedecode"
            | b"filter"
            | b"length"
            | b"subtype"
            | b"resources"
            | b"mediabox"
            | b"contents"
            | b"basefont"
            | b"encoding"
            | b"objstm"
            | b"xrefstm" => false,
            _ => true,
        }
    }

    let mut out = String::new();
    out.reserve(max_chars.min(4096));
    let mut tok: Vec<u8> = Vec::with_capacity(32);
    let mut last_space = true;

    let flush = |tok: &mut Vec<u8>, out: &mut String, last_space: &mut bool| {
        if tok.is_empty() {
            return;
        }
        if keep_token(tok.as_slice()) {
            if !out.is_empty() && !*last_space {
                out.push(' ');
                *last_space = true;
            }
            // Tokens are stored lowercase ASCII.
            out.push_str(&String::from_utf8_lossy(tok));
            *last_space = false;
        }
        tok.clear();
    };

    for &b in bytes {
        if b.is_ascii_alphanumeric() {
            if tok.len() < MAX_LEN {
                tok.push(b.to_ascii_lowercase());
            } else {
                // Token too long; stop accumulating and wait for a separator to flush+drop it.
                // (This avoids large base64/hex-ish runs dominating output.)
            }
        } else {
            flush(&mut tok, &mut out, &mut last_space);
            if out.len() >= max_chars {
                break;
            }
            if !out.is_empty() && !last_space {
                out.push(' ');
                last_space = true;
            }
        }
        if out.len() >= max_chars {
            break;
        }
    }
    flush(&mut tok, &mut out, &mut last_space);

    let s = out.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn env_usize(key: &str, default: usize) -> usize {
    env(key)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(default)
}

fn pdf_to_text_shellout(bytes: &[u8]) -> Result<(&'static str, String), &'static str> {
    // Shellout is a robustness fallback: pure-Rust `pdf-extract` is preferred.
    //
    // Controls:
    // - WEBPIPE_PDF_SHELLOUT=off | auto | pdftotext | mutool
    let mode = env("WEBPIPE_PDF_SHELLOUT").unwrap_or_else(|| "auto".to_string());
    if mode == "off" {
        return Err("pdf_shellout_disabled");
    }

    // Safety cap: bound worst-case output by limiting pages for tools that support it.
    // If you want “try everything”, increase this explicitly.
    let max_pages = env_usize("WEBPIPE_PDF_SHELLOUT_MAX_PAGES", 25).clamp(1, 500);

    let mut tmp = tempfile::Builder::new()
        .prefix("webpipe-")
        .suffix(".pdf")
        .tempfile()
        .map_err(|_| "pdf_shellout_tempfile_failed")?;
    use std::io::Write;
    tmp.write_all(bytes)
        .map_err(|_| "pdf_shellout_tempfile_write_failed")?;
    let path = tmp.path().to_string_lossy().to_string();

    let timeout = shellout::timeout_from_env_ms("WEBPIPE_PDF_SHELLOUT_TIMEOUT_MS", 25_000);
    let max_chars = shellout::max_chars_from_env("WEBPIPE_PDF_SHELLOUT_MAX_CHARS", 250_000);
    let max_stdout_bytes = max_chars.saturating_mul(4).clamp(1_000, 8_000_000);

    fn run(
        bin: &str,
        args: &[&str],
        timeout: std::time::Duration,
        max_stdout_bytes: usize,
    ) -> Result<String, &'static str> {
        let mut cmd = Command::new(bin);
        cmd.args(args);
        let out =
            shellout::run_stdout_bounded(cmd, timeout, max_stdout_bytes).map_err(|e| match e {
                "shellout_tool_not_found" => "pdf_shellout_tool_not_found",
                "shellout_spawn_failed" => "pdf_shellout_spawn_failed",
                "shellout_wait_failed" => "pdf_shellout_wait_failed",
                "shellout_nonzero_exit" => "pdf_shellout_nonzero_exit",
                "shellout_timeout" => "pdf_shellout_timeout",
                "shellout_read_failed" => "pdf_shellout_read_failed",
                _ => "pdf_shellout_failed",
            })?;
        let s = String::from_utf8_lossy(&out).to_string();
        if !has_any_text(&s) {
            return Err("pdf_shellout_empty_output");
        }
        Ok(s)
    }

    // Try pdftotext first in auto mode (widely available via poppler).
    if mode == "auto" || mode == "pdftotext" {
        if let Ok(s) = run(
            "pdftotext",
            &[
                "-f",
                "1",
                "-l",
                &max_pages.to_string(),
                "-layout",
                "-nopgbrk",
                "-enc",
                "UTF-8",
                &path,
                "-",
            ],
            timeout,
            max_stdout_bytes,
        ) {
            return Ok(("pdf-pdftotext", s));
        }
        if mode == "pdftotext" {
            return Err("pdf_shellout_failed");
        }
    }

    if mode == "auto" || mode == "mutool" {
        if let Ok(s) = run(
            "mutool",
            &["draw", "-F", "text", "-o", "-", &path],
            timeout,
            max_stdout_bytes,
        ) {
            return Ok(("pdf-mutool", s));
        }
        return Err("pdf_shellout_failed");
    }

    Err("pdf_shellout_disabled")
}

/// Best-effort sniff for PDF bytes (magic header).
pub fn bytes_look_like_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

/// Best-effort guess for whether bytes are HTML-ish.
pub fn bytes_look_like_html(bytes: &[u8]) -> bool {
    // Best-effort sniff for HTML bytes.
    //
    // Goal: avoid “HTML treated as plain text” (bad downstream chunking), while staying
    // deterministic and cheap.
    //
    // Strategy:
    // - skip whitespace and BOM
    // - skip a small number of leading HTML comments (`<!-- ... -->`)
    // - then check for common tag/doctype prefixes
    let mut i = 0usize;

    // Skip BOM (UTF-8) if present.
    if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
        i = 3;
    }

    // Skip leading whitespace.
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() {
        return false;
    }

    // Skip up to N leading HTML comments.
    // This is common for some CDNs/templates: `<!-- ... -->\n<!doctype html>...`
    let mut comments_skipped = 0usize;
    while comments_skipped < 8 && i + 4 <= bytes.len() && &bytes[i..i + 4] == b"<!--" {
        // Find the end marker `-->` (bounded search window).
        // If unterminated, stop trying to be clever and fall back to false.
        let mut j = i + 4;
        let mut found = None;
        // Limit scanning to avoid worst-case quadratic behavior on huge bodies.
        let max_scan = (i + 200_000).min(bytes.len());
        while j + 3 <= max_scan {
            if &bytes[j..j + 3] == b"-->" {
                found = Some(j + 3);
                break;
            }
            j += 1;
        }
        let Some(next_i) = found else {
            break;
        };
        i = next_i;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        comments_skipped += 1;
        if i >= bytes.len() {
            return false;
        }
    }

    let rest = &bytes[i..];
    // Common prefixes; keep it conservative but practical.
    rest.starts_with(b"<!doctype")
        || rest.starts_with(b"<!DOCTYPE")
        || rest.starts_with(b"<html")
        || rest.starts_with(b"<HTML")
        || rest.starts_with(b"<head")
        || rest.starts_with(b"<body")
        || rest.starts_with(b"<meta")
        || rest.starts_with(b"<META")
        || rest.starts_with(b"<title")
        || rest.starts_with(b"<TITLE")
}

/// Best-effort sniff for common image formats.
pub fn bytes_look_like_image(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return true;
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return true;
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return true;
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return true;
    }
    false
}

fn class_or_id_lc(el: &html_scraper::ElementRef) -> String {
    let mut out = String::new();
    if let Some(c) = el.value().attr("class") {
        out.push_str(c);
        out.push(' ');
    }
    if let Some(i) = el.value().attr("id") {
        out.push_str(i);
    }
    out.to_ascii_lowercase()
}

fn is_generic_boilerplate_container(el: &html_scraper::ElementRef) -> bool {
    // Keep this generic: avoid site/host heuristics; only structural UI words.
    let s = class_or_id_lc(el);
    if s.is_empty() {
        return false;
    }
    for bad in [
        "nav",
        "navbar",
        "menu",
        "sidebar",
        "footer",
        "header",
        "banner",
        "cookie",
        "consent",
        "ads",
        "advert",
        "promo",
        "subscribe",
        "newsletter",
        "share",
        "social",
        "actions",
        "related",
        "listen",
        "speech",
    ] {
        if s.contains(bad) {
            return true;
        }
    }
    false
}

fn element_text_chars(el: &html_scraper::ElementRef) -> usize {
    el.text().map(|t| t.chars().count()).sum()
}

fn element_link_text_chars(el: &html_scraper::ElementRef) -> usize {
    let sel = html_scraper::Selector::parse("a").ok();
    let Some(sel) = sel else { return 0 };
    el.select(&sel)
        .map(|a| a.text().map(|t| t.chars().count()).sum::<usize>())
        .sum()
}

fn pick_main_text(html: &str, max_elems: usize, width: usize) -> Option<String> {
    let max_elems = max_elems.clamp(50, 50_000);
    let doc = html_scraper::Html::parse_document(html);

    let sel = html_scraper::Selector::parse("article, main, section, div").ok()?;
    let sel_p = html_scraper::Selector::parse("p").ok();
    let sel_li = html_scraper::Selector::parse("li").ok();
    let mut seen = 0usize;
    let mut best_score: i64 = 0;
    let mut best_html: Option<String> = None;

    for el in doc.select(&sel) {
        seen += 1;
        if seen > max_elems {
            break;
        }
        if is_generic_boilerplate_container(&el) {
            continue;
        }
        let txt = element_text_chars(&el);
        // Keep this low enough to work for small “single article” pages.
        // We rely on:
        // - tag bonuses (article/main)
        // - link-density penalties
        // to avoid picking pure nav widgets.
        if txt < 20 {
            continue;
        }
        let link_txt = element_link_text_chars(&el);
        // Prefer dense non-link text. Link text is usually navigation / TOCs / tag clouds.
        let non_link = txt.saturating_sub(link_txt);
        let mut score = (non_link as i64) - 3 * (link_txt as i64);
        let tag = el.value().name();
        if tag == "article" {
            score += 500;
        } else if tag == "main" {
            score += 300;
        }
        // Penalize suspiciously link-heavy blocks with a smooth-ish curve.
        if txt > 0 {
            // density in [0,1]
            let density = (link_txt as f64) / (txt as f64);
            if density >= 0.66 {
                score -= 900;
            } else if density >= 0.50 {
                score -= 500;
            } else if density >= 0.33 {
                score -= 250;
            }
        }
        // Prefer blocks with multiple paragraphs/list items (more “article-like” structure).
        if let Some(sel) = sel_p.as_ref() {
            let pc = el.select(sel).take(50).count() as i64;
            score += 20 * pc.min(10);
        }
        if let Some(sel) = sel_li.as_ref() {
            let lc = el.select(sel).take(100).count() as i64;
            // Lists can be good (docs) or bad (nav). Keep the bonus small.
            score += 3 * lc.min(20);
        }
        // Avoid tiny blocks even if they have tag bonuses.
        if non_link < 80 {
            score -= 200;
        }
        if score > best_score {
            best_score = score;
            // Preserve paragraph-ish structure by re-running html2text on the chosen subtree,
            // rather than flattening it to a single whitespace-normalized line.
            best_html = Some(el.html());
        }
    }

    let frag = best_html?;
    let txt = html_to_text(&frag, width);
    has_any_text(&txt).then_some(txt)
}

pub fn html_main_to_text(html: &str, width: usize) -> Option<String> {
    let out = pick_main_text(html, 20_000, width)?;
    has_any_text(&out).then_some(out)
}

fn pick_readability_text(html: &str, max_elems: usize, width: usize) -> Option<String> {
    // Readability-lite: only consider article/main containers (avoid div/section explosion).
    let max_elems = max_elems.clamp(50, 50_000);
    let doc = html_scraper::Html::parse_document(html);
    let sel = html_scraper::Selector::parse("article, main").ok()?;
    let sel_p = html_scraper::Selector::parse("p").ok();
    let mut seen = 0usize;
    let mut best_score: i64 = 0;
    let mut best_html: Option<String> = None;
    for el in doc.select(&sel) {
        seen += 1;
        if seen > max_elems {
            break;
        }
        if is_generic_boilerplate_container(&el) {
            continue;
        }
        let txt = element_text_chars(&el);
        if txt < 50 {
            continue;
        }
        let link_txt = element_link_text_chars(&el);
        let non_link = txt.saturating_sub(link_txt);
        let mut score = (non_link as i64) - 4 * (link_txt as i64);
        let tag = el.value().name();
        if tag == "article" {
            score += 700;
        } else if tag == "main" {
            score += 400;
        }
        if let Some(sel) = sel_p.as_ref() {
            let pc = el.select(sel).take(80).count() as i64;
            score += 30 * pc.min(12);
        }
        if non_link < 150 {
            score -= 300;
        }
        if score > best_score {
            best_score = score;
            best_html = Some(el.html());
        }
    }
    let frag = best_html?;
    let txt = html_to_text(&frag, width);
    has_any_text(&txt).then_some(txt)
}

pub fn html_readability_to_text(html: &str, width: usize) -> Option<String> {
    let out = pick_readability_text(html, 20_000, width)?;
    has_any_text(&out).then_some(out)
}

#[derive(Debug, Clone)]
pub struct ExtractedText {
    pub engine: &'static str,
    pub text: String,
    pub warnings: Vec<&'static str>,
}

fn clean_extracted_text(mut s: String) -> String {
    // Make outputs “clean” for downstream JSON/logging/clients:
    // - Normalize CRLF/CR to LF.
    // - Replace C0/C1-ish control characters with spaces (except \n/\t).
    // - Drop BOM/ZERO WIDTH NO-BREAK SPACE.
    //
    // Keep this deterministic and Unicode-safe (char-based).
    s = s.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch == '\u{FEFF}' {
            // BOM / ZWNBSP: invisible footgun.
            continue;
        }
        if ch == '\u{000C}' {
            // Formfeed: keep as a paragraph break (helps PDF-ish outputs).
            out.push('\n');
            continue;
        }
        if (ch <= '\u{001F}' && ch != '\n' && ch != '\t') || ch == '\u{007F}' {
            out.push(' ');
            continue;
        }
        out.push(ch);
    }
    out
}

fn truncate_to_chars(s: &str, max_chars: usize) -> (String, usize, bool) {
    if max_chars == 0 {
        return (String::new(), 0, s.chars().any(|c| !c.is_whitespace()));
    }
    let mut out = String::new();
    let mut n = 0usize;
    let mut clipped = false;
    for ch in s.chars() {
        if n >= max_chars {
            clipped = true;
            break;
        }
        out.push(ch);
        n += 1;
    }
    // If we stopped early, count is exactly max_chars. Otherwise it’s n.
    (out, n, clipped)
}

fn smart_truncate_to_chars_for_query(
    text: &str,
    query: &str,
    max_chars: usize,
    max_chunk_chars: usize,
) -> (String, usize, bool, bool) {
    // Return a contiguous window of `text` sized to max_chars.
    //
    // Default truncation is prefix-based, which can be bad for JS-heavy docs where the extracted
    // text starts with navigation chrome and the actual content is further down. When we have a
    // query, we prefer to keep a window around the best-matching chunk.
    //
    // Returns: (window_text, window_chars, clipped, used_query_window)
    if max_chars == 0 {
        return (
            String::new(),
            0,
            text.chars().any(|c| !c.is_whitespace()),
            false,
        );
    }
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return (text.to_string(), total_chars, false, false);
    }
    let q = query.trim();
    if q.is_empty() {
        let (out, n, clipped) = truncate_to_chars(text, max_chars);
        return (out, n, clipped, false);
    }

    // Best-effort: pick the single best chunk (query token overlap + nav penalties).
    // If we can't find a chunk, fall back to prefix truncation.
    let best = best_chunks_for_query(text, q, 1, max_chunk_chars);
    let Some(c) = best.first() else {
        let (out, n, clipped) = truncate_to_chars(text, max_chars);
        return (out, n, clipped, false);
    };

    // Center-ish window around the best chunk.
    let _chunk_len = c.end_char.saturating_sub(c.start_char).max(1);
    let pre = (max_chars / 3).min(max_chars.saturating_sub(1));
    let mut start = c.start_char.saturating_sub(pre);
    let mut end = start.saturating_add(max_chars);

    // Ensure the whole chunk fits.
    if end < c.end_char {
        let need = c.end_char.saturating_sub(end);
        start = start.saturating_add(need);
        end = end.saturating_add(need);
    }

    // Clamp to document bounds.
    if end > total_chars {
        let over = end.saturating_sub(total_chars);
        end = total_chars;
        start = start.saturating_sub(over);
    }
    start = start.min(total_chars);
    end = end.min(total_chars);
    if end <= start {
        let (out, n, clipped) = truncate_to_chars(text, max_chars);
        return (out, n, clipped, false);
    }

    let window = slice_chars(text, start, end);
    let window_chars = window.chars().count();
    let clipped = true;
    let used_query_window = start > 0;

    // If we somehow produced an empty/whitespace-only window, fall back.
    if !has_any_text(&window) {
        let (out, n, clipped2) = truncate_to_chars(text, max_chars);
        return (out, n, clipped2, false);
    }
    (window, window_chars, clipped, used_query_window)
}

#[derive(Debug, Clone)]
pub struct ExtractPipelineResult {
    pub extracted: ExtractedText,
    pub text_chars: usize,
    pub text_truncated: bool,
    pub structure: Option<ExtractedStructure>,
    pub chunks: Vec<ScoredChunk>,
}

#[derive(Debug, Clone)]
pub struct ExtractPipelineCfg<'a> {
    pub query: Option<&'a str>,
    pub width: usize,
    pub max_chars: usize,
    pub top_chunks: usize,
    pub max_chunk_chars: usize,
    pub include_structure: bool,
    pub max_outline_items: usize,
    pub max_blocks: usize,
    pub max_block_chars: usize,
}

/// Shared “extract pipeline” used by multiple tools:
/// extracted text → truncate → optional structure → chunks.
///
/// This exists to avoid re-running the (sometimes expensive) “best-effort text extraction”
/// when callers already computed it (e.g. for empty-extraction detection).
pub fn extract_pipeline_from_extracted(
    bytes: &[u8],
    content_type: Option<&str>,
    final_url: &str,
    extracted0: ExtractedText,
    cfg: ExtractPipelineCfg<'_>,
) -> ExtractPipelineResult {
    let query = cfg.query.unwrap_or("").trim();
    let (text, text_chars, text_truncated, used_query_window) = smart_truncate_to_chars_for_query(
        &extracted0.text,
        query,
        cfg.max_chars,
        cfg.max_chunk_chars,
    );
    let mut warnings = extracted0.warnings;
    if used_query_window {
        warnings.push("text_windowed_for_query");
    }
    let extracted = ExtractedText {
        engine: extracted0.engine,
        text,
        warnings,
    };

    let structure = if cfg.include_structure {
        Some(best_effort_structure_from_bytes(
            bytes,
            content_type,
            final_url,
            &extracted,
            cfg.max_outline_items,
            cfg.max_blocks,
            cfg.max_block_chars,
        ))
    } else {
        None
    };

    let chunks = if query.is_empty() {
        best_chunks_default(&extracted.text, cfg.top_chunks, cfg.max_chunk_chars)
    } else if let Some(s) = structure.as_ref() {
        // “Amazing by default”: if query matching yields no chunks (e.g. misspellings,
        // synonyms, or short pages), fall back to a reasonable default chunk selection
        // rather than returning an empty chunk list.
        let out = best_chunks_for_query_in_structure(s, query, cfg.top_chunks, cfg.max_chunk_chars);
        if out.is_empty() && has_any_text(&s.structure_text) {
            best_chunks_default(&s.structure_text, cfg.top_chunks, cfg.max_chunk_chars)
        } else {
            out
        }
    } else {
        let out =
            best_chunks_for_query(&extracted.text, query, cfg.top_chunks, cfg.max_chunk_chars);
        if out.is_empty() && has_any_text(&extracted.text) {
            best_chunks_default(&extracted.text, cfg.top_chunks, cfg.max_chunk_chars)
        } else {
            out
        }
    };

    ExtractPipelineResult {
        extracted,
        text_chars,
        text_truncated,
        structure,
        chunks,
    }
}

fn best_chunks_default(text: &str, top_k: usize, max_chunk_chars: usize) -> Vec<ScoredChunk> {
    let top_k = top_k.clamp(1, 50);
    let max_chunk_chars = max_chunk_chars.clamp(50, 5_000);

    let spans = find_candidate_spans(text, max_chunk_chars);
    let mut out: Vec<ScoredChunk> = Vec::new();
    for (sb, eb) in spans {
        if out.len() >= top_k {
            break;
        }
        let slice = text.get(sb..eb).unwrap_or("");
        let slice_trim = slice.trim();
        if slice_trim.is_empty() {
            continue;
        }
        // Skip very short “nav-ish” fragments; we want something a human can read.
        if slice_trim.chars().count() < 60 {
            continue;
        }
        let (snippet, _clipped) = truncate_chars(slice_trim, max_chunk_chars);
        // Avoid returning obvious portal/nav/metadata chunks as “default” evidence.
        // This is best-effort; callers can still force include_text=true if needed.
        if chunk_penalty(&snippet) >= 120 {
            continue;
        }
        let start_char = byte_to_char_index(text, sb);
        let end_char = byte_to_char_index(text, eb);
        out.push(ScoredChunk {
            start_char,
            end_char,
            // Score is “present” but query-less; keep it non-zero for downstream selectors.
            score: 1,
            text: snippet,
        });
    }
    if out.is_empty() && !text.trim().is_empty() {
        // Fall back to the least-nav-like paragraph rather than taking the whole document prefix.
        // This avoids returning obvious portal/promotional chrome when paragraphs exist but were
        // filtered out by the "early pick" loop above.
        let spans = find_candidate_spans(text, max_chunk_chars);
        let mut best: Option<(u64, ScoredChunk)> = None;
        for (sb, eb) in spans {
            let slice = text.get(sb..eb).unwrap_or("");
            let slice_trim = slice.trim();
            if slice_trim.is_empty() {
                continue;
            }
            if slice_trim.chars().count() < 60 {
                continue;
            }
            let (snippet, _clipped) = truncate_chars(slice_trim, max_chunk_chars);
            let pen = chunk_penalty(&snippet);
            let start_char = byte_to_char_index(text, sb);
            let end_char = byte_to_char_index(text, eb);
            let cand = ScoredChunk {
                start_char,
                end_char,
                score: 1,
                text: snippet,
            };
            match best {
                None => best = Some((pen, cand)),
                Some((bp, _)) if pen < bp => best = Some((pen, cand)),
                _ => {}
            }
        }
        if let Some((_p, c)) = best {
            out.push(c);
        } else {
            let (snippet, _clipped) = truncate_chars(text.trim(), max_chunk_chars);
            out.push(ScoredChunk {
                start_char: 0,
                end_char: text.chars().count(),
                score: 1,
                text: snippet,
            });
        }
    }
    out
}

/// Shared “extract pipeline” used by multiple tools:
/// bytes → best-effort text → truncate → optional structure → chunks.
pub fn extract_pipeline_from_bytes(
    bytes: &[u8],
    content_type: Option<&str>,
    final_url: &str,
    cfg: ExtractPipelineCfg<'_>,
) -> ExtractPipelineResult {
    let extracted0 = best_effort_text_from_bytes(bytes, content_type, final_url, cfg.width, 500);
    extract_pipeline_from_extracted(bytes, content_type, final_url, extracted0, cfg)
}

fn push_block(
    blocks: &mut Vec<StructuredBlock>,
    out_text: &mut String,
    kind: &'static str,
    text: String,
    max_block_chars: usize,
) {
    if text.trim().is_empty() {
        return;
    }
    let max_block_chars = max_block_chars.clamp(20, 2_000);
    let mut clipped = String::new();
    for (n, ch) in text.chars().enumerate() {
        if n >= max_block_chars {
            break;
        }
        clipped.push(ch);
    }
    if !out_text.is_empty() {
        out_text.push_str("\n\n");
    }
    let start_char = out_text.chars().count();
    out_text.push_str(&clipped);
    let end_char = out_text.chars().count();
    blocks.push(StructuredBlock {
        kind,
        start_char,
        end_char,
        text: clipped,
    });
}

fn extract_structure_from_html(
    html: &str,
    max_outline: usize,
    max_blocks: usize,
    max_block_chars: usize,
) -> ExtractedStructure {
    let max_outline = max_outline.clamp(0, 200);
    let max_blocks = max_blocks.clamp(1, 200);
    let max_block_chars = max_block_chars.clamp(20, 2_000);

    let doc = html_scraper::Html::parse_document(html);
    let mut title: Option<String> = None;
    if let Ok(sel) = html_scraper::Selector::parse("title") {
        if let Some(el) = doc.select(&sel).next() {
            let t = norm_ws(&el.text().collect::<Vec<_>>().join(" "));
            if !t.is_empty() {
                title = Some(t);
            }
        }
    }

    let mut outline: Vec<String> = Vec::new();
    let mut blocks: Vec<StructuredBlock> = Vec::new();
    let mut structure_text = String::new();
    let mut warnings: Vec<&'static str> = Vec::new();

    let sel = html_scraper::Selector::parse("h1,h2,h3,p,li,pre").ok();
    if let Some(sel) = sel {
        for el in doc.select(&sel) {
            if blocks.len() >= max_blocks {
                break;
            }
            let tag = el.value().name();
            let raw = if tag == "pre" {
                // Preserve line breaks for code blocks (bounded later).
                el.text().collect::<Vec<_>>().join("\n")
            } else {
                el.text().collect::<Vec<_>>().join(" ")
            };
            let text = if tag == "pre" {
                raw.trim().to_string()
            } else {
                norm_ws(&raw)
            };
            if text.is_empty() {
                continue;
            }
            match tag {
                "h1" | "h2" | "h3" => {
                    if outline.len() < max_outline {
                        outline.push(text.clone());
                    }
                    push_block(
                        &mut blocks,
                        &mut structure_text,
                        "heading",
                        text,
                        max_block_chars,
                    );
                }
                "p" => {
                    push_block(
                        &mut blocks,
                        &mut structure_text,
                        "paragraph",
                        text,
                        max_block_chars,
                    );
                }
                "li" => {
                    push_block(
                        &mut blocks,
                        &mut structure_text,
                        "list_item",
                        text,
                        max_block_chars,
                    );
                }
                "pre" => {
                    // Treat as code; preserve newlines for readability.
                    push_block(
                        &mut blocks,
                        &mut structure_text,
                        "code",
                        text,
                        max_block_chars,
                    );
                }
                _ => {
                    push_block(
                        &mut blocks,
                        &mut structure_text,
                        "other",
                        text,
                        max_block_chars,
                    );
                }
            }
        }
    } else {
        warnings.push("structure_parse_failed");
    }

    ExtractedStructure {
        engine: "html",
        title,
        outline,
        text_chars: structure_text.chars().count(),
        structure_text,
        blocks,
        warnings,
    }
}

fn extract_structure_from_text(
    engine: &'static str,
    text: &str,
    max_outline: usize,
    max_blocks: usize,
    max_block_chars: usize,
) -> ExtractedStructure {
    let max_outline = max_outline.clamp(0, 200);
    let max_blocks = max_blocks.clamp(1, 200);
    let max_block_chars = max_block_chars.clamp(20, 2_000);

    let mut outline: Vec<String> = Vec::new();
    let mut blocks: Vec<StructuredBlock> = Vec::new();
    let mut structure_text = String::new();
    let warnings: Vec<&'static str> = Vec::new();

    // PDFs often come as single-newline wrapped text; normalize so we get real paragraphs.
    // Keep it generic + deterministic: preserve blank lines/formfeeds as paragraph breaks,
    // but otherwise treat line breaks as spaces.
    let pdfish = engine == "pdf-extract" || engine.starts_with("pdf-");
    let normalized = if pdfish {
        let mut out = String::new();
        let mut prev_blank = true;
        let t = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\u{000C}', "\n\n"); // formfeed
        for line in t.split('\n') {
            let l = line.trim();
            if l.is_empty() {
                if !prev_blank {
                    out.push_str("\n\n");
                }
                prev_blank = true;
                continue;
            }
            if !prev_blank {
                out.push(' ');
            }
            out.push_str(l);
            prev_blank = false;
        }
        out
    } else {
        text.to_string()
    };

    // Paragraph splitter (2+ newlines).
    for para in normalized.split("\n\n") {
        if blocks.len() >= max_blocks {
            break;
        }
        let p = norm_ws(para);
        if p.is_empty() {
            continue;
        }
        let kind = if engine == "markdown" && p.starts_with('#') {
            "heading"
        } else {
            "paragraph"
        };
        let cleaned = if kind == "heading" {
            p.trim_start_matches('#').trim().to_string()
        } else {
            p
        };
        if kind == "heading" && outline.len() < max_outline && !cleaned.is_empty() {
            outline.push(cleaned.clone());
        }
        push_block(
            &mut blocks,
            &mut structure_text,
            kind,
            cleaned,
            max_block_chars,
        );
    }

    ExtractedStructure {
        engine,
        title: None,
        outline,
        text_chars: structure_text.chars().count(),
        structure_text,
        blocks,
        warnings,
    }
}

pub fn best_effort_structure_from_bytes(
    bytes: &[u8],
    content_type: Option<&str>,
    final_url: &str,
    extracted: &ExtractedText,
    max_outline: usize,
    max_blocks: usize,
    max_block_chars: usize,
) -> ExtractedStructure {
    let _ = final_url;
    fn html_has_pathologically_long_token(bytes: &[u8]) -> bool {
        // `html_scraper::Html::parse_document` can be surprisingly slow on documents that contain
        // extremely long “words” (e.g. minified bundles, base64 blobs, or adversarial pages).
        //
        // If we detect a huge unbroken alnum run, skip HTML parsing for structure and fall back to
        // text-based structure on the already-extracted text.
        let mut run: usize = 0;
        for &b in bytes.iter().take(500_000) {
            if b.is_ascii_alphanumeric() {
                run = run.saturating_add(1);
                if run >= 20_000 {
                    return true;
                }
            } else {
                run = 0;
            }
        }
        false
    }

    // Prefer HTML structure if we actually have HTML-ish bytes and it's not likely to be
    // pathological for the parser.
    if bytes_look_like_html(bytes) && !html_has_pathologically_long_token(bytes) {
        let html = String::from_utf8_lossy(bytes).to_string();
        return extract_structure_from_html(&html, max_outline, max_blocks, max_block_chars);
    }

    // JSON: pretty-print if parseable, and use top-level keys as outline.
    let ct0 = content_type_lc_prefix(content_type);
    let is_json = ct0 == "application/json" || ct0.ends_with("+json") || extracted.engine == "json";
    if is_json {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
            let mut outline: Vec<String> = Vec::new();
            if let Some(obj) = v.as_object() {
                for (k, _) in obj.iter().take(max_outline.min(200)) {
                    outline.push(k.clone());
                }
            }
            let pretty =
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| extracted.text.clone());
            let mut s =
                extract_structure_from_text("json", &pretty, max_outline, 1, max_block_chars);
            s.outline = outline;
            return s;
        }
        return extract_structure_from_text(
            "json",
            &extracted.text,
            max_outline,
            max_blocks,
            max_block_chars,
        );
    }

    // Use extracted text for everything else (pdf/markdown/xml/text/html_hint/html_main).
    let engine = match extracted.engine {
        "pdf-extract" => "pdf-extract",
        "markdown" => "markdown",
        "xml" => "xml",
        "text" => "text",
        "firecrawl" => "markdown",
        "html_main" | "html2text" | "html_hint" => "text",
        other => other,
    };
    let mut s = extract_structure_from_text(
        engine,
        &extracted.text,
        max_outline,
        max_blocks,
        max_block_chars,
    );
    if bytes_look_like_html(bytes) {
        s.warnings.push("structure_html_skipped_long_token");
    }
    s
}

fn content_type_lc_prefix(ct: Option<&str>) -> String {
    ct.unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn strip_tag_blocks(html: &str, tag: &str) -> String {
    // Minimal, best-effort stripper for <tag ...> ... </tag> blocks.
    //
    // This is intentionally conservative: it only removes when it finds a close tag,
    // and it is ASCII-case-insensitive on tag names.
    let tag_lc = tag.to_ascii_lowercase();
    let open_pat = format!("<{}", tag_lc);
    let close_pat = format!("</{}>", tag_lc);

    let mut out = String::new();
    let mut i = 0usize;
    let lower = html.to_ascii_lowercase();
    while let Some(rel_start) = lower[i..].find(&open_pat) {
        let start = i + rel_start;
        // Find the matching close tag after start.
        let after_open = start + open_pat.len();
        if let Some(rel_end) = lower[after_open..].find(&close_pat) {
            let end = after_open + rel_end + close_pat.len();
            out.push_str(&html[i..start]);
            i = end;
        } else {
            // No close tag; stop stripping.
            break;
        }
    }
    out.push_str(&html[i..]);
    out
}

fn detect_client_redirect(html: &str) -> Option<String> {
    let doc = html_scraper::Html::parse_document(html);
    // 1. Meta refresh: <meta http-equiv="refresh" content="0; url=..." />
    if let Ok(sel) = html_scraper::Selector::parse("meta[http-equiv]") {
        for el in doc.select(&sel) {
            if let Some(equiv) = el.value().attr("http-equiv") {
                if equiv.trim().eq_ignore_ascii_case("refresh") {
                    if let Some(content) = el.value().attr("content") {
                        // content="0; url=http://..."
                        // We are loose with parsing: just look for url=...
                        if let Some(url_part) = content
                            .split(';')
                            .map(|p| p.trim())
                            .find(|p| p.to_ascii_lowercase().starts_with("url="))
                        {
                            let url = url_part
                                .split_once('=')
                                .map(|(_, u)| u.trim())
                                .unwrap_or("");
                            if !url.is_empty() {
                                return Some(url.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. "Click here to be redirected" patterns (common on some static site generators).
    // Only check if the body is very short to avoid false positives.
    if html.len() < 4000 {
        let text = html_to_text(html, 1000);
        let lower = text.to_ascii_lowercase();
        if lower.contains("click here to be redirected") || lower.contains("moved permanently") {
            // Try to find the first link in the doc.
            if let Ok(sel_a) = html_scraper::Selector::parse("a[href]") {
                if let Some(el) = doc.select(&sel_a).next() {
                    if let Some(href) = el.value().attr("href") {
                        return Some(href.to_string());
                    }
                }
            }
        }
    }

    None
}

fn openreview_notes_api_to_text(v: &serde_json::Value) -> Option<String> {
    // OpenReview notes API shape: {"notes":[{...,"content":{...}}],"count":...}
    let notes = v.get("notes")?.as_array()?;
    let n0 = notes.first()?.as_object()?;
    // Heuristic: require OpenReview-ish fields so we don't accidentally “pretty” other APIs.
    if !n0.contains_key("forum") && !n0.contains_key("invitation") {
        return None;
    }
    let id = n0.get("id").and_then(|x| x.as_str()).unwrap_or("").trim();
    let content = n0.get("content")?.as_object()?;
    let title = content
        .get("title")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim();
    let abstract0 = content
        .get("abstract")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim();
    if title.is_empty() && abstract0.is_empty() {
        return None;
    }

    fn clamp_chars(s: &str, max: usize) -> String {
        let mut out = String::new();
        for (i, ch) in s.chars().enumerate() {
            if i >= max {
                break;
            }
            out.push(ch);
        }
        out
    }

    let mut out = String::new();
    out.push_str("OpenReview note\n");
    if !id.is_empty() {
        out.push_str("id: ");
        out.push_str(id);
        out.push('\n');
    }
    if !title.is_empty() {
        out.push_str("title: ");
        out.push_str(&clamp_chars(title, 4_000));
        out.push('\n');
    }
    if let Some(authors) = content.get("authors").and_then(|x| x.as_array()) {
        let names: Vec<&str> = authors.iter().filter_map(|x| x.as_str()).collect();
        if !names.is_empty() {
            out.push_str("authors: ");
            out.push_str(&clamp_chars(&names.join(", "), 4_000));
            out.push('\n');
        }
    }
    if let Some(venue) = content.get("venue").and_then(|x| x.as_str()) {
        let v = venue.trim();
        if !v.is_empty() {
            out.push_str("venue: ");
            out.push_str(&clamp_chars(v, 1_000));
            out.push('\n');
        }
    }
    if !abstract0.is_empty() {
        out.push_str("abstract: ");
        out.push_str(&clamp_chars(abstract0, 20_000));
        out.push('\n');
    }
    Some(out)
}

/// Extract best-effort readable text from a fetched body.
///
/// The goal is “good enough” evidence text for downstream chunking/LLM use:
/// - HTML: html2text, with hint-text fallback when empty.
/// - PDF: pdf-extract.
/// - Markdown/text/json/xml: treat as text (no HTML rendering).
/// - Unknown/binary: empty text + warning.
pub fn best_effort_text_from_bytes(
    bytes: &[u8],
    content_type: Option<&str>,
    final_url: &str,
    width: usize,
    hint_max_chars: usize,
) -> ExtractedText {
    let mut warnings: Vec<&'static str> = Vec::new();

    let ct0 = content_type_lc_prefix(content_type);
    let is_pdf = ct0 == "application/pdf" || bytes_look_like_pdf(bytes);
    if is_pdf {
        return match pdf_to_text(bytes) {
            Ok(t) => {
                if has_any_text(&t) {
                    ExtractedText {
                        engine: "pdf-extract",
                        text: clean_extracted_text(t),
                        warnings,
                    }
                } else {
                    // Some PDFs have no extractable text in the Rust-native backend (scanned / weird encodings).
                    // Opportunistically try shellout tools if available.
                    warnings.push("pdf_extract_failed");
                    match pdf_to_text_shellout(bytes) {
                        Ok((engine, text)) => {
                            warnings.push("pdf_shellout_used");
                            ExtractedText {
                                engine,
                                text: clean_extracted_text(text),
                                warnings,
                            }
                        }
                        Err(_) => {
                            warnings.push("pdf_shellout_unavailable");
                            if let Some(s) = pdf_strings_fallback(bytes, 50_000) {
                                warnings.push("pdf_strings_fallback_used");
                                ExtractedText {
                                    engine: "pdf-strings",
                                    text: clean_extracted_text(s),
                                    warnings,
                                }
                            } else {
                                ExtractedText {
                                    engine: "pdf-extract",
                                    text: String::new(),
                                    warnings,
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warnings.push("pdf_extract_failed");
                if e == "pdf_extract_panicked" {
                    warnings.push("pdf_extract_panicked");
                }
                match pdf_to_text_shellout(bytes) {
                    Ok((engine, text)) => {
                        warnings.push("pdf_shellout_used");
                        ExtractedText {
                            engine,
                            text: clean_extracted_text(text),
                            warnings,
                        }
                    }
                    Err(_) => {
                        warnings.push("pdf_shellout_unavailable");
                        if let Some(s) = pdf_strings_fallback(bytes, 50_000) {
                            warnings.push("pdf_strings_fallback_used");
                            ExtractedText {
                                engine: "pdf-strings",
                                text: clean_extracted_text(s),
                                warnings,
                            }
                        } else {
                            ExtractedText {
                                engine: "pdf-extract",
                                text: String::new(),
                                warnings,
                            }
                        }
                    }
                }
            }
        };
    }

    // YouTube transcripts (produced by LocalFetcher as text) should have a stable engine.
    if ct0 == "text/x-youtube-transcript" {
        let text = clean_extracted_text(String::from_utf8_lossy(bytes).to_string());
        return ExtractedText {
            engine: "youtube_transcript",
            text,
            warnings,
        };
    }

    // Pandoc: opportunistic conversion for common “document” formats.
    // This is a robustness feature: without it, these content-types are “unknown” (empty).
    if shellout::looks_like_doc_or_epub(&ct0, final_url) {
        match shellout::pandoc_to_text(bytes, content_type, final_url) {
            Ok(text) => {
                warnings.push("pandoc_used");
                return ExtractedText {
                    engine: "pandoc",
                    text,
                    warnings,
                };
            }
            Err(code) => {
                // Only surface the attempt as a warning in auto mode.
                // (If the user wants strict, they can set WEBPIPE_PANDOC=strict and handle it upstream.)
                let mode = shellout::pandoc_mode_from_env();
                if mode == "auto" {
                    warnings.push("pandoc_failed");
                    // Keep going: fall through to image/text/html heuristics.
                } else if mode == "strict" {
                    warnings.push(code);
                    return ExtractedText {
                        engine: "pandoc",
                        text: String::new(),
                        warnings,
                    };
                }
            }
        }
    }

    // Images: we don't have a Rust-native OCR/vision backend yet.
    // Treat as supported-but-empty so downstream callers get a stable engine + warning.
    let is_webp = bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP";
    let is_image = ct0.starts_with("image/")
        || bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xff\xd8\xff")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || is_webp;
    if is_image {
        // Opportunistic OCR via tesseract, if installed and enabled.
        match shellout::tesseract_ocr(bytes, content_type, final_url) {
            Ok(text) => {
                warnings.push("tesseract_used");
                return ExtractedText {
                    engine: "image_ocr",
                    text,
                    warnings,
                };
            }
            Err(code) => {
                let mode = shellout::ocr_mode_from_env();
                if mode == "auto" {
                    warnings.push("image_no_text_extraction");
                } else if mode == "strict" {
                    warnings.push(code);
                } else {
                    warnings.push("image_no_text_extraction");
                }
                return ExtractedText {
                    engine: "image",
                    text: String::new(),
                    warnings,
                };
            }
        }
    }

    // Video/media: opportunistically extract embedded subtitle streams via ffmpeg.
    if shellout::looks_like_video(&ct0, final_url) {
        match shellout::ffmpeg_extract_subtitles_vtt(bytes, content_type, final_url) {
            Ok(vtt) => {
                // Reuse the VTT normalizer from youtube module (it’s deterministic and bounded).
                let max_chars =
                    shellout::max_chars_from_env("WEBPIPE_MEDIA_SUBS_MAX_CHARS", 200_000);
                let text = crate::youtube::vtt_to_text_for_media(&vtt, max_chars);
                if text.chars().any(|c| !c.is_whitespace()) {
                    warnings.push("ffmpeg_subtitles_used");
                    return ExtractedText {
                        engine: "media_subtitles",
                        text,
                        warnings,
                    };
                }
            }
            Err(code) => {
                let mode = shellout::media_subs_mode_from_env();
                if mode == "auto" {
                    warnings.push("media_no_text_extraction");
                } else if mode == "strict" {
                    warnings.push(code);
                } else {
                    warnings.push("media_no_text_extraction");
                }
                return ExtractedText {
                    engine: "media",
                    text: String::new(),
                    warnings,
                };
            }
        }
    }

    // Treat common text-like content types as plain text. This avoids trying to “render”
    // JSON/markdown/xml through html2text, which usually produces noisy output.
    let is_markdown = ct0 == "text/markdown" || ct0 == "text/x-markdown";
    let is_json = ct0 == "application/json" || ct0.ends_with("+json");
    let is_xml = ct0 == "application/xml" || ct0 == "text/xml" || ct0.ends_with("+xml");
    let is_html_ct = ct0.starts_with("text/html") || ct0 == "application/xhtml+xml";
    let is_text = ct0.starts_with("text/") || is_markdown || is_json || is_xml;
    if is_text && !is_html_ct && !bytes_look_like_html(bytes) {
        let engine = if is_markdown {
            "markdown"
        } else if is_json {
            "json"
        } else if is_xml {
            "xml"
        } else {
            "text"
        };
        let mut text0 = String::from_utf8_lossy(bytes).to_string();
        if engine == "json" {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
                if let Some(t) = openreview_notes_api_to_text(&v) {
                    text0 = t;
                }
            }
        }
        let text = clean_extracted_text(text0);
        return ExtractedText {
            engine,
            text,
            warnings,
        };
    }

    // Default: HTML-ish (or unknown-but-seems-text). Prefer a “main content” extraction when
    // it clearly improves signal; otherwise fall back to whole-page html2text.
    // Bound worst-case HTML parsing cost: extraction output is bounded by max_chars anyway.
    let max_html_bytes =
        env_usize("WEBPIPE_EXTRACT_MAX_BYTES", 2_000_000).clamp(50_000, 20_000_000);
    let html_bytes = if bytes.len() > max_html_bytes {
        warnings.push("extract_input_truncated");
        let n = truncate_len_utf8_boundary(bytes, max_html_bytes);
        &bytes[..n]
    } else {
        bytes
    };
    let html0 = String::from_utf8_lossy(html_bytes).to_string();

    // Check for client-side redirects before stripping tags.
    if let Some(target) = detect_client_redirect(&html0) {
        warnings.push("client_side_redirect");
        return ExtractedText {
            engine: "redirect",
            text: target,
            warnings,
        };
    }

    // Strip script/style/noscript blocks before html2text to avoid counting JS/CSS as “content”.
    // This keeps “script-only” pages as empty (so higher-level fallbacks can trigger).
    let html1 = strip_tag_blocks(&html0, "script");
    let html2 = strip_tag_blocks(&html1, "style");
    let html = strip_tag_blocks(&html2, "noscript");
    let full = html_to_text(&html, width);
    let main = html_main_to_text(&html, width);
    let readability = html_readability_to_text(&html, width);

    fn quality_score(s: &str) -> i64 {
        let non_ws = s.chars().filter(|c| !c.is_whitespace()).count() as i64;
        let url_hits = s.matches("http").count() as i64;
        // Penalize “link soup”.
        let mut score = non_ws - 200 * url_hits;

        // Penalize pages dominated by many short lines (common for nav/menus).
        // This is generic (structure-based), not domain-specific.
        let short_lines = s
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .filter(|l| l.chars().count() <= 30)
            .count() as i64;
        score -= 20 * short_lines;

        // Penalize common UI boilerplate tokens (kept small + generic).
        let sl = s.to_ascii_lowercase();
        for needle in [
            "sign up",
            "log in",
            "login",
            "cookie",
            "consent",
            "privacy",
            "terms",
            // Generic docs UI chrome signals (helps JS-heavy doc sites).
            "skip to content",
            "search documentation",
            "search docs",
            "feedback",
            "edit this page",
            "listen",
            "share",
            "follow",
        ] {
            let hits = sl.matches(needle).count() as i64;
            score -= 250 * hits;
        }

        score
    }

    let full_ok = has_any_text(&full);
    let main_ok = main.as_ref().map(|t| has_any_text(t)).unwrap_or(false);
    let read_ok = readability
        .as_ref()
        .map(|t| has_any_text(t))
        .unwrap_or(false);

    // Prefer the best “main-like” output when it meaningfully improves over whole-page text.
    if full_ok || main_ok || read_ok {
        let s_full = if full_ok { quality_score(&full) } else { 0 };
        let s_main = if main_ok {
            quality_score(main.as_ref().unwrap())
        } else {
            i64::MIN / 4
        };
        let s_read = if read_ok {
            quality_score(readability.as_ref().unwrap())
        } else {
            i64::MIN / 4
        };
        let (best_engine, best_text, best_score) = if s_read > s_main {
            ("readability", readability, s_read)
        } else {
            ("html_main", main, s_main)
        };
        if (best_engine != "html_main" && read_ok) || main_ok {
            // Require a gap vs full (unless full is empty).
            // For nav-heavy docs pages, "full" is often dominated by chrome; use a smaller
            // threshold when the full text has many short lines.
            let short_lines = full
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .filter(|l| l.chars().count() <= 30)
                .count() as i64;
            let threshold = if short_lines >= 80 { 120 } else { 300 };
            if !full_ok || best_score >= s_full + threshold {
                warnings.push("boilerplate_reduced");
                return ExtractedText {
                    engine: best_engine,
                    text: clean_extracted_text(best_text.unwrap()),
                    warnings,
                };
            }
        }
    }

    if full_ok {
        return ExtractedText {
            engine: "html2text",
            text: clean_extracted_text(full),
            warnings,
        };
    }

    // If html2text yields nothing but the body isn't empty, use a tiny hint fallback.
    if !bytes.is_empty() {
        let hint = html_hint_text(&html, hint_max_chars);
        if hint.chars().any(|c| !c.is_whitespace()) {
            warnings.push("hint_text_fallback");
            return ExtractedText {
                engine: "html_hint",
                text: clean_extracted_text(norm_ws(&hint)),
                warnings,
            };
        }
    }

    // If we still have nothing, treat as unsupported/binary.
    if !final_url.trim().is_empty() {
        warnings.push("unsupported_content_no_text");
    }
    ExtractedText {
        engine: "unknown",
        text: String::new(),
        warnings,
    }
}

/// Extract a small, deterministic “hint text” for URL selection.
///
/// Intended for cheap pre-ranking of URL seed lists, not full retrieval:
/// - title + first h1/h2 (if present)
/// - bounded to a small character budget
pub fn html_hint_text(html: &str, max_chars: usize) -> String {
    let max_chars = max_chars.clamp(50, 2_000);
    let doc = html_scraper::Html::parse_document(html);

    fn first_text(doc: &html_scraper::Html, selector: &str) -> Option<String> {
        let sel = html_scraper::Selector::parse(selector).ok()?;
        let el = doc.select(&sel).next()?;
        let t = el.text().collect::<Vec<_>>().join(" ");
        let t = t.trim().to_string();
        (!t.is_empty()).then_some(t)
    }

    fn first_attr(doc: &html_scraper::Html, selector: &str, attr: &str) -> Option<String> {
        let sel = html_scraper::Selector::parse(selector).ok()?;
        let el = doc.select(&sel).next()?;
        let v = el.value().attr(attr)?;
        let v = v.trim().to_string();
        (!v.is_empty()).then_some(v)
    }

    let mut parts = Vec::new();
    if let Some(t) = first_text(&doc, "title") {
        parts.push(t);
    }
    // Meta descriptions are often the only “human text” on JS-heavy docs shells.
    if let Some(d) = first_attr(&doc, "meta[name=\"description\"]", "content") {
        parts.push(d);
    }
    // Twitter cards are common on docs/blogs; keep it as a best-effort hint source.
    if let Some(d) = first_attr(&doc, "meta[name=\"twitter:description\"]", "content") {
        parts.push(d);
    }
    if let Some(d) = first_attr(&doc, "meta[property=\"og:description\"]", "content") {
        parts.push(d);
    }
    if let Some(t) = first_text(&doc, "h1") {
        parts.push(t);
    }
    if let Some(t) = first_text(&doc, "h2") {
        parts.push(t);
    }
    if let Some(t) = first_text(&doc, "h3") {
        parts.push(t);
    }

    let joined = parts.join("\n");
    let (out, _clipped) = truncate_chars(&joined, max_chars);
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct ScoredChunk {
    /// Character offset into the provided `text`.
    pub start_char: usize,
    /// Character offset into the provided `text`.
    pub end_char: usize,
    /// Simple integer score (higher is better).
    pub score: u64,
    /// Snippet text for this chunk (bounded).
    pub text: String,
}

fn is_query_stopword(t: &str) -> bool {
    // Stopwords are high-recall across most pages and usually harm ranking quality
    // when treated as evidence-bearing query tokens.
    //
    // Keep the list centralized (shared across the workspace) rather than duplicating
    // bespoke sets in each consumer.
    textprep_crate::stopwords::is_english_stopword(t)
}

fn is_query_noise_token(t: &str) -> bool {
    let s = t.trim();
    if s.is_empty() {
        return true;
    }
    // Drop ultra-common URL noise tokens that match broadly in extracted link soup.
    // This directly improves cache-corpus precision (and reduces false overlap).
    //
    // Note: keep tokens like "http2" / "httpsig" etc; only drop the bare scheme-ish tokens.
    let sl = s.to_ascii_lowercase();
    if matches!(sl.as_str(), "http" | "https" | "www") {
        return true;
    }
    // Drop tiny numeric tokens (too broad).
    if s.chars().all(|c| c.is_ascii_digit()) {
        return s.len() <= 2;
    }
    // Drop small "vN" / "vN.N" version markers.
    if s.len() <= 6 {
        if let Some(rest) = sl.strip_prefix('v') {
            let rest = rest.trim();
            if !rest.is_empty()
                && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
                && rest.chars().any(|c| c.is_ascii_digit())
            {
                return true;
            }
        }
        // Drop small "rcN" markers.
        if let Some(rest) = sl.strip_prefix("rc") {
            let rest = rest.trim();
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                return true;
            }
        }
    }
    false
}

fn tokenize_query_for_match(query: &str) -> Vec<String> {
    // Use the same core normalization as other webpipe matching paths:
    // Unicode-aware scrub (case + diacritics) plus punctuation-as-separator,
    // then a conservative token split.
    //
    // We intentionally avoid fancy stemming here; we just want to be resilient to
    // punctuation and case differences like "MCP-stdio" vs "mcp stdio".
    // NOTE: This tokenization directly influences ranking and cache-corpus recall.
    // We bias toward **precision**:
    // - drop "version noise" tokens like v1/rc1 that match broadly across many pages
    // - drop very short purely-numeric tokens (1/2/3) that also match broadly
    let q = textprep::scrub(query);
    let mut toks: Vec<String> = q
        .split(|ch: char| !ch.is_alphanumeric())
        .filter_map(|t| {
            let t = t.trim();
            if t.len() >= 2 && !is_query_noise_token(t) && !is_query_stopword(t) {
                Some(t.to_string())
            } else {
                None
            }
        })
        .collect();

    // If the query contains any “meaningful” token (len>=3), drop short tokens (len<3).
    // This reduces false overlaps like "to" -> "tokio" after prefix matching.
    let has_meaningful = toks.iter().any(|t| t.len() >= 3);
    if has_meaningful {
        toks.retain(|t| t.len() >= 3);
    }
    toks
}

fn tokenize_query_for_phrases(query: &str) -> Vec<String> {
    // Similar to `tokenize_query_for_match`, but preserve order for phrase extraction.
    // This is intentionally conservative: phrases are a bonus signal, not a primary matcher.
    let q = textprep::scrub(query);
    q.split_whitespace()
        .filter_map(|t| {
            let t = t.trim();
            if t.len() >= 3 && !is_query_noise_token(t) && !is_query_stopword(t) {
                Some(t.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn build_query_phrase_matcher(query: &str) -> Option<textprep_crate::FlashText> {
    let toks = tokenize_query_for_phrases(query);
    if toks.len() < 2 || toks.len() > 8 {
        return None;
    }

    let mut phrases: Vec<String> = Vec::new();
    for i in 0..toks.len().saturating_sub(1) {
        phrases.push(format!(" {} {} ", toks[i], toks[i + 1]));
    }
    if toks.len() <= 6 {
        for i in 0..toks.len().saturating_sub(2) {
            phrases.push(format!(" {} {} {} ", toks[i], toks[i + 1], toks[i + 2]));
        }
    }
    phrases.sort();
    phrases.dedup();
    phrases.truncate(16);

    if phrases.is_empty() {
        return None;
    }

    let mut ft = textprep_crate::FlashText::new();
    for p in phrases {
        // Keyword and value are identical; we only care about match spans/count.
        ft.add_keyword(p.clone(), p);
    }
    Some(ft)
}

fn phrase_bonus_for_chunk(
    phrase_matcher: &mut Option<textprep_crate::FlashText>,
    padded_scrub_buf: &mut String,
    matches_buf: &mut Vec<textprep_crate::KeywordMatch>,
    chunk_scrub: &str,
) -> u64 {
    let Some(ft) = phrase_matcher.as_mut() else {
        return 0;
    };

    // Matcher patterns include leading+trailing spaces to enforce whole-token phrase boundaries.
    padded_scrub_buf.clear();
    padded_scrub_buf.push(' ');
    padded_scrub_buf.push_str(chunk_scrub);
    padded_scrub_buf.push(' ');

    ft.find_into(padded_scrub_buf, matches_buf);
    if matches_buf.is_empty() {
        return 0;
    }

    // Trigram matches are stronger than bigrams; keep the scale modest vs token weights.
    let mut bonus = 0u64;
    for m in matches_buf.iter().take(8) {
        let n = m.keyword.split_whitespace().count();
        bonus = bonus.saturating_add(match n {
            2 => 120,
            3 => 200,
            _ => 150,
        });
    }
    bonus
}

fn query_tok_match_strength(qtok: &str, w: &str) -> u8 {
    // Query tokens are scrubbed ASCII alnum runs.
    //
    // - Numeric tokens: exact match only (404 should not match 4040).
    // - Very short tokens: exact match only to avoid prefix false positives ("go" vs "google").
    // - Otherwise: exact match is strongest; prefix match is a weaker fallback (timeout -> timeouts).
    if qtok.chars().all(|c| c.is_ascii_digit()) || qtok.len() < 3 {
        if w == qtok {
            2
        } else {
            0
        }
    } else if w == qtok {
        2
    } else if w.starts_with(qtok) {
        1
    } else {
        0
    }
}

fn query_tok_matches_word(qtok: &str, w: &str) -> bool {
    query_tok_match_strength(qtok, w) > 0
}

fn chunk_penalty(text: &str) -> u64 {
    // Penalize chunks that look like navigation / UI boilerplate.
    //
    // Design goals:
    // - Keep it deterministic and generic (no site-specific CSS class heuristics here).
    // - Prefer “real paragraphs” over menus (many short lines) even when they share query tokens.
    // - Avoid over-penalizing legitimate docs content: only apply heavy penalties when multiple
    //   signals agree (e.g. many short lines + multiple nav tokens).
    let t = text.trim();
    if t.is_empty() {
        return 0;
    }

    let short_lines = t
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter(|l| l.chars().count() <= 30)
        .count() as u64;

    let lc = t.to_ascii_lowercase();

    // Table-of-contents / navigation-chrome patterns (common on docs sites).
    //
    // These frequently contain many query tokens (section titles) but are not evidence.
    let toc_like = lc.contains("table of contents") || lc.contains("table-of-contents");
    let bracket_refs = t.matches('[').count() as u64;
    let period_count = t.matches('.').count() as u64;
    let bracket_nav_like = bracket_refs >= 10 && period_count < 2 && t.chars().count() >= 220;
    let promo_like = lc.contains("buy your ticket")
        || lc.contains("join us for")
        || lc.contains("register now")
        || lc.contains("limited time");
    let nav_chrome_like = (lc.contains("navigation")
        && (lc.contains("next") || lc.contains("previous")))
        || (lc.contains("this page") && lc.contains("report a bug") && lc.contains("show source"))
        || (lc.contains("index")
            && lc.contains("modules")
            && (lc.contains("next") || lc.contains("previous")));

    // “Hard” UI words: strong indicators of chrome rather than content.
    let mut hard_hits = 0u64;
    for needle in [
        "sign up", "log in", "login", "register", "cookie", "consent", "privacy", "terms",
    ] {
        if lc.contains(needle) {
            hard_hits += 1;
        }
    }

    // “Soft” nav tokens: only penalize heavily when the chunk is already nav-shaped.
    let mut nav_hits = 0u64;
    for needle in [
        "models",
        "datasets",
        "spaces",
        "pricing",
        "docs",
        "documentation",
        "about",
        "blog",
        "download",
        "dependencies",
        "repository",
        "crates.io",
        "license",
        "owners",
        "builds",
        "badges",
        "metadata",
        "permalink",
    ] {
        if lc.contains(needle) {
            nav_hits += 1;
        }
    }

    // Penalize link soup in a snippet (common on portals).
    //
    // Important: don't treat the plain token "HTTP" (as in "HTTP 404") as link soup.
    // Only count explicit URL-ish patterns.
    let url_hits = (lc.matches("http://").count()
        + lc.matches("https://").count()
        + lc.matches("www.").count()) as u64;

    // Metadata-heavy portal signals (common on “crate pages”, app listings, etc.).
    // This is intentionally conservative: we only apply a big penalty when multiple markers appear.
    let mut meta_hits = 0u64;
    for needle in [
        "dependencies",
        "repository",
        "crates.io",
        "owners",
        "license",
        "permalink",
    ] {
        if lc.contains(needle) {
            meta_hits += 1;
        }
    }

    // Penalize extremely repetitive chunks (common when extracted text loses layout/newlines and
    // navigation becomes a long single-line “word salad”).
    //
    // Keep this bounded and conservative: only trigger on long snippets with *very* low lexical
    // diversity or long runs of the same token.
    let repetition_like = {
        let scrub = textprep::scrub(t);
        let mut unique = std::collections::BTreeSet::<&str>::new();
        let mut total = 0u64;
        let mut max_run = 0u64;
        let mut cur_run = 0u64;
        let mut prev: Option<&str> = None;
        for w in scrub.split_whitespace().take(400) {
            total += 1;
            unique.insert(w);
            if prev == Some(w) {
                cur_run += 1;
            } else {
                cur_run = 1;
                prev = Some(w);
            }
            if cur_run > max_run {
                max_run = cur_run;
            }
        }
        let unique_count = unique.len() as u64;
        total >= 50
            && (unique_count.saturating_mul(100) <= total.saturating_mul(12) || max_run >= 25)
    };

    let mut p = 0u64;
    // Many short lines is the core nav-shape signal.
    p = p.saturating_add(short_lines.saturating_mul(4));
    // TOC / chrome is strongly non-evidentiary, even if it contains query tokens.
    if toc_like {
        p = p.saturating_add(260);
    }
    if bracket_nav_like {
        p = p.saturating_add(160);
    }
    if promo_like {
        p = p.saturating_add(140);
    }
    if nav_chrome_like {
        p = p.saturating_add(160);
    }
    // Hard UI tokens are strong, even without short_lines.
    p = p.saturating_add(hard_hits.saturating_mul(60));
    // Nav tokens should mostly matter when the chunk looks like nav (many short lines).
    if short_lines >= 10 && nav_hits >= 2 {
        p = p.saturating_add(220);
    } else {
        p = p.saturating_add(nav_hits.saturating_mul(8));
    }
    p = p.saturating_add(url_hits.saturating_mul(40));
    if meta_hits >= 3 {
        p = p.saturating_add(180);
    }
    if repetition_like {
        p = p.saturating_add(200);
    }
    p
}

fn truncate_chars(s: &str, max_chars: usize) -> (String, bool) {
    if max_chars == 0 {
        return ("".to_string(), !s.is_empty());
    }
    let mut out = String::new();
    for (n, ch) in s.chars().enumerate() {
        if n >= max_chars {
            return (out, true);
        }
        out.push(ch);
    }
    (out, false)
}

fn byte_to_char_index(s: &str, byte: usize) -> usize {
    s.get(..byte).map(|p| p.chars().count()).unwrap_or(0)
}

fn find_paragraph_spans(text: &str) -> Vec<(usize, usize)> {
    // Split on 2+ newlines, keeping spans in bytes.
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;

    while i + 1 < bytes.len() {
        if bytes[i] == b'\n' && bytes[i + 1] == b'\n' {
            let end = i;
            if end > start {
                spans.push((start, end));
            }
            // Skip all consecutive newlines.
            while i < bytes.len() && bytes[i] == b'\n' {
                i += 1;
            }
            start = i;
            continue;
        }
        i += 1;
    }
    if start < bytes.len() {
        spans.push((start, bytes.len()));
    }
    spans
}

fn avg_bytes_per_char(text: &str) -> usize {
    let chars = text.chars().count().max(1);
    // Bounded estimate: UTF-8 is at most 4 bytes per char.
    text.len().div_ceil(chars).clamp(1, 4)
}

fn find_candidate_spans(text: &str, max_chunk_chars: usize) -> Vec<(usize, usize)> {
    let spans = find_paragraph_spans(text);
    if spans.len() >= 2 || text.trim().is_empty() {
        return spans;
    }

    // Fallback: when we only got 0-1 “paragraph spans”, generate more candidate spans using a
    // recursive separator hierarchy (via `slabs`). This helps with:
    // - text/plain pages that use single newlines
    // - extracted HTML where blank lines were lost
    // - “nav word-salad” pages that become one huge line
    let max_chunk_chars = max_chunk_chars.clamp(50, 5_000);
    let bpc = avg_bytes_per_char(text);
    let max_size_bytes = max_chunk_chars
        .saturating_mul(bpc)
        // Keep this bounded; we only need “reasonable chunks”, not a perfect segmentation.
        .clamp(120, 50_000);

    let chunker = slabs_crate::RecursiveChunker::prose(max_size_bytes);
    let slabs = chunker.chunk(text);
    if slabs.len() < 2 {
        return spans;
    }

    let mut out: Vec<(usize, usize)> = Vec::with_capacity(slabs.len());
    for s in slabs.into_iter().take(500) {
        if s.end > s.start {
            out.push((s.start, s.end));
        }
    }
    if out.len() >= 2 {
        out
    } else {
        spans
    }
}

pub fn best_chunks_for_query(
    text: &str,
    query: &str,
    top_k: usize,
    max_chunk_chars: usize,
) -> Vec<ScoredChunk> {
    let top_k = top_k.clamp(1, 50);
    let max_chunk_chars = max_chunk_chars.clamp(50, 5_000);

    let mut q_toks = tokenize_query_for_match(query);
    // Dedup for determinism and to avoid accidental over-weighting of repeated query words.
    q_toks.sort();
    q_toks.dedup();
    if q_toks.is_empty() {
        return Vec::new();
    }

    let mut phrase_matcher = build_query_phrase_matcher(query);
    let mut padded_scrub_buf = String::new();
    let mut phrase_matches_buf: Vec<textprep_crate::KeywordMatch> = Vec::new();

    let spans = find_candidate_spans(text, max_chunk_chars);
    let use_masks = q_toks.len() <= 63;

    struct Cand {
        start_char: usize,
        end_char: usize,
        penalty: u64,
        phrase_bonus: u64,
        exact_mask: u64,
        prefix_mask: u64,
        hits: u64,
        text: String,
    }

    let mut cands: Vec<Cand> = Vec::new();
    for (sb, eb) in spans {
        let slice = text.get(sb..eb).unwrap_or("");
        let slice_trim = slice.trim();
        if slice_trim.is_empty() {
            continue;
        }

        let slice_scrub = textprep::scrub(slice_trim);
        let toks = textprep_crate::tokenize::tokenize_refs_with_offsets(&slice_scrub);

        let mut hits = 0u64;
        let mut exact_mask = 0u64;
        let mut prefix_mask = 0u64;
        if use_masks {
            for tok in toks.iter().take(1_000) {
                let w = tok.text;
                for (i, t) in q_toks.iter().enumerate() {
                    // Prefix match within tokens:
                    // - keeps light “stemming-ish” behavior (timeout -> timeouts)
                    // - avoids infix matches (art should not match cart)
                    let bit = 1u64 << i;
                    let strength = query_tok_match_strength(t.as_str(), w);
                    if strength == 2 {
                        exact_mask |= bit;
                        prefix_mask &= !bit;
                    } else if strength == 1 && (exact_mask & bit) == 0 {
                        prefix_mask |= bit;
                    }
                }
                if (exact_mask | prefix_mask).count_ones() as usize == q_toks.len() {
                    break;
                }
            }
            hits = (exact_mask | prefix_mask).count_ones() as u64;
        } else {
            for t in &q_toks {
                if toks
                    .iter()
                    .any(|w| query_tok_matches_word(t.as_str(), w.text))
                {
                    hits += 1;
                }
            }
        }
        if hits == 0 {
            continue;
        }

        let (snippet, _clipped) = truncate_chars(slice_trim, max_chunk_chars);
        let start_char = byte_to_char_index(text, sb);
        let end_char = byte_to_char_index(text, eb);

        let penalty = chunk_penalty(&snippet);
        let phrase_bonus = phrase_bonus_for_chunk(
            &mut phrase_matcher,
            &mut padded_scrub_buf,
            &mut phrase_matches_buf,
            &slice_scrub,
        );

        cands.push(Cand {
            start_char,
            end_char,
            penalty,
            phrase_bonus,
            exact_mask,
            prefix_mask,
            hits,
            text: snippet,
        });
    }

    if cands.is_empty() {
        return Vec::new();
    }

    // If the query token set is small enough, apply a lightweight within-page rarity weighting:
    // tokens that appear in many candidate chunks contribute less than rare tokens.
    let mut scored: Vec<ScoredChunk> = Vec::with_capacity(cands.len());
    if use_masks {
        let mut df = vec![0u64; q_toks.len()];
        for c in &cands {
            let mut m = c.exact_mask | c.prefix_mask;
            while m != 0 {
                let i = m.trailing_zeros() as usize;
                df[i] = df[i].saturating_add(1);
                m &= m - 1;
            }
        }
        let mut weights = vec![0u64; q_toks.len()];
        for (i, t) in q_toks.iter().enumerate() {
            let len = t.chars().count().clamp(2, 12) as u64;
            let dfi = df[i].max(1);
            weights[i] = len.saturating_mul(100) / dfi;
        }

        // Prefix matches contribute less than exact matches.
        const PREFIX_SCALE_PCT: u64 = 70;

        for c in cands {
            let mut base = 0u64;
            // Exact hits.
            let mut m = c.exact_mask;
            while m != 0 {
                let i = m.trailing_zeros() as usize;
                base = base.saturating_add(weights[i]);
                m &= m - 1;
            }
            // Prefix hits (not exact).
            let mut pm = c.prefix_mask;
            while pm != 0 {
                let i = pm.trailing_zeros() as usize;
                base = base.saturating_add(weights[i].saturating_mul(PREFIX_SCALE_PCT) / 100);
                pm &= pm - 1;
            }
            base = base.saturating_add(c.phrase_bonus);
            let score = base.saturating_sub(c.penalty);
            scored.push(ScoredChunk {
                start_char: c.start_char,
                end_char: c.end_char,
                score,
                text: c.text,
            });
        }
    } else {
        // Fallback for very long queries: keep the older "hits * 100" scoring.
        for c in cands {
            let base = c.hits.saturating_mul(100).saturating_add(c.phrase_bonus);
            let score = base.saturating_sub(c.penalty);
            scored.push(ScoredChunk {
                start_char: c.start_char,
                end_char: c.end_char,
                score,
                text: c.text,
            });
        }
    }

    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.start_char.cmp(&b.start_char))
    });
    dedupe_and_truncate_chunks(scored, top_k)
}

fn slice_chars(s: &str, start_char: usize, end_char: usize) -> String {
    if end_char <= start_char {
        return String::new();
    }
    s.chars()
        .skip(start_char)
        .take(end_char.saturating_sub(start_char))
        .collect()
}

fn chunk_dedupe_key(text: &str) -> String {
    // Keep this cheap + stable: normalize whitespace, lowercase, and cap the key length.
    // This helps collapse repeated “heading + paragraph” expansions that produce identical snippets.
    let mut words = Vec::new();
    for w in text.split_whitespace() {
        words.push(w);
        if words.len() >= 80 {
            break;
        }
    }
    words.join(" ").to_ascii_lowercase()
}

fn dedupe_and_truncate_chunks(chunks: Vec<ScoredChunk>, top_k: usize) -> Vec<ScoredChunk> {
    let mut out = Vec::new();
    let mut seen_ranges = std::collections::BTreeSet::<(usize, usize)>::new();
    let mut seen_text = std::collections::BTreeSet::<String>::new();
    for c in chunks {
        if out.len() >= top_k {
            break;
        }
        let r = (c.start_char, c.end_char);
        if !seen_ranges.insert(r) {
            continue;
        }
        let k = chunk_dedupe_key(&c.text);
        if k.is_empty() || !seen_text.insert(k) {
            continue;
        }
        out.push(c);
    }
    out
}

/// Structure-aware chunking: prefer blocks and keep heading context.
///
/// Returns chunks in the same `ScoredChunk` shape, but offsets are in `structure.structure_text`.
pub fn best_chunks_for_query_in_structure(
    structure: &ExtractedStructure,
    query: &str,
    top_k: usize,
    max_chunk_chars: usize,
) -> Vec<ScoredChunk> {
    let top_k = top_k.clamp(1, 50);
    let max_chunk_chars = max_chunk_chars.clamp(50, 5_000);

    let mut q_toks = tokenize_query_for_match(query);
    q_toks.sort();
    q_toks.dedup();
    if q_toks.is_empty() {
        return Vec::new();
    }

    // Candidate: pick any block containing any token; then expand to include heading context.
    let blocks = &structure.blocks;
    if blocks.is_empty() || structure.structure_text.is_empty() {
        return Vec::new();
    }

    let mut phrase_matcher = build_query_phrase_matcher(query);
    let mut padded_scrub_buf = String::new();
    let mut phrase_matches_buf: Vec<textprep_crate::KeywordMatch> = Vec::new();
    let use_masks = q_toks.len() <= 63;

    struct Cand {
        start_char: usize,
        end_char: usize,
        penalty: u64,
        phrase_bonus: u64,
        block_exact_mask: u64,
        block_prefix_mask: u64,
        heading_exact_mask: u64,
        heading_prefix_mask: u64,
        hits: u64,
        heading_bonus_hits: u64,
        text: String,
    }

    let mut cands: Vec<Cand> = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        let b_scrub = textprep::scrub(&b.text);
        let btoks = textprep_crate::tokenize::tokenize_refs_with_offsets(&b_scrub);

        let mut hits = 0u64;
        let mut block_exact_mask = 0u64;
        let mut block_prefix_mask = 0u64;
        if use_masks {
            for tok in btoks.iter().take(1_000) {
                let w = tok.text;
                for (qi, t) in q_toks.iter().enumerate() {
                    let bit = 1u64 << qi;
                    let strength = query_tok_match_strength(t.as_str(), w);
                    if strength == 2 {
                        block_exact_mask |= bit;
                        block_prefix_mask &= !bit;
                    } else if strength == 1 && (block_exact_mask & bit) == 0 {
                        block_prefix_mask |= bit;
                    }
                }
                if (block_exact_mask | block_prefix_mask).count_ones() as usize == q_toks.len() {
                    break;
                }
            }
            hits = (block_exact_mask | block_prefix_mask).count_ones() as u64;
        } else {
            for t in &q_toks {
                if btoks
                    .iter()
                    .any(|w| query_tok_matches_word(t.as_str(), w.text))
                {
                    hits += 1;
                }
            }
        }
        if hits == 0 {
            continue;
        }

        // Find nearest preceding heading (bounded lookback).
        let mut j = i;
        let mut heading_exact_mask = 0u64;
        let mut heading_prefix_mask = 0u64;
        let mut heading_bonus_hits = 0u64;
        for back in 0..=10usize {
            if i < back {
                break;
            }
            let k = i - back;
            if blocks[k].kind == "heading" {
                j = k;
                // If the heading itself matches tokens, reward it.
                let h_scrub = textprep::scrub(&blocks[k].text);
                let htoks = textprep_crate::tokenize::tokenize_refs_with_offsets(&h_scrub);

                if use_masks {
                    for tok in htoks.iter().take(400) {
                        let w = tok.text;
                        for (qi, t) in q_toks.iter().enumerate() {
                            let bit = 1u64 << qi;
                            let strength = query_tok_match_strength(t.as_str(), w);
                            if strength == 2 {
                                heading_exact_mask |= bit;
                                heading_prefix_mask &= !bit;
                            } else if strength == 1 && (heading_exact_mask & bit) == 0 {
                                heading_prefix_mask |= bit;
                            }
                        }
                        if (heading_exact_mask | heading_prefix_mask).count_ones() as usize
                            == q_toks.len()
                        {
                            break;
                        }
                    }
                } else {
                    for t in &q_toks {
                        if htoks
                            .iter()
                            .any(|w| query_tok_matches_word(t.as_str(), w.text))
                        {
                            heading_bonus_hits = heading_bonus_hits.saturating_add(2);
                        }
                    }
                }
                break;
            }
        }

        // Expand forward to include a small neighborhood until char budget is approached.
        let mut end = blocks[i].end_char;
        let mut k = i;
        while k + 1 < blocks.len() {
            let next = &blocks[k + 1];
            let span_chars = next.end_char.saturating_sub(blocks[j].start_char);
            if span_chars > max_chunk_chars.saturating_mul(2) {
                break;
            }
            // Stop after including a couple blocks past the match unless they are short.
            if k >= i + 2 && span_chars > max_chunk_chars {
                break;
            }
            k += 1;
            end = next.end_char;
        }

        let start_char = blocks[j].start_char;
        let end_char = end.max(start_char);
        let raw = slice_chars(&structure.structure_text, start_char, end_char);
        let raw_trim = raw.trim();
        if raw_trim.is_empty() {
            continue;
        }
        let (snippet, _clipped) = truncate_chars(raw_trim, max_chunk_chars);

        let penalty = chunk_penalty(&snippet);
        let raw_scrub = textprep::scrub(raw_trim);
        let phrase_bonus = phrase_bonus_for_chunk(
            &mut phrase_matcher,
            &mut padded_scrub_buf,
            &mut phrase_matches_buf,
            &raw_scrub,
        );

        cands.push(Cand {
            start_char,
            end_char,
            penalty,
            phrase_bonus,
            block_exact_mask,
            block_prefix_mask,
            heading_exact_mask,
            heading_prefix_mask,
            hits,
            heading_bonus_hits,
            text: snippet,
        });
    }

    if cands.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<ScoredChunk> = Vec::with_capacity(cands.len());
    if use_masks {
        let mut df = vec![0u64; q_toks.len()];
        for c in &cands {
            let mut m = c.block_exact_mask
                | c.block_prefix_mask
                | c.heading_exact_mask
                | c.heading_prefix_mask;
            while m != 0 {
                let i = m.trailing_zeros() as usize;
                df[i] = df[i].saturating_add(1);
                m &= m - 1;
            }
        }
        let mut weights = vec![0u64; q_toks.len()];
        for (i, t) in q_toks.iter().enumerate() {
            let len = t.chars().count().clamp(2, 12) as u64;
            let dfi = df[i].max(1);
            weights[i] = len.saturating_mul(100) / dfi;
        }

        // Prefix matches contribute less than exact matches.
        const PREFIX_SCALE_PCT: u64 = 70;

        for c in cands {
            let mut base = 0u64;
            let mut m = c.block_exact_mask;
            while m != 0 {
                let i = m.trailing_zeros() as usize;
                base = base.saturating_add(weights[i]);
                m &= m - 1;
            }
            let mut pm = c.block_prefix_mask;
            while pm != 0 {
                let i = pm.trailing_zeros() as usize;
                base = base.saturating_add(weights[i].saturating_mul(PREFIX_SCALE_PCT) / 100);
                pm &= pm - 1;
            }
            let mut hm = c.heading_exact_mask;
            while hm != 0 {
                let i = hm.trailing_zeros() as usize;
                base = base.saturating_add(weights[i].saturating_mul(2));
                hm &= hm - 1;
            }
            let mut hpm = c.heading_prefix_mask;
            while hpm != 0 {
                let i = hpm.trailing_zeros() as usize;
                base = base.saturating_add(
                    weights[i]
                        .saturating_mul(2)
                        .saturating_mul(PREFIX_SCALE_PCT)
                        / 100,
                );
                hpm &= hpm - 1;
            }
            base = base.saturating_add(c.phrase_bonus);
            let score = base.saturating_sub(c.penalty);
            scored.push(ScoredChunk {
                start_char: c.start_char,
                end_char: c.end_char,
                score,
                text: c.text,
            });
        }
    } else {
        // Fallback for very long queries: keep older "hits * 100" scoring.
        for c in cands {
            let base = c
                .hits
                .saturating_add(c.heading_bonus_hits)
                .saturating_mul(100)
                .saturating_add(c.phrase_bonus);
            let score = base.saturating_sub(c.penalty);
            scored.push(ScoredChunk {
                start_char: c.start_char,
                end_char: c.end_char,
                score,
                text: c.text,
            });
        }
    }

    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.start_char.cmp(&b.start_char))
    });
    dedupe_and_truncate_chunks(scored, top_k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_text_from_simple_html() {
        let html = r#"<html><body><h1>Hello</h1><p>world</p></body></html>"#;
        let out = html_to_text(html, 80);
        assert!(out.contains("Hello"));
        assert!(out.contains("world"));
    }

    #[test]
    fn bytes_look_like_pdf_sniffs_magic_header() {
        assert!(bytes_look_like_pdf(b"%PDF-1.7\n%..."));
        assert!(!bytes_look_like_pdf(b"<!doctype html><html>"));
        assert!(!bytes_look_like_pdf(b""));
    }

    #[test]
    fn pdf_to_text_never_panics_on_malformed_pdf_bytes() {
        // `pdf_extract::extract_text_from_mem` has had panics on some malformed PDFs.
        // Our wrapper must convert panics into a normal error so higher-level flows
        // (like cache-corpus scanning) can keep going.
        let bad = b"WEBPIPE_TEST_PDF_EXTRACT_PANIC not actually a pdf";
        let r = pdf_to_text(bad);
        assert!(r.is_err());
        assert_eq!(r.err().unwrap_or_default(), "pdf_extract_panicked");
    }

    #[test]
    fn best_effort_pdf_extract_panicked_falls_back_to_strings() {
        // Real-world motivation: some PDFs trigger panics in the pure-Rust backend. When that
        // happens (and shellout tools are unavailable), we should still produce a small amount
        // of evidence text rather than returning empty.
        //
        // Use the deterministic panic hook to avoid relying on crate-internal failure modes.
        let bad_pdfish = b"WEBPIPE_TEST_PDF_EXTRACT_PANIC Fenchel Young losses Omega Theta 2026";
        let ex = best_effort_text_from_bytes(
            bad_pdfish,
            Some("application/pdf"),
            "https://example.com/paper.pdf",
            100,
            500,
        );
        assert_eq!(ex.engine, "pdf-strings");
        assert!(
            ex.text.to_ascii_lowercase().contains("fenchel"),
            "expected fallback text to contain a plausible token; got text={:?}",
            ex.text
        );
        assert!(ex.warnings.contains(&"pdf_extract_failed"));
        assert!(ex.warnings.contains(&"pdf_extract_panicked"));
        assert!(ex.warnings.contains(&"pdf_shellout_unavailable"));
        assert!(ex.warnings.contains(&"pdf_strings_fallback_used"));
    }

    #[test]
    fn bytes_look_like_html_sniffs_common_prefixes() {
        assert!(bytes_look_like_html(b"<!doctype html><html>"));
        assert!(bytes_look_like_html(b"   <html><body>x</body></html>"));
        assert!(bytes_look_like_html(
            b"<!-- comment -->\n<!doctype html><html>"
        ));
        assert!(!bytes_look_like_html(br#"{"a":1}"#));
        assert!(!bytes_look_like_html(b""));
    }

    #[test]
    fn best_effort_text_from_bytes_openreview_notes_api_compacts_json_to_metadata_text() {
        let j = serde_json::json!({
            "notes": [{
                "id": "RUzSobdYy0V",
                "forum": "RUzSobdYy0V",
                "invitation": "ICLR.cc/2023/Conference/-/Blind_Submission",
                "content": {
                    "title": "Quantifying and Mitigating the Impact of Label Errors on Model Disparity Metrics",
                    "authors": ["Julius Adebayo", "Melissa Hall"],
                    "venue": "ICLR 2023 poster",
                    "abstract": "Errors in labels obtained via human annotation adversely affect group disparity metrics."
                }
            }],
            "count": 1
        })
        .to_string();
        let ex = best_effort_text_from_bytes(
            j.as_bytes(),
            Some("application/json; charset=utf-8"),
            "https://api.openreview.net/notes?id=RUzSobdYy0V",
            100,
            500,
        );
        assert_eq!(ex.engine, "json");
        assert!(
            ex.text.to_ascii_lowercase().contains("openreview note"),
            "text={:?}",
            ex.text
        );
        assert!(
            ex.text
                .to_ascii_lowercase()
                .contains("title: quantifying and mitigating"),
            "text={:?}",
            ex.text
        );
        assert!(
            ex.text.to_ascii_lowercase().contains("abstract: errors"),
            "text={:?}",
            ex.text
        );
        assert!(
            !ex.text.contains("\"notes\""),
            "expected non-raw JSON text; text={:?}",
            ex.text
        );
    }

    #[test]
    fn best_effort_text_from_bytes_recognizes_images_as_supported_empty() {
        // Keep the test deterministic: don't depend on local `tesseract` presence.
        std::env::set_var("WEBPIPE_OCR", "off");
        // Minimal PNG header.
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR";
        let ex = best_effort_text_from_bytes(png, Some("image/png"), "https://x", 80, 200);
        assert_eq!(ex.engine, "image");
        assert!(ex.text.is_empty());
        assert!(ex.warnings.contains(&"image_no_text_extraction"));
    }

    #[test]
    fn html_main_to_text_prefers_article_like_blocks() {
        let html = r#"
        <html><body>
          <nav class="nav"><a href="/x">Home</a></nav>
          <article><h1>Title</h1><p>Hello world.</p><p>More text here.</p></article>
          <footer class="footer"><a href="/y">Privacy</a></footer>
        </body></html>
        "#;
        // Some HTML→text renderers may omit very short paragraphs depending on wrapping.
        // Ensure we still select the article-like block and keep its substantive text.
        let out = html_main_to_text(html, 120).unwrap_or_default();
        assert!(out.to_lowercase().contains("hello"));
        assert!(out.to_lowercase().contains("more"));
        assert!(!out.to_lowercase().contains("privacy"));
    }

    #[test]
    fn best_effort_text_from_bytes_prefers_main_when_full_is_nav_heavy() {
        // This simulates JS-heavy docs pages where the "full page" text is mostly chrome.
        let html = r#"
        <!doctype html>
        <html><body>
          <nav>
            <a href="/">Skip to content</a>
            <div>Search documentation... ⌘K</div>
            <ul>
              <li>Getting Started</li>
              <li>Installation</li>
              <li>Project Structure</li>
              <li>API Reference</li>
            </ul>
          </nav>
          <main>
            <h1>headers</h1>
            <p>headers is an async function that allows you to read the HTTP incoming request headers.</p>
            <pre><code>const h = await headers()</code></pre>
          </main>
          <footer>Privacy Terms</footer>
        </body></html>
        "#;
        let ex = best_effort_text_from_bytes(
            html.as_bytes(),
            Some("text/html"),
            "https://example.com/docs",
            100,
            20_000,
        );
        // We don't care whether it chooses readability vs html_main, but it should avoid "full page".
        assert!(
            ex.engine == "readability" || ex.engine == "html_main",
            "expected main-like engine, got: {}",
            ex.engine
        );
        assert!(ex
            .text
            .to_lowercase()
            .contains("headers is an async function"));
        assert!(!ex.text.to_lowercase().contains("getting started"));
        assert!(ex.warnings.contains(&"boilerplate_reduced"));
    }

    #[test]
    fn tokenize_query_for_match_drops_version_noise_tokens() {
        // Version tokens like "v1"/"rc1" are extremely high-recall and degrade ranking
        // (especially in cache-corpus mode).
        let toks = tokenize_query_for_match("predicateType slsa provenance v1 rc1 2 2022");
        assert!(
            toks.contains(&"predicatetype".to_string()) || toks.contains(&"predicate".to_string())
        );
        assert!(toks.contains(&"slsa".to_string()));
        assert!(toks.contains(&"provenance".to_string()));
        assert!(!toks.contains(&"v1".to_string()), "toks={toks:?}");
        assert!(!toks.contains(&"rc1".to_string()), "toks={toks:?}");
        assert!(!toks.contains(&"2".to_string()), "toks={toks:?}");
        // Keep year-like tokens (useful for papers).
        assert!(toks.contains(&"2022".to_string()), "toks={toks:?}");
    }

    #[test]
    fn tokenize_query_for_match_keeps_http_status_codes() {
        // Three-digit numeric tokens are often meaningful (HTTP status codes, RFC numbers, etc.).
        let toks = tokenize_query_for_match("HTTP status 404 403");
        assert!(toks.contains(&"status".to_string()), "toks={toks:?}");
        assert!(toks.contains(&"404".to_string()), "toks={toks:?}");
        assert!(toks.contains(&"403".to_string()), "toks={toks:?}");
    }

    #[test]
    fn tokenize_query_for_match_drops_url_noise_tokens() {
        let toks = tokenize_query_for_match("http https www status code");
        assert!(!toks.contains(&"http".to_string()), "toks={toks:?}");
        assert!(!toks.contains(&"https".to_string()), "toks={toks:?}");
        assert!(!toks.contains(&"www".to_string()), "toks={toks:?}");
        assert!(toks.contains(&"status".to_string()), "toks={toks:?}");
        assert!(toks.contains(&"code".to_string()), "toks={toks:?}");
    }

    #[test]
    fn tokenize_query_for_match_drops_stopwords_when_other_tokens_exist() {
        let toks = tokenize_query_for_match("How to use tokio timeout");
        assert!(toks.contains(&"tokio".to_string()), "toks={toks:?}");
        assert!(toks.contains(&"timeout".to_string()), "toks={toks:?}");
        assert!(!toks.contains(&"how".to_string()), "toks={toks:?}");
        assert!(!toks.contains(&"to".to_string()), "toks={toks:?}");
    }

    #[test]
    fn query_aware_truncation_keeps_evidence_beyond_nav_prefix() {
        // Simulate a JS-heavy docs page where extraction produces a long nav-ish prefix
        // and the actual content (with query tokens) occurs later.
        let mut s = String::new();
        for _ in 0..600 {
            s.push_str("Menu\nDocs\nAPI\nGuides\n");
        }
        s.push_str("\n\n");
        s.push_str("Actual content starts here.\n\n");
        s.push_str("The headers() function returns a read-only Headers object.\n");
        s.push_str("It can be used in Server Components and Route Handlers.\n");

        let extracted0 = ExtractedText {
            engine: "html2text",
            text: s,
            warnings: vec![],
        };
        let cfg = ExtractPipelineCfg {
            query: Some("headers function"),
            width: 120,
            max_chars: 400, // smaller than the nav prefix; forces truncation
            top_chunks: 3,
            max_chunk_chars: 200,
            include_structure: false,
            max_outline_items: 40,
            max_blocks: 20,
            max_block_chars: 200,
        };
        let r =
            extract_pipeline_from_extracted(b"", None, "https://nextjs.org/docs", extracted0, cfg);
        assert!(r.text_truncated);
        assert!(r.extracted.warnings.contains(&"text_windowed_for_query"));
        assert!(
            r.extracted.text.to_ascii_lowercase().contains("headers"),
            "windowed text should include evidence: {}",
            r.extracted.text
        );
        assert!(!r.chunks.is_empty(), "expected evidence chunks");
        assert!(
            r.chunks[0].text.to_ascii_lowercase().contains("headers"),
            "expected top chunk to match query: {:?}",
            r.chunks.first().map(|c| &c.text)
        );
    }

    #[test]
    fn structure_extraction_skips_html_parse_on_pathologically_long_tokens() {
        // A single enormous alnum run can make HTML structure parsing slow; we should fall back to
        // text-based structure in that case.
        let html = format!(
            "<html><body><main><h1>Doc</h1><p>{}</p></main></body></html>",
            "x".repeat(25_000)
        );
        let extracted = ExtractedText {
            engine: "html2text",
            text: "Doc\n\nx".to_string(),
            warnings: vec![],
        };
        let s = best_effort_structure_from_bytes(
            html.as_bytes(),
            Some("text/html"),
            "https://example.com/",
            &extracted,
            25,
            40,
            400,
        );
        assert!(
            s.warnings.contains(&"structure_html_skipped_long_token"),
            "warnings={:?}",
            s.warnings
        );
        // Should still produce some blocks from extracted text.
        assert!(!s.blocks.is_empty());
    }

    #[test]
    fn best_chunks_for_query_prefers_matching_paragraphs() {
        let text = "alpha beta\n\nbravo CHARLIE delta\n\nzzz";
        let chunks = best_chunks_for_query(text, "charlie", 5, 200);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.to_lowercase().contains("charlie"));
        assert!(chunks[0].start_char < chunks[0].end_char);
    }

    #[test]
    fn best_chunks_for_query_handles_hyphens_and_case_like_real_docs() {
        let text = "MCP-stdio transport protocol\n\nOther paragraph unrelated.\n";
        let chunks = best_chunks_for_query(text, "MCP stdio transport", 5, 200);
        assert!(!chunks.is_empty());
        assert!(chunks[0].score >= 2);
    }

    #[test]
    fn best_chunks_for_query_avoids_infix_false_positives() {
        // `art` should not match `cartography` (infix), but should match a real `art` token.
        let text = "cartography is a word\n\nart is evidence\n";
        let chunks = best_chunks_for_query(text, "art", 3, 240);
        assert!(!chunks.is_empty());
        assert!(
            chunks[0]
                .text
                .to_ascii_lowercase()
                .contains("art is evidence"),
            "expected real art paragraph; got: {:?}",
            chunks[0].text
        );
    }

    #[test]
    fn best_chunks_for_query_short_tokens_require_exact_match() {
        // Short query tokens like "go" are ambiguous. We should avoid prefix false positives
        // like "go" matching "google" when the query is only the short token.
        let text = "google is a company\n\ngo is a language\n";
        let chunks = best_chunks_for_query(text, "go", 3, 240);
        assert!(!chunks.is_empty());
        assert!(
            chunks[0]
                .text
                .to_ascii_lowercase()
                .contains("go is a language"),
            "expected real 'go' paragraph first; got: {:?}",
            chunks[0].text
        );
    }

    #[test]
    fn best_chunks_for_query_prefers_rare_token_over_generic_tie() {
        // Regression: when two chunks match the same number of query tokens, we should prefer
        // the chunk containing the rarer/more-specific token (within-page), not just the earlier chunk.
        //
        // Here, paragraph 1 matches {install, timeout} and paragraph 4 matches {tokio, install}.
        // Without rarity weighting, this can tie and pick paragraph 1 due to ordering.
        let p1 = "Install steps are documented. Use a client-side deadline for requests.";
        let p2 = "A timeout can be enforced at the transport layer for reliability.";
        let p3 = "Timeout values should be chosen based on tail latency and retries.";
        let p4 = "Tokio install is straightforward: add tokio to Cargo.toml and use tokio::time.";
        let text = format!("{p1}\n\n{p2}\n\n{p3}\n\n{p4}\n");

        let chunks = best_chunks_for_query(&text, "tokio install timeout", 3, 240);
        assert!(!chunks.is_empty());
        assert!(
            chunks[0]
                .text
                .to_ascii_lowercase()
                .contains("tokio install"),
            "expected tokio paragraph first; got: {:?}",
            chunks[0].text
        );
    }

    #[test]
    fn best_chunks_for_query_prefers_exact_token_over_prefix_false_positive() {
        // Regression: exact token matches should beat prefix-only matches when both chunks
        // otherwise look comparable (common in docs where "art" vs "article", "net" vs "network", etc.).
        let p1 = "This article covers tokio runtime basics.";
        // Keep tokens non-adjacent so phrase bonus doesn't decide the test.
        let p2 = "This art gallery mentions the tokio runtime too.";
        let text = format!("{p1}\n\n{p2}\n");

        let chunks = best_chunks_for_query(&text, "art tokio", 3, 240);
        assert!(!chunks.is_empty());
        assert!(
            chunks[0].text.to_ascii_lowercase().contains("art gallery"),
            "expected exact 'art' paragraph first; got: {:?}",
            chunks[0].text
        );
    }

    #[test]
    fn best_chunks_for_query_downranks_nav_like_chunks() {
        // Nav-shaped text often contains query tokens but should not beat a real paragraph.
        let nav = [
            "Docs", "Models", "Datasets", "Spaces", "Pricing", "Sign up", "Log in", "GLiNER",
        ]
        .join("\n");
        let content =
            "GLiNER is a span-based NER model. It takes text input and produces labeled spans.";
        let text = format!("{nav}\n\n{content}\n");

        let chunks = best_chunks_for_query(&text, "gliner model input", 3, 240);
        assert!(!chunks.is_empty());
        assert!(
            chunks[0].text.contains("span-based NER model"),
            "expected content paragraph first; got: {:?}",
            chunks[0].text
        );
    }

    #[test]
    fn best_chunks_for_query_downranks_table_of_contents_like_chunks() {
        // TOC chunks often contain query tokens (section titles) but are not evidence.
        let toc = "Table of Contents Coroutines and Tasks Coroutines Awaitables Creating Tasks Task Cancellation Task Groups \
Sleeping Running Tasks Concurrently Shielding From Cancellation Timeouts Waiting Primitives Scheduling From Other Threads";
        let content = "Use asyncio.timeout() to apply a timeout to a block of async code. This section explains the semantics.";
        let text = format!("{toc}\n\n{content}\n");

        let chunks = best_chunks_for_query(&text, "timeout", 3, 240);
        assert!(!chunks.is_empty());
        assert!(
            chunks[0].text.to_lowercase().contains("asyncio.timeout"),
            "expected content paragraph first; got: {:?}",
            chunks[0].text
        );
    }

    #[test]
    fn best_chunks_for_query_downranks_promo_like_chunks_even_with_token_hits() {
        let promo = "Join us for four days of incredible opportunities. Buy your ticket now! Timeout Timeout Timeout";
        let content = "Timeouts in distributed systems should be enforced at the client and server with clear semantics.";
        let text = format!("{promo}\n\n{content}\n");

        let chunks = best_chunks_for_query(&text, "timeout", 3, 240);
        assert!(!chunks.is_empty());
        assert!(
            chunks[0]
                .text
                .to_lowercase()
                .contains("distributed systems"),
            "expected content paragraph first; got: {:?}",
            chunks[0].text
        );
    }

    #[test]
    fn best_chunks_for_query_in_structure_keeps_heading_context() {
        let mut s = ExtractedStructure {
            engine: "text",
            title: None,
            outline: vec!["Heading".to_string()],
            structure_text: String::new(),
            text_chars: 0,
            blocks: Vec::new(),
            warnings: Vec::new(),
        };
        // Build a tiny structure with one heading and one paragraph.
        push_block(
            &mut s.blocks,
            &mut s.structure_text,
            "heading",
            "Heading".to_string(),
            400,
        );
        push_block(
            &mut s.blocks,
            &mut s.structure_text,
            "paragraph",
            "This paragraph mentions transformers and attention.".to_string(),
            400,
        );
        s.text_chars = s.structure_text.chars().count();

        let chunks = best_chunks_for_query_in_structure(&s, "attention", 5, 200);
        assert!(!chunks.is_empty());
        assert!(chunks[0].text.contains("Heading"));
        assert!(chunks[0].text.to_lowercase().contains("attention"));
    }

    #[test]
    fn chunk_penalty_counts_urls_but_not_plain_http_words() {
        let http_words =
            "HTTP 404 Not Found indicates the origin server did not find a current representation.";
        let p_words = chunk_penalty(http_words);
        assert!(
            p_words < 40,
            "expected low/no url penalty for plain HTTP words; p={p_words}"
        );

        let link_soup =
            "See http://example.com and https://foo.invalid (also www.example.org for docs).";
        let p_links = chunk_penalty(link_soup);
        assert!(
            p_links >= 120,
            "expected url penalty for explicit URLs; p={p_links}"
        );
    }

    #[test]
    fn chunk_penalty_downranks_repetitive_nav_filler() {
        // Keep this within typical chunk caps (e.g. max_chunk_chars=300).
        let nav = "nav ".repeat(75);
        let p = chunk_penalty(&nav);
        assert!(
            p >= 120,
            "expected repetitive filler to be nav-like; p={p} text_len={}",
            nav.len()
        );
    }

    #[test]
    fn extract_pipeline_falls_back_when_query_has_no_overlap() {
        let text = r#"
Intro paragraph with enough length to pass the chunk minimum.

Second paragraph also long enough to be a usable chunk for humans.
"#;
        let extracted0 = ExtractedText {
            engine: "text",
            text: text.to_string(),
            warnings: vec![],
        };
        let cfg = ExtractPipelineCfg {
            query: Some("definitely-not-present"),
            width: 80,
            max_chars: 10_000,
            top_chunks: 3,
            max_chunk_chars: 200,
            include_structure: false,
            max_outline_items: 0,
            max_blocks: 0,
            max_block_chars: 0,
        };
        let r = extract_pipeline_from_extracted(b"", None, "https://example.com/", extracted0, cfg);
        assert!(
            !r.chunks.is_empty(),
            "expected fallback chunks when query matching yields none"
        );
    }

    #[test]
    fn extract_pipeline_fallback_prefers_non_promo_paragraph_over_prefix() {
        let promo = "Join us for four days of incredible opportunities. Buy your ticket now!";
        let content =
            "This paragraph contains substantive documentation text and should be preferred.";
        let text = format!("{promo}\n\n{content}\n");
        let extracted0 = ExtractedText {
            engine: "text",
            text,
            warnings: vec![],
        };
        let cfg = ExtractPipelineCfg {
            query: Some("definitely-not-present"),
            width: 80,
            max_chars: 10_000,
            top_chunks: 3,
            max_chunk_chars: 240,
            include_structure: false,
            max_outline_items: 0,
            max_blocks: 0,
            max_block_chars: 0,
        };
        let r = extract_pipeline_from_extracted(
            &[],
            Some("text/plain"),
            "https://example.com",
            extracted0,
            cfg,
        );
        assert!(!r.chunks.is_empty());
        assert!(
            r.chunks[0].text.contains("substantive documentation"),
            "expected fallback to avoid promo prefix; got: {:?}",
            r.chunks[0].text
        );
    }

    #[test]
    fn detect_client_redirect_finds_meta_refresh() {
        let html = r#"<html><head><meta http-equiv="refresh" content="0; url=https://example.com/" /></head></html>"#;
        assert_eq!(
            detect_client_redirect(html),
            Some("https://example.com/".to_string())
        );
    }

    #[test]
    fn detect_client_redirect_finds_click_here() {
        let html = r#"<html><body><p>Click here to be redirected.</p><a href="https://example.com/">Link</a></body></html>"#;
        assert_eq!(
            detect_client_redirect(html),
            Some("https://example.com/".to_string())
        );
    }
}
