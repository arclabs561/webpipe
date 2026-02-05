#[test]
fn webpipe_mcp_stdio_anonymous_mode_contract() {
    // Contract:
    // - WEBPIPE_PRIVACY_MODE=anonymous should not break MCP discovery
    // - Without a configured proxy, outbound fetches should fail closed (not_configured)
    // - web_search should be disabled (not_supported) because API providers + keys deanonymize
    //
    // This test is offline-safe: it must not touch the network.

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
        use rmcp::{model::CallToolRequestParam, service::ServiceExt};

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;

        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    // This test asserts behavior of tools that are not exposed in the normal toolset.
                    cmd.env("WEBPIPE_MCP_TOOLSET", "debug");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    cmd.env("WEBPIPE_PRIVACY_MODE", "anonymous");
                    // Ensure no proxy is configured for this test.
                    cmd.env_remove("WEBPIPE_ANON_PROXY");
                    cmd.env_remove("WEBPIPE_PROXY");
                    cmd.env_remove("ALL_PROXY");
                    cmd.env_remove("HTTPS_PROXY");
                    cmd.env_remove("HTTP_PROXY");
                }),
            )?)
            .await?;

        // Discovery should still work.
        let tools = service.list_tools(Default::default()).await?;
        assert!(!tools.tools.is_empty(), "expected non-empty tool list");

        // Search-based discovery should fail closed in anonymous mode (API providers deanonymize).
        let search = service
            .call_tool(CallToolRequestParam {
                name: "search_evidence".into(),
                arguments: Some(
                    serde_json::json!({"provider":"auto","query":"x","max_results":1})
                        .as_object()
                        .cloned()
                        .unwrap(),
                ),
            })
            .await?;
        let v: serde_json::Value = search
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(v["ok"].as_bool(), Some(false));
        assert_eq!(v["error"]["code"].as_str(), Some("not_supported"));

        // Outbound fetch should fail with not_configured (proxy required), without attempting network.
        let fetch = service
            .call_tool(CallToolRequestParam {
                name: "web_fetch".into(),
                arguments: Some(
                    serde_json::json!({
                        "url":"https://example.com/",
                        "fetch_backend":"local",
                        "include_text": false,
                        "include_headers": false
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let fv: serde_json::Value = fetch
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(fv["ok"].as_bool(), Some(false));
        assert_eq!(fv["error"]["code"].as_str(), Some("not_configured"));

        // Outbound extract should also fail closed (it fetches under the hood).
        let ex = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url":"https://example.com/",
                        "fetch_backend":"local",
                        "include_text": false,
                        "include_links": false,
                        "include_structure": false,
                        "max_chars": 2000,
                        "top_chunks": 3,
                        "max_chunk_chars": 200
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let exv: serde_json::Value = ex
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(exv["ok"].as_bool(), Some(false));
        assert_eq!(exv["error"]["code"].as_str(), Some("not_configured"));

        // Search+extract with urls should also fail closed without a proxy (it fetches under the hood).
        let se = service
            .call_tool(CallToolRequestParam {
                name: "search_evidence".into(),
                arguments: Some(
                    serde_json::json!({
                        "query":"example",
                        "urls":["https://example.com/"],
                        "top_chunks": 3,
                        "max_chunk_chars": 200,
                        "include_text": false
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let sev: serde_json::Value = se
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(sev["ok"].as_bool(), Some(false));
        assert_eq!(sev["error"]["code"].as_str(), Some("not_configured"));

        // Explore (crawl) should fail closed without a proxy.
        let explore = service
            .call_tool(CallToolRequestParam {
                name: "web_explore_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "start_url":"https://example.com/",
                        "max_pages": 2,
                        "max_depth": 1,
                        "include_text": false
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let explore_v: serde_json::Value = explore
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(explore_v["ok"].as_bool(), Some(false));
        assert_eq!(explore_v["error"]["code"].as_str(), Some("not_configured"));

        // Sitemap discovery should fail closed without a proxy.
        let sm = service
            .call_tool(CallToolRequestParam {
                name: "web_sitemap_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "site_url":"https://example.com/",
                        "max_sitemaps": 1,
                        "max_urls": 10,
                        "extract": false
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let sm_v: serde_json::Value = sm
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(sm_v["ok"].as_bool(), Some(false));
        assert_eq!(sm_v["error"]["code"].as_str(), Some("not_configured"));

        // Repo ingest should fail closed without a proxy.
        let ing = service
            .call_tool(CallToolRequestParam {
                name: "repo_ingest".into(),
                arguments: Some(
                    serde_json::json!({
                        "repo_url":"https://github.com/rust-lang/rust",
                        "max_files": 1
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let ing_v: serde_json::Value = ing
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(ing_v["ok"].as_bool(), Some(false));
        assert_eq!(ing_v["error"]["code"].as_str(), Some("not_configured"));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio anonymous mode contract");
}
