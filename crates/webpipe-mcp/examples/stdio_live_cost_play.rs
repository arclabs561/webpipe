// Run (from webpipe/):
//   WEBPIPE_LIVE=1 WEBPIPE_BIN=/abs/path/to/webpipe cargo run -p webpipe-mcp --features stdio --example stdio_live_cost_play
//
// Notes:
// - This is a live “play” script. It intentionally performs *paid* calls when keys are configured.
// - It is bounded (small max_results/max_urls/top_chunks) and prints only counts/cost units.

#[cfg(not(feature = "stdio"))]
fn main() {
    eprintln!("stdio_live_cost_play requires `--features stdio` (or default features enabled)");
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
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping live cost play: set WEBPIPE_LIVE=1 to run");
        return Ok(());
    }

    // Cost discipline: keep “auto semantic fallback” off for attribution in this script.
    // This script already explicitly compares semantic_rerank off vs on later.
    let semantic_auto = std::env::var("WEBPIPE_LIVE_SEMANTIC_AUTO")
        .ok()
        .as_deref()
        .is_some_and(|v| v == "1");

    let bin = std::env::var("WEBPIPE_BIN")
        .map_err(|_| "WEBPIPE_BIN is required; point it at the built `webpipe` binary")?;

    let service = ()
        .serve(TokioChildProcess::new(Command::new(bin).configure(
            |cmd| {
                cmd.arg("mcp-stdio");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-live-cost-cache"),
                );
            },
        ))?)
        .await?;

    fn print_usage(v: &serde_json::Value) {
        let sp = v["usage"]["search_providers"].clone();
        let llm = v["usage"]["llm_backends"].clone();
        let fetch = v["usage"]["fetch_backends"].clone();
        println!("usage.search_providers={}", sp);
        println!("usage.llm_backends={}", llm);
        println!("usage.fetch_backends={}", fetch);
    }

    // Reset usage counters so the report is attributable to this run.
    let _ = service
        .call_tool(CallToolRequestParam {
            name: "webpipe_usage_reset".into(),
            arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
        })
        .await?;

    // 1) Paid search (Brave): minimal.
    let r1 = service
        .call_tool(CallToolRequestParam {
            name: "web_search".into(),
            arguments: Some(
                serde_json::json!({
                    "provider": "brave",
                    "query": "webpipe mcp stdio",
                    "max_results": 2
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await?;
    if let Some(v) = payload_from_result(&r1) {
        let ok = v["ok"].as_bool().unwrap_or(false);
        let backend = v["backend_provider"].as_str().unwrap_or("");
        let cost = v["cost_units"].as_u64().unwrap_or(0);
        let n = v["results"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("web_search(brave): ok={ok} backend={backend} results={n} cost_units={cost}");
    }

    // 2) Auto search (should pick Brave first if configured).
    let r2 = service
        .call_tool(CallToolRequestParam {
            name: "web_search".into(),
            arguments: Some(
                serde_json::json!({
                    "provider": "auto",
                    "auto_mode": "fallback",
                    "query": "rust book ownership",
                    "max_results": 2
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await?;
    if let Some(v) = payload_from_result(&r2) {
        let ok = v["ok"].as_bool().unwrap_or(false);
        let backend = v["backend_provider"].as_str().unwrap_or("");
        let cost = v["cost_units"].as_u64().unwrap_or(0);
        let n = v["results"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("web_search(auto): ok={ok} backend={backend} results={n} cost_units={cost}");
    }

    // 3) Extract on known-good URLs (bounded). We'll warm cache first, then compare embeddings OFF vs ON.
    let urls = vec![
        "https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html",
        "https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html",
    ];
    for url in &urls {
        let r = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": url,
                        "fetch_backend": "local",
                        "timeout_ms": 20_000,
                        "max_bytes": 2_000_000,
                        "max_chars": 20_000,
                        "query": "ownership",
                        "top_chunks": 3,
                        "max_chunk_chars": 300,
                        "semantic_auto_fallback": semantic_auto,
                        "include_text": false,
                        "include_links": false,
                        "include_structure": true
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        if let Some(v) = payload_from_result(&r) {
            let ok = v["ok"].as_bool().unwrap_or(false);
            let status = v["status"].as_u64().unwrap_or(0);
            let engine = v["extract"]["engine"].as_str().unwrap_or("");
            let elapsed = v["elapsed_ms"].as_u64().unwrap_or(0);
            println!(
                "warm.web_extract: ok={ok} status={status} engine={engine} elapsed_ms={elapsed}"
            );
        }
    }

    for (label, semantic_rerank) in [
        ("urls_mode.semantic_off", false),
        ("urls_mode.semantic_on", true),
    ] {
        let r = service
            .call_tool(CallToolRequestParam {
                name: "web_search_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "provider": "auto",
                        "query": "Rust ownership",
                        "urls": urls,
                        "max_urls": 2,
                        "timeout_ms": 20_000,
                        "max_bytes": 2_000_000,
                        "top_chunks": 5,
                        "max_chunk_chars": 400,
                        "semantic_rerank": semantic_rerank,
                        "semantic_top_k": 5,
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
            let backend = v["backend_provider"].as_str().unwrap_or("");
            let top = v["top_chunks"].as_array().map(|a| a.len()).unwrap_or(0);
            let elapsed = v["elapsed_ms"].as_u64().unwrap_or(0);
            let warnings = v["warning_codes"].as_array().map(|a| a.len()).unwrap_or(0);
            println!(
                "{label}: ok={ok} backend={backend} top_chunks={top} warnings={warnings} elapsed_ms={elapsed}"
            );
            if let Some(first) = v["results"].as_array().and_then(|a| a.first()) {
                let per_url_chunks = first["extract"]["chunks"]
                    .as_array()
                    .map(|a| a.len())
                    .unwrap_or(0);
                println!("  {label}.per_url.chunks(first_url)={per_url_chunks}");
                let sem_backend = first["extract"]["semantic"]["backend"]
                    .as_str()
                    .unwrap_or("");
                if !sem_backend.is_empty() {
                    println!("  {label}.semantic.backend(first_url)={sem_backend}");
                }
            }
        }
    }

    // Usage summary: shows provider cost_units and call counts.
    let u = service
        .call_tool(CallToolRequestParam {
            name: "webpipe_usage".into(),
            arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
        })
        .await?;
    if let Some(v) = payload_from_result(&u) {
        // Keep it bounded: print only the provider backend summaries (no per-call logs).
        print_usage(&v);
    }

    service.cancel().await?;
    Ok(())
}
