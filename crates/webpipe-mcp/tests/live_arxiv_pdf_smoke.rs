fn get_env_any(keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            if !v.trim().is_empty() {
                return Some(v);
            }
        }
    }
    None
}

#[test]
fn webpipe_live_arxiv_pdf_smoke_opt_in() {
    // This test makes a real network call (arXiv PDF). Opt-in only.
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping: set WEBPIPE_LIVE=1 to run live arxiv pdf smoke");
        return;
    }

    // Ensure we *don't* accidentally rely on paid search keys here.
    if get_env_any(&[
        "WEBPIPE_TAVILY_API_KEY",
        "TAVILY_API_KEY",
        "WEBPIPE_BRAVE_API_KEY",
        "BRAVE_SEARCH_API_KEY",
    ])
    .is_some()
    {
        eprintln!("note: search keys are set, but this test only uses local fetch");
    }

    // A stable, well-known paper PDF.
    let url = "https://arxiv.org/pdf/1706.03762.pdf";
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };

        fn payload_from_result(result: &rmcp::model::CallToolResult) -> Option<serde_json::Value> {
            result.structured_content.clone().or_else(|| {
                result
                    .content
                    .first()
                    .and_then(|c| c.as_text())
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(&t.text).ok())
            })
        }

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env(
                        "WEBPIPE_CACHE_DIR",
                        std::env::temp_dir().join("webpipe-live-cache"),
                    );
                }),
            )?)
            .await?;

        // Bounded fetch+extract. Don't assert exact content (it can drift), only shape/signal.
        let resp = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": url,
                        "fetch_backend": "local",
                        "timeout_ms": 30_000,
                        "max_bytes": 10_000_000,
                        "max_chars": 20_000,
                        // Force chunks and keep them bounded.
                        "query": "attention",
                        "top_chunks": 3,
                        "max_chunk_chars": 500,
                        "include_text": false,
                        "include_links": false
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        let v = payload_from_result(&resp).ok_or("missing structured_content")?;
        assert_eq!(v["schema_version"].as_u64(), Some(2));
        assert_eq!(v["kind"].as_str(), Some("web_extract"));
        assert_eq!(v["ok"].as_bool(), Some(true));
        assert_eq!(v["url"].as_str(), Some(url));
        let engine = v["extract"]["engine"].as_str().unwrap_or("");
        assert!(
            engine.starts_with("pdf-"),
            "expected pdf engine, got {engine}"
        );
        assert!(
            v["extract"]["chunks"].is_array(),
            "expected extract.chunks array"
        );
        assert!(
            !v["extract"]["chunks"].as_array().unwrap().is_empty(),
            "expected at least one chunk"
        );

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("live arxiv pdf smoke");
}
