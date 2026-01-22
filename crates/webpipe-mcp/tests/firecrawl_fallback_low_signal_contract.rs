use axum::{routing::get, routing::post, Json, Router};
use rmcp::{
    model::CallToolRequestParam,
    service::{RoleClient, RunningService, ServiceExt},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde_json::json;
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
async fn web_search_extract_firecrawl_fallback_on_low_signal_is_bounded_and_marked() {
    // Local server with “non-empty but useless” JS bundle-ish payload.
    let js_gunk = "(self.__next_s=self.__next_s||[]).push([0,{\"suppressHydrationWarning\":true,\"children\":\"(function(a,b,c,d){var e;let f\"".to_string();
    let app = Router::new()
        .route(
            "/page",
            get(move || {
                let js_gunk = js_gunk.clone();
                async move {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                        format!("<html><body><div>{js_gunk}</div></body></html>"),
                    )
                }
            }),
        )
        .route(
            "/v2/scrape",
            post(|_body: Json<serde_json::Value>| async move {
                Json(json!({ "success": true, "data": { "markdown": "# Hi\n\nHello world" } }))
            }),
        );
    let addr = serve(app).await;

    // Spawn MCP server with Firecrawl endpoint pointed at our local fake.
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache");
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                cmd.env("WEBPIPE_FIRECRAWL_API_KEY", "test-key");
                cmd.env(
                    "WEBPIPE_FIRECRAWL_ENDPOINT_V2",
                    format!("http://{}/v2/scrape", addr),
                );
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let url = format!("http://{}/page", addr);
    let v = call(
        &service,
        "web_search_extract",
        json!({
            "query": "hello",
            "urls": [url],
            "url_selection_mode": "preserve",
            "fetch_backend": "local",
            "max_urls": 1,
            "max_chars": 2000,
            "top_chunks": 3,
            "max_chunk_chars": 200,
            "include_text": true,
            "include_links": false,
            "include_structure": false,
            "agentic": false,
            "compact": false,
            "firecrawl_fallback_on_empty_extraction": false,
            "firecrawl_fallback_on_low_signal": true
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true));
    let results = v["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(results.len(), 1);
    let one = &results[0];
    assert_eq!(one["ok"].as_bool(), Some(true));
    assert_eq!(one["fetch_backend"].as_str(), Some("firecrawl"));
    assert!(one["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x.as_str() == Some("firecrawl_fallback_on_low_signal")));
    assert!(one["extract"]["text"]
        .as_str()
        .unwrap_or("")
        .contains("Hello world"));
    // Attempts should show local low_signal=true.
    assert_eq!(one["attempts"]["local"]["low_signal"].as_bool(), Some(true));
}
