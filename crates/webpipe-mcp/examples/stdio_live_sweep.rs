// Run (from webpipe/):
//   WEBPIPE_LIVE=1 cargo run -p webpipe-mcp --features stdio --example stdio_live_sweep
//
// Notes:
// - This is a *live* network sweep (public URLs). It is intentionally small + bounded.
// - It does not require search provider keys (uses urls-mode for web_search_extract).

#[cfg(not(feature = "stdio"))]
fn main() {
    eprintln!("stdio_live_sweep requires `--features stdio` (or default features enabled)");
}

#[cfg(feature = "stdio")]
use rmcp::{
    model::CallToolRequestParam,
    service::ServiceExt,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
#[cfg(feature = "stdio")]
use serde_json as _;
#[cfg(feature = "stdio")]
use tokio::process::Command;

#[cfg(feature = "stdio")]
fn first_text(result: &rmcp::model::CallToolResult) -> Option<&str> {
    result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.as_str())
}

#[cfg(feature = "stdio")]
fn first_text_owned(result: &rmcp::model::CallToolResult) -> Option<String> {
    result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
}

#[cfg(feature = "stdio")]
fn has_control_chars(s: &str) -> bool {
    s.chars().any(|c| {
        c == '\u{FEFF}'
            || c == '\u{0000}'
            || c == '\u{000C}'
            || (c <= '\u{001F}' && c != '\n' && c != '\t')
            || c == '\u{007F}'
            || c == '\r'
    })
}

#[cfg(feature = "stdio")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping live sweep: set WEBPIPE_LIVE=1 to run");
        return Ok(());
    }

    // Prefer an explicit binary path (portable across machines/CI).
    // Example:
    //   WEBPIPE_BIN=/abs/path/to/webpipe WEBPIPE_LIVE=1 cargo run -p webpipe-mcp --example stdio_live_sweep --features stdio
    let bin = if let Ok(p) = std::env::var("WEBPIPE_BIN") {
        p
    } else {
        // Resolve to the `webpipe/` workspace root, then use its `target/` by default.
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = root
            .parent()
            .and_then(|p| p.parent())
            .ok_or("failed to compute webpipe workspace root")?;
        workspace_root
            .join("target")
            .join("debug")
            .join("webpipe")
            .to_string_lossy()
            .to_string()
    };
    let service = ()
        .serve(TokioChildProcess::new(Command::new(bin).configure(
            |cmd| {
                cmd.arg("mcp-stdio");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-live-sweep-cache"),
                );
            },
        ))?)
        .await?;

    // “Real-ish” URLs, but keep it small:
    // - HTML landing page
    // - Docs HTML
    // - Markdown (raw)
    // - JSON API response
    // - PDF (bounded bytes; likely truncated)
    let targets: Vec<(&str, &str)> = vec![
        ("rust-home", "https://www.rust-lang.org/"),
        ("rust-book", "https://doc.rust-lang.org/book/"),
        (
            "gh-raw-md",
            "https://raw.githubusercontent.com/rust-lang/rust/master/README.md",
        ),
        ("gh-api-json", "https://api.github.com/repos/rust-lang/rust"),
        ("arxiv-pdf", "https://arxiv.org/pdf/1706.03762.pdf"),
    ];

    for (name, url) in targets {
        // Prefer extract (bounded, structured) over fetch (raw) for most usage.
        let r = tokio::time::timeout(
            std::time::Duration::from_secs(25),
            service.call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": url,
                        "fetch_backend": "local",
                        "cache_read": true,
                        "cache_write": true,
                        "timeout_ms": 12_000,
                        "max_bytes": 2_000_000,
                        "max_chars": 15_000,
                        "width": 100,
                        "include_text": false,
                        "include_links": false,
                        "include_structure": true,
                        "max_outline_items": 30,
                        "max_blocks": 50,
                        "max_block_chars": 500,
                        "top_chunks": 5,
                        "max_chunk_chars": 500
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            }),
        )
        .await;

        let Some(s) = r
            .ok()
            .and_then(|r| r.ok())
            .and_then(|r| first_text_owned(&r))
        else {
            println!("{name}: web_extract: timeout_or_transport_error url={url}");
            continue;
        };
        let v: serde_json::Value = serde_json::from_str(&s)?;

        let ok = v["ok"].as_bool().unwrap_or(false);
        let status = v["status"].as_u64().unwrap_or(0);
        let ct = v["content_type"].as_str().unwrap_or("");
        let engine = v["extract"]["engine"].as_str().unwrap_or("");
        let elapsed = v["elapsed_ms"].as_u64().unwrap_or(0);
        let warnings_n = v["warning_codes"].as_array().map(|a| a.len()).unwrap_or(0);

        println!(
            "{name}: web_extract ok={ok} status={status} ct={ct} engine={engine} warnings={warnings_n} elapsed_ms={elapsed}"
        );
    }

    // One raw fetch with include_text=true to sanity-check “clean text” on real HTML.
    let r2 = tokio::time::timeout(
        std::time::Duration::from_secs(25),
        service.call_tool(CallToolRequestParam {
            name: "web_fetch".into(),
            arguments: Some(
                serde_json::json!({
                    "url": "https://www.rust-lang.org/",
                    "fetch_backend": "local",
                    "cache_read": true,
                    "cache_write": true,
                    "timeout_ms": 12_000,
                    "max_bytes": 500_000,
                    "include_text": true,
                    "max_text_chars": 20_000,
                    "include_headers": false
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        }),
    )
    .await??;
    let s2 = first_text(&r2).unwrap_or("");
    let v2: serde_json::Value = serde_json::from_str(s2)?;
    if let Some(t) = v2["text"].as_str() {
        println!(
            "web_fetch(clean_text): has_control_chars={} text_chars={}",
            has_control_chars(t),
            t.chars().count()
        );
    } else {
        println!("web_fetch(clean_text): no text field (unexpected)");
    }

    service.cancel().await?;
    Ok(())
}
