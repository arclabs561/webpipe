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
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    serde_json::from_str(&text).expect("parse json")
}

#[tokio::test]
async fn searxng_provider_e2e_via_stdio_server() {
    // Local SearXNG-like endpoint: /search?format=json -> { results: [{url,title,content}] }
    let app = Router::new().route(
        "/search",
        get(|| async {
            axum::Json(serde_json::json!({
                "results": [
                    {"url": "https://example.com/a", "title": "A", "content": "alpha"},
                    {"url": "https://example.com/b", "title": "B", "content": "beta"}
                ]
            }))
        }),
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

    service.cancel().await.expect("cancel");
}
