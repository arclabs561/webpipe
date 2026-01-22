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
    let text = r
        .content
        .get(0)
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    serde_json::from_str(&text).expect("parse json")
}

#[tokio::test]
async fn web_search_extract_compact_urls_mode_drops_agentic_trace_and_keeps_extract() {
    // Local server with distinctive content.
    let app = Router::new().route(
        "/doc",
        get(|| async {
            (
                [(header::CONTENT_TYPE, "text/html")],
                "<html><body><h1>Doc</h1><p>compacttoken unique token</p></body></html>",
            )
        }),
    );
    let addr = serve(app).await;
    let url = format!("http://{addr}/doc");

    // Spawn MCP server with a temp cache dir.
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache");
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    // Use urls-mode (no provider keys required). Keep agentic=true to ensure trace exists.
    let v = call(
        &service,
        "web_search_extract",
        serde_json::json!({
            "query": "compacttoken",
            "urls": [url],
            "url_selection_mode": "preserve",
            "fetch_backend": "local",
            "max_urls": 1,
            "max_chars": 1200,
            "top_chunks": 2,
            "max_chunk_chars": 200,
            "include_links": false,
            "include_structure": false,
            "include_text": false,
            "agentic": true,
            "compact": true
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true));
    // Compact agentic info should omit full trace.
    assert_eq!(v["agentic"]["enabled"].as_bool(), Some(true));
    assert!(v["agentic"].get("trace").is_none());
    assert!(v["agentic"].get("trace_len").is_some());

    let rs = v["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(rs.len(), 1);
    assert!(rs[0].get("extract").is_some());
    // Per-url compact should omit fetch_source/truncated, but keep attempts (null or object) for debuggability.
    assert!(rs[0].get("fetch_source").is_none());
    assert!(rs[0].get("attempts").is_some());
    assert!(rs[0].get("truncated").is_none());
}
