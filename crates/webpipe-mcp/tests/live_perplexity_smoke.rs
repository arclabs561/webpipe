#[test]
fn webpipe_live_perplexity_smoke_opt_in() {
    // This test makes a real paid network call. Opt-in only.
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1")
        || std::env::var("WEBPIPE_LIVE_PAID").ok().as_deref() != Some("1")
    {
        eprintln!("skipping: set WEBPIPE_LIVE=1 WEBPIPE_LIVE_PAID=1 to run live perplexity smoke");
        return;
    }

    // Require Perplexity key to be present in *this* test process; we pass it to the child.
    let pplx_key = match std::env::var("WEBPIPE_PERPLEXITY_API_KEY")
        .ok()
        .or_else(|| std::env::var("PERPLEXITY_API_KEY").ok())
    {
        Some(k) if !k.trim().is_empty() => k,
        _ => panic!("missing WEBPIPE_PERPLEXITY_API_KEY (or PERPLEXITY_API_KEY)"),
    };

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env(
                        "WEBPIPE_CACHE_DIR",
                        std::env::temp_dir().join("webpipe-live-cache"),
                    );
                    cmd.env("WEBPIPE_PERPLEXITY_API_KEY", &pplx_key);
                }),
            )?)
            .await?;

        let resp = service
            .call_tool(CallToolRequestParam {
                name: "web_perplexity".into(),
                arguments: Some(
                    serde_json::json!({
                        "query": "What is 2+2? Answer with just the number.",
                        "timeout_ms": 30_000,
                        "max_answer_chars": 2000
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let v = resp
            .structured_content
            .clone()
            .ok_or("missing structured_content")?;

        assert_eq!(v["schema_version"].as_u64(), Some(2));
        assert_eq!(v["kind"].as_str(), Some("web_perplexity"));
        assert_eq!(v["ok"].as_bool(), Some(true));
        assert_eq!(v["provider"].as_str(), Some("perplexity"));
        assert!(v["answer"]["text"].as_str().unwrap_or("").contains('4'));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("live perplexity smoke");
}
