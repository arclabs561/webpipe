use axum::{http::header, response::IntoResponse, routing::get, Router};
use rmcp::{
    model::CallToolRequestParam,
    service::{RoleClient, RunningService, ServiceExt},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::net::SocketAddr;

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
    r.structured_content
        .clone()
        .expect("expected structured_content")
}

#[tokio::test]
async fn webpipe_web_fetch_handles_variety_of_urls_and_content_types() {
    // A local server with different content-types, redirect, and “URL shape” variants.
    let app = Router::new()
        .route(
            "/html",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html")],
                    "<html><body><h1>Hello</h1><p>alpha</p></body></html>",
                )
            }),
        )
        .route(
            "/json",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"k":"v","n":1}"#,
                )
            }),
        )
        .route(
            "/md",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/markdown")],
                    "Hello markdown\n\n- item\n",
                )
            }),
        )
        .route(
            "/text",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    "plain text body",
                )
            }),
        )
        .route(
            "/redirect",
            get(|| async {
                // 302 -> /html (fragment should never be sent; query should be ok).
                (
                    axum::http::StatusCode::FOUND,
                    [(header::LOCATION, "/html")],
                    "".to_string(),
                )
                    .into_response()
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");

    // Spawn the real MCP stdio server binary and talk to it over stdio.
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                // Disable `.env` autoload so this test stays hermetic.
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                // Ensure no provider keys are present.
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

    // URL shapes: query params and fragments should not break the fetcher.
    let urls = vec![
        format!("{base}/html?x=1"),
        format!("{base}/json"),
        format!("{base}/md#frag"),
        format!("{base}/text?y=2#z"),
        format!("{base}/redirect?via=1#frag"),
    ];

    for url in &urls {
        let v = call(
            &service,
            "web_fetch",
            serde_json::json!({
                "url": url,
                "fetch_backend": "local",
                "cache_read": true,
                "cache_write": true,
                "timeout_ms": 5000,
                "max_bytes": 1_000_000,
                "include_text": true,
                "max_text_chars": 4000
            }),
        )
        .await;
        assert_eq!(v["schema_version"].as_u64(), Some(2));
        assert_eq!(v["kind"].as_str(), Some("web_fetch"));
        assert_eq!(v["ok"].as_bool(), Some(true), "fetch failed for {url}");
        assert!(v["final_url"].as_str().unwrap_or("").starts_with(&base));
        assert!(v["content_type"].as_str().unwrap_or("").len() >= 4);
        assert!(v["status"].as_u64().unwrap_or(0) >= 200);
    }

    // Redirect should land on /html.
    let v_redir = call(
        &service,
        "web_fetch",
        serde_json::json!({
            "url": format!("{base}/redirect"),
            "fetch_backend": "local",
            "include_text": true,
            "max_text_chars": 2000
        }),
    )
    .await;
    assert_eq!(v_redir["ok"].as_bool(), Some(true));
    assert!(
        v_redir["final_url"]
            .as_str()
            .unwrap_or("")
            .contains("/html"),
        "expected redirect final_url to include /html"
    );
    assert!(
        v_redir["body_text"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("hello"),
        "expected html text"
    );

    service.cancel().await.expect("cancel");
}
