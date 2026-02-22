use axum::{http::header, response::IntoResponse, routing::get, Router};
use rmcp::{
    model::CallToolRequestParam,
    service::ServiceExt,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::net::SocketAddr;

#[tokio::test]
async fn webpipe_mcp_stdio_web_extract_markdown_surfaces_warning_hints() {
    // End-to-end (spawns child process). Local-only fixture.
    //
    // Contract: web_extract Markdown should surface warning hints so Cursor users can act
    // without inspecting structured JSON.

    let app = Router::new().route(
        "/rate_limited",
        get(|| async {
            (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                [
                    (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                    (header::RETRY_AFTER, "5"),
                ],
                "rate limited",
            )
                .into_response()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let url = format!("http://{addr}/rate_limited");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let cache_dir = tempfile::TempDir::new().unwrap();
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_MCP_TOOLSET", "normal");
                cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                // Keep output stable: Markdown-first only, no raw JSON text.
                cmd.env("WEBPIPE_MCP_INCLUDE_JSON_TEXT", "0");
                cmd.env("WEBPIPE_MCP_MARKDOWN_CHUNK_EXCERPTS", "0");
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let r = service
        .call_tool(CallToolRequestParam {
            name: "web_extract".into(),
            arguments: Some(
                serde_json::json!({
                    "url": url,
                    "fetch_backend": "local",
                    "include_text": false,
                    "include_links": false,
                    "include_structure": false,
                    "timeout_ms": 5000,
                    "max_bytes": 200_000,
                    "max_chars": 20_000,
                    "top_chunks": 5,
                    "max_chunk_chars": 200
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await
        .expect("call_tool");

    let payload = r
        .structured_content
        .clone()
        .expect("structured_content exists");
    assert_eq!(payload["kind"].as_str(), Some("web_extract"));
    assert_eq!(payload["ok"].as_bool(), Some(true));
    assert_eq!(payload["status"].as_u64(), Some(429));

    let txt0 = r
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert!(
        txt0.starts_with("## Summary"),
        "expected Markdown heading, got: {txt0:?}"
    );
    assert!(
        txt0.contains("### Warning hints"),
        "expected warning hints section in markdown; markdown={txt0:?}"
    );
    assert!(
        txt0.contains("http_rate_limited"),
        "expected http_rate_limited in markdown; markdown={txt0:?}"
    );
    assert!(
        txt0.to_ascii_lowercase().contains("http 429"),
        "expected hint mentioning HTTP 429; markdown={txt0:?}"
    );

    service.cancel().await.expect("cancel");
}

