/// Contract: web_perplexity tool visibility and error shape.
///
/// Design invariant: the Normal toolset must work with zero API keys.
/// - When WEBPIPE_PERPLEXITY_API_KEY is absent: tool is NOT in tools/list (Normal mode).
/// - When the key is present: tool IS visible and works.
/// - Calling via Debug mode without a key: fails with not_configured (stable shape).
///
/// This ensures agents in Normal mode are never surprised by a tool that hard-fails.
#[test]
fn webpipe_mcp_stdio_web_perplexity_contract_not_configured_is_stable() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;

        // ── Part 1: Normal mode without key — tool must be ABSENT ────────────────
        let svc_normal = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(&bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    cmd.env("WEBPIPE_MCP_TOOLSET", "normal");
                    cmd.env_remove("WEBPIPE_PERPLEXITY_API_KEY");
                    cmd.env_remove("PERPLEXITY_API_KEY");
                }),
            )?)
            .await?;

        let tools_normal = svc_normal.list_tools(Default::default()).await?;
        let names_normal: std::collections::HashSet<String> = tools_normal
            .tools
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert!(
            !names_normal.contains("web_perplexity"),
            "web_perplexity must NOT appear in Normal mode without API key; got: {names_normal:?}"
        );
        // The 7 zero-key tools must all be present.
        for must in [
            "webpipe_meta",
            "web_fetch",
            "web_extract",
            "search_evidence",
            "arxiv_search",
            "arxiv_enrich",
            "paper_search",
        ] {
            assert!(
                names_normal.contains(must),
                "missing required keyless tool: {must}"
            );
        }
        svc_normal.cancel().await?;

        // ── Part 2: Debug mode without key — tool exists but fails with not_configured ──
        let svc_debug = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(&bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    cmd.env("WEBPIPE_MCP_TOOLSET", "debug");
                    cmd.env_remove("WEBPIPE_PERPLEXITY_API_KEY");
                    cmd.env_remove("PERPLEXITY_API_KEY");
                }),
            )?)
            .await?;

        let tools_debug = svc_debug.list_tools(Default::default()).await?;
        let names_debug: std::collections::HashSet<String> = tools_debug
            .tools
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert!(
            names_debug.contains("web_perplexity"),
            "web_perplexity must be present in Debug mode"
        );

        // Call it — should fail with not_configured and return proper Markdown (not raw JSON).
        let r = svc_debug
            .call_tool(CallToolRequestParam {
                name: "web_perplexity".into(),
                arguments: Some(
                    serde_json::json!({"query": "what is 2+2?", "timeout_ms": 5000})
                        .as_object()
                        .cloned()
                        .unwrap(),
                ),
            })
            .await?;

        // Structured content contract.
        let v = r
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(v["schema_version"].as_u64(), Some(2), "schema_version");
        assert_eq!(v["kind"].as_str(), Some("web_perplexity"), "kind");
        assert_eq!(v["ok"].as_bool(), Some(false), "ok");
        assert_eq!(
            v["error"]["code"].as_str(),
            Some("not_configured"),
            "error code"
        );

        // Markdown contract: content[0] must be Markdown (not raw JSON).
        let txt0 = r
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        assert!(
            txt0.starts_with("## "),
            "content[0] should be Markdown (starts with '## '), got: {txt0:?}"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(&txt0).is_err(),
            "content[0] should not be raw JSON"
        );

        svc_debug.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio web_perplexity contract");
}
