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
async fn webpipe_web_extract_handles_js_shell_redirects_long_pages_and_link_farms() {
    // A local server with “docs-like” failure modes and variety:
    // - JS-heavy UI shell
    // - Redirect chain
    // - Long body to trigger truncation
    // - Link farm to test link bounding
    let js_shell = {
        let big = "#".repeat(2000);
        format!(
            r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <title>Docs</title>
    <script>
      // Simulate a SPA shell (lots of JS, little real HTML body text)
      window.__NEXT_DATA__ = {{"buildId":"x","page":"/docs","props":{{"pageProps":{{"data":"{big}"}}}}}};
    </script>
  </head>
  <body>
    <div id="__next"></div>
    <noscript>This site requires JavaScript.</noscript>
  </body>
</html>
"#
        )
    };

    let link_farm = {
        let mut s = String::new();
        s.push_str("<html><body><main><h1>Index</h1><ul>");
        for i in 0..60 {
            s.push_str(&format!(
                r#"<li><a href="/article?x={i}">link{i}</a></li>"#
            ));
        }
        s.push_str("</ul></main></body></html>");
        s
    };

    let long_body = {
        let mut s = String::new();
        s.push_str("<html><body><main><h1>Long</h1>");
        for _ in 0..20_000 {
            s.push_str("alpha ");
        }
        s.push_str("</main></body></html>");
        s
    };

    let app = Router::new()
        .route(
            "/spa",
            get(move || async move { ([(header::CONTENT_TYPE, "text/html")], js_shell) }),
        )
        .route(
            "/article",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html")],
                    "<html><body><main><h1>Article</h1><p>alpha beta gamma</p><p>delta epsilon</p></main></body></html>",
                )
            }),
        )
        .route(
            "/links",
            get(move || async move { ([(header::CONTENT_TYPE, "text/html")], link_farm) }),
        )
        .route(
            "/long",
            get(move || async move { ([(header::CONTENT_TYPE, "text/html")], long_body) }),
        )
        .route(
            "/redir1",
            get(|| async {
                (
                    axum::http::StatusCode::FOUND,
                    [(header::LOCATION, "/article")],
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

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
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

    // JS shell: should extract something, but often low signal.
    let v_spa = call(
        &service,
        "web_extract",
        serde_json::json!({
            "url": format!("{base}/spa"),
            "fetch_backend": "local",
            "cache_read": true,
            "cache_write": true,
            "timeout_ms": 5000,
            "max_bytes": 1_000_000,
            "max_chars": 8000,
            "width": 80,
            "include_text": false,
            "include_links": false,
            "include_structure": true,
            "top_chunks": 5,
            "max_chunk_chars": 400,
        }),
    )
    .await;
    assert_eq!(v_spa["schema_version"].as_u64(), Some(1));
    assert_eq!(v_spa["kind"].as_str(), Some("web_extract"));
    assert_eq!(v_spa["ok"].as_bool(), Some(true));
    assert!(v_spa.get("warnings").is_some(), "expected warnings array to exist");

    // Redirect: final_url should land on /article.
    let v_redir = call(
        &service,
        "web_extract",
        serde_json::json!({
            "url": format!("{base}/redir1"),
            "fetch_backend": "local",
            "cache_read": true,
            "cache_write": true,
            "timeout_ms": 5000,
            "max_bytes": 1_000_000,
            "max_chars": 8000,
            "width": 80,
            "include_text": true,
            "include_links": false,
            "include_structure": false,
            "top_chunks": 5,
            "max_chunk_chars": 400,
        }),
    )
    .await;
    assert_eq!(v_redir["ok"].as_bool(), Some(true));
    assert!(v_redir["final_url"].as_str().unwrap_or("").contains("/article"));

    // Link farm: include_links bounded.
    let v_links = call(
        &service,
        "web_extract",
        serde_json::json!({
            "url": format!("{base}/links"),
            "fetch_backend": "local",
            "cache_read": true,
            "cache_write": true,
            "timeout_ms": 5000,
            "max_bytes": 1_000_000,
            "max_chars": 8000,
            "width": 80,
            "include_text": false,
            "include_links": true,
            "max_links": 10,
            "include_structure": false,
            "top_chunks": 5,
            "max_chunk_chars": 400,
        }),
    )
    .await;
    assert_eq!(v_links["ok"].as_bool(), Some(true));
    let links = v_links.get("links").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    assert!(!links.is_empty(), "expected some links extracted");
    assert!(links.len() <= 10, "links should be bounded");

    // Long page with small max_bytes should warn about truncation.
    let v_long = call(
        &service,
        "web_extract",
        serde_json::json!({
            "url": format!("{base}/long"),
            "fetch_backend": "local",
            "cache_read": false,
            "cache_write": false,
            "timeout_ms": 5000,
            "max_bytes": 2000,
            "max_chars": 8000,
            "width": 80,
            "include_text": false,
            "include_links": false,
            "include_structure": false,
            "top_chunks": 5,
            "max_chunk_chars": 400,
        }),
    )
    .await;
    assert_eq!(v_long["ok"].as_bool(), Some(true));
    let ws = v_long
        .get("warnings")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        ws.iter().any(|w| w.as_str() == Some("body_truncated_by_max_bytes")),
        "expected body_truncated_by_max_bytes warning"
    );
}

