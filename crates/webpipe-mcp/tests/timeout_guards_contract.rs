use axum::{http::header, routing::get, Router};
use rmcp::{
    model::CallToolRequestParam,
    service::{RoleClient, RunningService, ServiceExt},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::net::SocketAddr;

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

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
async fn web_extract_pipeline_timeout_returns_warning_code_instead_of_hanging() {
    // Make extraction non-trivial so `spawn_blocking` won't complete immediately.
    let filler = "x".repeat(200_000);
    let body = format!(
        "<html><body><main><h1>Doc</h1><p>{filler}</p><p>TAIL_MARKER_123</p></main></body></html>"
    );
    let app = Router::new().route(
        "/doc",
        get(move || {
            let b = body.clone();
            async move { ([(header::CONTENT_TYPE, "text/html")], b) }
        }),
    );
    let addr = serve(app).await;
    let url = format!("http://{addr}/doc");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                // Force immediate timeout to make this deterministic.
                cmd.env("WEBPIPE_EXTRACT_PIPELINE_TIMEOUT_MS", "0");
                // Keep this test hermetic.
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-timeout-guards"),
                );
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let v = call(
        &service,
        "web_extract",
        serde_json::json!({
            "url": url,
            "fetch_backend": "local",
            "query": "TAIL_MARKER_123",
            "timeout_ms": 10_000,
            "max_bytes": 600_000,
            "max_chars": 30_000,
            "include_text": false,
            "include_structure": true
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(false));
    let codes = v["warning_codes"].as_array().cloned().unwrap_or_default();
    assert!(
        codes
            .iter()
            .any(|c| c.as_str() == Some("extract_pipeline_timeout")),
        "warning_codes={codes:?} payload={v}"
    );
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("timed out"),
        "error={}",
        v["error"]
    );

    service.cancel().await.expect("cancel");
}

#[tokio::test]
async fn web_extract_links_timeout_is_bounded_and_returns_empty_links() {
    let body =
        "<html><body><main><h1>Doc</h1><p>hello</p><a href=\"/a\">A</a></main></body></html>";
    let body_s = body.to_string();
    let app = Router::new()
        .route(
            "/doc",
            get(move || {
                let b = body_s.clone();
                async move { ([(header::CONTENT_TYPE, "text/html")], b) }
            }),
        )
        .route(
            "/a",
            get(|| async { ([(header::CONTENT_TYPE, "text/html")], "ok") }),
        );
    let addr = serve(app).await;
    let url = format!("http://{addr}/doc");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                // Force immediate link-timeout deterministically.
                cmd.env("WEBPIPE_LINKS_TIMEOUT_MS", "0");
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-timeout-guards"),
                );
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let v = call(
        &service,
        "web_extract",
        serde_json::json!({
            "url": url,
            "fetch_backend": "local",
            "query": "hello",
            "include_links": true,
            "max_links": 50,
            "timeout_ms": 10_000,
            "max_bytes": 200_000,
            "max_chars": 5_000,
            "include_text": false,
            "include_structure": false
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true));
    // When link extraction times out, links should be empty and warning code present.
    assert_eq!(
        v.pointer("/extract/links")
            .and_then(|x| x.as_array())
            .map(|a| a.len()),
        Some(0),
        "extract.links={}",
        v["extract"]["links"]
    );
    let codes = v["warning_codes"].as_array().cloned().unwrap_or_default();
    assert!(
        codes.iter().any(|c| c.as_str() == Some("links_timeout")),
        "warning_codes={codes:?} payload={v}"
    );

    service.cancel().await.expect("cancel");
}

#[tokio::test]
async fn web_extract_semantic_timeout_is_deterministic_at_zero() {
    let body = "<html><body><main><h1>Doc</h1><p>cleanup function example</p></main></body></html>";
    let body_s = body.to_string();
    let app = Router::new().route(
        "/doc",
        get(move || {
            let b = body_s.clone();
            async move { ([(header::CONTENT_TYPE, "text/html")], b) }
        }),
    );
    let addr = serve(app).await;
    let url = format!("http://{addr}/doc");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_SEMANTIC_TIMEOUT_MS", "0");
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-timeout-guards"),
                );
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let v = call(
        &service,
        "web_extract",
        serde_json::json!({
            "url": url,
            "fetch_backend": "local",
            "query": "cleanup function",
            "semantic_rerank": true,
            "semantic_top_k": 3,
            "timeout_ms": 10_000,
            "max_bytes": 200_000,
            "max_chars": 5_000,
            "include_text": false,
            "include_structure": false
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true));
    let codes = v["warning_codes"].as_array().cloned().unwrap_or_default();
    assert!(
        codes
            .iter()
            .any(|c| c.as_str() == Some("semantic_rerank_timeout")),
        "warning_codes={codes:?} payload={v}"
    );
    let sem_warns = v
        .pointer("/extract/semantic/warnings")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        sem_warns
            .iter()
            .any(|c| c.as_str() == Some("semantic_rerank_timeout")),
        "semantic.warnings={sem_warns:?} payload={v}"
    );

    service.cancel().await.expect("cancel");
}

#[tokio::test]
async fn web_search_extract_links_timeout_is_deterministic_at_zero() {
    let body =
        "<html><body><main><h1>Doc</h1><p>hello</p><a href=\"/a\">A</a></main></body></html>";
    let body_s = body.to_string();
    let app = Router::new()
        .route(
            "/doc",
            get(move || {
                let b = body_s.clone();
                async move { ([(header::CONTENT_TYPE, "text/html")], b) }
            }),
        )
        .route(
            "/a",
            get(|| async { ([(header::CONTENT_TYPE, "text/html")], "ok") }),
        );
    let addr = serve(app).await;
    let url = format!("http://{addr}/doc");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                // Force immediate link-timeout deterministically.
                cmd.env("WEBPIPE_LINKS_TIMEOUT_MS", "0");
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-timeout-guards"),
                );
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let v = call(
        &service,
        "search_evidence",
        serde_json::json!({
            "query": "hello",
            "urls": [url],
            "fetch_backend": "local",
            "include_links": true,
            "max_links": 50,
            "max_urls": 1,
            "timeout_ms": 10_000,
            "max_bytes": 200_000,
            "max_chars": 5_000,
            "top_chunks": 3,
            "max_chunk_chars": 200,
            "include_text": false,
            "include_structure": false,
            "compact": false
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true));
    assert_eq!(v["kind"].as_str(), Some("web_search_extract"));
    let res0 = v["results"]
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    assert_eq!(
        res0.pointer("/extract/links")
            .and_then(|x| x.as_array())
            .map(|a| a.len()),
        Some(0),
        "extract.links={}",
        res0["extract"]["links"]
    );
    let codes = res0["warning_codes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        codes.iter().any(|c| c.as_str() == Some("links_timeout")),
        "warning_codes={codes:?} payload={v}"
    );

    service.cancel().await.expect("cancel");
}

#[tokio::test]
async fn web_explore_extract_links_timeout_is_deterministic_at_zero() {
    let start_html = r#"
    <html><body>
      <h1>Start</h1>
      <a href="/deep">needle docs</a>
      <a href="/noise">other</a>
    </body></html>
    "#;
    let deep_html = r#"
    <html><body>
      <h1>Deep</h1>
      <p>This page contains the needle content.</p>
    </body></html>
    "#;

    let start_s = start_html.to_string();
    let deep_s = deep_html.to_string();
    let app = Router::new()
        .route(
            "/start",
            get(move || {
                let body = start_s.clone();
                async move { ([(header::CONTENT_TYPE, "text/html")], body) }
            }),
        )
        .route(
            "/deep",
            get(move || {
                let body = deep_s.clone();
                async move { ([(header::CONTENT_TYPE, "text/html")], body) }
            }),
        );
    let addr = serve(app).await;
    let start_url = format!("http://{addr}/start");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_MCP_TOOLSET", "debug");
                cmd.env("WEBPIPE_LINKS_TIMEOUT_MS", "0");
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-timeout-guards"),
                );
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let v = call(
        &service,
        "web_explore_extract",
        serde_json::json!({
            "start_url": start_url,
            "query": "needle",
            "max_pages": 1,
            "max_depth": 0,
            "same_host_only": true,
            "max_links_per_page": 50,
            "timeout_ms": 10_000,
            "max_bytes": 500_000,
            "max_chars": 10_000,
            "top_chunks": 3,
            "max_chunk_chars": 300,
            "include_links": true,
            "no_network": false,
            "cache_read": false,
            "cache_write": false
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true));
    let pages0 = v["pages"]
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    assert_eq!(
        pages0
            .get("links")
            .and_then(|x| x.as_array())
            .map(|a| a.len()),
        Some(0),
        "links={}",
        pages0
            .get("links")
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    );
    let codes = pages0["warning_codes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        codes.iter().any(|c| c.as_str() == Some("links_timeout")),
        "warning_codes={codes:?} payload={v}"
    );

    service.cancel().await.expect("cancel");
}

#[tokio::test]
async fn web_cache_search_extract_timeout_is_deterministic_at_zero() {
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_MCP_TOOLSET", "debug");
                cmd.env("WEBPIPE_CACHE_SEARCH_TIMEOUT_MS", "0");
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-timeout-guards"),
                );
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let v = call(
        &service,
        "web_cache_search_extract",
        serde_json::json!({
            "query": "anything",
            "max_docs": 10,
            "max_scan_entries": 50,
            "max_chars": 10_000,
            "max_bytes": 200_000,
            "top_chunks": 3,
            "include_text": false,
            "include_structure": false
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(false));
    let codes = v["warning_codes"].as_array().cloned().unwrap_or_default();
    assert!(
        codes
            .iter()
            .any(|c| c.as_str() == Some("cache_search_timeout")),
        "warning_codes={codes:?} payload={v}"
    );

    service.cancel().await.expect("cancel");
}

#[tokio::test]
async fn web_fetch_cache_io_timeout_is_deterministic_at_zero() {
    let body = "<html><body><main><h1>Doc</h1><p>hello</p></main></body></html>";
    let body_s = body.to_string();
    let app = Router::new().route(
        "/doc",
        get(move || {
            let b = body_s.clone();
            async move { ([(header::CONTENT_TYPE, "text/html")], b) }
        }),
    );
    let addr = serve(app).await;
    let url = format!("http://{addr}/doc");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_CACHE_IO_TIMEOUT_MS", "0");
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-timeout-guards"),
                );
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let v = call(
        &service,
        "web_fetch",
        serde_json::json!({
            "url": url,
            "fetch_backend": "local",
            "timeout_ms": 10_000,
            "max_bytes": 200_000,
            "no_network": false,
            "cache_read": true,
            "cache_write": true,
            "include_text": false
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true));
    let codes = v["warning_codes"].as_array().cloned().unwrap_or_default();
    assert!(
        codes.iter().any(|c| c.as_str() == Some("cache_io_timeout")),
        "warning_codes={codes:?} payload={v}"
    );

    service.cancel().await.expect("cancel");
}

#[tokio::test]
async fn search_evidence_surfaces_cache_io_timeout_when_cache_io_disabled() {
    let body = "<html><body><main><h1>Doc</h1><p>hello</p></main></body></html>";
    let body_s = body.to_string();
    let app = Router::new().route(
        "/doc",
        get(move || {
            let b = body_s.clone();
            async move { ([(header::CONTENT_TYPE, "text/html")], b) }
        }),
    );
    let addr = serve(app).await;
    let url = format!("http://{addr}/doc");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_CACHE_IO_TIMEOUT_MS", "0");
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-timeout-guards"),
                );
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let v = call(
        &service,
        "search_evidence",
        serde_json::json!({
            "query": "hello",
            "urls": [url],
            "fetch_backend": "local",
            "timeout_ms": 10_000,
            "max_bytes": 200_000,
            "max_chars": 5_000,
            "top_chunks": 3,
            "max_chunk_chars": 200,
            "cache_read": true,
            "cache_write": true,
            "include_text": false,
            "include_structure": false,
            "include_links": false
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true));
    let res0 = v["results"]
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let codes = res0["warning_codes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        codes.iter().any(|c| c.as_str() == Some("cache_io_timeout")),
        "warning_codes={codes:?} payload={v}"
    );

    service.cancel().await.expect("cancel");
}
