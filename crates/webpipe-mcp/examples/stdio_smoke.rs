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
fn payload(result: &rmcp::model::CallToolResult) -> serde_json::Value {
    // Prefer MCP structured payload: tool `content` is often Markdown.
    if let Some(v) = result.structured_content.clone() {
        return v;
    }
    // Fallback: scan text parts for embedded JSON.
    for c in &result.content {
        if let Some(t) = c.as_text() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t.text) {
                return v;
            }
        }
    }
    serde_json::json!({})
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
        // Respect explicit CARGO_TARGET_DIR so examples work with custom target dirs.
        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .ok()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| workspace_root.join("target"));
        target_dir
            .join("debug")
            .join("webpipe")
            .to_string_lossy()
            .to_string()
    };

    // Local fixture server for deterministic fetch/extract checks.
    //
    // We also host:
    // - explore pages (/a -> /p2)
    // - sitemap discovery (/robots.txt, /sitemap.xml)
    // - repo_ingest GitHub-ish API + raw endpoints (/api/*, /owner/repo/*)
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    let base = format!("http://{addr}");
    let fixture_url = format!("{base}/");

    let robots_body = format!("User-agent: *\nSitemap: {base}/sitemap.xml\n");
    let sitemap_body =
        format!("<urlset><url><loc>{base}/</loc></url><url><loc>{base}/a</loc></url></urlset>");

    let app = axum::Router::new()
        .route(
            "/",
            axum::routing::get(|| async {
                (
                    [("content-type", "text/html")],
                    "<html><body><h1>Hello</h1><p>world</p><a href=\"/a#x\">A</a></body></html>",
                )
            }),
        )
        .route(
            "/a",
            axum::routing::get(|| async {
                (
                    [("content-type", "text/html")],
                    "<html><body><main><h2>Page A</h2><p>alpha beta</p><a href=\"/p2\">P2</a></main></body></html>",
                )
            }),
        )
        .route(
            "/p2",
            axum::routing::get(|| async {
                (
                    [("content-type", "text/html")],
                    "<html><body><main><h2>Page P2</h2><p>beta gamma</p></main></body></html>",
                )
            }),
        )
        .route(
            "/robots.txt",
            axum::routing::get(move || {
                let body = robots_body.clone();
                async move { ([("content-type", "text/plain")], body) }
            }),
        )
        .route(
            "/sitemap.xml",
            axum::routing::get(move || {
                let body = sitemap_body.clone();
                async move { ([("content-type", "application/xml")], body) }
            }),
        )
        // GitHub-ish API endpoints for repo_ingest.
        .route(
            "/api/repos/owner/repo",
            axum::routing::get(|| async {
                (
                    [("content-type", "application/json")],
                    "{\"default_branch\":\"main\"}",
                )
            }),
        )
        .route(
            "/api/repos/owner/repo/git/trees/main",
            axum::routing::get(|| async {
                (
                    [("content-type", "application/json")],
                    "{\"tree\":[{\"path\":\"README.md\",\"type\":\"blob\"},{\"path\":\"src/lib.rs\",\"type\":\"blob\"}]}",
                )
            }),
        )
        // Raw file endpoints.
        .route(
            "/owner/repo/main/README.md",
            axum::routing::get(|| async {
                (
                    [("content-type", "text/plain")],
                    "# Hello\n\nThis is the README.\n",
                )
            }),
        )
        .route(
            "/owner/repo/main/src/lib.rs",
            axum::routing::get(|| async {
                (
                    [("content-type", "text/plain")],
                    "pub fn hello() -> &'static str { \"hello\" }\n",
                )
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("axum serve");
    });

    let api_base = format!("{base}/api");
    let raw_host = format!("127.0.0.1:{}", addr.port());
    let service = ()
        .serve(TokioChildProcess::new(Command::new(bin).configure(
            |cmd| {
                cmd.arg("mcp-stdio");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-smoke-cache"),
                );
                cmd.env("WEBPIPE_GITHUB_API_BASE", &api_base);
                cmd.env("WEBPIPE_GITHUB_RAW_HOST", &raw_host);
            },
        ))?)
        .await?;

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
            "web_seed_urls",
            "web_fetch",
            "web_extract",
            "web_search",
            "web_search_extract",
            "web_deep_research",
            "web_explore_extract",
            "web_sitemap_extract",
            "web_cache_search_extract",
            "repo_ingest",
        ] {
            if !names.contains(must_have) {
                return Err(format!("missing tool {must_have}").into());
            }
        }
    }

    // Seed urls should work offline and return stable ids.
    let seed_urls = service
        .call_tool(CallToolRequestParam {
            name: "web_seed_urls".into(),
            arguments: Some(
                serde_json::json!({"max": 3, "ids_only": true})
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        })
        .await?;
    println!("web_seed_urls: {:?}", seed_urls.is_error);
    let v = payload(&seed_urls);
    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert_eq!(v["kind"].as_str(), Some("web_seed_urls"));
    assert_eq!(v["ok"].as_bool(), Some(true));
    assert_eq!(v["max"].as_u64(), Some(3));
    assert!(v["ids"].is_array());
    assert!(v["seeds"].is_array());
    // ids_only should omit kind/note in seeds entries.
    let seeds = v["seeds"].as_array().unwrap();
    assert_eq!(seeds.len(), 3);
    assert!(seeds[0].get("kind").is_none());
    assert!(seeds[0].get("note").is_none());

    // Some IDE clients occasionally send no arguments (or null) for tools with optional params.
    // This should still return a valid payload shape.
    let seed_urls_no_args = service
        .call_tool(CallToolRequestParam {
            name: "web_seed_urls".into(),
            arguments: None,
        })
        .await?;
    println!("web_seed_urls(no args): {:?}", seed_urls_no_args.is_error);
    let v = payload(&seed_urls_no_args);
    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert_eq!(v["kind"].as_str(), Some("web_seed_urls"));
    assert_eq!(v["ok"].as_bool(), Some(true));
    assert!(v["seeds"].is_array());

    // Meta should always work offline and without keys.
    let meta = service
        .call_tool(CallToolRequestParam {
            name: "webpipe_meta".into(),
            arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
        })
        .await?;
    println!("webpipe_meta: {:?}", meta.is_error);
    let v = payload(&meta);
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

    // This should return either:
    // - ok=true (if a provider key is configured), or
    // - ok=false error.code=not_configured (MCP is_error may be true; still returns a structured payload).
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
    let v = payload(&search);
    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert!(v.get("ok").and_then(|x| x.as_bool()).is_some());
    // Always echo enough input context to debug failures.
    assert!(v.get("provider").and_then(|x| x.as_str()).is_some());
    assert!(v.get("query").and_then(|x| x.as_str()).is_some());
    assert!(v.get("max_results").and_then(|x| x.as_u64()).is_some());

    // Some IDE clients send missing args even for required-arg tools. Ensure this does not become
    // a transport-level -32602; it should return ok=false + invalid_params in-band.
    let search_no_args = service
        .call_tool(CallToolRequestParam {
            name: "web_search".into(),
            arguments: None,
        })
        .await?;
    println!("web_search(no args): {:?}", search_no_args.is_error);
    let v = payload(&search_no_args);
    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert_eq!(v["kind"].as_str(), Some("web_search"));
    assert_eq!(v["ok"].as_bool(), Some(false));
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_params"));

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
    let v = payload(&search_extract);
    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert_eq!(v["kind"].as_str(), Some("web_search_extract"));
    assert_eq!(v["ok"].as_bool(), Some(true));
    assert!(v["top_chunks"].is_array());

    // Explore+extract should crawl within bounds.
    let explore = service
        .call_tool(CallToolRequestParam {
            name: "web_explore_extract".into(),
            arguments: Some(
                serde_json::json!({
                    "start_url": fixture_url,
                    "query": "beta",
                    "max_pages": 3,
                    "max_depth": 2,
                    "same_host_only": true,
                    "max_links_per_page": 10,
                    "include_links": false
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await?;
    println!("web_explore_extract: {:?}", explore.is_error);
    let v = payload(&explore);
    assert_eq!(v["kind"].as_str(), Some("web_explore_extract"));
    assert_eq!(v["ok"].as_bool(), Some(true));
    assert!(v["top_chunks"].is_array());

    // Sitemap discovery should find URLs and (with query) return extracted top_chunks.
    let sitemap = service
        .call_tool(CallToolRequestParam {
            name: "web_sitemap_extract".into(),
            arguments: Some(
                serde_json::json!({
                    "site_url": fixture_url,
                    "query": "alpha",
                    "max_sitemaps": 2,
                    "max_urls": 10,
                    "extract": true,
                    "fetch_backend": "local",
                    "max_results": 2,
                    "top_chunks": 3,
                    "max_chunk_chars": 160
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await?;
    println!("web_sitemap_extract: {:?}", sitemap.is_error);
    let v = payload(&sitemap);
    assert_eq!(v["kind"].as_str(), Some("web_sitemap_extract"));
    assert_eq!(v["ok"].as_bool(), Some(true));
    assert!(v["urls"].is_array());
    // When query is set and extract=true, we should include `extract` payload.
    assert!(v.get("extract").is_some());

    // Repo ingest should build a small combined text corpus.
    let ingest = service
        .call_tool(CallToolRequestParam {
            name: "repo_ingest".into(),
            arguments: Some(
                serde_json::json!({
                    "repo_url": "http://github.com/owner/repo",
                    "reference": "main",
                    "max_files": 10,
                    "max_total_bytes": 200000,
                    "max_file_bytes": 200000,
                    "timeout_ms": 10000,
                    "include_combined_text": true
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await?;
    println!("repo_ingest: {:?}", ingest.is_error);
    let v = payload(&ingest);
    assert_eq!(v["kind"].as_str(), Some("repo_ingest"));
    assert_eq!(v["ok"].as_bool(), Some(true));
    let combined = v["combined_text"].as_str().unwrap_or("");
    assert!(combined.contains("Repository: owner/repo"));
    assert!(combined.contains("FILE: README.md"));

    // Cache search should return some results after we warmed cache via earlier fetch/extract calls.
    let cache_search = service
        .call_tool(CallToolRequestParam {
            name: "web_cache_search_extract".into(),
            arguments: Some(
                serde_json::json!({
                    "query": "Hello",
                    "max_docs": 10,
                    "max_scan_entries": 200,
                    "top_chunks": 3,
                    "max_chunk_chars": 120,
                    "include_text": false,
                    "include_structure": false,
                    "compact": true
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await?;
    println!("web_cache_search_extract: {:?}", cache_search.is_error);
    let v = payload(&cache_search);
    assert_eq!(v["kind"].as_str(), Some("web_cache_search_extract"));
    assert!(v.get("ok").and_then(|x| x.as_bool()).is_some());

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
    let v = payload(&fetch);
    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert_eq!(v["ok"].as_bool(), Some(true));
    assert!(v["status"].as_u64().unwrap_or(0) >= 200);

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
    let v = payload(&extract);
    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert_eq!(v["ok"].as_bool(), Some(true));
    // include_text=false should omit "text" when query is set
    assert!(v.get("text").is_none());
    assert!(v.get("chunks").is_some());
    assert!(v.get("links").is_some());
    let empty = vec![];
    assert!(v["links"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .any(|x| x.as_str() == Some(&format!("http://{}/a", addr))));

    service.cancel().await?;
    Ok(())
}
