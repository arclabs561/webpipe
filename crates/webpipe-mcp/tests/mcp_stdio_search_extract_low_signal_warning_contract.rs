use std::collections::BTreeSet;

#[test]
fn webpipe_mcp_stdio_search_extract_marks_low_signal_portal_with_warning_codes() {
    // E2E (spawns child process), local-only:
    // - portal page includes a JS-bundle-gunk chunk + normal prose
    // - expect per-URL warnings include chunks_filtered_low_signal for portal
    // - article page should not be flagged as low-signal

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use axum::{routing::get, Router};
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };
        use std::net::SocketAddr;

        const PORTAL_HTML: &str = r#"
<html><body>
<h1>Portal</h1>
<p>Welcome to the portal. This is mostly navigation.</p>
<pre>
self.__next_s.push([0,{"suppressHydrationWarning":true}]);
webpackChunk.push([0,{"__webpack_require__":{}}]);
</pre>
<p>new | past | comments | ask | show | jobs | login</p>
<a href="/article">Incident writeup</a>
</body></html>
"#;
        const ARTICLE_HTML: &str = r#"
<html><body>
<h1>Dependency confusion incident writeup</h1>
<p>
This incident describes a dependency confusion attack against an AI tooling package.
It includes a timeline, affected package names, and mitigation steps.
</p>
</body></html>
"#;

        let app = Router::new()
            .route(
                "/portal",
                get(|| async { ([("content-type", "text/html")], PORTAL_HTML) }),
            )
            .route(
                "/article",
                get(|| async { ([("content-type", "text/html")], ARTICLE_HTML) }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });

        let portal_url = format!("http://{addr}/portal");
        let article_url = format!("http://{addr}/article");

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    // Deterministic keyless behavior.
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
        assert!(names.contains("web_search_extract"));

        let resp = service
            .call_tool(CallToolRequestParam {
                name: "web_search_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "query": "dependency confusion incident",
                        "urls": [portal_url, article_url],
                        "url_selection_mode": "preserve",
                        "fetch_backend": "local",
                        "no_network": false,
                        "agentic": false,
                        "max_urls": 2,
                        "top_chunks": 5,
                        "max_chunk_chars": 400,
                        "include_text": false,
                        "include_links": false,
                        "cache_read": false,
                        "cache_write": false
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        assert_eq!(resp.is_error, Some(false));
        let v: serde_json::Value = resp
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(v["kind"].as_str(), Some("web_search_extract"));
        assert_eq!(v["ok"].as_bool(), Some(true));
        let s =
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| "<json encode failed>".to_string());

        let per_url = v["results"].as_array().cloned().unwrap_or_default();
        assert_eq!(per_url.len(), 2, "expected 2 per-url results");

        let find = |url: &str| -> Option<&serde_json::Value> {
            per_url
                .iter()
                .find(|it| it.get("url").and_then(|x| x.as_str()) == Some(url))
        };
        let p0 = find(&portal_url).expect("portal per-url result");
        let p1 = find(&article_url).expect("article per-url result");

        // For web_search_extract, low-signal detection is surfaced via a stable `quality` scorecard
        // (rather than a separate warnings array).
        let portal_issues: Vec<&str> = p0["extract"]["quality"]["issues"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str())
            .collect();
        assert!(
            portal_issues.iter().any(|x| *x == "gunk"),
            "expected portal quality to include 'gunk'; got issues={portal_issues:?}"
        );
        assert_eq!(
            p0["extract"]["quality"]["signals"]["bundle_gunk"].as_bool(),
            Some(true),
            "expected portal bundle_gunk=true"
        );
        assert_eq!(
            p0["extract"]["quality"]["signals"]["has_low_signal"].as_bool(),
            Some(true),
            "expected portal has_low_signal=true"
        );

        let article_issues: Vec<&str> = p1["extract"]["quality"]["issues"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str())
            .collect();
        assert!(
            !article_issues.iter().any(|x| *x == "gunk"),
            "expected article not gunk; got issues={article_issues:?} full={}",
            s
        );
        assert_eq!(
            p1["extract"]["quality"]["signals"]["bundle_gunk"].as_bool(),
            Some(false),
            "expected article bundle_gunk=false; full={}",
            s
        );
        assert_eq!(
            p1["extract"]["quality"]["signals"]["has_low_signal"].as_bool(),
            Some(false),
            "expected article has_low_signal=false; full={}",
            s
        );

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio low-signal warning contract");
}
