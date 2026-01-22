//! Opt-in smoke suite for real-world URLs seen in prior sessions.
//!
//! This test is intentionally **not** deterministic; it is guarded by an env var
//! so it doesn't fail CI or offline dev.

use std::collections::BTreeSet;

#[test]
fn webpipe_live_url_seed_smoke_opt_in() {
    if std::env::var("WEBPIPE_LIVE_URL_SEED_SMOKE").ok().as_deref() != Some("1") {
        eprintln!("skipping: set WEBPIPE_LIVE_URL_SEED_SMOKE=1 to run this test");
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
                        std::env::temp_dir().join("webpipe-test-cache"),
                    );
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
            "web_extract",
            "web_search_extract",
            "web_deep_research",
        ] {
            assert!(names.contains(must_have), "missing tool {must_have}");
        }

        // Curated seed list (from prior chat/transcript).
        // Keep this small; expand over time.
        let seeds = [
            "https://example.com",
            "http://example.com",
            "https://docs.perplexity.ai/api-reference/chat-completions-post",
            "https://modelcontextprotocol.io/docs/develop/connect-local-servers",
            "https://exdst.com/posts/20250524-cursor-mcp-troubleshooting",
        ];

        for url in seeds {
            // Fetch (local)
            let r = service
                .call_tool(CallToolRequestParam {
                    name: "web_fetch".into(),
                    arguments: Some(
                        serde_json::json!({
                            "url": url,
                            "fetch_backend": "local",
                            "include_text": false,
                            "max_text_chars": 2000,
                            "max_bytes": 2_000_000
                        })
                        .as_object()
                        .cloned()
                        .unwrap(),
                    ),
                })
                .await?;
            let text = r
                .content
                .get(0)
                .and_then(|c| c.as_text())
                .map(|t| t.text.clone())
                .unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&text)?;
            assert_eq!(v["schema_version"].as_u64(), Some(1));
            assert_eq!(v["kind"].as_str(), Some("web_fetch"));
            assert_eq!(v["request"]["fetch_backend"].as_str(), Some("local"));

            // Extract (local)
            let r = service
                .call_tool(CallToolRequestParam {
                    name: "web_extract".into(),
                    arguments: Some(
                        serde_json::json!({
                            "url": url,
                            "fetch_backend": "local",
                            "include_text": false,
                            "include_links": true,
                            "max_links": 25,
                            "max_chars": 5000
                        })
                        .as_object()
                        .cloned()
                        .unwrap(),
                    ),
                })
                .await?;
            let text = r
                .content
                .get(0)
                .and_then(|c| c.as_text())
                .map(|t| t.text.clone())
                .unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&text)?;
            assert_eq!(v["schema_version"].as_u64(), Some(1));
            assert_eq!(v["kind"].as_str(), Some("web_extract"));
            assert_eq!(v["request"]["fetch_backend"].as_str(), Some("local"));
        }

        Ok::<(), anyhow::Error>(())
    })
    .expect("live_url_seed_smoke");
}
