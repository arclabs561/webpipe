use std::collections::BTreeSet;

#[test]
fn webpipe_live_meta_reports_brave_configured_opt_in() {
    // This test does NOT call Brave, but it does validate that the MCP server
    // sees the env key and reports it in webpipe_meta. Opt-in to avoid
    // surprising behavior in environments without the key.
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping: set WEBPIPE_LIVE=1 to run live meta smoke");
        return;
    }

    // We don't need the value here, just ensure it's present for the spawned server.
    let brave_key = match std::env::var("WEBPIPE_BRAVE_API_KEY")
        .ok()
        .or_else(|| std::env::var("BRAVE_SEARCH_API_KEY").ok())
    {
        Some(v) if !v.trim().is_empty() => v,
        _ => {
            eprintln!("skipping: missing WEBPIPE_BRAVE_API_KEY (or BRAVE_SEARCH_API_KEY)");
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
        assert!(names.contains("webpipe_meta"), "missing webpipe_meta tool");

        let resp = service
            .call_tool(CallToolRequestParam {
                name: "webpipe_meta".into(),
                arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
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
        assert_eq!(v["kind"].as_str(), Some("webpipe_meta"));
        assert_eq!(v["ok"].as_bool(), Some(true));
        assert_eq!(v["configured"]["providers"]["brave"].as_bool(), Some(true));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("live meta brave smoke");
}
