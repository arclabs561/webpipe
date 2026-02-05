use axum::{routing::get, Router};
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
async fn search_evidence_parallel_hydration_preserves_input_order() {
    // Make one URL slow and the other fast; when running in parallel we still must preserve
    // the caller-provided URL order in `results`.
    let app = Router::new()
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(450)).await;
                ([("content-type", "text/html")], "<h1>Slow</h1> slowtoken")
            }),
        )
        .route(
            "/fast",
            get(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                ([("content-type", "text/html")], "<h1>Fast</h1> fasttoken")
            }),
        );
    let addr = serve(app).await;
    let slow = format!("http://127.0.0.1:{}/slow", addr.port());
    let fast = format!("http://127.0.0.1:{}/fast", addr.port());

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
            "query": "slowtoken fasttoken",
            "urls": [slow, fast],
            "url_selection_mode": "preserve",
            "fetch_backend": "local",
            "agentic": false,
            "max_urls": 2,
            "max_parallel_urls": 2,
            "top_chunks": 3,
            "max_chunk_chars": 200,
            "include_text": false,
            "include_links": false,
            "compact": true,
            "timeout_ms": 10_000,
            "deadline_ms": 30_000
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true));
    let rs = v["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(rs.len(), 2);
    assert_eq!(rs[0]["url"].as_str(), Some(slow.as_str()));
    assert_eq!(rs[1]["url"].as_str(), Some(fast.as_str()));

    service.cancel().await.expect("cancel");
}
