use axum::{routing::get, routing::post, Json, Router};
use rmcp::{
    model::CallToolRequestParam,
    service::{RoleClient, RunningService, ServiceExt},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::net::SocketAddr;
use std::sync::Arc;

async fn call(
    service: &RunningService<RoleClient, ()>,
    name: &'static str,
    args: serde_json::Value,
) -> serde_json::Value {
    let r = service
        .call_tool(CallToolRequestParam {
            name: name.to_string().into(),
            arguments: Some(args.as_object().cloned().unwrap()),
        })
        .await
        .expect("call_tool");
    // Prefer MCP structured content (stable machine payload). Tool `content` may be Markdown.
    if let Some(v) = r.structured_content.clone() {
        return v;
    }
    for c in &r.content {
        if let Some(t) = c.as_text() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t.text) {
                return v;
            }
        }
    }
    panic!("expected structured_content or JSON text content");
}

#[tokio::test]
async fn webpipe_search_extract_stubbed_search_and_offline_cache() {
    // Deterministic localhost server that provides:
    // - "docs" pages for fetch/extract
    // - search-provider shaped endpoints for Brave/Tavily/SearXNG
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base = Arc::new(format!("http://{addr}"));

    let base_for_brave = Arc::clone(&base);
    let base_for_tavily = Arc::clone(&base);
    let base_for_searxng = Arc::clone(&base);

    let app = Router::new()
        .route(
            "/docs",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/html")],
                    r#"<html><body><h1>Docs</h1><p>See route handlers.</p></body></html>"#,
                )
            }),
        )
        .route(
            "/docs/app/getting-started/route-handlers",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/html")],
                    r#"<html><body><h1>Route handlers</h1><p>Use GET/POST handlers.</p></body></html>"#,
                )
            }),
        )
        .route(
            "/install",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/html")],
                    r#"<html><body><h1>Install</h1><p>Linux install instructions.</p></body></html>"#,
                )
            }),
        )
        .route(
            "/install/linux",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/html")],
                    r#"<html><body><h1>Install on Linux</h1><p>Use apt.</p></body></html>"#,
                )
            }),
        )
        // Brave-like: GET /brave -> { web: { results: [{url,title,description}] } }
        .route(
            "/brave",
            get(move || {
                let base = Arc::clone(&base_for_brave);
                async move {
                    Json(serde_json::json!({
                        "web": {
                            "results": [
                                {
                                    "url": format!("{}/docs/app/getting-started/route-handlers", base),
                                    "title": "Route handlers",
                                    "description": "GET/POST handlers"
                                }
                            ]
                        }
                    }))
                }
            }),
        )
        // Tavily-like: POST /tavily -> { results: [{url,title,content}], usage: {credits} }
        .route(
            "/tavily",
            post(move |_body: Json<serde_json::Value>| {
                let base = Arc::clone(&base_for_tavily);
                async move {
                    Json(serde_json::json!({
                        "results": [
                            {"url": format!("{}/install/linux", base), "title":"Install on Linux", "content":"Use apt."}
                        ],
                        "usage": { "credits": 1 }
                    }))
                }
            }),
        )
        // SearXNG-like: GET /searxng/search?format=json -> { results: [{url,title,content}] }
        .route(
            "/searxng/search",
            get(move || {
                let base = Arc::clone(&base_for_searxng);
                async move {
                    Json(serde_json::json!({
                        "results": [
                            {"url": format!("{}/docs/app/getting-started/route-handlers", base), "title":"Route handlers", "content":"GET/POST handlers"}
                        ]
                    }))
                }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("axum serve");
    });

    let doc_url = format!("{}/docs/app/getting-started/route-handlers", base);
    let searxng_endpoint = format!("{}/searxng", base);

    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                // Disable `.env` autoload so this contract stays hermetic.
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                cmd.env("WEBPIPE_SEARXNG_ENDPOINT", &searxng_endpoint);
                // Ensure we don't accidentally use real keys on dev machines.
                cmd.env_remove("WEBPIPE_BRAVE_API_KEY");
                cmd.env_remove("BRAVE_SEARCH_API_KEY");
                cmd.env_remove("WEBPIPE_TAVILY_API_KEY");
                cmd.env_remove("TAVILY_API_KEY");
                cmd.env_remove("WEBPIPE_FIRECRAWL_API_KEY");
                cmd.env_remove("FIRECRAWL_API_KEY");
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    // --- 1) Search+fetch+extract via stubbed SearXNG endpoint ---
    let v1 = call(
        &service,
        "web_search_extract",
        serde_json::json!({
            "provider": "searxng",
            "query": "route handlers",
            "max_results": 1,
            "max_urls": 1,
            "agentic": false,
            "url_selection_mode": "preserve",
            "selection_mode": "score",
            "fetch_backend": "local",
            "timeout_ms": 2000,
            "max_bytes": 200000,
            "cache_read": true,
            "cache_write": true,
            "no_network": false,
            "top_chunks": 2,
            "max_chunk_chars": 200,
            "include_links": false,
            "include_text": false
        }),
    )
    .await;
    assert_eq!(v1["ok"].as_bool(), Some(true));
    assert_eq!(v1["request"]["provider"].as_str(), Some("searxng"));
    assert!(v1["results"]
        .as_array()
        .map(|a| !a.is_empty())
        .unwrap_or(false));
    assert!(v1["search"]["steps"]
        .as_array()
        .map(|a| !a.is_empty())
        .unwrap_or(false));

    // --- 2) Warm cache via urls-mode, then verify offline replay succeeds ---
    let warm = call(
        &service,
        "web_search_extract",
        serde_json::json!({
            "provider": "auto",
            "auto_mode": "fallback",
            "urls": [doc_url],
            "agentic": false,
            "url_selection_mode": "preserve",
            "selection_mode": "score",
            "fetch_backend": "local",
            "timeout_ms": 2000,
            "max_bytes": 200000,
            "cache_read": true,
            "cache_write": true,
            "no_network": false,
            "top_chunks": 2,
            "max_chunk_chars": 200,
            "include_links": false,
            "include_text": false
        }),
    )
    .await;
    assert_eq!(warm["ok"].as_bool(), Some(true));

    let offline = call(
        &service,
        "web_search_extract",
        serde_json::json!({
            "provider": "auto",
            "auto_mode": "fallback",
            "urls": [doc_url],
            "agentic": false,
            "url_selection_mode": "preserve",
            "selection_mode": "score",
            "fetch_backend": "local",
            "timeout_ms": 2000,
            "max_bytes": 200000,
            "cache_read": true,
            "cache_write": false,
            "no_network": true,
            "top_chunks": 2,
            "max_chunk_chars": 200,
            "include_links": false,
            "include_text": false
        }),
    )
    .await;
    assert_eq!(offline["ok"].as_bool(), Some(true));

    service.cancel().await.expect("cancel");
}
