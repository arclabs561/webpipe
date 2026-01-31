use axum::{routing::get, Router};
use rmcp::{
    model::CallToolRequestParam,
    service::{RoleClient, RunningService, ServiceExt},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::collections::BTreeSet;
use std::net::SocketAddr;

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn call(
    service: &RunningService<RoleClient, ()>,
    name: &'static str,
    args: serde_json::Value,
) -> serde_json::Value {
    let r = service
        .call_tool(CallToolRequestParam {
            name: name.to_string().into(),
            arguments: Some(args.as_object().cloned().unwrap()),
        })
        .await
        .expect("call_tool");
    if let Some(v) = r.structured_content.clone() {
        return v;
    }
    for c in &r.content {
        if let Some(t) = c.as_text() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t.text) {
                return v;
            }
        }
    }
    serde_json::json!({})
}

// Validates GitHub repo root → raw README rewrite using a local fixture server.
#[test]
fn web_extract_rewrites_github_repo_to_raw_readme() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let repo_html = r#"
        <html><body>
          <div class="Header">GitHub</div>
          <div class="Sidebar">nav nav nav</div>
          <div class="Main">This is the repo page shell, not the README.</div>
        </body></html>
        "#;
        let readme_md = r#"
# Hello README

This is the real README content, not the GitHub app shell.
"#;
        let repo_html_s = repo_html.to_string();
        let readme_md_s = readme_md.to_string();
        let app = Router::new()
            .route(
                "/owner/repo",
                get(move || {
                    let body = repo_html_s.clone();
                    async move { ([("content-type", "text/html")], body) }
                }),
            )
            .route(
                "/owner/repo/main/README.md",
                get(move || {
                    let body = readme_md_s.clone();
                    async move { ([("content-type", "text/plain")], body) }
                }),
            );
        let addr = serve(app).await;

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let raw_host = format!("127.0.0.1:{}", addr.port());
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env(
                        "WEBPIPE_CACHE_DIR",
                        std::env::temp_dir().join("webpipe-github-rewrite-cache"),
                    );
                    cmd.env("WEBPIPE_GITHUB_REWRITE_HOSTS", "127.0.0.1");
                    cmd.env("WEBPIPE_GITHUB_RAW_HOST", &raw_host);
                    cmd.env("WEBPIPE_GITHUB_REWRITE_BRANCHES", "main");
                }),
            )?)
            .await?;

        let tools = service.list_tools(Default::default()).await?;
        let names: BTreeSet<String> = tools
            .tools
            .iter()
            .map(|t| t.name.clone().into_owned())
            .collect();
        assert!(names.contains("web_extract"), "missing web_extract tool");

        let repo_url = format!("http://{addr}/owner/repo");
        let v = call(
            &service,
            "web_extract",
            serde_json::json!({
                "url": repo_url,
                "fetch_backend": "local",
                "timeout_ms": 10_000,
                "max_bytes": 500_000,
                "max_chars": 10_000,
                "query": "Hello README",
                "top_chunks": 3,
                "max_chunk_chars": 300,
                "include_text": false,
                "include_links": false,
                "include_structure": false
            }),
        )
        .await;

        assert_eq!(v["ok"].as_bool(), Some(true));
        assert!(
            v["final_url"]
                .as_str()
                .unwrap_or("")
                .contains("/owner/repo/main/README.md"),
            "expected final_url to be raw README: {}",
            v["final_url"]
        );
        let empty = vec![];
        let codes: Vec<&str> = v["warning_codes"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|x| x.as_str())
            .collect();
        assert!(
            codes
                .iter()
                .any(|c| *c == "github_repo_rewritten_to_raw_readme"),
            "expected github rewrite warning; got {codes:?}"
        );
        assert!(v.get("attempts").is_some(), "expected attempts object");

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("github rewrite contract");
}
