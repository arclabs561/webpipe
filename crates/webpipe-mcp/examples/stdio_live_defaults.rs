// Run (from webpipe/):
//   WEBPIPE_LIVE=1 WEBPIPE_BIN=/abs/path/to/webpipe cargo run -p webpipe-mcp --features stdio --example stdio_live_defaults
//
// Notes:
// - This is a live network smoke. Keep it small + bounded.
// - It is explicitly designed to exercise *defaults* (omit most knobs).

#[cfg(not(feature = "stdio"))]
fn main() {
    eprintln!("stdio_live_defaults requires `--features stdio` (or default features enabled)");
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
fn first_text_owned(result: &rmcp::model::CallToolResult) -> Option<String> {
    result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
}

#[cfg(feature = "stdio")]
fn payload_from_result(result: &rmcp::model::CallToolResult) -> Option<serde_json::Value> {
    // Prefer structured_content (canonical). Fall back to parsing content[0].text if present.
    if let Some(v) = result.structured_content.clone() {
        return Some(v);
    }
    let s = first_text_owned(result)?;
    serde_json::from_str(&s).ok()
}

#[cfg(feature = "stdio")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping live defaults smoke: set WEBPIPE_LIVE=1 to run");
        return Ok(());
    }

    // Cost discipline: disable auto semantic fallback by default for live “defaults” play,
    // so we don't accidentally trigger embeddings. Opt in explicitly if you want to measure it.
    let semantic_auto = std::env::var("WEBPIPE_LIVE_SEMANTIC_AUTO")
        .ok()
        .as_deref()
        .is_some_and(|v| v == "1");

    let bin = std::env::var("WEBPIPE_BIN").map_err(|_| {
        "WEBPIPE_BIN is required for this smoke; point it at the built `webpipe` binary"
    })?;

    let service = ()
        .serve(TokioChildProcess::new(Command::new(bin).configure(
            |cmd| {
                cmd.arg("mcp-stdio");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-live-defaults-cache"),
                );
            },
        ))?)
        .await?;

    // Targets chosen to exercise:
    // - HTML landing page
    // - Docs HTML
    // - Raw markdown
    // - JSON API
    let urls = vec![
        ("rust-home", "https://www.rust-lang.org/", "rust"),
        ("rust-book", "https://doc.rust-lang.org/book/", "cargo"),
        (
            "gh-raw-md",
            "https://raw.githubusercontent.com/rust-lang/rust/master/README.md",
            "compiler",
        ),
        (
            "gh-api-json",
            "https://api.github.com/repos/rust-lang/rust",
            "rust",
        ),
    ];

    for (name, url, query) in urls {
        let r = tokio::time::timeout(
            std::time::Duration::from_secs(25),
            service.call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                // Intentionally omit include_structure / selection knobs: use defaults.
                arguments: Some(
                    serde_json::json!({
                        "url": url,
                        "query": query,
                        "fetch_backend": "local",
                        "timeout_ms": 12_000,
                        "max_bytes": 2_000_000,
                        "max_chars": 20_000,
                        "top_chunks": 5,
                        "max_chunk_chars": 500,
                        "semantic_auto_fallback": semantic_auto,
                        "include_links": false
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            }),
        )
        .await;

        let Some(v) = r
            .ok()
            .and_then(|r| r.ok())
            .and_then(|r| payload_from_result(&r))
        else {
            println!("{name}: web_extract: timeout_or_transport_error url={url}");
            continue;
        };

        let ok = v["ok"].as_bool().unwrap_or(false);
        let status = v["status"].as_u64().unwrap_or(0);
        let ct = v["content_type"].as_str().unwrap_or("");
        let engine = v["extract"]["engine"].as_str().unwrap_or("");
        let warnings_n = v["warning_codes"].as_array().map(|a| a.len()).unwrap_or(0);
        let blocks_n = v["structure"]["blocks"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        let top_chunks_n = v["chunks"].as_array().map(|a| a.len()).unwrap_or(0);
        println!(
            "{name}: ok={ok} status={status} ct={ct} engine={engine} structure_blocks={blocks_n} chunks={top_chunks_n} warnings={warnings_n}"
        );

        // Sanity: HTML pages should not come back as engine="text" with raw markup chunks.
        if ct.starts_with("text/html") {
            assert_ne!(
                engine, "text",
                "unexpected: HTML content treated as plain text (engine=text)"
            );
        }
        // Sanity: query is provided, so this tool should include `chunks`.
        if ok {
            assert!(
                top_chunks_n > 0,
                "expected chunks when ok=true and query is set"
            );
        }
    }

    // Defaults test for search+extract in urls-mode (no search provider required).
    let r2 = tokio::time::timeout(
        std::time::Duration::from_secs(25),
        service.call_tool(CallToolRequestParam {
            name: "web_search_extract".into(),
            // Intentionally omit selection_mode/url_selection_mode/include_structure: use defaults.
            arguments: Some(
                serde_json::json!({
                    "query": "install",
                    "urls": [
                        "https://doc.rust-lang.org/book/",
                        "https://www.rust-lang.org/"
                    ],
                    "max_urls": 2,
                    "max_results": 2,
                    "timeout_ms": 12_000,
                    "max_bytes": 2_000_000,
                    "top_chunks": 5,
                    "max_chunk_chars": 400,
                    "include_text": false,
                    "include_links": false
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        }),
    )
    .await??;
    let v2 = payload_from_result(&r2).ok_or("missing structured_content")?;
    assert_eq!(v2["ok"].as_bool(), Some(true));
    let mode = v2["request"]["selection_mode"].as_str().unwrap_or("");
    let url_mode = v2["request"]["url_selection_mode"].as_str().unwrap_or("");
    println!("web_search_extract(defaults): selection_mode={mode} url_selection_mode={url_mode}");

    service.cancel().await?;
    Ok(())
}
