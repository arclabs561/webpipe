// Run (from anywhere):
//   cargo run --manifest-path webpipe/Cargo.toml -p webpipe-mcp --features stdio --example stdio_smoke

#[cfg(not(feature = "stdio"))]
fn main() {
    eprintln!("stdio_smoke requires `--features stdio` (or default features enabled)");
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
use std::net::SocketAddr;
#[cfg(feature = "stdio")]
use tokio::process::Command;

#[cfg(feature = "stdio")]
fn first_text(result: &rmcp::model::CallToolResult) -> Option<&str> {
    result
        .content
        .get(0)
        .and_then(|c| c.as_text())
        .map(|t| t.text.as_str())
}

#[cfg(feature = "stdio")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Prefer an explicit binary path (portable across machines/CI).
    // Example:
    //   WEBPIPE_BIN=/abs/path/to/webpipe cargo run -p webpipe-mcp --example stdio_smoke --features stdio
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
                    std::env::temp_dir().join("webpipe-smoke-cache"),
                );
            },
        ))?)
        .await?;

    // Local fixture server for deterministic fetch/extract checks.
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(|| async {
            (
                [("content-type", "text/html")],
                "<html><body><h1>Hello</h1><p>world</p><a href=\"/a#x\">A</a></body></html>",
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("axum serve");
    });
    let fixture_url = format!("http://{}/", addr);

    let tools = service.list_tools(Default::default()).await?;
    println!("tools: {}", tools.tools.len());
    {
        use std::collections::BTreeSet;
        let names: BTreeSet<String> = tools
            .tools
            .iter()
            .map(|t| t.name.clone().into_owned())
            .collect();
        for must_have in [
            "webpipe_meta",
            "web_fetch",
            "web_extract",
            "web_search",
            "web_search_extract",
            "web_deep_research",
        ] {
            if !names.contains(must_have) {
                return Err(format!("missing tool {must_have}").into());
            }
        }
    }

    // Meta should always work offline and without keys.
    let meta = service
        .call_tool(CallToolRequestParam {
            name: "webpipe_meta".into(),
            arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
        })
        .await?;
    println!("webpipe_meta: {:?}", meta.is_error);
    if let Some(s) = first_text(&meta) {
        let v: serde_json::Value = serde_json::from_str(s)?;
        assert_eq!(v["schema_version"].as_u64(), Some(1));
        assert_eq!(v["kind"].as_str(), Some("webpipe_meta"));
        assert_eq!(v["ok"].as_bool(), Some(true));
        assert_eq!(v["name"].as_str(), Some("webpipe"));
        assert!(v["configured"].is_object());
        // Secret-safety by shape: these must be booleans, never key material.
        assert!(v["configured"]["providers"]["brave"].is_boolean());
        assert!(v["configured"]["providers"]["tavily"].is_boolean());
        assert!(v["configured"]["remote_fetch"]["firecrawl"].is_boolean());
        // Meta should advertise provider values + defaults.
        assert!(v["supported"]["providers"].is_array());
        assert!(v["defaults"]["web_search"]["provider"].is_string());
    }

    // This should return either:
    // - ok=true (if a provider key is configured), or
    // - ok=false error.code=not_configured (still a successful tool call).
    let search = service
        .call_tool(CallToolRequestParam {
            name: "web_search".into(),
            arguments: Some(
                serde_json::json!({"query":"test"})
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        })
        .await?;
    println!("web_search: {:?}", search.is_error);
    if let Some(s) = first_text(&search) {
        let v: serde_json::Value = serde_json::from_str(s)?;
        assert_eq!(v["schema_version"].as_u64(), Some(1));
        assert!(v.get("ok").and_then(|x| x.as_bool()).is_some());
        // Always echo enough input context to debug failures.
        assert!(v.get("provider").and_then(|x| x.as_str()).is_some());
        assert!(v.get("query").and_then(|x| x.as_str()).is_some());
        assert!(v.get("max_results").and_then(|x| x.as_u64()).is_some());
    }

    // Offline mode: urls provided, no paid search required.
    let search_extract = service
        .call_tool(CallToolRequestParam {
            name: "web_search_extract".into(),
            arguments: Some(
                serde_json::json!({
                    "query": "hello",
                    "urls": [fixture_url],
                    "max_urls": 1,
                    "top_chunks": 3,
                    "max_chunk_chars": 120,
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
    println!("web_search_extract: {:?}", search_extract.is_error);
    if let Some(s) = first_text(&search_extract) {
        let v: serde_json::Value = serde_json::from_str(s)?;
        assert_eq!(v["schema_version"].as_u64(), Some(1));
        assert_eq!(v["kind"].as_str(), Some("web_search_extract"));
        assert_eq!(v["ok"].as_bool(), Some(true));
        assert!(v["top_chunks"].is_array());
    }

    // Exercise web_fetch (bounded, deterministic JSON).
    let fetch = service
        .call_tool(CallToolRequestParam {
            name: "web_fetch".into(),
            arguments: Some(
                serde_json::json!({
                    "url": fixture_url,
                    "timeout_ms": 10_000,
                    "max_bytes": 200_000,
                    "include_headers": false
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await?;
    println!("web_fetch: {:?}", fetch.is_error);
    if let Some(s) = first_text(&fetch) {
        let v: serde_json::Value = serde_json::from_str(s)?;
        assert_eq!(v["schema_version"].as_u64(), Some(1));
        assert_eq!(v["ok"].as_bool(), Some(true));
        assert!(v["status"].as_u64().unwrap_or(0) >= 200);
    }

    // Exercise web_extract with query/chunks/links (bounded output).
    let extract = service
        .call_tool(CallToolRequestParam {
            name: "web_extract".into(),
            arguments: Some(
                serde_json::json!({
                    "url": fixture_url,
                    "width": 80,
                    "max_chars": 5000,
                    "query": "example domain",
                    "top_chunks": 3,
                    "max_chunk_chars": 240,
                    "include_text": false,
                    "include_links": true,
                    "max_links": 10,
                    "timeout_ms": 10_000,
                    "max_bytes": 200_000
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await?;
    println!("web_extract: {:?}", extract.is_error);
    if let Some(s) = first_text(&extract) {
        let v: serde_json::Value = serde_json::from_str(s)?;
        assert_eq!(v["schema_version"].as_u64(), Some(1));
        assert_eq!(v["ok"].as_bool(), Some(true));
        // include_text=false should omit "text" when query is set
        assert!(v.get("text").is_none());
        assert!(v.get("chunks").is_some());
        assert!(v.get("links").is_some());
        assert!(v["links"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .any(|x| x.as_str() == Some(&format!("http://{}/a", addr))));
    }

    service.cancel().await?;
    Ok(())
}
