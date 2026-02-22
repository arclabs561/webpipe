#[test]
fn webpipe_mcp_stdio_search_evidence_markdown_surfaces_warning_hints() {
    // End-to-end (spawns child process), local-only fixtures.
    //
    // Contract: search_evidence Markdown should surface warning hints so Cursor users can act
    // without inspecting structured JSON.

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use axum::{http::header, response::IntoResponse, routing::get, Router};
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };
        use std::net::SocketAddr;

        let ok_body =
            "<html><body><main><h1>Ok</h1><p>NEEDLE_429 good content</p></main></body></html>"
                .to_string();
        let app = Router::new()
            .route(
                "/ok",
                get({
                    let ok_body = ok_body.clone();
                    move || {
                        let b = ok_body.clone();
                        async move { ([(header::CONTENT_TYPE, "text/html")], b) }
                    }
                }),
            )
            .route(
                "/rate_limited",
                get(|| async {
                    (
                        axum::http::StatusCode::TOO_MANY_REQUESTS,
                        [
                            (header::CONTENT_TYPE, "text/html"),
                            (header::RETRY_AFTER, "9"),
                        ],
                        "<html><body><h1>429</h1><p>rate limited</p></body></html>",
                    )
                        .into_response()
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });
        let ok_url = format!("http://{addr}/ok");
        let rl_url = format!("http://{addr}/rate_limited");

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_MCP_TOOLSET", "normal");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    // Keep output stable: Markdown-first only, no raw JSON text.
                    cmd.env("WEBPIPE_MCP_INCLUDE_JSON_TEXT", "0");
                    cmd.env("WEBPIPE_MCP_MARKDOWN_CHUNK_EXCERPTS", "0");
                }),
            )?)
            .await?;

        let r = service
            .call_tool(CallToolRequestParam {
                name: "search_evidence".into(),
                arguments: Some(
                    serde_json::json!({
                        "query": "NEEDLE_429",
                        "urls": [rl_url, ok_url],
                        "url_selection_mode": "preserve",
                        "fetch_backend": "local",
                        "agentic": false,
                        "max_urls": 2,
                        "max_parallel_urls": 2,
                        "top_chunks": 3,
                        "max_chunk_chars": 200,
                        "include_text": false,
                        "include_links": false,
                        "include_structure": false,
                        "compact": true,
                        "timeout_ms": 10_000,
                        "deadline_ms": 30_000
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

        // Markdown should surface hints for the rate-limited URL.
        let txt0 = r
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        assert!(
            txt0.contains("### Warning hints"),
            "expected warning hints in markdown; got: {txt0:?}"
        );
        assert!(
            txt0.contains("http_rate_limited"),
            "expected http_rate_limited in markdown; got: {txt0:?}"
        );

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio search_evidence warning hints contract");
}

