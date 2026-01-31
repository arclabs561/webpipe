use std::collections::BTreeSet;

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
fn webpipe_live_search_caps_smoke_opt_in() {
    // This test makes real paid network calls. Opt-in only.
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping: set WEBPIPE_LIVE=1 to run live search caps smoke");
        return;
    }

    // We run both providers if both keys are available; otherwise skip.
    let brave_key = get_env_any(&["WEBPIPE_BRAVE_API_KEY", "BRAVE_SEARCH_API_KEY"]);
    let tavily_key = get_env_any(&["WEBPIPE_TAVILY_API_KEY", "TAVILY_API_KEY"]);
    if brave_key.is_none() || tavily_key.is_none() {
        eprintln!("skipping: need both Brave + Tavily keys to run this test");
        return;
    }

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
                    cmd.env("WEBPIPE_BRAVE_API_KEY", brave_key.as_ref().unwrap());
                    cmd.env("WEBPIPE_TAVILY_API_KEY", tavily_key.as_ref().unwrap());
                }),
            )?)
            .await?;

        let tools = service.list_tools(Default::default()).await?;
        let names: BTreeSet<String> = tools
            .tools
            .iter()
            .map(|t| t.name.clone().into_owned())
            .collect();
        assert!(names.contains("web_search"), "missing web_search tool");

        // Exercise caps:
        // - max_results clamp (999 -> 20)
        // - language/country are passed through (best-effort; we only assert echo + bounds)
        let brave_resp = service
            .call_tool(CallToolRequestParam {
                name: "web_search".into(),
                arguments: Some(
                    serde_json::json!({
                        "provider": "brave",
                        "query": "webpipe live caps smoke brave",
                        "max_results": 999,
                        "language": "en",
                        "country": "us"
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let brave_s = brave_resp
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let brave: serde_json::Value = serde_json::from_str(&brave_s)?;
        assert_eq!(brave["schema_version"].as_u64(), Some(2));
        assert_eq!(brave["ok"].as_bool(), Some(true));
        assert_eq!(brave["provider"].as_str(), Some("brave"));
        assert_eq!(brave["max_results"].as_u64(), Some(20));
        assert!(brave["query"].as_str().unwrap_or("").contains("brave"));
        assert!(brave["results"].is_array());
        assert!(brave["results"].as_array().unwrap().len() <= 20);

        let tavily_resp = service
            .call_tool(CallToolRequestParam {
                name: "web_search".into(),
                arguments: Some(
                    serde_json::json!({
                        "provider": "tavily",
                        "query": "webpipe live caps smoke tavily",
                        "max_results": 999,
                        "language": "en",
                        "country": "us"
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let tavily_s = tavily_resp
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let tavily: serde_json::Value = serde_json::from_str(&tavily_s)?;
        assert_eq!(tavily["schema_version"].as_u64(), Some(2));
        assert_eq!(tavily["ok"].as_bool(), Some(true));
        assert_eq!(tavily["provider"].as_str(), Some("tavily"));
        assert_eq!(tavily["max_results"].as_u64(), Some(20));
        assert!(tavily["query"].as_str().unwrap_or("").contains("tavily"));
        assert!(tavily["results"].is_array());
        assert!(tavily["results"].as_array().unwrap().len() <= 20);
        // If Tavily fell back, the backend provider will be Brave and we'll carry evidence.
        if tavily.get("fallback").is_some() {
            assert_eq!(tavily["backend_provider"].as_str(), Some("brave"));
            let fb = &tavily["fallback"];
            assert_eq!(fb["requested_provider"].as_str(), Some("tavily"));
            assert_eq!(fb["reason"].as_str(), Some("tavily_http_433"));
            assert!(fb["tavily_error"]
                .as_str()
                .unwrap_or("")
                .contains("HTTP 433"));
        } else {
            assert_eq!(tavily["backend_provider"].as_str(), Some("tavily"));
        }

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("live search caps smoke");
}
