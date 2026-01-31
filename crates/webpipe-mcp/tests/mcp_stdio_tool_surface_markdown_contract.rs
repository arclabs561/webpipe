#[test]
fn webpipe_mcp_stdio_tool_surface_markdown_contract_offlineish() {
    // End-to-end (spawns child process), no external network.
    //
    // Contract:
    // - Each core MCP tool returns Markdown in `content[0]` (Cursor UX),
    // - while `structured_content` carries the canonical JSON payload (automation).
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use axum::{http::header, response::IntoResponse, routing::get, Router};
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };
        use std::net::SocketAddr;

        fn content0_markdown(r: &rmcp::model::CallToolResult) -> String {
            r.content
                .first()
                .and_then(|c| c.as_text())
                .map(|t| t.text.clone())
                .unwrap_or_default()
        }

        fn assert_markdown_view(r: &rmcp::model::CallToolResult) {
            let txt0 = content0_markdown(r);
            assert!(
                txt0.starts_with("## "),
                "expected Markdown heading, got: {txt0:?}"
            );
            assert!(
                serde_json::from_str::<serde_json::Value>(&txt0).is_err(),
                "content[0] should not be JSON by default"
            );
            assert!(
                r.structured_content.is_some(),
                "expected structured_content for canonical JSON payload"
            );
        }

        // Local “docs-like” fixture server for fetch/extract.
        let app = Router::new()
            .route(
                "/doc",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/html")],
                        "<html><body><main><h1>Doc</h1><p>mcp-surface unique token</p></main></body></html>",
                    )
                }),
            )
            .route(
                "/redir",
                get(|| async {
                    (
                        axum::http::StatusCode::FOUND,
                        [(header::LOCATION, "/doc")],
                        "".to_string(),
                    )
                        .into_response()
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });
        let base = format!("http://{addr}");
        let doc_url = format!("{base}/doc");
        let redir_url = format!("{base}/redir");

        // Local ArXiv stub endpoint (Atom XML), used for arxiv_search/arxiv_enrich success paths.
        let atom_xml = r#"
<feed xmlns="http://www.w3.org/2005/Atom"
      xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">
  <opensearch:totalResults>2</opensearch:totalResults>
  <entry>
    <id>http://arxiv.org/abs/0805.3415v1</id>
    <updated>2008-05-22T00:00:00Z</updated>
    <published>2008-05-22T00:00:00Z</published>
    <title> On Upper-Confidence Bound Policies for Non-Stationary Bandit Problems </title>
    <summary>  Some abstract here.  </summary>
    <author><name>A. Author</name></author>
    <author><name>B. Author</name></author>
    <category term="cs.LG" />
    <category term="stat.ML" />
    <link rel="related" type="application/pdf" href="http://arxiv.org/pdf/0805.3415v1"/>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/1305.2545v2</id>
    <updated>2013-05-11T00:00:00Z</updated>
    <published>2013-05-11T00:00:00Z</published>
    <title>Bandits with Knapsacks</title>
    <summary>Abstract two.</summary>
    <author><name>C. Author</name></author>
    <category term="cs.DS" />
  </entry>
</feed>
"#;
        let arxiv_app = Router::new().route(
            "/api/query",
            get(move || async move { ([(header::CONTENT_TYPE, "application/atom+xml")], atom_xml) }),
        );
        let arxiv_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let arxiv_addr: SocketAddr = arxiv_listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(arxiv_listener, arxiv_app)
                .await
                .expect("axum serve arxiv stub");
        });
        let arxiv_endpoint = format!("http://{arxiv_addr}/api/query");

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    cmd.env("WEBPIPE_ARXIV_ENDPOINT", &arxiv_endpoint);
                    cmd.env("WEBPIPE_MCP_INCLUDE_JSON_TEXT", "0");
                }),
            )?)
            .await?;

        // webpipe_meta
        let meta = service
            .call_tool(CallToolRequestParam {
                name: "webpipe_meta".into(),
                arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
            })
            .await?;
        assert_markdown_view(&meta);
        let meta_v = meta.structured_content.clone().unwrap();
        assert_eq!(meta_v["kind"].as_str(), Some("webpipe_meta"));

        // webpipe_usage
        let usage = service
            .call_tool(CallToolRequestParam {
                name: "webpipe_usage".into(),
                arguments: Some(serde_json::json!({}).as_object().cloned().unwrap()),
            })
            .await?;
        assert_markdown_view(&usage);
        assert_eq!(
            usage.structured_content.clone().unwrap()["kind"].as_str(),
            Some("webpipe_usage")
        );

        // web_seed_urls
        let seeds = service
            .call_tool(CallToolRequestParam {
                name: "web_seed_urls".into(),
                arguments: Some(serde_json::json!({"max": 5}).as_object().cloned().unwrap()),
            })
            .await?;
        assert_markdown_view(&seeds);
        assert_eq!(
            seeds.structured_content.clone().unwrap()["kind"].as_str(),
            Some("web_seed_urls")
        );

        // web_fetch (also warms cache)
        let fetch = service
            .call_tool(CallToolRequestParam {
                name: "web_fetch".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": redir_url,
                        "fetch_backend": "local",
                        "cache_read": true,
                        "cache_write": true,
                        "timeout_ms": 5000,
                        "max_bytes": 1_000_000,
                        "include_text": true,
                        "max_text_chars": 2000
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        assert_markdown_view(&fetch);
        assert_eq!(
            fetch.structured_content.clone().unwrap()["kind"].as_str(),
            Some("web_fetch")
        );

        // web_extract
        let extract = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": doc_url,
                        "fetch_backend": "local",
                        "cache_read": true,
                        "cache_write": true,
                        "timeout_ms": 5000,
                        "max_bytes": 1_000_000,
                        "max_chars": 8000,
                        "width": 80,
                        "include_text": false,
                        "include_links": false,
                        "include_structure": true,
                        "top_chunks": 5,
                        "max_chunk_chars": 200,
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        assert_markdown_view(&extract);
        assert_eq!(
            extract.structured_content.clone().unwrap()["kind"].as_str(),
            Some("web_extract")
        );

        // web_search (error path is fine; should still be Markdown)
        let search_err = service
            .call_tool(CallToolRequestParam {
                name: "web_search".into(),
                arguments: Some(serde_json::json!({"query": ""}).as_object().cloned().unwrap()),
            })
            .await?;
        assert_markdown_view(&search_err);
        assert_eq!(
            search_err.structured_content.clone().unwrap()["kind"].as_str(),
            Some("web_search")
        );

        // web_cache_search_extract (should work with warmed cache + bounded params)
        let cache_search = service
            .call_tool(CallToolRequestParam {
                name: "web_cache_search_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "query": "mcp-surface",
                        "max_docs": 10,
                        "max_scan_entries": 500,
                        "include_text": false,
                        "compact": true
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        assert_markdown_view(&cache_search);
        assert_eq!(
            cache_search.structured_content.clone().unwrap()["kind"].as_str(),
            Some("web_cache_search_extract")
        );

        // arxiv_search (success path via local stub)
        let arxiv_s = service
            .call_tool(CallToolRequestParam {
                name: "arxiv_search".into(),
                arguments: Some(
                    serde_json::json!({
                        "query": "bandit",
                        "page": 1,
                        "per_page": 2,
                        "timeout_ms": 5000
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        assert_markdown_view(&arxiv_s);
        let arxiv_v = arxiv_s.structured_content.clone().unwrap();
        assert_eq!(arxiv_v["kind"].as_str(), Some("arxiv_search"));
        assert_eq!(arxiv_v["ok"].as_bool(), Some(true));
        assert_eq!(arxiv_v["papers"].as_array().map(|a| a.len()), Some(2));

        // arxiv_enrich (success path via local stub; no discussions)
        let arxiv_e = service
            .call_tool(CallToolRequestParam {
                name: "arxiv_enrich".into(),
                arguments: Some(
                    serde_json::json!({
                        "id_or_url": "0805.3415v1",
                        "timeout_ms": 5000,
                        "include_discussions": false
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;
        assert_markdown_view(&arxiv_e);
        let arxiv_ev = arxiv_e.structured_content.clone().unwrap();
        assert_eq!(arxiv_ev["kind"].as_str(), Some("arxiv_enrich"));
        assert_eq!(arxiv_ev["ok"].as_bool(), Some(true));
        assert_eq!(arxiv_ev["arxiv_id"].as_str(), Some("0805.3415v1"));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio tool surface markdown contract");
}
