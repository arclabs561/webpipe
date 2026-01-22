use std::collections::BTreeSet;

#[test]
fn webpipe_live_tavily_search_smoke_opt_in() {
    // This test makes a real paid network call. Opt-in only.
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping: set WEBPIPE_LIVE=1 to run live provider smoke");
        return;
    }

    // Require Tavily key to be present in *this* test process; we pass it to the child.
    let tavily_key = match std::env::var("WEBPIPE_TAVILY_API_KEY")
        .ok()
        .or_else(|| std::env::var("TAVILY_API_KEY").ok())
    {
        Some(k) if !k.trim().is_empty() => k,
        _ => {
            eprintln!("skipping: missing WEBPIPE_TAVILY_API_KEY (or TAVILY_API_KEY)");
            return;
        }
    };

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
                    cmd.env("WEBPIPE_TAVILY_API_KEY", &tavily_key);
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

        // Minimal, bounded search. Don't assert content stability; just shape.
        let resp = service
            .call_tool(CallToolRequestParam {
                name: "web_search".into(),
                arguments: Some(
                    serde_json::json!({
                        "provider": "tavily",
                        "query": "tavily search api",
                        "max_results": 1
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        let s = resp
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&s)?;

        assert_eq!(v["schema_version"].as_u64(), Some(1));
        assert_eq!(v["ok"].as_bool(), Some(true));
        assert_eq!(v["provider"].as_str(), Some("tavily"));
        assert_eq!(v["query"].as_str(), Some("tavily search api"));
        assert_eq!(v["max_results"].as_u64(), Some(1));
        assert!(v["results"].is_array());
        // If Tavily fell back, backend_provider will differ and a fallback object is present.
        if v.get("fallback").is_some() {
            assert_eq!(v["backend_provider"].as_str(), Some("brave"));
        } else {
            assert_eq!(v["backend_provider"].as_str(), Some("tavily"));
        }

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("live tavily search smoke");
}
