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

fn payload_from_result(r: &rmcp::model::CallToolResult) -> serde_json::Value {
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
    serde_json::json!({})
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
    payload_from_result(&r)
}

#[tokio::test]
async fn search_evidence_marks_http_status_error_and_excludes_chunks_from_top() {
    let ok_body =
        "<html><body><main><h1>Ok</h1><p>NEEDLE_123 good content</p></main></body></html>"
            .to_string();
    let bad_body =
        "<html><body><main><h1>Bad</h1><p>NEEDLE_123 error-like content</p></main></body></html>"
            .to_string();
    let app = Router::new()
        .route(
            "/ok",
            get({
                let ok_body = ok_body.clone();
                move || {
                    let b = ok_body.clone();
                    async move { ([(header::CONTENT_TYPE, "text/html")], b) }
                }
            }),
        )
        .route(
            "/bad",
            get({
                let bad_body = bad_body.clone();
                move || {
                    let b = bad_body.clone();
                    async move { (axum::http::StatusCode::NOT_FOUND, b) }
                }
            }),
        );
    let addr = serve(app).await;
    let ok_url = format!("http://{addr}/ok");
    let bad_url = format!("http://{addr}/bad");

    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache");
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                // Deterministic keyless behavior.
                cmd.env_remove("WEBPIPE_BRAVE_API_KEY");
                cmd.env_remove("BRAVE_SEARCH_API_KEY");
                cmd.env_remove("WEBPIPE_TAVILY_API_KEY");
                cmd.env_remove("TAVILY_API_KEY");
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let v = call(
        &service,
        "search_evidence",
        serde_json::json!({
            "query": "NEEDLE_123",
            "urls": [bad_url, ok_url],
            "url_selection_mode": "preserve",
            "fetch_backend": "local",
            "agentic": false,
            "max_urls": 2,
            "max_parallel_urls": 2,
            "top_chunks": 5,
            "max_chunk_chars": 200,
            "include_text": false,
            "include_links": false,
            "include_structure": false,
            "compact": true,
            "timeout_ms": 10_000,
            "deadline_ms": 30_000
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true), "payload={v}");

    let rs = v["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(rs.len(), 2, "results={rs:?} payload={v}");
    let bad = &rs[0];
    assert_eq!(bad["url"].as_str(), Some(bad_url.as_str()));
    let bad_codes = bad["warning_codes"].as_array().cloned().unwrap_or_default();
    assert!(
        bad_codes
            .iter()
            .any(|c| c.as_str() == Some("http_status_error")),
        "warning_codes={bad_codes:?} payload={v}"
    );

    let top = v["top_chunks"].as_array().cloned().unwrap_or_default();
    assert!(!top.is_empty(), "top_chunks empty payload={v}");
    assert!(
        top.iter()
            .all(|c| c.get("url").and_then(|u| u.as_str()) == Some(ok_url.as_str())),
        "top_chunks={top:?} payload={v}"
    );

    service.cancel().await.expect("cancel");
}
