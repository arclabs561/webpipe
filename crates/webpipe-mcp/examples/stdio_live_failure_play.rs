// Run (from webpipe/):
//   WEBPIPE_LIVE=1 WEBPIPE_BIN=/abs/path/to/webpipe cargo run -p webpipe-mcp --features stdio --example stdio_live_failure_play
//
// Notes:
// - This is a live “failure mode” play script. It intentionally triggers common tail risks in a bounded way:
//   - cache-only miss
//   - truncation (and truncation-retry) behavior
//   - PDF link/structure limitations
// - It prints warnings + usage summaries, not full content.

#[cfg(not(feature = "stdio"))]
fn main() {
    eprintln!("stdio_live_failure_play requires `--features stdio` (or default features enabled)");
}

#[cfg(feature = "stdio")]
use rmcp::{
    model::CallToolRequestParam,
    service::ServiceExt,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
#[cfg(feature = "stdio")]
use tokio::process::Command;

#[cfg(feature = "stdio")]
fn payload_from_result(result: &rmcp::model::CallToolResult) -> Option<serde_json::Value> {
    result.structured_content.clone().or_else(|| {
        result
            .content
            .first()
            .and_then(|c| c.as_text())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t.text).ok())
    })
}

#[cfg(feature = "stdio")]
fn print_usage(v: &serde_json::Value) {
    println!("usage.search_providers={}", v["usage"]["search_providers"]);
    println!("usage.fetch_backends={}", v["usage"]["fetch_backends"]);
    println!("usage.llm_backends={}", v["usage"]["llm_backends"]);
    println!("warnings.counts={}", v["warnings"]["counts"]);
}

#[cfg(feature = "stdio")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping live failure play: set WEBPIPE_LIVE=1 to run");
        return Ok(());
    }

    // Cost discipline: failure-mode play should not accidentally trigger embeddings.
    let semantic_auto = std::env::var("WEBPIPE_LIVE_SEMANTIC_AUTO")
        .ok()
        .as_deref()
        .is_some_and(|v| v == "1");

    let bin = std::env::var("WEBPIPE_BIN")
        .map_err(|_| "WEBPIPE_BIN is required; point it at the built `webpipe` binary")?;

    // Use a fresh cache dir to make cache-only miss deterministic.
    let empty_cache_dir = std::env::temp_dir().join("webpipe-live-empty-cache");
    let _ = std::fs::create_dir_all(&empty_cache_dir);

    let service = ()
        .serve(TokioChildProcess::new(Command::new(bin).configure(
            |cmd| {
                cmd.arg("mcp-stdio");
                cmd.env("WEBPIPE_CACHE_DIR", &empty_cache_dir);
            },
        ))?)
        .await?;

    // Reset usage so this run is attributable.
    let _ = service
        .call_tool(CallToolRequestParam {
            name: "webpipe_usage_reset".into(),
            arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
        })
        .await?;

    // 1) Cache-only miss (no_network=true on fresh cache dir).
    let miss_url = "https://example.com/";
    let r1 = service
        .call_tool(CallToolRequestParam {
            name: "web_extract".into(),
            arguments: Some(
                serde_json::json!({
                    "url": miss_url,
                    "fetch_backend": "local",
                    "no_network": true,
                    "cache_read": true,
                    "cache_write": false,
                    "timeout_ms": 5_000,
                    "max_bytes": 200_000,
                    "query": "example domain",
                    "top_chunks": 3,
                    "max_chunk_chars": 200,
                    "semantic_auto_fallback": semantic_auto,
                    "include_text": false,
                    "include_links": false
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await?;
    if let Some(v) = payload_from_result(&r1) {
        println!(
            "cache_only_miss: ok={} error.code={} fetch_backend={}",
            v["ok"].as_bool().unwrap_or(false),
            v["error"]["code"].as_str().unwrap_or(""),
            v["request"]["fetch_backend"].as_str().unwrap_or("")
        );
    }

    // 2) Truncation retry (on a long-ish doc page): deliberately small max_bytes + retry enabled.
    // We use Rust book chapter, which is stable and public.
    let long_url = "https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html";
    for (label, retry_on_truncation) in [("no_retry", false), ("with_retry", true)] {
        let r = service
            .call_tool(CallToolRequestParam {
                name: "web_search_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "provider": "auto",
                        "query": "Rust ownership",
                        "urls": [long_url],
                        "max_urls": 1,
                        "fetch_backend": "local",
                        "timeout_ms": 20_000,
                        "max_bytes": 80_000,
                        "retry_on_truncation": retry_on_truncation,
                        "truncation_retry_max_bytes": 400_000,
                        "top_chunks": 5,
                        "max_chunk_chars": 300,
                        "agentic": false,
                        "include_text": false,
                        "include_links": false,
                        "compact": false
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        if let Some(v) = payload_from_result(&r) {
            let ok = v["ok"].as_bool().unwrap_or(false);
            let elapsed = v["elapsed_ms"].as_u64().unwrap_or(0);
            let w = v["warning_codes"].as_array().map(|a| a.len()).unwrap_or(0);
            println!("truncation.{label}: ok={ok} warning_codes={w} elapsed_ms={elapsed}");
            if let Some(first) = v["results"].as_array().and_then(|a| a.first()) {
                let truncated = first["truncated"].as_bool().unwrap_or(false);
                let attempts = first
                    .get("attempts")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                println!("  truncation.{label}.per_url.truncated={truncated}");
                println!("  truncation.{label}.per_url.attempts={attempts}");
            }
        }
    }

    // 3) PDF limitations: links are unavailable for PDFs; ensure we see the warning.
    let pdf_url = "https://arxiv.org/pdf/1706.03762.pdf";
    let r3 = service
        .call_tool(CallToolRequestParam {
            name: "web_extract".into(),
            arguments: Some(
                serde_json::json!({
                    "url": pdf_url,
                    "fetch_backend": "local",
                    "timeout_ms": 25_000,
                    "max_bytes": 2_000_000,
                    "max_chars": 10_000,
                    "query": "attention",
                    "top_chunks": 3,
                    "max_chunk_chars": 250,
                    "semantic_auto_fallback": semantic_auto,
                    "include_text": false,
                    "include_links": true,
                    "max_links": 10
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await?;
    if let Some(v) = payload_from_result(&r3) {
        let ok = v["ok"].as_bool().unwrap_or(false);
        let engine = v["extract"]["engine"].as_str().unwrap_or("");
        let warnings = v["warning_codes"].clone();
        println!("pdf_extract: ok={ok} engine={engine} warning_codes={warnings}");
    }

    // Usage summary: confirms the backend mix we exercised.
    let u = service
        .call_tool(CallToolRequestParam {
            name: "webpipe_usage".into(),
            arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
        })
        .await?;
    if let Some(v) = payload_from_result(&u) {
        print_usage(&v);
    }

    service.cancel().await?;
    Ok(())
}
