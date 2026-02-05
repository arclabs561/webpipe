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
async fn web_extract_marks_js_challenge_and_http_status_error() {
    let body = r#"
<html><head><title>Just a moment...</title></head>
<body>
wolfgarbe.medium.com needs to review the security of your connection before proceeding.
Verification successful
Waiting for wolfgarbe.medium.com to respond...
Ray ID: 9c8d57706851e633
Performance &amp; security by Cloudflare
</body></html>
"#
    .to_string();
    let app = Router::new().route(
        "/cf",
        get(move || {
            let b = body.clone();
            async move {
                (
                    axum::http::StatusCode::FORBIDDEN,
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    b,
                )
            }
        }),
    );
    let addr = serve(app).await;
    let url = format!("http://{addr}/cf");

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
        "web_extract",
        serde_json::json!({
            "url": url,
            "fetch_backend": "local",
            "timeout_ms": 10_000,
            "max_bytes": 200_000,
            "max_chars": 5_000,
            "include_text": false,
            "include_structure": false,
            "include_links": false
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true), "payload={v}");
    assert_eq!(v["status"].as_u64(), Some(403), "payload={v}");
    let codes = v["warning_codes"].as_array().cloned().unwrap_or_default();
    assert!(
        codes.iter().any(|c| c.as_str() == Some("http_status_error")),
        "warning_codes={codes:?} payload={v}"
    );
    assert!(
        codes.iter().any(|c| c.as_str() == Some("blocked_by_js_challenge")),
        "warning_codes={codes:?} payload={v}"
    );

    service.cancel().await.expect("cancel");
}

