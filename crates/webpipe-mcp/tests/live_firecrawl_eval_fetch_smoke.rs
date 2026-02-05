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
fn webpipe_live_firecrawl_eval_fetch_smoke_opt_in() {
    // This test makes real paid network calls. Opt-in only.
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1")
        || std::env::var("WEBPIPE_LIVE_PAID").ok().as_deref() != Some("1")
    {
        eprintln!(
            "skipping: set WEBPIPE_LIVE=1 WEBPIPE_LIVE_PAID=1 to run live firecrawl eval-fetch smoke"
        );
        return;
    }

    let firecrawl_key = match get_env_any(&["WEBPIPE_FIRECRAWL_API_KEY", "FIRECRAWL_API_KEY"]) {
        Some(k) => k,
        None => {
            panic!("missing WEBPIPE_FIRECRAWL_API_KEY (or FIRECRAWL_API_KEY)");
        }
    };

    // Use a stable URL with predictable content. (Still a live network call.)
    let url = "https://example.com/";
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
                    cmd.env("WEBPIPE_FIRECRAWL_API_KEY", &firecrawl_key);
                }),
            )?)
            .await?;

        // Firecrawl fetch+extract (bounded).
        let resp = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": url,
                        "fetch_backend": "firecrawl",
                        "timeout_ms": 30_000,
                        "max_bytes": 500_000,
                        "max_chars": 200,
                        "query": "example domain",
                        "top_chunks": 3,
                        "max_chunk_chars": 200,
                        "include_text": true,
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
        assert_eq!(v["fetch_backend"].as_str(), Some("firecrawl"));
        let engine = v["extract"]["engine"].as_str().unwrap_or("");
        assert_eq!(engine, "firecrawl");
        // Since we forced max_chars=200, any returned text should be <= 200 chars.
        if let Some(text) = v["extract"]["text"].as_str() {
            assert!(text.chars().count() <= 200);
        }

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("live firecrawl web_extract smoke");
}
