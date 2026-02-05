use axum::{routing::get, Router};
use rmcp::{
    model::CallToolRequestParam,
    service::{RoleClient, RunningService, ServiceExt},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::net::SocketAddr;
use std::time::Duration;

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
async fn searxng_provider_e2e_via_stdio_server() {
    // Local SearXNG-like endpoint: /search?format=json -> { results: [{url,title,content}] }
    let app = Router::new().route(
        "/search",
        get(
            |q: axum::extract::Query<std::collections::HashMap<String, String>>| async move {
                if q.get("q")
                    .map(|s| s.to_ascii_lowercase().contains("slow"))
                    .unwrap_or(false)
                {
                    tokio::time::sleep(Duration::from_millis(1_500)).await;
                }
                axum::Json(serde_json::json!({
                    "results": [
                        {"url": "https://example.com/a", "title": "A", "content": "alpha"},
                        {"url": "https://example.com/b", "title": "B", "content": "beta"}
                    ]
                }))
            },
        ),
    );
    let addr = serve(app).await;
    let endpoint = format!("http://{addr}");

    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache");
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                // Disable `.env` autoload so this test stays hermetic.
                cmd.env("WEBPIPE_DOTENV", "0");
                // `web_search` is a debug-only tool; enable full surface for this provider test.
                cmd.env("WEBPIPE_MCP_TOOLSET", "debug");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                cmd.env("WEBPIPE_SEARXNG_ENDPOINT", &endpoint);
                // Ensure we don't accidentally use real keys on dev machines.
                cmd.env_remove("WEBPIPE_BRAVE_API_KEY");
                cmd.env_remove("BRAVE_SEARCH_API_KEY");
                cmd.env_remove("WEBPIPE_TAVILY_API_KEY");
                cmd.env_remove("TAVILY_API_KEY");
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    // Direct provider call.
    let v = call(
        &service,
        "web_search",
        serde_json::json!({
            "provider": "searxng",
            "query": "hello",
            "max_results": 2
        }),
    )
    .await;
    assert_eq!(v["ok"].as_bool(), Some(true));
    assert_eq!(v["backend_provider"].as_str(), Some("searxng"));
    assert!(v["results"]
        .as_array()
        .map(|a| !a.is_empty())
        .unwrap_or(false));

    // Auto fallback should pick searxng when it is the only configured provider.
    let v2 = call(
        &service,
        "web_search",
        serde_json::json!({
            "provider": "auto",
            "auto_mode": "fallback",
            "query": "hello",
            "max_results": 2
        }),
    )
    .await;
    assert_eq!(v2["ok"].as_bool(), Some(true));
    assert_eq!(v2["backend_provider"].as_str(), Some("searxng"));

    // MAB should also pick searxng (single-arm).
    let v3 = call(
        &service,
        "web_search",
        serde_json::json!({
            "provider": "auto",
            "auto_mode": "mab",
            "query": "hello",
            "max_results": 2
        }),
    )
    .await;
    assert_eq!(v3["ok"].as_bool(), Some(true));
    assert_eq!(v3["backend_provider"].as_str(), Some("searxng"));

    // Timeout propagation: a slow endpoint should error quickly when timeout_ms is low.
    let v4 = call(
        &service,
        "web_search",
        serde_json::json!({
            "provider": "searxng",
            "query": "slow",
            "max_results": 2,
            "timeout_ms": 1_000
        }),
    )
    .await;
    assert_eq!(v4["ok"].as_bool(), Some(false));
    assert!(v4.get("error").is_some());

    service.cancel().await.expect("cancel");
}
