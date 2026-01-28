#[test]
fn webpipe_mcp_stdio_search_extract_markdown_default_contract() {
    // End-to-end (spawns child process), strictly offline:
    // - ensures web_search_extract returns Markdown in content[0] by default
    // - ensures structured_content carries the canonical JSON payload
    // - ensures we don't include raw JSON text unless explicitly opted in

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use axum::{http::header, routing::get, Router};
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };
        use std::net::SocketAddr;

        // Local fixture server with distinctive content for extraction.
        let app = Router::new().route(
            "/doc",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html")],
                    "<html><body><h1>Doc</h1><p>markdown-contract unique token</p></body></html>",
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });
        let url = format!("http://{addr}/doc");

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    cmd.env("WEBPIPE_MCP_INCLUDE_JSON_TEXT", "0");
                    cmd.env("WEBPIPE_MCP_MARKDOWN_CHUNK_EXCERPTS", "0");
                }),
            )?)
            .await?;

        let r = service
            .call_tool(CallToolRequestParam {
                name: "web_search_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "query": "unique token",
                        "urls": [url],
                        "no_network": true,
                        "max_urls": 1,
                        "top_chunks": 3,
                        "max_chunk_chars": 120,
                        "include_text": false,
                        "include_links": false,
                        "cache_read": true,
                        "cache_write": false
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        // Canonical payload must be present in structured_content.
        let payload = r
            .structured_content
            .clone()
            .expect("structured_content exists");
        assert_eq!(payload["kind"].as_str(), Some("web_search_extract"));
        assert_eq!(payload["ok"].as_bool(), Some(true));
        assert!(payload["top_chunks"].is_array());

        // Markdown display should be first and should include the Request + Summary sections.
        let txt0 = r
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        assert!(
            txt0.starts_with("## Query") || txt0.starts_with("## Request"),
            "expected Markdown heading, got: {txt0:?}"
        );
        assert!(txt0.contains("## Request"), "expected Request section");
        assert!(txt0.contains("## Summary"), "expected Summary section");
        assert!(
            serde_json::from_str::<serde_json::Value>(&txt0).is_err(),
            "content[0] should not be JSON by default"
        );
        assert_eq!(
            r.content.len(),
            1,
            "default should not include raw JSON text; set WEBPIPE_MCP_INCLUDE_JSON_TEXT=true to opt in"
        );

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio search_extract markdown contract");
}
