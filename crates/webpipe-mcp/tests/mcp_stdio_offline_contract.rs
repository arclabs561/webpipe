use std::collections::BTreeSet;

#[test]
fn webpipe_mcp_stdio_offline_contract() {
    // End-to-end (spawns child process) but strictly offline:
    // - uses local fixture server for fetch/extract
    // - does not require any API keys

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use axum::{http::header, http::HeaderMap, http::StatusCode, routing::get, Router};
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };
        use std::net::SocketAddr;

        // Local fixture server: stable, offline, and validates fetch+extract+links.
        // It also fails closed if request-secret headers are forwarded.
        let app = Router::new().route(
            "/",
            get(|headers: HeaderMap| async move {
                let mut present = Vec::new();
                if headers.contains_key(header::AUTHORIZATION) {
                    present.push("authorization");
                }
                if headers.contains_key(header::COOKIE) {
                    present.push("cookie");
                }
                if headers.contains_key(header::PROXY_AUTHORIZATION) {
                    present.push("proxy-authorization");
                }
                if !present.is_empty() {
                    return (
                        StatusCode::BAD_REQUEST,
                        [(header::CONTENT_TYPE, "text/plain")],
                        format!("sensitive header forwarded: {}", present.join(",")),
                    );
                }

                let accept_lang = headers
                    .get(header::ACCEPT_LANGUAGE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/html")],
                    format!(
                        "<html><body><h1>Hello</h1><p>world</p><p>accept-language={}</p><a href=\"/a#x\">A</a></body></html>",
                        accept_lang
                    ),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });
        let fixture_url = format!("http://{}/", addr);

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env(
                        "WEBPIPE_CACHE_DIR",
                        std::env::temp_dir().join("webpipe-test-cache"),
                    );
                    cmd.env_remove("WEBPIPE_ALLOW_UNSAFE_HEADERS");
                    // Ensure no provider keys are present for deterministic error-path checks.
                    cmd.env_remove("WEBPIPE_BRAVE_API_KEY");
                    cmd.env_remove("BRAVE_SEARCH_API_KEY");
                    cmd.env_remove("WEBPIPE_TAVILY_API_KEY");
                    cmd.env_remove("TAVILY_API_KEY");
                }),
            )?)
            .await?;

        let tools = service.list_tools(Default::default()).await?;
        let names: BTreeSet<String> = tools
            .tools
            .iter()
            .map(|t| t.name.clone().into_owned())
            .collect();
        for must_have in [
            "webpipe_meta",
            "web_fetch",
            "web_search",
            "web_extract",
            "web_search_extract",
        ] {
            assert!(names.contains(must_have), "missing tool {must_have}");
        }

        // Meta: always ok, and configured flags are boolean.
        let meta = service
            .call_tool(CallToolRequestParam {
                name: "webpipe_meta".into(),
                arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
            })
            .await?;
        let meta_s = meta
            .content
            .get(0)
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let meta_v: serde_json::Value = serde_json::from_str(&meta_s)?;
        assert_eq!(meta_v["schema_version"].as_u64(), Some(1));
        assert_eq!(meta_v["kind"].as_str(), Some("webpipe_meta"));
        assert_eq!(meta_v["ok"].as_bool(), Some(true));
        assert!(meta_v["configured"]["providers"]["brave"].is_boolean());
        assert!(meta_v["configured"]["providers"]["tavily"].is_boolean());
        assert!(meta_v["configured"]["remote_fetch"]["firecrawl"].is_boolean());

        // web_search with auto and no keys should yield ok=false not_configured (retryable=false).
        let search = service
            .call_tool(CallToolRequestParam {
                name: "web_search".into(),
                arguments: Some(
                    serde_json::json!({"provider":"auto","query":"x","max_results":1})
                        .as_object()
                        .cloned()
                        .unwrap(),
                ),
            })
            .await?;
        let search_s = search
            .content
            .get(0)
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let search_v: serde_json::Value = serde_json::from_str(&search_s)?;
        assert_eq!(search_v["schema_version"].as_u64(), Some(1));
        assert_eq!(search_v["kind"].as_str(), Some("web_search"));
        assert_eq!(search_v["ok"].as_bool(), Some(false));
        assert_eq!(search_v["provider"].as_str(), Some("auto"));
        assert_eq!(search_v["error"]["code"].as_str(), Some("not_configured"));
        assert_eq!(search_v["error"]["retryable"].as_bool(), Some(false));

        // web_search_extract should reject missing query+urls with invalid_params.
        let bad = service
            .call_tool(CallToolRequestParam {
                name: "web_search_extract".into(),
                arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
            })
            .await?;
        let bad_s = bad
            .content
            .get(0)
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let bad_v: serde_json::Value = serde_json::from_str(&bad_s)?;
        assert_eq!(bad_v["kind"].as_str(), Some("web_search_extract"));
        assert_eq!(bad_v["ok"].as_bool(), Some(false));
        assert_eq!(bad_v["error"]["code"].as_str(), Some("invalid_params"));

        // web_search_extract in offline mode (urls provided) should return top chunks.
        let hydrate = service
            .call_tool(CallToolRequestParam {
                name: "web_search_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "query": "hello",
                        "urls": [fixture_url],
                        "top_chunks": 3,
                        "max_chunk_chars": 120,
                        "include_text": false,
                        "include_links": true,
                        "max_links": 10
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let hydrate_s = hydrate
            .content
            .get(0)
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let hydrate_v: serde_json::Value = serde_json::from_str(&hydrate_s)?;
        assert_eq!(hydrate_v["kind"].as_str(), Some("web_search_extract"));
        assert_eq!(hydrate_v["ok"].as_bool(), Some(true));
        assert!(hydrate_v["top_chunks"].is_array());
        assert!(hydrate_v["top_chunks"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .any(|c| c
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .contains("Hello")));

        // web_fetch empty URL: invalid_params, retryable=false.
        let fetch = service
            .call_tool(CallToolRequestParam {
                name: "web_fetch".into(),
                arguments: Some(serde_json::json!({"url":""}).as_object().cloned().unwrap()),
            })
            .await?;
        let fetch_s = fetch
            .content
            .get(0)
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let fetch_v: serde_json::Value = serde_json::from_str(&fetch_s)?;
        assert_eq!(fetch_v["ok"].as_bool(), Some(false));
        assert_eq!(fetch_v["error"]["code"].as_str(), Some("invalid_params"));
        assert_eq!(fetch_v["error"]["retryable"].as_bool(), Some(false));

        // web_fetch should *not* forward Authorization/Cookie by default.
        let fetch2 = service
            .call_tool(CallToolRequestParam {
                name: "web_fetch".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": fixture_url,
                        "include_text": true,
                        "max_text_chars": 2000,
                        "headers": {
                            "Authorization": "Bearer secret",
                            "Accept-Language": "en-US"
                        }
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let fetch2_s = fetch2
            .content
            .get(0)
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let fetch2_v: serde_json::Value = serde_json::from_str(&fetch2_s)?;
        assert_eq!(fetch2_v["ok"].as_bool(), Some(true), "fetch2={fetch2_s}");
        assert!(fetch2_v["text"]
            .as_str()
            .unwrap_or("")
            .contains("accept-language=en-US"));
        // We intentionally provided Authorization; webpipe should drop it and surface a warning + name-only trace.
        assert!(fetch2_v["warning_codes"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .any(|x| x.as_str() == Some("unsafe_request_headers_dropped")));
        assert!(fetch2_v["request"]["dropped_request_headers"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .any(|x| x.as_str() == Some("authorization")));

        // web_extract: local fixture, query path should produce chunks and (optionally) omit text.
        let extract = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": fixture_url,
                        "query": "hello",
                        "include_text": false,
                        "include_links": true,
                        "max_links": 10,
                        "max_chars": 2000
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let extract_s = extract
            .content
            .get(0)
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let extract_v: serde_json::Value = serde_json::from_str(&extract_s)?;
        assert_eq!(extract_v["ok"].as_bool(), Some(true));
        assert!(extract_v.get("text").is_none());
        assert!(extract_v.get("chunks").is_some());
        assert!(extract_v.get("links").is_some());
        assert!(extract_v["extraction"]["engine"].is_string());

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio offline contract");
}
