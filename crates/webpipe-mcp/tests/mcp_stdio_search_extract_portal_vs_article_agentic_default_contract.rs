use std::collections::BTreeSet;

#[test]
fn webpipe_mcp_stdio_search_extract_portal_vs_article_agentic_default_surfaces_article() {
    // E2E (spawns child process), local-only:
    // - local fixture server provides a "portal" page and a specific "article" page
    // - call web_search_extract with agentic=true (default behavior) over urls=[...]
    // - assert top_chunks includes article content

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
<p>Welcome to the portal. Browse categories:</p>
<ul>
  <li><a href="/a">A</a></li>
  <li><a href="/b">B</a></li>
  <li><a href="/c">C</a></li>
  <li><a href="/article">Incident writeup</a></li>
</ul>
<p>new | past | comments | ask | show | jobs | login</p>
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
                    // Ensure deterministic keyless behavior.
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
                        // Default behavior: agentic=true, but keep planner off so it stays deterministic.
                        "agentic": true,
                        "planner_max_calls": 0,
                        "agentic_max_search_rounds": 1,
                        "max_urls": 2,
                        "top_chunks": 5,
                        "max_chunk_chars": 300,
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

        let top = v["top_chunks"].as_array().cloned().unwrap_or_default();
        assert!(!top.is_empty(), "expected top_chunks");
        let has_article = top.iter().any(|c| {
            c.get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .contains("dependency confusion")
        });
        assert!(has_article, "expected article chunk in top_chunks");

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio portal-vs-article agentic-default contract");
}
