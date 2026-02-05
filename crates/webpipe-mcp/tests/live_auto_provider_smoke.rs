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

fn payload_from_result(result: &rmcp::model::CallToolResult) -> Option<serde_json::Value> {
    result.structured_content.clone().or_else(|| {
        result
            .content
            .first()
            .and_then(|c| c.as_text())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t.text).ok())
    })
}

#[test]
fn webpipe_live_auto_search_smoke_opt_in() {
    // This test makes a real paid network call. Opt-in only.
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1")
        || std::env::var("WEBPIPE_LIVE_PAID").ok().as_deref() != Some("1")
    {
        eprintln!(
            "skipping: set WEBPIPE_LIVE=1 WEBPIPE_LIVE_PAID=1 to run live auto provider smoke"
        );
        return;
    }

    // Need at least one provider key to make auto usable.
    let brave_key = get_env_any(&["WEBPIPE_BRAVE_API_KEY", "BRAVE_SEARCH_API_KEY"]);
    let tavily_key = get_env_any(&["WEBPIPE_TAVILY_API_KEY", "TAVILY_API_KEY"]);
    if brave_key.is_none() && tavily_key.is_none() {
        panic!("need WEBPIPE_BRAVE_API_KEY/BRAVE_SEARCH_API_KEY and/or WEBPIPE_TAVILY_API_KEY/TAVILY_API_KEY");
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
                    // Allow `.env` autoload so `dev/.env` keys work.
                    cmd.env(
                        "WEBPIPE_CACHE_DIR",
                        std::env::temp_dir().join("webpipe-live-cache"),
                    );
                    if let Some(k) = brave_key.as_ref() {
                        cmd.env("WEBPIPE_BRAVE_API_KEY", k);
                    }
                    if let Some(k) = tavily_key.as_ref() {
                        cmd.env("WEBPIPE_TAVILY_API_KEY", k);
                    }
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

        let resp = service
            .call_tool(CallToolRequestParam {
                name: "web_search".into(),
                arguments: Some(
                    serde_json::json!({
                        "provider": "auto",
                        "query": "webpipe auto search smoke",
                        "max_results": 1
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        let v = payload_from_result(&resp).ok_or("missing structured_content")?;

        assert_eq!(v["schema_version"].as_u64(), Some(2));
        assert_eq!(v["kind"].as_str(), Some("web_search"));
        assert_eq!(v["ok"].as_bool(), Some(true));
        assert_eq!(v["request"]["provider"].as_str(), Some("auto"));
        assert_eq!(v["query"].as_str(), Some("webpipe auto search smoke"));
        assert_eq!(v["max_results"].as_u64(), Some(1));
        assert_eq!(v["provider"].as_str(), Some("auto"));
        assert!(matches!(
            v["backend_provider"].as_str(),
            Some("brave") | Some("tavily") | Some("searxng")
        ));
        assert!(v["results"].is_array());
        assert!(v["results"].as_array().unwrap().len() <= 1);

        // Auto should always explain selection (even without fallback).
        assert_eq!(v["selection"]["requested_provider"].as_str(), Some("auto"));
        assert!(matches!(
            v["selection"]["selected_provider"].as_str(),
            Some("brave") | Some("tavily") | Some("searxng")
        ));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("live auto search smoke");
}
