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
                }),
            )?)
            .await?;

        let tools = service.list_tools(Default::default()).await?;
        let names: std::collections::HashSet<String> =
            tools.tools.iter().map(|t| t.name.to_string()).collect();

        // Normal surface should include these.
        for must in [
            "webpipe_meta",
            "webpipe_usage",
            "web_fetch",
            "web_extract",
            "search_evidence",
            "web_perplexity",
            "arxiv_search",
            "arxiv_enrich",
            "paper_search",
        ] {
            assert!(names.contains(must), "missing required tool: {must}");
        }

        // And should exclude these (keeps UI tight).
        for must_not in [
            "repo_ingest",
            "web_explore_extract",
            "web_sitemap_extract",
            "web_search",
            "web_seed_urls",
            "web_seed_search_extract",
            "web_deep_research",
            // Redundant aliases / legacy names:
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
