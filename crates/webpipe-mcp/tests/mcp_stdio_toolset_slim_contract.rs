#[test]
fn webpipe_mcp_stdio_toolset_slim_contract() {
    // End-to-end (spawns child process).
    //
    // Contract:
    // - Setting WEBPIPE_MCP_TOOLSET=normal reduces the exposed tool surface for Cursor UX.
    // - The remaining tool set is small, opinionated, and stable (Cursor-first).
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use rmcp::{
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
                    // Ensure API-key-gated tools are not visible.
                    cmd.env_remove("WEBPIPE_PERPLEXITY_API_KEY");
                    cmd.env_remove("PERPLEXITY_API_KEY");
                }),
            )?)
            .await?;

        let tools = service.list_tools(Default::default()).await?;
        let names: std::collections::HashSet<String> =
            tools.tools.iter().map(|t| t.name.to_string()).collect();

        // Normal surface must include these zero-key-required tools.
        // webpipe_usage is accessible via webpipe_meta?method=usage (not a separate tool).
        // web_perplexity is only visible when WEBPIPE_PERPLEXITY_API_KEY is configured
        // (tested separately in mcp_stdio_web_perplexity_contract).
        for must in [
            "webpipe_meta",
            "web_extract",
            "search_evidence",
            "arxiv",
            "paper_search",
        ] {
            assert!(names.contains(must), "missing required tool: {must}");
        }

        // And should exclude these (keeps UI tight).
        // webpipe_usage / webpipe_usage_reset are accessible via webpipe_meta?method=...
        // web_perplexity is only visible when WEBPIPE_PERPLEXITY_API_KEY is configured
        // (this test runs without any keys set).
        // web_fetch, arxiv_search, arxiv_enrich are deprecated → web_extract / arxiv.
        for must_not in [
            "webpipe_usage",
            "webpipe_usage_reset",
            "web_perplexity",
            "web_fetch",
            "repo_ingest",
            "web_explore_extract",
            "web_sitemap_extract",
            "web_search",
            "web_seed_urls",
            "web_seed_search_extract",
            "web_deep_research",
            // Deprecated aliases / legacy names:
            "arxiv_search",
            "arxiv_enrich",
            "web_search_extract",
            "web_cache_search_extract",
            "http_fetch",
            "page_extract",
            "page_extract_text",
        ] {
            assert!(
                !names.contains(must_not),
                "normal toolset should not include: {must_not}"
            );
        }

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio slim toolset contract");
}
