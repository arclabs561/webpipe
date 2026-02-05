// Run (from webpipe/):
//   WEBPIPE_LIVE=1 WEBPIPE_BIN=/abs/path/to/webpipe \
//   WEBPIPE_PROBE_URLS="https://example.com/,https://example.org/" \
//   WEBPIPE_PROBE_QUERY="your question here" \
//   cargo run -p webpipe-mcp --features stdio --example stdio_live_probe_urls
//
// Notes:
// - This is a small, bounded “what does the tool return right now?” harness.
// - It uses urls-mode (no search provider required).

#[cfg(not(feature = "stdio"))]
fn main() {
    eprintln!("stdio_live_probe_urls requires `--features stdio`");
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
fn parse_urls_env(v: &str) -> Vec<String> {
    v.split(|c: char| c == ',' || c.is_whitespace())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(feature = "stdio")]
fn short(s: &str, n: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= n {
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(feature = "stdio")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping live probe: set WEBPIPE_LIVE=1 to run");
        return Ok(());
    }

    let bin = std::env::var("WEBPIPE_BIN")
        .map_err(|_| "WEBPIPE_BIN is required; point it at the built `webpipe` binary")?;
    let urls_raw =
        std::env::var("WEBPIPE_PROBE_URLS").map_err(|_| "WEBPIPE_PROBE_URLS is required")?;
    let query = std::env::var("WEBPIPE_PROBE_QUERY")
        .unwrap_or_else(|_| "what is this page about".to_string());
    let urls = parse_urls_env(&urls_raw);
    if urls.is_empty() {
        return Err("WEBPIPE_PROBE_URLS parsed to 0 urls".into());
    }

    let service = ()
        .serve(TokioChildProcess::new(Command::new(bin).configure(
            |cmd| {
                cmd.arg("mcp-stdio");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-live-probe-cache"),
                );
            },
        ))?)
        .await?;

    let r = service
        .call_tool(CallToolRequestParam {
            name: "search_evidence".into(),
            arguments: Some(
                serde_json::json!({
                    "query": query,
                    "urls": urls,
                    "max_urls": 2,
                    "max_results": 0,
                    "fetch_backend": "local",
                    "include_text": false,
                    "include_links": false,
                    "top_chunks": 8,
                    "max_chunk_chars": 800
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await?;

    let Some(v) = payload_from_result(&r) else {
        return Err("missing structured_content payload".into());
    };

    println!("ok={}", v["ok"].as_bool().unwrap_or(false));
    println!(
        "warnings={}",
        v.get("warning_codes")
            .cloned()
            .unwrap_or(serde_json::json!([]))
    );

    if let Some(arr) = v.get("top_chunks").and_then(|x| x.as_array()) {
        println!("top_chunks={}", arr.len());
        for (i, c) in arr.iter().take(3).enumerate() {
            let url = c.get("url").and_then(|x| x.as_str()).unwrap_or("");
            let txt = c.get("text").and_then(|x| x.as_str()).unwrap_or("").trim();
            println!(
                "\n-- chunk {} --\nurl={}\ntext={}\n",
                i + 1,
                url,
                short(txt, 420)
            );
        }
    }

    service.cancel().await?;
    Ok(())
}
