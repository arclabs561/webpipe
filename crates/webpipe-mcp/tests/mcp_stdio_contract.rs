use std::collections::BTreeSet;

#[test]
fn webpipe_stdio_lists_tools() {
    // This is a true end-to-end check (spawns a child process).
    // It can be flaky across environments and is skipped by default.
    if std::env::var("WEBPIPE_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping: set WEBPIPE_E2E=1 to run this test");
        return;
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use axum::{routing::get, Router};
        use rmcp::{
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };
        use std::net::SocketAddr;

        // Local fixture server: stable, offline, and validates fetch+extract+links.
        let app = Router::new().route(
            "/",
            get(|| async {
                (
                    [("content-type", "text/html")],
                    "<html><body><h1>Hello</h1><p>world</p><a href=\"/a#x\">A</a></body></html>",
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    // Keep cache local and test-only.
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
            "web_search",
            "web_extract",
            "web_search_extract",
            "web_deep_research",
        ] {
            assert!(names.contains(must_have), "missing tool {must_have}");
        }

        // Verify web_extract produces text and links for local HTML.
        use rmcp::model::CallToolRequestParam;
        let resp = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": format!("http://{}/", addr),
                        "include_text": true,
                        "include_links": true,
                        "max_links": 10,
                        "max_chars": 5000
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
        assert_eq!(v["ok"].as_bool(), Some(true));
        assert!(v["text"].as_str().unwrap_or("").contains("Hello"));
        assert!(v["links"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .any(|x| x.as_str() == Some(&format!("http://{}/a", addr))));

        // Verify query/chunks path omits full text by default.
        let resp2 = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": format!("http://{}/", addr),
                        "query": "hello world",
                        "include_text": false,
                        "include_links": true,
                        "max_links": 10,
                        "max_chars": 5000,
                        "top_chunks": 3,
                        "max_chunk_chars": 200
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        let s2 = resp2
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let v2: serde_json::Value = serde_json::from_str(&s2)?;
        assert_eq!(v2["ok"].as_bool(), Some(true));
        assert!(v2.get("text").is_none());
        assert!(v2.get("chunks").is_some());

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio contract");
}
