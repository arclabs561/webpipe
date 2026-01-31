// Run (from webpipe/):
//   WEBPIPE_LIVE=1 WEBPIPE_BIN=/abs/path/to/webpipe cargo run -p webpipe-mcp --features stdio --example stdio_live_research_extraction
//
// Notes:
// - This is a bounded live research helper: it searches a few known terms and extracts short evidence chunks.
// - It prints URLs + short excerpts so you can cite what we actually observed.

#[cfg(not(feature = "stdio"))]
fn main() {
    eprintln!("stdio_live_research_extraction requires `--features stdio`");
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
fn first_nonempty_str(v: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
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
fn maybe_rewrite_research_url(url: &str) -> String {
    // Research-oriented rewrites to avoid low-signal HTML shells.
    // - GitHub repo → raw README (HEAD) for higher signal.
    if let Ok(u) = reqwest::Url::parse(url.trim()) {
        if u.host_str() == Some("github.com") {
            let parts: Vec<&str> = u.path().trim_matches('/').split('/').collect();
            if parts.len() >= 2 {
                let owner = parts[0];
                let repo = parts[1];
                return format!("https://raw.githubusercontent.com/{owner}/{repo}/HEAD/README.md");
            }
        }
    }
    url.trim().to_string()
}

#[cfg(feature = "stdio")]
fn pick_best_url(results: &[serde_json::Value]) -> Option<String> {
    // Prefer direct PDFs (papers/datasets). Otherwise use first URL.
    for r in results {
        if let Some(u) = first_nonempty_str(r, &["url", "link"]) {
            let lc = u.to_ascii_lowercase();
            if lc.ends_with(".pdf") {
                return Some(u);
            }
        }
    }
    results
        .first()
        .and_then(|r| first_nonempty_str(r, &["url", "link"]))
}

#[cfg(feature = "stdio")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping live research: set WEBPIPE_LIVE=1 to run");
        return Ok(());
    }

    // Cost discipline: avoid accidental embeddings during “research” pulls.
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
                    std::env::temp_dir().join("webpipe-live-research-cache"),
                );
            },
        ))?)
        .await?;

    let queries = vec![
        // Datasets / eval
        "CleanEval corpus boilerplate removal",
        // Classic papers
        "\"Boilerplate Detection using Shallow Text Features\" PDF",
        // Common real-world algorithms/libs
        "Mozilla Readability algorithm arc90",
        "trafilatura evaluation boilerplate removal",
        "jusText boilerplate removal paper",
    ];

    for q in queries {
        // Bounded search.
        let sr = service
            .call_tool(CallToolRequestParam {
                name: "web_search".into(),
                arguments: Some(
                    serde_json::json!({
                        "provider": "brave",
                        "query": q,
                        "max_results": 3
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let Some(sv) = payload_from_result(&sr) else {
            println!("search: query={q} payload_missing");
            continue;
        };
        let ok = sv["ok"].as_bool().unwrap_or(false);
        if !ok {
            let code = sv["error"]["code"].as_str().unwrap_or("unknown_error");
            println!("search: query={q} ok=false error.code={code}");
            continue;
        }
        let results = sv["results"].as_array().cloned().unwrap_or_default();
        let Some(url0) = pick_best_url(&results) else {
            println!("search: query={q} ok=true results=0");
            continue;
        };
        let url = maybe_rewrite_research_url(&url0);
        let title = results
            .first()
            .and_then(|r| first_nonempty_str(r, &["title"]))
            .unwrap_or_default();
        println!("\n== query: {q}\n- title: {title}\n- url: {url0}");
        if url != url0 {
            println!("- rewrite: {url}");
        }

        // Bounded extract: prefer chunks; keep output short.
        let er = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": url,
                        "fetch_backend": "local",
                        "timeout_ms": 20_000,
                        "max_bytes": 2_000_000,
                        "max_chars": 12_000,
                        "query": q,
                        "top_chunks": 3,
                        "max_chunk_chars": 350,
                        "semantic_auto_fallback": semantic_auto,
                        "include_text": false,
                        "include_links": false,
                        "include_structure": true,
                        "max_outline_items": 20,
                        "max_blocks": 25,
                        "max_block_chars": 250
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        let Some(ev) = payload_from_result(&er) else {
            println!("- extract: payload_missing");
            continue;
        };
        let ok = ev["ok"].as_bool().unwrap_or(false);
        let engine = ev["extract"]["engine"].as_str().unwrap_or("");
        let warning_codes = ev
            .get("warning_codes")
            .cloned()
            .unwrap_or(serde_json::json!([]));
        println!("- extract: ok={ok} engine={engine} warning_codes={warning_codes}");
        if let Some(ch) = ev["chunks"].as_array().and_then(|a| a.first()) {
            let txt = ch["text"].as_str().unwrap_or("").trim();
            if !txt.is_empty() {
                println!("- excerpt: {}", short(txt, 280));
            }
        }
    }

    service.cancel().await?;
    Ok(())
}
