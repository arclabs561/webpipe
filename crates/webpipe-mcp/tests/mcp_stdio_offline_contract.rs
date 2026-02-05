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
        use rmcp::model::GetPromptRequestParam;
        use rmcp::model::ReadResourceRequestParam;
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
                        // Include nasty-but-valid bytes so MCP results prove “clean text” handling:
                        // - CRLFs
                        // - NUL
                        // - formfeed
                        "<html><body>\r\n<h1>Hello</h1>\u{0000}<p>world</p>\u{000C}<p>accept-language={}</p><a href=\"/a#x\">A</a></body></html>\r\n",
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
        let cache_dir = tempfile::TempDir::new()?;
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    // Disable `.env` autoload so this test stays hermetic.
                    cmd.env("WEBPIPE_DOTENV", "0");
                    // This contract asserts the full tool surface (including debug-only tools).
                    cmd.env("WEBPIPE_MCP_TOOLSET", "debug");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    // For contract tests, force full offline-only behavior (localhost fixtures still work).
                    cmd.env("WEBPIPE_OFFLINE_ONLY", "1");
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
        // Discovery contract: ensure key tools have non-empty descriptions and sensible annotations.
        let mut by_name = std::collections::BTreeMap::new();
        for t in &tools.tools {
            by_name.insert(t.name.clone().into_owned(), t);
        }
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
            "search_evidence",
        ] {
            assert!(names.contains(must_have), "missing tool {must_have}");
        }
        for must_have in [
            "webpipe_meta",
            "web_fetch",
            "web_search",
            "web_extract",
            "search_evidence",
        ] {
            let t = by_name.get(must_have).expect("tool exists");
            let desc = t.description.as_deref().unwrap_or("");
            assert!(!desc.trim().is_empty(), "{must_have} missing description");
            assert!(
                desc.contains("JSON"),
                "{must_have} description should mention JSON output for discovery"
            );
        }
        // Annotations contract: titles exist and hints match expected shape.
        // These are hints, not a security boundary, but we want them stable for client UX.
        let expected_hints: &[(&str, bool, bool)] = &[
            ("webpipe_meta", true, false),
            ("webpipe_usage", true, false),
            ("web_seed_urls", true, false),
            ("web_cache_search_extract", true, false),
            ("web_fetch", true, true),
            ("web_extract", true, true),
            ("web_search", true, true),
            ("search_evidence", true, true),
            ("web_seed_search_extract", true, true),
            ("web_deep_research", true, true),
            ("arxiv_search", true, true),
            ("arxiv_enrich", true, true),
        ];
        for (name, read_only, open_world) in expected_hints {
            let t = by_name.get(*name).expect("tool exists");
            let a = t.annotations.as_ref().expect("tool annotations");
            let title = a.title.as_deref().unwrap_or("");
            assert!(!title.trim().is_empty(), "{name} missing annotations.title");
            assert_eq!(a.read_only_hint, Some(*read_only), "{name} read_only_hint");
            assert_eq!(a.open_world_hint, Some(*open_world), "{name} open_world_hint");
        }
        // Side-effect hint: usage reset is not read-only, is destructive, and is idempotent.
        let t = by_name
            .get("webpipe_usage_reset")
            .expect("webpipe_usage_reset exists");
        let a = t
            .annotations
            .as_ref()
            .expect("webpipe_usage_reset annotations");
        assert_eq!(a.read_only_hint, Some(false));
        assert_eq!(a.destructive_hint, Some(true));
        assert_eq!(a.idempotent_hint, Some(true));
        assert_eq!(a.open_world_hint, Some(false));

        // Prompts: server should expose a small set of workflow templates for clients.
        let prompts = service.list_prompts(Default::default()).await?;
        let prompt_names: BTreeSet<String> = prompts
            .prompts
            .iter()
            .map(|p| p.name.clone().to_owned())
            .collect();
        for must_have in [
            "webpipe_search_extract",
            "webpipe_offline_cache_first",
            "webpipe_deep_research",
        ] {
            assert!(prompt_names.contains(must_have), "missing prompt {must_have}");
        }
        // Discovery contract: prompt descriptions should be non-empty.
        for p in &prompts.prompts {
            let d = p.description.as_deref().unwrap_or("");
            assert!(
                !d.trim().is_empty(),
                "prompt {} missing description",
                p.name
            );
        }
        // get_prompt should return at least one message (rmcp supports user/assistant roles only).
        let p = service
            .get_prompt(GetPromptRequestParam {
                name: "webpipe_search_extract".to_string(),
                arguments: Some(
                    serde_json::json!({"query":"test query","no_network":true})
                        .as_object()
                        .cloned()
                        .unwrap(),
                ),
            })
            .await?;
        assert!(
            !p.messages.is_empty(),
            "expected >=1 prompt message, got {}",
            p.messages.len()
        );

        // Resources: server should expose a small set of “documents” for clients.
        let resources = service.list_resources(Default::default()).await?;
        let resource_uris: BTreeSet<String> = resources
            .resources
            .iter()
            .map(|r| r.raw.uri.clone())
            .collect();
        for must_have in ["webpipe://meta", "webpipe://usage"] {
            assert!(
                resource_uris.contains(must_have),
                "missing resource uri {must_have}"
            );
        }
        // Discovery contract: resources have non-empty descriptions (for client UX).
        for r in &resources.resources {
            let d = r.raw.description.as_deref().unwrap_or("");
            assert!(
                !d.trim().is_empty(),
                "resource {} missing description",
                r.raw.uri
            );
        }
        let rr = service
            .read_resource(ReadResourceRequestParam {
                uri: "webpipe://meta".to_string(),
            })
            .await?;
        assert!(!rr.contents.is_empty(), "expected resource contents");
        // Resource contents should be valid JSON (meta mirrors webpipe_meta).
        let meta_text = rr.contents.iter().find_map(|c| match c {
            rmcp::model::ResourceContents::TextResourceContents { text, .. } => Some(text),
            _ => None,
        });
        let meta_text = meta_text.expect("text content");
        let meta_v: serde_json::Value = serde_json::from_str(meta_text).expect("meta json");
        assert_eq!(meta_v["kind"].as_str(), Some("webpipe_meta"));

        // Meta: always ok, and configured flags are boolean.
        let meta = service
            .call_tool(CallToolRequestParam {
                name: "webpipe_meta".into(),
                arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
            })
            .await?;
        assert_eq!(
            meta.is_error,
            Some(false),
            "webpipe_meta should be MCP-success (ok=true)"
        );
        let meta_v: serde_json::Value = meta
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(meta_v["schema_version"].as_u64(), Some(2));
        assert_eq!(meta_v["kind"].as_str(), Some("webpipe_meta"));
        assert_eq!(meta_v["ok"].as_bool(), Some(true));
        assert_eq!(meta_v["offline_only"].as_bool(), Some(true));
        assert!(meta_v["configured"]["providers"]["brave"].is_boolean());
        assert!(meta_v["configured"]["providers"]["tavily"].is_boolean());
        assert!(meta_v["configured"]["remote_fetch"]["firecrawl"].is_boolean());

        // MCP-level is_error should be set when ok=false for tool calls.
        let fetch = service
            .call_tool(CallToolRequestParam {
                name: "web_fetch".into(),
                arguments: Some(serde_json::json!({"url":""}).as_object().cloned().unwrap()),
            })
            .await?;
        assert_eq!(fetch.is_error, Some(true));

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
        let search_v: serde_json::Value = search
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(search_v["schema_version"].as_u64(), Some(2));
        assert_eq!(search_v["kind"].as_str(), Some("web_search"));
        assert_eq!(search_v["ok"].as_bool(), Some(false));
        assert_eq!(search_v["provider"].as_str(), Some("auto"));
        assert_eq!(search_v["error"]["code"].as_str(), Some("not_supported"));
        assert_eq!(search_v["error"]["retryable"].as_bool(), Some(false));

        // search_evidence should fail closed in offline mode when urls are missing.
        let bad = service
            .call_tool(CallToolRequestParam {
                name: "search_evidence".into(),
                arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
            })
            .await?;
        let bad_v: serde_json::Value = bad
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(bad_v["kind"].as_str(), Some("web_search_extract"));
        assert_eq!(bad_v["ok"].as_bool(), Some(false));
        assert_eq!(bad_v["error"]["code"].as_str(), Some("not_supported"));

        // search_evidence in offline mode (urls provided) should return top chunks.
        let hydrate = service
            .call_tool(CallToolRequestParam {
                name: "search_evidence".into(),
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
        let hydrate_v: serde_json::Value = hydrate
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
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
        let fetch_v: serde_json::Value = fetch
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
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
        let fetch2_v: serde_json::Value = fetch2
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(fetch2_v["ok"].as_bool(), Some(true));
        assert!(fetch2_v["body_text"]
            .as_str()
            .unwrap_or("")
            .contains("accept-language=en-US"));
        // Text should be cleaned for JSON/client display (no CR/NUL/formfeed).
        let t2 = fetch2_v["body_text"].as_str().unwrap_or("");
        assert!(!t2.contains('\r'), "web_fetch text contains CR");
        assert!(
            !t2.chars().any(|c| c == '\u{0000}'),
            "web_fetch text contains NUL"
        );
        assert!(
            !t2.chars().any(|c| c == '\u{000C}'),
            "web_fetch text contains formfeed"
        );
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
        let extract_v: serde_json::Value = extract
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(extract_v["ok"].as_bool(), Some(true));
        assert!(extract_v.get("text").is_none());
        assert!(extract_v.get("chunks").is_none());
        assert!(extract_v.get("links").is_none());
        assert!(extract_v["extract"]["chunks"].is_array());
        assert!(extract_v["extract"]["links"].is_array());
        assert!(extract_v["extract"]["engine"].is_string());
        assert_eq!(
            extract_v["extract"]["quality"]["kind"].as_str(),
            Some("webpipe_extract_quality")
        );
        assert!(extract_v["extract"].get("quality").is_some());

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio offline contract");
}
