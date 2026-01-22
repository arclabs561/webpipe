//! YouTube helpers (transcripts).
//!
//! Design goals:
//! - Bounded (size/time).
//! - Deterministic knobs (env-configured; no silent network).
//! - Avoid brittle HTML scraping; prefer `yt-dlp` when available (it already implements
//!   YouTube’s moving target logic).

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

pub fn is_youtube_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h == "youtube.com"
        || h == "www.youtube.com"
        || h == "m.youtube.com"
        || h == "youtu.be"
        || h.ends_with(".youtube.com")
}

pub fn youtube_video_id(u: &url::Url) -> Option<String> {
    let host = u.host_str()?;
    if !is_youtube_host(host) {
        return None;
    }

    // youtu.be/<id>
    if host.eq_ignore_ascii_case("youtu.be") {
        let seg = u.path_segments()?.next()?.trim();
        if !seg.is_empty() {
            return Some(seg.to_string());
        }
    }

    // youtube.com/watch?v=<id>
    if u.path().starts_with("/watch") {
        for (k, v) in u.query_pairs() {
            if k == "v" {
                let s = v.trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }

    // youtube.com/shorts/<id>, /embed/<id>
    if let Some(mut segs) = u.path_segments() {
        let a = segs.next().unwrap_or("");
        let b = segs.next().unwrap_or("");
        if (a == "shorts" || a == "embed") && !b.trim().is_empty() {
            return Some(b.to_string());
        }
    }

    None
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

pub fn youtube_transcripts_mode_from_env() -> String {
    env("WEBPIPE_YOUTUBE_TRANSCRIPTS").unwrap_or_else(|| "auto".to_string())
}

pub fn youtube_langs_from_env() -> Vec<String> {
    let s = env("WEBPIPE_YOUTUBE_LANGS").unwrap_or_else(|| "en,en-US".to_string());
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn vtt_to_text(vtt: &str, max_chars: usize) -> String {
    // Very small, deterministic VTT -> text:
    // - drop WEBVTT header and timing lines
    // - keep cue text lines
    // - collapse whitespace-ish runs per line, preserve paragraph breaks between cues
    let mut out = String::new();
    let mut prev_blank = true;
    for line in vtt.lines() {
        let l = line.trim();
        if l.is_empty() {
            if !prev_blank {
                out.push_str("\n\n");
                prev_blank = true;
            }
            continue;
        }
        if l.eq_ignore_ascii_case("webvtt") {
            continue;
        }
        if l.contains("-->") {
            // timing line
            prev_blank = false;
            continue;
        }
        // ignore numeric cue ids
        if l.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // actual cue text
        if !prev_blank {
            out.push(' ');
        }
        let cleaned = l.split_whitespace().collect::<Vec<_>>().join(" ");
        out.push_str(&cleaned);
        prev_blank = false;

        if out.chars().count() >= max_chars {
            break;
        }
    }
    // Trim trailing blank lines
    out.trim().to_string()
}

/// Shared VTT → text normalizer for non-YouTube media subtitles.
///
/// Kept here so we reuse the same deterministic normalization logic across:
/// - yt-dlp captions (YouTube)
/// - ffmpeg-extracted subtitle streams (generic video)
pub fn vtt_to_text_for_media(vtt: &str, max_chars: usize) -> String {
    vtt_to_text(vtt, max_chars)
}

pub fn fetch_transcript_via_ytdlp(url: &str, timeout: Duration) -> Result<String, String> {
    // Bounds:
    // - yt-dlp itself can take time; caller supplies timeout.
    // - transcript is clipped post-hoc.
    let max_chars = env_usize("WEBPIPE_YOUTUBE_MAX_CHARS", 200_000).min(2_000_000);
    let langs = youtube_langs_from_env();
    let langs_arg = if langs.is_empty() {
        "en,en-US".to_string()
    } else {
        langs.join(",")
    };

    let tmpdir = tempfile::tempdir().map_err(|_| "youtube_tempdir_failed".to_string())?;
    let out_tmpl = tmpdir.path().join("%(id)s.%(ext)s");

    // Use VTT when possible (easy to parse deterministically).
    // We ask for both human subs and auto subs; yt-dlp will use what exists.
    let mut cmd = Command::new("yt-dlp");
    cmd.arg("--skip-download")
        .arg("--write-sub")
        .arg("--write-auto-sub")
        .arg("--sub-lang")
        .arg(&langs_arg)
        .arg("--sub-format")
        .arg("vtt")
        .arg("-o")
        .arg(out_tmpl.to_string_lossy().to_string())
        .arg("--no-warnings")
        .arg(url);

    // Best-effort timeout: if it runs too long, kill.
    // std::process::Command doesn't have builtin timeout; we do a coarse approach:
    // spawn and wait with a sleep loop.
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("youtube_ytdlp_spawn_failed: {e}"))?;
    let start = std::time::Instant::now();
    loop {
        if let Some(st) = child
            .try_wait()
            .map_err(|e| format!("youtube_ytdlp_wait_failed: {e}"))?
        {
            if !st.success() {
                return Err("youtube_ytdlp_nonzero_exit".to_string());
            }
            break;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            return Err("youtube_ytdlp_timeout".to_string());
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Pick the first VTT file produced.
    let mut best: Option<PathBuf> = None;
    if let Ok(rd) = std::fs::read_dir(tmpdir.path()) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.extension().and_then(|s| s.to_str()) == Some("vtt") {
                best = Some(p);
                break;
            }
        }
    }
    let Some(p) = best else {
        return Err("youtube_no_captions_found".to_string());
    };
    let vtt = std::fs::read_to_string(&p).map_err(|e| format!("youtube_read_failed: {e}"))?;
    Ok(vtt_to_text(&vtt, max_chars))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_video_id_variants() {
        let u = url::Url::parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap();
        assert_eq!(youtube_video_id(&u).as_deref(), Some("dQw4w9WgXcQ"));
        let u = url::Url::parse("https://youtu.be/dQw4w9WgXcQ").unwrap();
        assert_eq!(youtube_video_id(&u).as_deref(), Some("dQw4w9WgXcQ"));
        let u = url::Url::parse("https://www.youtube.com/shorts/dQw4w9WgXcQ").unwrap();
        assert_eq!(youtube_video_id(&u).as_deref(), Some("dQw4w9WgXcQ"));
        let u = url::Url::parse("https://www.youtube.com/embed/dQw4w9WgXcQ").unwrap();
        assert_eq!(youtube_video_id(&u).as_deref(), Some("dQw4w9WgXcQ"));
    }

    #[test]
    fn vtt_to_text_drops_timings() {
        let vtt = r#"WEBVTT

00:00:00.000 --> 00:00:01.000
Hello   world

00:00:01.000 --> 00:00:02.000
Second line
"#;
        let t = vtt_to_text(vtt, 10_000);
        assert!(t.contains("Hello world"));
        assert!(t.contains("Second line"));
        assert!(!t.contains("-->"));
    }
}
