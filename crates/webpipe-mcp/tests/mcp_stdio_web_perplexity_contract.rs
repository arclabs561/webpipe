#[test]
fn webpipe_mcp_stdio_web_perplexity_contract_not_configured_is_stable() {
    // End-to-end (spawns child process), keyless:
    // - tool exists in normal toolset
    // - calling without API key fails with not_configured (stable shape)
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
                    cmd.env("WEBPIPE_MCP_TOOLSET", "normal");
                    cmd.env_remove("WEBPIPE_PERPLEXITY_API_KEY");
                    cmd.env_remove("PERPLEXITY_API_KEY");
                }),
            )?)
            .await?;

        let tools = service.list_tools(Default::default()).await?;
        let names: std::collections::HashSet<String> =
            tools.tools.iter().map(|t| t.name.to_string()).collect();
        assert!(
            names.contains("web_perplexity"),
            "missing web_perplexity tool"
        );

        let r = service
            .call_tool(CallToolRequestParam {
                name: "web_perplexity".into(),
                arguments: Some(
                    serde_json::json!({
                        "query": "what is 2+2?",
                        "timeout_ms": 5000
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let v = r
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(v["schema_version"].as_u64(), Some(2));
        assert_eq!(v["kind"].as_str(), Some("web_perplexity"));
        assert_eq!(v["ok"].as_bool(), Some(false));
        assert_eq!(v["error"]["code"].as_str(), Some("not_configured"));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio web_perplexity contract");
}
