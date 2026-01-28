#[test]
fn webpipe_mcp_stdio_deep_research_markdown_default_contract_openai_compat_localhost() {
    // End-to-end (spawns child process) with a localhost OpenAI-compatible stub:
    // - verifies web_deep_research returns Markdown in content[0] by default
    // - verifies structured_content carries the canonical JSON payload
    //
    // NOTE: do not set WEBPIPE_MCP_INCLUDE_JSON_TEXT here; default should be Markdown-only.

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use axum::{http::StatusCode, routing::get, routing::post, Json, Router};
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };
        use std::net::SocketAddr;

        // Local stub server:
        // - / docs page for fetch/extract evidence
        // - /v1/chat/completions for openai_compat synthesis
        let app = Router::new()
            .route(
                "/",
                get(|| async move {
                    (
                        StatusCode::OK,
                        [("content-type", "text/html")],
                        "<html><body><h1>Hello</h1><p>world</p></body></html>\n",
                    )
                }),
            )
            .route(
                "/v1/chat/completions",
                post(|_body: Json<serde_json::Value>| async move {
                    // Minimal OpenAI-compatible response shape.
                    Json(serde_json::json!({
                        "id": "stub",
                        "object": "chat.completion",
                        "created": 0,
                        "model": "stub-model",
                        "choices": [{
                            "index": 0,
                            "message": { "role": "assistant", "content": "Stub answer.\n\n- bullet\n- bullet" },
                            "finish_reason": "stop"
                        }],
                        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                    }))
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });

        let base = format!("http://{addr}");
        let fixture_url = format!("{base}/");

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    cmd.env("WEBPIPE_OPENAI_COMPAT_BASE_URL", &base);
                    cmd.env("WEBPIPE_OPENAI_COMPAT_MODEL", "stub-model");
                    cmd.env("WEBPIPE_MCP_INCLUDE_JSON_TEXT", "0");
                    // Ensure we don't accidentally use real keys.
                    cmd.env_remove("WEBPIPE_PERPLEXITY_API_KEY");
                    cmd.env_remove("PERPLEXITY_API_KEY");
                }),
            )?)
            .await?;

        let r = service
            .call_tool(CallToolRequestParam {
                name: "web_deep_research".into(),
                arguments: Some(
                    serde_json::json!({
                        "query": "What does the page say?",
                        "urls": [fixture_url],
                        "no_network": true,
                        "llm_backend": "auto",
                        "synthesize": true,
                        "include_evidence": false,
                        "timeout_ms": 2_000,
                        "max_bytes": 200_000,
                        "max_chars": 5_000,
                        "top_chunks": 3,
                        "max_chunk_chars": 200
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        // Canonical machine payload must be in structured_content.
        let payload = r
            .structured_content
            .clone()
            .expect("structured_content exists");
        assert_eq!(payload["kind"].as_str(), Some("web_deep_research"));
        assert_eq!(payload["ok"].as_bool(), Some(true));
        assert_eq!(payload["answer"]["text"].as_str(), Some("Stub answer.\n\n- bullet\n- bullet"));

        // Human-facing text should be Markdown-ish by default (not JSON).
        let txt0 = r
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        assert!(
            txt0.starts_with("## Query") || txt0.starts_with("## Answer"),
            "expected Markdown heading, got: {txt0:?}"
        );
        assert!(txt0.contains("## Request"), "expected Request section for richer context");
        assert!(
            serde_json::from_str::<serde_json::Value>(&txt0).is_err(),
            "content[0] should not be JSON by default"
        );
        assert!(
            r.content.len() == 1,
            "default should not include raw JSON text; set WEBPIPE_MCP_INCLUDE_JSON_TEXT=true to opt in"
        );

        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio deep research markdown contract");
}

#[test]
fn webpipe_mcp_stdio_deep_research_markdown_on_error_contract() {
    // End-to-end (spawns child process) but uses an error path to ensure we still return
    // human-friendly Markdown in content[0] while keeping structured_content authoritative.

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    cmd.env("WEBPIPE_MCP_INCLUDE_JSON_TEXT", "0");
                    // Ensure we don't accidentally use real keys.
                    cmd.env_remove("WEBPIPE_PERPLEXITY_API_KEY");
                    cmd.env_remove("PERPLEXITY_API_KEY");
                }),
            )?)
            .await?;

        let r = service
            .call_tool(CallToolRequestParam {
                name: "web_deep_research".into(),
                arguments: Some(serde_json::json!({ "query": "" }).as_object().cloned().unwrap()),
            })
            .await?;

        // MCP error flag should be set (ok=false).
        assert_eq!(r.is_error, Some(true));

        // Canonical machine payload must be in structured_content.
        let payload = r
            .structured_content
            .clone()
            .expect("structured_content exists");
        assert_eq!(payload["kind"].as_str(), Some("web_deep_research"));
        assert_eq!(payload["ok"].as_bool(), Some(false));
        assert_eq!(payload["error"]["code"].as_str(), Some("invalid_params"));

        // Human-facing Markdown should include an Error section.
        let txt0 = r
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        assert!(
            txt0.contains("## Error"),
            "expected Markdown error section, got: {txt0:?}"
        );
        assert!(
            r.content.len() == 1,
            "default should not include raw JSON text; set WEBPIPE_MCP_INCLUDE_JSON_TEXT=true to opt in"
        );

        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio deep research markdown error contract");
}
