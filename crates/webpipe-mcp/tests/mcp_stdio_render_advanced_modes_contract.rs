#[test]
fn webpipe_mcp_stdio_render_advanced_modes_fail_closed_contract() {
    // End-to-end (spawns child process).
    //
    // Contract:
    // - Advanced render modes are opt-in via env vars.
    // - Misconfiguration should fail *before* attempting to spawn Node/Playwright so the error path
    //   remains deterministic in minimal environments.
    // - In anonymous mode, “use my browser” / persistent profile is not supported for non-localhost
    //   URLs (identity leak risk).
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
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());

                    // Trigger deterministic “not supported” before any Node spawn:
                    // - anonymous mode => web_extract(render) would normally pass a proxy (if set)
                    // - cdp endpoint set => “use my browser” mode
                    // - non-localhost URL => should be rejected as not_supported (identity leak risk)
                    cmd.env("WEBPIPE_PRIVACY_MODE", "anonymous");
                    cmd.env("WEBPIPE_RENDER_CDP_ENDPOINT", "http://127.0.0.1:9222");

                    // Also set a proxy so, if the implementation order changes, we still reject
                    // quickly (proxy+cdp is not supported).
                    cmd.env("WEBPIPE_ANON_PROXY", "http://127.0.0.1:8888");

                    // Ensure we are not trivially disabled.
                    cmd.env_remove("WEBPIPE_RENDER_DISABLE");
                }),
            )?)
            .await?;

        let ex = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url":"https://example.com/",
                        "fetch_backend":"render",
                        "no_network": false,
                        "include_text": false,
                        "include_links": false,
                        "include_structure": false,
                        "max_chars": 2000,
                        "top_chunks": 3,
                        "max_chunk_chars": 200,
                        "timeout_ms": 2000
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        // Tool should return ok=false and mark MCP error.
        assert_eq!(ex.is_error, Some(true));
        let v: serde_json::Value = ex
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(v["kind"].as_str(), Some("web_extract"));
        assert_eq!(v["ok"].as_bool(), Some(false));
        assert_eq!(v["error"]["code"].as_str(), Some("not_supported"));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio render advanced modes fail-closed contract");
}

#[test]
fn webpipe_mcp_stdio_webpipe_meta_advertises_render_advanced_knobs() {
    // End-to-end (spawns child process), offline-safe.
    //
    // Contract:
    // - webpipe_meta advertises the render advanced knobs (names only).
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
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    // Keep this test hermetic.
                    cmd.env_remove("WEBPIPE_BRAVE_API_KEY");
                    cmd.env_remove("BRAVE_SEARCH_API_KEY");
                    cmd.env_remove("WEBPIPE_TAVILY_API_KEY");
                    cmd.env_remove("TAVILY_API_KEY");
                }),
            )?)
            .await?;

        let meta = service
            .call_tool(CallToolRequestParam {
                name: "webpipe_meta".into(),
                arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
            })
            .await?;
        assert_eq!(meta.is_error, Some(false));
        let v: serde_json::Value = meta
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;

        let knobs = v
            .pointer("/supported/knobs")
            .and_then(|x| x.as_array())
            .ok_or("expected supported.knobs array")?;
        let knobs: std::collections::BTreeSet<String> = knobs
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();

        for k in [
            "WEBPIPE_RENDER_CDP_ENDPOINT",
            "WEBPIPE_RENDER_USER_DATA_DIR",
        ] {
            assert!(knobs.contains(k), "missing knob {k}");
        }

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio meta render knobs contract");
}

#[test]
fn webpipe_mcp_stdio_webpipe_meta_render_support_contract() {
    // End-to-end (spawns child process), offline-safe.
    //
    // Contract:
    // - webpipe_meta.supported.fetch_backends includes "render"
    // - webpipe_meta.supported.anon_proxy_schemes_render is stable and includes http/https only
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
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    // Keep this test hermetic.
                    cmd.env_remove("WEBPIPE_BRAVE_API_KEY");
                    cmd.env_remove("BRAVE_SEARCH_API_KEY");
                    cmd.env_remove("WEBPIPE_TAVILY_API_KEY");
                    cmd.env_remove("TAVILY_API_KEY");
                }),
            )?)
            .await?;

        let meta = service
            .call_tool(CallToolRequestParam {
                name: "webpipe_meta".into(),
                arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
            })
            .await?;
        assert_eq!(meta.is_error, Some(false));
        let v: serde_json::Value = meta
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;

        let fetch_backends = v
            .pointer("/supported/fetch_backends")
            .and_then(|x| x.as_array())
            .ok_or("expected supported.fetch_backends array")?;
        let fetch_backends: std::collections::BTreeSet<String> = fetch_backends
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();
        assert!(
            fetch_backends.contains("render"),
            "supported.fetch_backends must include render"
        );

        let schemes = v
            .pointer("/supported/anon_proxy_schemes_render")
            .and_then(|x| x.as_array())
            .ok_or("expected supported.anon_proxy_schemes_render array")?;
        let schemes: Vec<String> = schemes
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();
        assert_eq!(schemes, vec!["http".to_string(), "https".to_string()]);

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio meta render support contract");
}

#[test]
fn webpipe_mcp_stdio_render_mutually_exclusive_env_is_not_supported() {
    // End-to-end (spawns child process).
    //
    // Contract:
    // - Setting both WEBPIPE_RENDER_CDP_ENDPOINT and WEBPIPE_RENDER_USER_DATA_DIR is invalid and
    //   should fail deterministically as not_supported.
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
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    cmd.env("WEBPIPE_PRIVACY_MODE", "normal");
                    cmd.env("WEBPIPE_RENDER_CDP_ENDPOINT", "http://127.0.0.1:9222");
                    cmd.env("WEBPIPE_RENDER_USER_DATA_DIR", "/tmp/webpipe-test-profile");
                    cmd.env_remove("WEBPIPE_RENDER_DISABLE");
                }),
            )?)
            .await?;

        let ex = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url":"https://example.com/",
                        "fetch_backend":"render",
                        "no_network": false,
                        "include_text": false,
                        "include_links": false,
                        "include_structure": false,
                        "max_chars": 2000,
                        "top_chunks": 3,
                        "max_chunk_chars": 200,
                        "timeout_ms": 2000
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        assert_eq!(ex.is_error, Some(true));
        let v: serde_json::Value = ex
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(v["kind"].as_str(), Some("web_extract"));
        assert_eq!(v["ok"].as_bool(), Some(false));
        assert_eq!(v["error"]["code"].as_str(), Some("not_supported"));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio render mutually exclusive env contract");
}

#[test]
fn webpipe_mcp_stdio_web_extract_render_disabled_is_not_configured() {
    // End-to-end (spawns child process), offline-safe.
    //
    // Contract:
    // - WEBPIPE_RENDER_DISABLE=1 makes fetch_backend="render" fail as not_configured without
    //   attempting to spawn Node/Playwright.
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
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    cmd.env("WEBPIPE_PRIVACY_MODE", "normal");
                    cmd.env("WEBPIPE_RENDER_DISABLE", "1");
                }),
            )?)
            .await?;

        let ex = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url":"https://example.com/",
                        "fetch_backend":"render",
                        "no_network": false,
                        "include_text": false,
                        "include_links": false,
                        "include_structure": false,
                        "max_chars": 2000,
                        "top_chunks": 3,
                        "max_chunk_chars": 200,
                        "timeout_ms": 2000
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        assert_eq!(ex.is_error, Some(true));
        let v: serde_json::Value = ex
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(v["kind"].as_str(), Some("web_extract"));
        assert_eq!(v["ok"].as_bool(), Some(false));
        assert_eq!(v["error"]["code"].as_str(), Some("not_configured"));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio web_extract render disabled contract");
}

#[test]
fn webpipe_mcp_stdio_web_search_extract_render_disabled_is_not_configured() {
    // End-to-end (spawns child process), offline-safe.
    //
    // Contract:
    // - WEBPIPE_RENDER_DISABLE=1 makes fetch_backend="render" fail as not_configured without
    //   attempting to spawn Node/Playwright.
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
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    cmd.env("WEBPIPE_PRIVACY_MODE", "normal");
                    cmd.env("WEBPIPE_RENDER_DISABLE", "1");
                }),
            )?)
            .await?;

        // In normal toolset, `web_search_extract` is hidden; use the primary alias.
        let se = service
            .call_tool(CallToolRequestParam {
                name: "search_evidence".into(),
                arguments: Some(
                    serde_json::json!({
                        "query":"x",
                        "urls":["https://example.com/"],
                        "url_selection_mode":"preserve",
                        "provider":"auto",
                        "selection_mode":"pareto",
                        "fetch_backend":"render",
                        "no_network": false,
                        "include_text": false,
                        "include_links": false,
                        "include_structure": false,
                        "max_chars": 2000,
                        "top_chunks": 3,
                        "max_chunk_chars": 200,
                        "timeout_ms": 2000
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        assert_eq!(se.is_error, Some(true));
        let v: serde_json::Value = se
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(v["kind"].as_str(), Some("web_search_extract"));
        assert_eq!(v["ok"].as_bool(), Some(false));
        assert_eq!(v["error"]["code"].as_str(), Some("not_configured"));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio search_evidence render disabled contract");
}
