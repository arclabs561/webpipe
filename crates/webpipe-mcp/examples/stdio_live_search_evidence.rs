// Run (from webpipe/):
//   WEBPIPE_LIVE=1 WEBPIPE_BIN=/abs/path/to/webpipe \
//   WEBPIPE_QUERY="your research question" \
//   cargo run -p webpipe-mcp --features stdio --example stdio_live_search_evidence
//
// Notes:
// - This is a bounded “research pull” harness for the `search_evidence` tool.
// - It uses configured search providers (default: brave) and prints a short citation-friendly
//   Sources list + top chunks.

#[cfg(not(feature = "stdio"))]
fn main() {
    eprintln!("stdio_live_search_evidence requires `--features stdio`");
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
        eprintln!("skipping live search: set WEBPIPE_LIVE=1 to run");
        return Ok(());
    }
    let bin = std::env::var("WEBPIPE_BIN")
        .map_err(|_| "WEBPIPE_BIN is required; point it at the built `webpipe` binary")?;
    let query = std::env::var("WEBPIPE_QUERY").map_err(|_| "WEBPIPE_QUERY is required")?;

    let provider = std::env::var("WEBPIPE_PROVIDER").unwrap_or_else(|_| "brave".to_string());

    let service = ()
        .serve(TokioChildProcess::new(Command::new(bin).configure(
            |cmd| {
                cmd.arg("mcp-stdio");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-live-search-evidence-cache"),
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
                    "provider": provider,
                    "max_results": 5,
                    "max_urls": 3,
                    "top_chunks": 8,
                    "max_chunk_chars": 800,
                    "include_text": false,
                    "include_links": false
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
    let ok = v["ok"].as_bool().unwrap_or(false);
    println!("ok={ok}");
    if let Some(w) = v.get("warning_codes") {
        println!("warnings={w}");
    }

    if let Some(sources) = v.get("sources").and_then(|x| x.as_array()) {
        println!("\n== sources ==");
        for (i, s) in sources.iter().take(8).enumerate() {
            let url = s.get("url").and_then(|x| x.as_str()).unwrap_or("");
            let title = s.get("title").and_then(|x| x.as_str()).unwrap_or("");
            println!("[{}] {} — {}", i + 1, short(title, 90), url);
        }
    }

    if let Some(chunks) = v.get("top_chunks").and_then(|x| x.as_array()) {
        println!("\n== top chunks ==");
        for (i, c) in chunks.iter().take(6).enumerate() {
            let url = c.get("url").and_then(|x| x.as_str()).unwrap_or("");
            let txt = c.get("text").and_then(|x| x.as_str()).unwrap_or("").trim();
            println!(
                "\n-- chunk {} --\nurl={}\ntext={}\n",
                i + 1,
                url,
                short(txt, 520)
            );
        }
    }

    service.cancel().await?;
    Ok(())
}
