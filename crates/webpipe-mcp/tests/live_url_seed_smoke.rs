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
        use anyhow::anyhow;
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };

        fn payload_from_result(
            r: &rmcp::model::CallToolResult,
        ) -> Result<serde_json::Value, anyhow::Error> {
            if let Some(v) = r.structured_content.clone() {
                return Ok(v);
            }
            for c in &r.content {
                if let Some(t) = c.as_text() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t.text) {
                        return Ok(v);
                    }
                }
            }
            Err(anyhow!("missing structured_content/JSON text content"))
        }

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
            "search_evidence",
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
            let v: serde_json::Value = payload_from_result(&r)?;
            assert_eq!(v["schema_version"].as_u64(), Some(2));
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
            let v: serde_json::Value = payload_from_result(&r)?;
            assert_eq!(v["schema_version"].as_u64(), Some(2));
            assert_eq!(v["kind"].as_str(), Some("web_extract"));
            assert_eq!(v["request"]["fetch_backend"].as_str(), Some("local"));

            // Unified pipeline (urls-mode): ensure the primary workflow runs end-to-end.
            let r = service
                .call_tool(CallToolRequestParam {
                    name: "search_evidence".into(),
                    arguments: Some(
                        serde_json::json!({
                            "query": "seed smoke",
                            "urls": [url],
                            "url_selection_mode": "preserve",
                            "fetch_backend": "local",
                            "agentic": false,
                            "max_urls": 1,
                            "max_chars": 6000,
                            "top_chunks": 3,
                            "max_chunk_chars": 400,
                            "include_text": false,
                            "include_links": false,
                            "compact": true,
                            "timeout_ms": 20_000,
                            "deadline_ms": 25_000
                        })
                        .as_object()
                        .cloned()
                        .unwrap(),
                    ),
                })
                .await?;
            let v: serde_json::Value = payload_from_result(&r)?;
            assert_eq!(v["schema_version"].as_u64(), Some(2));
            assert!(
                matches!(
                    v["kind"].as_str(),
                    Some("web_search_extract") | Some("search_evidence")
                ),
                "unexpected kind: {:?}",
                v.get("kind").and_then(|k| k.as_str())
            );
            assert!(v["ok"].is_boolean());
            assert!(v["top_chunks"].is_array(), "expected top_chunks array");
        }

        Ok::<(), anyhow::Error>(())
    })
    .expect("live_url_seed_smoke");
}
