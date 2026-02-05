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
async fn search_evidence_domains_allow_and_deny_work_and_fail_closed_when_all_filtered() {
    let app = Router::new()
        .route(
            "/a",
            get(|| async { ([("content-type", "text/html")], "<h1>A</h1> allowtoken") }),
        )
        .route(
            "/b",
            get(|| async { ([("content-type", "text/html")], "<h1>B</h1> denytoken") }),
        );
    let addr = serve(app).await;

    // Same server, different hostnames so domain filters can distinguish:
    // - localhost
    // - 127.0.0.1
    let url_localhost = format!("http://localhost:{}/a", addr.port());
    let url_loopback = format!("http://127.0.0.1:{}/b", addr.port());

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

    // Allow only localhost → expect only that URL is processed.
    let v = call(
        &service,
        "search_evidence",
        serde_json::json!({
            "query": "allowtoken",
            "urls": [url_localhost, url_loopback],
            "url_selection_mode": "preserve",
            "fetch_backend": "local",
            "agentic": false,
            "max_urls": 2,
            "top_chunks": 3,
            "max_chunk_chars": 200,
            "include_text": false,
            "include_links": false,
            "compact": true,
            "domains_allow": ["localhost"],
        }),
    )
    .await;
    assert_eq!(v["ok"].as_bool(), Some(true));
    let rs = v["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(rs.len(), 1, "expected only one URL after domains_allow");
    assert_eq!(
        rs[0].get("url").and_then(|u| u.as_str()),
        Some(url_localhost.as_str())
    );

    // Deny localhost + 127.0.0.1 → fail closed with invalid params.
    let v2 = call(
        &service,
        "search_evidence",
        serde_json::json!({
            "query": "whatever",
            "urls": [url_localhost, url_loopback],
            "url_selection_mode": "preserve",
            "fetch_backend": "local",
            "agentic": false,
            "max_urls": 2,
            "top_chunks": 3,
            "max_chunk_chars": 200,
            "include_text": false,
            "include_links": false,
            "compact": true,
            "domains_deny": ["localhost", "127.0.0.1"],
        }),
    )
    .await;
    assert_eq!(v2["ok"].as_bool(), Some(false));
    assert_eq!(v2["error"]["code"].as_str(), Some("invalid_params"));

    service.cancel().await.expect("cancel");
}
