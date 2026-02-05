#[test]
fn webpipe_mcp_stdio_slim_workflow_contract() {
    // End-to-end (spawns child process), no external network.
    //
    // Contract:
    // - With WEBPIPE_MCP_TOOLSET=normal, the remaining tools compose into a usable workflow:
    //   web_fetch (warm cache) -> web_extract -> search_evidence(urls=..., no_network=true)
    //   -> search_evidence(query=..., no_network=true) (cache-corpus follow-up; urls omitted).
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use axum::{http::header, routing::get, Router};
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };
        use std::net::SocketAddr;

        fn content0_markdown(r: &rmcp::model::CallToolResult) -> String {
            r.content
                .first()
                .and_then(|c| c.as_text())
                .map(|t| t.text.clone())
                .unwrap_or_default()
        }

        // Local doc fixture that is "nav-heavy" but has a clear <main> section.
        let app = Router::new().route(
            "/doc",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html")],
                    r#"
<html><body>
  <nav>Skip to content Search documentation... Menu Getting Started Installation</nav>
  <main><h1>headers</h1><p>headers is an async function that allows you to read the HTTP incoming request headers.</p></main>
</body></html>
"#,
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });
        let doc_url = format!("http://{addr}/doc");

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_MCP_TOOLSET", "normal");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                }),
            )?)
            .await?;

        // 1) Warm cache via fetch.
        let fetch = service
            .call_tool(CallToolRequestParam {
                name: "web_fetch".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": doc_url,
                        "cache_read": true,
                        "cache_write": true,
                        "include_text": false,
                        "max_bytes": 200_000
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        assert!(!content0_markdown(&fetch).is_empty());
        assert!(fetch.structured_content.is_some());

        // 2) Extract should prefer main-like engine.
        let extract = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": doc_url,
                        "fetch_backend": "local",
                        "cache_read": true,
                        "cache_write": true,
                        "include_text": false,
                        "max_chars": 20_000,
                        "top_chunks": 5
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let ev: serde_json::Value = extract
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(ev["ok"].as_bool(), Some(true));
        let engine = ev["extract"]["engine"].as_str().unwrap_or("");
        assert!(
            engine == "readability" || engine == "html_main",
            "expected readability/html_main engine, got: {engine}"
        );

        // 3) Evidence gather: URLs mode in offline mode should work (cache-only).
        let se = service
            .call_tool(CallToolRequestParam {
                name: "search_evidence".into(),
                arguments: Some(
                    serde_json::json!({
                        "urls": [doc_url],
                        "query": "incoming request headers",
                        "no_network": true,
                        "max_urls": 1,
                        "top_chunks": 3,
                        "compact": true
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let sev: serde_json::Value = se
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(sev["ok"].as_bool(), Some(true));
        assert!(sev["top_chunks"].as_array().is_some());

        // 4) Cache-corpus follow-up should find the phrase (still offline; urls omitted).
        let cs = service
            .call_tool(CallToolRequestParam {
                name: "search_evidence".into(),
                arguments: Some(
                    serde_json::json!({
                        "query": "incoming request headers",
                        "no_network": true,
                        "compact": true
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let csv: serde_json::Value = cs
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(csv["ok"].as_bool(), Some(true));
        assert!(csv["results"].as_array().is_some());

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio slim workflow contract");
}
