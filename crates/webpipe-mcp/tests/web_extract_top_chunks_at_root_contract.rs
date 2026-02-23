/// Contract: web_extract exposes top_chunks at the top level of its response.
///
/// Both `search_evidence` and `web_extract` return `top_chunks[]` at the root of
/// structuredContent. This lets agents write consistent `response.top_chunks` access
/// patterns without knowing which tool was called.
///
/// The full chunks also remain at `extract.top_chunks` for backward compat.
#[test]
fn web_extract_top_chunks_at_root_contract() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use axum::{http::header, routing::get, Router};
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::ConfigureCommandExt,
        };
        use std::net::SocketAddr;

        let app = Router::new().route(
            "/doc",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html")],
                    "<html><body><main><h1>Title</h1>\
                     <p>top_chunks_consistency unique_token_xyz</p>\
                     <p>Second paragraph with more content here.</p>\
                     </main></body></html>",
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let doc_url = format!("http://{addr}/doc");

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;
        let service = ()
            .serve(rmcp::transport::TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                }),
            )?)
            .await?;

        let r = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": doc_url,
                        "query": "unique_token_xyz",
                        "top_chunks": 3,
                        "max_chunk_chars": 200,
                        "fetch_backend": "local",
                        "timeout_ms": 5000,
                        "include_text": false,
                        "include_links": false,
                        "include_structure": false,
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        let sc = r.structured_content.clone().unwrap();
        assert_eq!(sc["ok"].as_bool(), Some(true), "ok; sc={sc}");
        assert_eq!(sc["kind"].as_str(), Some("web_extract"), "kind");

        // ── Top-level top_chunks contract ────────────────────────────────────────
        let root_chunks = sc
            .get("top_chunks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(
            !root_chunks.is_empty(),
            "top_chunks must be present at root level of web_extract response; sc={sc}"
        );

        // ── extract.chunks backward compat ───────────────────────────────────────
        // extract.chunks is the original location; top_chunks at root mirrors it.
        let extract_chunks = sc["extract"]
            .get("chunks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(
            !extract_chunks.is_empty(),
            "extract.chunks must be present for backward compat; sc={sc}"
        );

        // ── Both must be identical ────────────────────────────────────────────────
        assert_eq!(
            root_chunks, extract_chunks,
            "top_chunks at root and extract.chunks must be identical"
        );

        // ── Chunk shape contract ──────────────────────────────────────────────────
        for chunk in &root_chunks {
            assert!(
                chunk.get("url").is_some() || chunk.get("score").is_some(),
                "chunk missing url/score: {chunk}"
            );
            assert!(chunk.get("text").is_some(), "chunk missing text: {chunk}");
        }

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("web_extract top_chunks at root contract");
}
