use axum::{http::header, routing::get, Router};
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
    // Prefer MCP structured content (stable machine payload). Tool `content` may be empty/Markdown.
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
async fn webpipe_web_fetch_sets_truncated_and_warning_codes_when_body_is_cut() {
    let big = "x".repeat(20_000);
    let app = Router::new().route(
        "/",
        get(move || {
            let body = big.clone();
            async move { ([(header::CONTENT_TYPE, "text/html")], body) }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let url = format!("http://{addr}/");

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
            "timeout_ms": 2000,
            "max_bytes": 200,
            "include_text": false,
            "cache_read": false,
            "cache_write": false
        }),
    )
    .await;
    assert_eq!(v["schema_version"].as_u64(), Some(2));
    assert_eq!(v["kind"].as_str(), Some("web_fetch"));
    assert_eq!(v["ok"].as_bool(), Some(true));
    assert_eq!(v["truncated"].as_bool(), Some(true));
    assert!(v["warning_codes"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .any(|x| x.as_str() == Some("body_truncated_by_max_bytes")));

    service.cancel().await.expect("cancel");
}
