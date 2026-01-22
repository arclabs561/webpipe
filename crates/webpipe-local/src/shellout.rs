//! Opportunistic shellouts to well-known local CLIs.
//!
//! Goals:
//! - **Opportunistic**: use tools when present.
//! - **Bounded**: timeouts + output caps to avoid hangs/huge output.
//! - **Deterministic preference order**: stable “best available” selection.
//! - **No secrets**: no env dumps; caller decides what to surface as warnings.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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

pub fn timeout_from_env_ms(key: &str, default_ms: u64) -> Duration {
    let ms = env(key)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(default_ms)
        .clamp(50, 300_000);
    Duration::from_millis(ms)
}

pub fn max_chars_from_env(key: &str, default_chars: usize) -> usize {
    env_usize(key, default_chars).clamp(200, 2_000_000)
}

pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let sep = std::path::MAIN_SEPARATOR;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(bin);
        if cand.is_file() {
            return Some(cand);
        }
        // Windows exe suffix not relevant here, but keep it harmless.
        if sep != '\\' {
            continue;
        }
        let cand = dir.join(format!("{bin}.exe"));
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

pub fn has(bin: &str) -> bool {
    which(bin).is_some()
}

/// Run a command and capture stdout (bounded) with a coarse timeout.
///
/// This intentionally does **not** stream; it is “robust enough” for typical small utilities
/// like `pdftotext`, `pandoc`, `tesseract`, and `ffmpeg` subtitle extraction.
pub fn run_stdout_bounded(
    mut cmd: Command,
    timeout: Duration,
    max_stdout_bytes: usize,
) -> Result<Vec<u8>, &'static str> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "shellout_tool_not_found"
        } else {
            "shellout_spawn_failed"
        }
    })?;

    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|_| "shellout_wait_failed")? {
            if !status.success() {
                return Err("shellout_nonzero_exit");
            }
            break;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            return Err("shellout_timeout");
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    let mut out = Vec::new();
    if let Some(s) = child.stdout.take() {
        use std::io::Read;
        s.take(max_stdout_bytes as u64)
            .read_to_end(&mut out)
            .map_err(|_| "shellout_read_failed")?;
    }
    Ok(out)
}

pub fn write_temp_file(
    bytes: &[u8],
    suffix: &str,
) -> Result<tempfile::NamedTempFile, &'static str> {
    let mut tmp = tempfile::Builder::new()
        .prefix("webpipe-")
        .suffix(suffix)
        .tempfile()
        .map_err(|_| "shellout_tempfile_failed")?;
    use std::io::Write;
    tmp.write_all(bytes)
        .map_err(|_| "shellout_tempfile_write_failed")?;
    Ok(tmp)
}

pub fn suffix_for_content_type(ct: &str) -> &'static str {
    let ct = ct.trim().to_ascii_lowercase();
    if ct == "application/pdf" {
        ".pdf"
    } else if ct == "application/vnd.openxmlformats-officedocument.wordprocessingml.document" {
        ".docx"
    } else if ct == "application/msword" {
        ".doc"
    } else if ct == "application/epub+zip" {
        ".epub"
    } else if ct == "application/rtf" {
        ".rtf"
    } else if ct.starts_with("image/png") {
        ".png"
    } else if ct.starts_with("image/jpeg") {
        ".jpg"
    } else if ct.starts_with("image/webp") {
        ".webp"
    } else if ct.starts_with("image/gif") {
        ".gif"
    } else if ct.starts_with("video/mp4") {
        ".mp4"
    } else if ct.starts_with("video/") {
        ".video"
    } else {
        ".bin"
    }
}

pub fn url_suffix_hint(final_url: &str) -> &'static str {
    let u = final_url.to_ascii_lowercase();
    if u.ends_with(".pdf") {
        ".pdf"
    } else if u.ends_with(".docx") {
        ".docx"
    } else if u.ends_with(".doc") {
        ".doc"
    } else if u.ends_with(".epub") {
        ".epub"
    } else if u.ends_with(".rtf") {
        ".rtf"
    } else if u.ends_with(".png") {
        ".png"
    } else if u.ends_with(".jpg") || u.ends_with(".jpeg") {
        ".jpg"
    } else if u.ends_with(".webp") {
        ".webp"
    } else if u.ends_with(".gif") {
        ".gif"
    } else if u.ends_with(".mp4") {
        ".mp4"
    } else if u.ends_with(".mkv") {
        ".mkv"
    } else {
        ".bin"
    }
}

fn normalize_mode(s: Option<String>) -> String {
    match s.as_deref() {
        Some("off") => "off".to_string(),
        Some("strict") => "strict".to_string(),
        Some("auto") | None => "auto".to_string(),
        // Unknown value: treat as auto (bounded, best-effort).
        Some(_) => "auto".to_string(),
    }
}

pub fn pandoc_mode_from_env() -> String {
    normalize_mode(env("WEBPIPE_PANDOC"))
}

pub fn ocr_mode_from_env() -> String {
    normalize_mode(env("WEBPIPE_OCR"))
}

pub fn media_subs_mode_from_env() -> String {
    normalize_mode(env("WEBPIPE_MEDIA_SUBTITLES"))
}

pub fn pandoc_to_text(
    bytes: &[u8],
    content_type: Option<&str>,
    final_url: &str,
) -> Result<String, &'static str> {
    let mode = pandoc_mode_from_env();
    if mode == "off" {
        return Err("pandoc_disabled");
    }
    if !has("pandoc") {
        return Err("pandoc_not_found");
    }
    let timeout = timeout_from_env_ms("WEBPIPE_PANDOC_TIMEOUT_MS", 20_000);
    let max_chars = max_chars_from_env("WEBPIPE_PANDOC_MAX_CHARS", 200_000);
    let max_stdout_bytes = max_chars.saturating_mul(4).clamp(1_000, 8_000_000);
    let suffix = content_type
        .map(suffix_for_content_type)
        .unwrap_or_else(|| url_suffix_hint(final_url));
    let tmp = write_temp_file(bytes, suffix)?;
    let path = tmp.path().to_string_lossy().to_string();

    // pandoc <in> -t plain --wrap=none
    let mut cmd = Command::new("pandoc");
    cmd.arg(&path).arg("-t").arg("plain").arg("--wrap=none");
    let out = run_stdout_bounded(cmd, timeout, max_stdout_bytes)?;
    let s = String::from_utf8_lossy(&out).to_string();
    let clipped: String = s.chars().take(max_chars).collect();
    if clipped.chars().any(|c| !c.is_whitespace()) {
        Ok(clipped)
    } else {
        Err("pandoc_empty_output")
    }
}

pub fn tesseract_ocr(
    bytes: &[u8],
    content_type: Option<&str>,
    final_url: &str,
) -> Result<String, &'static str> {
    let mode = ocr_mode_from_env();
    if mode == "off" {
        return Err("ocr_disabled");
    }
    if !has("tesseract") {
        return Err("tesseract_not_found");
    }
    let timeout = timeout_from_env_ms("WEBPIPE_OCR_TIMEOUT_MS", 30_000);
    let max_chars = max_chars_from_env("WEBPIPE_OCR_MAX_CHARS", 50_000);
    let max_stdout_bytes = max_chars.saturating_mul(4).clamp(1_000, 4_000_000);
    let suffix = content_type
        .map(suffix_for_content_type)
        .unwrap_or_else(|| url_suffix_hint(final_url));
    let tmp = write_temp_file(bytes, suffix)?;
    let in_path = tmp.path().to_string_lossy().to_string();

    // Optional: ImageMagick convert step for odd formats is not enabled by default.
    // Keep it simple + predictable; callers can pre-normalize images if desired.
    let mut cmd = Command::new("tesseract");
    cmd.arg(&in_path).arg("stdout");
    let out = run_stdout_bounded(cmd, timeout, max_stdout_bytes)?;
    let s = String::from_utf8_lossy(&out).to_string();
    let clipped: String = s.chars().take(max_chars).collect();
    if clipped.chars().any(|c| !c.is_whitespace()) {
        Ok(clipped)
    } else {
        Err("tesseract_empty_output")
    }
}

pub fn ffmpeg_extract_subtitles_vtt(
    bytes: &[u8],
    content_type: Option<&str>,
    final_url: &str,
) -> Result<String, &'static str> {
    let mode = media_subs_mode_from_env();
    if mode == "off" {
        return Err("media_subtitles_disabled");
    }
    if !has("ffmpeg") {
        return Err("ffmpeg_not_found");
    }
    let timeout = timeout_from_env_ms("WEBPIPE_MEDIA_SUBS_TIMEOUT_MS", 25_000);
    let max_chars = max_chars_from_env("WEBPIPE_MEDIA_SUBS_MAX_CHARS", 200_000);
    let max_stdout_bytes = max_chars.saturating_mul(4).clamp(1_000, 8_000_000);
    let suffix = content_type
        .map(suffix_for_content_type)
        .unwrap_or_else(|| url_suffix_hint(final_url));
    let tmp = write_temp_file(bytes, suffix)?;
    let in_path = tmp.path().to_string_lossy().to_string();

    // Extract first subtitle stream as WebVTT (best-effort).
    // ffmpeg logs to stderr; stdout is the VTT payload.
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-i")
        .arg(&in_path)
        .arg("-map")
        .arg("0:s:0")
        .arg("-f")
        .arg("webvtt")
        .arg("pipe:1");
    let out = run_stdout_bounded(cmd, timeout, max_stdout_bytes)?;
    let s = String::from_utf8_lossy(&out).to_string();
    let clipped: String = s.chars().take(max_chars).collect();
    if clipped.chars().any(|c| !c.is_whitespace()) {
        Ok(clipped)
    } else {
        Err("ffmpeg_subtitles_empty_output")
    }
}

pub fn looks_like_doc_or_epub(ct0: &str, final_url: &str) -> bool {
    let ct = ct0.trim().to_ascii_lowercase();
    // Avoid false positives when a “.docx” URL serves an HTML error page.
    if ct.starts_with("text/html") {
        return false;
    }
    if ct == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        || ct == "application/msword"
        || ct == "application/epub+zip"
        || ct == "application/rtf"
        || ct.starts_with("application/vnd.oasis.opendocument")
    {
        return true;
    }
    let u = final_url.to_ascii_lowercase();
    u.ends_with(".docx")
        || u.ends_with(".doc")
        || u.ends_with(".epub")
        || u.ends_with(".rtf")
        || u.ends_with(".odt")
}

pub fn looks_like_video(ct0: &str, final_url: &str) -> bool {
    let ct = ct0.trim().to_ascii_lowercase();
    if ct.starts_with("text/html") {
        return false;
    }
    if ct.starts_with("video/") {
        return true;
    }
    let u = final_url.to_ascii_lowercase();
    u.ends_with(".mp4") || u.ends_with(".mkv") || u.ends_with(".webm")
}

pub fn looks_like_image(ct0: &str) -> bool {
    let ct = ct0.trim().to_ascii_lowercase();
    ct.starts_with("image/")
}
