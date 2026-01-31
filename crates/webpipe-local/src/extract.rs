use crate::shellout;
use crate::textprep;
use serde::Serialize;
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
    pdf_extract::extract_text_from_mem(bytes).map_err(|e| e.to_string())
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
    let (text, text_chars, text_truncated) = truncate_to_chars(&extracted0.text, cfg.max_chars);
    let extracted = ExtractedText {
        engine: extracted0.engine,
        text,
        warnings: extracted0.warnings,
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

    let query = cfg.query.unwrap_or("").trim();
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

    let spans = find_paragraph_spans(text);
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
        let (snippet, _clipped) = truncate_chars(text.trim(), max_chunk_chars);
        out.push(ScoredChunk {
            start_char: 0,
            end_char: text.chars().count(),
            score: 1,
            text: snippet,
        });
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
    // Prefer HTML structure if we actually have HTML-ish bytes.
    if bytes_look_like_html(bytes) {
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
    extract_structure_from_text(
        engine,
        &extracted.text,
        max_outline,
        max_blocks,
        max_block_chars,
    )
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
                            ExtractedText {
                                engine: "pdf-extract",
                                text: String::new(),
                                warnings,
                            }
                        }
                    }
                }
            }
            Err(_) => {
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
                        ExtractedText {
                            engine: "pdf-extract",
                            text: String::new(),
                            warnings,
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
        let text = clean_extracted_text(String::from_utf8_lossy(bytes).to_string());
        let engine = if is_markdown {
            "markdown"
        } else if is_json {
            "json"
        } else if is_xml {
            "xml"
        } else {
            "text"
        };
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
            "sign up", "log in", "login", "cookie", "consent", "privacy", "terms",
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
            if !full_ok || best_score >= s_full + 300 {
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

fn tokenize_query_for_match(query: &str) -> Vec<String> {
    // Use the same normalization as our agent-facing `query_key`:
    // NFC + lowercase, then a conservative token split.
    //
    // We intentionally avoid fancy stemming here; we just want to be resilient to
    // punctuation and case differences like "MCP-stdio" vs "mcp stdio".
    let q = textprep::scrub(query);
    q.split(|ch: char| !ch.is_alphanumeric())
        .filter_map(|t| {
            let t = t.trim();
            if t.len() >= 2 {
                Some(t.to_string())
            } else {
                None
            }
        })
        .collect()
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

pub fn best_chunks_for_query(
    text: &str,
    query: &str,
    top_k: usize,
    max_chunk_chars: usize,
) -> Vec<ScoredChunk> {
    let top_k = top_k.clamp(1, 50);
    let max_chunk_chars = max_chunk_chars.clamp(50, 5_000);

    let q_toks = tokenize_query_for_match(query);
    if q_toks.is_empty() {
        return Vec::new();
    }

    let spans = find_paragraph_spans(text);
    let mut scored = Vec::new();
    for (sb, eb) in spans {
        let slice = text.get(sb..eb).unwrap_or("");
        let slice_trim = slice.trim();
        if slice_trim.is_empty() {
            continue;
        }

        let slice_scrub = textprep::scrub(slice_trim);
        let mut score = 0u64;
        for t in &q_toks {
            if slice_scrub.contains(t) {
                score += 1;
            }
        }
        if score == 0 {
            continue;
        }

        let (snippet, _clipped) = truncate_chars(slice_trim, max_chunk_chars);
        let start_char = byte_to_char_index(text, sb);
        let end_char = byte_to_char_index(text, eb);

        scored.push(ScoredChunk {
            start_char,
            end_char,
            score,
            text: snippet,
        });
    }

    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.start_char.cmp(&b.start_char))
    });
    scored.truncate(top_k);
    scored
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

    let q_toks = tokenize_query_for_match(query);
    if q_toks.is_empty() {
        return Vec::new();
    }

    // Candidate: pick any block containing any token; then expand to include heading context.
    let blocks = &structure.blocks;
    if blocks.is_empty() || structure.structure_text.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<ScoredChunk> = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        let b_scrub = textprep::scrub(&b.text);
        let mut hits = 0u64;
        for t in &q_toks {
            if b_scrub.contains(t) {
                hits += 1;
            }
        }
        if hits == 0 {
            continue;
        }

        // Find nearest preceding heading (bounded lookback).
        let mut j = i;
        let mut heading_bonus = 0u64;
        for back in 0..=10usize {
            if i < back {
                break;
            }
            let k = i - back;
            if blocks[k].kind == "heading" {
                j = k;
                // If the heading itself matches tokens, reward it.
                let h_scrub = textprep::scrub(&blocks[k].text);
                for t in &q_toks {
                    if h_scrub.contains(t) {
                        heading_bonus += 2;
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

        scored.push(ScoredChunk {
            start_char,
            end_char,
            score: hits + heading_bonus,
            text: snippet,
        });
    }

    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.start_char.cmp(&b.start_char))
    });
    scored.truncate(top_k);
    scored
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
}
