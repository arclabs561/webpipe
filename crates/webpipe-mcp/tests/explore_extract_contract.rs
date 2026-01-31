use axum::{routing::get, Router};
use rmcp::{
    model::CallToolRequestParam,
    service::{RoleClient, RunningService, ServiceExt},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::collections::BTreeSet;
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

#[test]
fn web_explore_extract_crawls_and_finds_linked_page() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let start_html = r#"
        <html><body>
          <h1>Start</h1>
          <a href="/deep">needle docs</a>
          <a href="/noise">other</a>
        </body></html>
        "#;
        let deep_html = r#"
        <html><body>
          <h1>Deep</h1>
          <p>This page contains the needle content.</p>
        </body></html>
        "#;
        let noise_html = r#"<html><body><p>nothing here</p></body></html>"#;

        let start_s = start_html.to_string();
        let deep_s = deep_html.to_string();
        let noise_s = noise_html.to_string();

        let app = Router::new()
            .route(
                "/start",
                get(move || {
                    let body = start_s.clone();
                    async move { ([("content-type", "text/html")], body) }
                }),
            )
            .route(
                "/deep",
                get(move || {
                    let body = deep_s.clone();
                    async move { ([("content-type", "text/html")], body) }
                }),
            )
            .route(
                "/noise",
                get(move || {
                    let body = noise_s.clone();
                    async move { ([("content-type", "text/html")], body) }
                }),
            );
        let addr = serve(app).await;

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env(
                        "WEBPIPE_CACHE_DIR",
                        std::env::temp_dir().join("webpipe-explore-cache"),
                    );
                }),
            )?)
            .await?;

        let tools = service.list_tools(Default::default()).await?;
        let names: BTreeSet<String> = tools
            .tools
            .iter()
            .map(|t| t.name.clone().into_owned())
            .collect();
        assert!(
            names.contains("web_explore_extract"),
            "missing web_explore_extract tool"
        );

        let start_url = format!("http://{addr}/start");
        let v = call(
            &service,
            "web_explore_extract",
            serde_json::json!({
                "start_url": start_url,
                "query": "needle",
                "max_pages": 2,
                "max_depth": 1,
                "same_host_only": true,
                "max_links_per_page": 50,
                "timeout_ms": 10_000,
                "max_bytes": 500_000,
                "max_chars": 10_000,
                "top_chunks": 3,
                "max_chunk_chars": 300,
                "include_links": false,
                "no_network": false,
                "cache_read": false,
                "cache_write": false
            }),
        )
        .await;

        assert_eq!(v["ok"].as_bool(), Some(true));
        let pages = v["pages"].as_array().expect("pages");
        assert!(!pages.is_empty());
        let top = v["top_chunks"].as_array().expect("top_chunks");
        let any = top.iter().any(|c| {
            c.get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .contains("needle")
        });
        assert!(any, "expected top_chunks to contain 'needle'");

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("explore extract contract");
}
