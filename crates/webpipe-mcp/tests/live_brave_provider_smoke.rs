use std::collections::BTreeSet;

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
fn webpipe_live_brave_search_smoke_opt_in() {
    // This test makes a real paid network call. Opt-in only.
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1")
        || std::env::var("WEBPIPE_LIVE_PAID").ok().as_deref() != Some("1")
    {
        eprintln!(
            "skipping: set WEBPIPE_LIVE=1 WEBPIPE_LIVE_PAID=1 to run live brave provider smoke"
        );
        return;
    }

    // Require Brave key to be present in *this* test process; we pass it to the child.
    let brave_key = match std::env::var("WEBPIPE_BRAVE_API_KEY")
        .ok()
        .or_else(|| std::env::var("BRAVE_SEARCH_API_KEY").ok())
    {
        Some(k) if !k.trim().is_empty() => k,
        _ => {
            panic!("missing WEBPIPE_BRAVE_API_KEY (or BRAVE_SEARCH_API_KEY)");
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
                    cmd.env("WEBPIPE_BRAVE_API_KEY", &brave_key);
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
                        "provider": "brave",
                        "query": "brave search api",
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
        assert_eq!(v["provider"].as_str(), Some("brave"));
        assert_eq!(v["query"].as_str(), Some("brave search api"));
        assert_eq!(v["max_results"].as_u64(), Some(1));
        assert!(v["results"].is_array());
        assert!(v["results"].as_array().unwrap().len() <= 1);

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("live brave search smoke");
}
