use axum::extract::State;
use axum::http::StatusCode;
use axum::{routing::get, Router};
use rmcp::{
    model::CallToolRequestParam,
    service::{RoleClient, RunningService, ServiceExt},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;

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

#[test]
fn repo_ingest_fetches_bounded_files_and_combines_text() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let readme = "# Hello\n\nThis is the README.\n";
        let lib_rs = "pub fn hello() -> &'static str { \"hello\" }\n";
        let api_repo = serde_json::json!({"default_branch":"main"});
        let api_tree = serde_json::json!({
            "tree": [
                {"path":"README.md","type":"blob","size": 30},
                {"path":"src/lib.rs","type":"blob","size": 45},
                {"path":"assets/logo.png","type":"blob","size": 9999}
            ]
        });

        let api_repo_s = api_repo.to_string();
        let api_tree_s = api_tree.to_string();
        let readme_s = readme.to_string();
        let lib_s = lib_rs.to_string();

        let app = Router::new()
            // GitHub-ish API endpoints
            .route(
                "/api/repos/owner/repo",
                get(move || {
                    let body = api_repo_s.clone();
                    async move { ([("content-type", "application/json")], body) }
                }),
            )
            .route(
                "/api/repos/owner/repo/git/trees/main",
                get(move || {
                    let body = api_tree_s.clone();
                    async move { ([("content-type", "application/json")], body) }
                }),
            )
            // Raw file endpoints
            .route(
                "/owner/repo/main/README.md",
                get(move || {
                    let body = readme_s.clone();
                    async move { ([("content-type", "text/plain")], body) }
                }),
            )
            .route(
                "/owner/repo/main/src/lib.rs",
                get(move || {
                    let body = lib_s.clone();
                    async move { ([("content-type", "text/plain")], body) }
                }),
            );
        let addr = serve(app).await;

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let api_base = format!("http://{addr}/api");
        let raw_host = format!("127.0.0.1:{}", addr.port());
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    // repo_ingest is a debug-only tool; enable full surface for contract tests.
                    cmd.env("WEBPIPE_MCP_TOOLSET", "debug");
                    cmd.env(
                        "WEBPIPE_CACHE_DIR",
                        std::env::temp_dir().join("webpipe-repo-ingest-cache"),
                    );
                    cmd.env("WEBPIPE_GITHUB_API_BASE", &api_base);
                    cmd.env("WEBPIPE_GITHUB_RAW_HOST", &raw_host);
                }),
            )?)
            .await?;

        let tools = service.list_tools(Default::default()).await?;
        let names: BTreeSet<String> = tools
            .tools
            .iter()
            .map(|t| t.name.clone().into_owned())
            .collect();
        assert!(names.contains("repo_ingest"), "missing repo_ingest tool");

        let v = call(
            &service,
            "repo_ingest",
            serde_json::json!({
                "repo_url": "http://github.com/owner/repo",
                "reference": "main",
                "max_files": 10,
                "max_total_bytes": 200000,
                "max_file_bytes": 200000,
                "timeout_ms": 10000,
                "include_combined_text": true
            }),
        )
        .await;

        assert_eq!(v["ok"].as_bool(), Some(true));
        let files = v["files"].as_array().expect("files");
        assert!(files.iter().any(|f| f
            .get("url")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .contains("README.md")));
        assert!(files.iter().any(|f| f
            .get("url")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .contains("src/lib.rs")));
        let combined = v["combined_text"].as_str().unwrap_or("");
        assert!(combined.contains("This is the README"));
        assert!(combined.contains("pub fn hello"));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("repo ingest contract");
}

#[test]
fn repo_ingest_private_like_succeeds_with_token_and_api_contents_fallback() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let readme = "# Hello\n\nThis is the README.\n";
        let lib_rs = "pub fn hello() -> &'static str { \"hello\" }\n";
        let api_repo = serde_json::json!({"default_branch":"main"});
        let api_tree = serde_json::json!({
            "tree": [
                {"path":"README.md","type":"blob","size": 30},
                {"path":"src/lib.rs","type":"blob","size": 45}
            ]
        });

        #[derive(Clone)]
        struct S {
            token: String,
            api_repo_s: String,
            api_tree_s: String,
            readme_s: String,
            lib_s: String,
        }
        let st = Arc::new(S {
            token: "test-token".to_string(),
            api_repo_s: api_repo.to_string(),
            api_tree_s: api_tree.to_string(),
            readme_s: readme.to_string(),
            lib_s: lib_rs.to_string(),
        });

        // API requires Authorization. Raw host routes are intentionally absent to force fallback.
        let app = Router::new()
            .route(
                "/api/repos/owner/repo",
                get(|State(st): State<Arc<S>>, req: axum::http::Request<axum::body::Body>| async move {
                    let auth = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    if auth != format!("Bearer {}", st.token) {
                        return (StatusCode::UNAUTHORIZED, "nope".to_string());
                    }
                    (StatusCode::OK, st.api_repo_s.clone())
                }),
            )
            .route(
                "/api/repos/owner/repo/git/trees/main",
                get(|State(st): State<Arc<S>>, req: axum::http::Request<axum::body::Body>| async move {
                    let auth = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    if auth != format!("Bearer {}", st.token) {
                        return (StatusCode::UNAUTHORIZED, "nope".to_string());
                    }
                    (StatusCode::OK, st.api_tree_s.clone())
                }),
            )
            .route(
                "/api/repos/owner/repo/contents/README.md",
                get(|State(st): State<Arc<S>>, req: axum::http::Request<axum::body::Body>| async move {
                    let auth = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    if auth != format!("Bearer {}", st.token) {
                        return (StatusCode::UNAUTHORIZED, "nope".to_string());
                    }
                    (StatusCode::OK, st.readme_s.clone())
                }),
            )
            .route(
                "/api/repos/owner/repo/contents/src/lib.rs",
                get(|State(st): State<Arc<S>>, req: axum::http::Request<axum::body::Body>| async move {
                    let auth = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    if auth != format!("Bearer {}", st.token) {
                        return (StatusCode::UNAUTHORIZED, "nope".to_string());
                    }
                    (StatusCode::OK, st.lib_s.clone())
                }),
            )
            .with_state(st.clone());
        let addr = serve(app).await;

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let api_base = format!("http://{addr}/api");
        // Raw host points to an address with *no* raw routes in the server (so raw-host fetch fails).
        let raw_host = format!("127.0.0.1:{}", addr.port());
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    // repo_ingest is a debug-only tool; enable full surface for contract tests.
                    cmd.env("WEBPIPE_MCP_TOOLSET", "debug");
                    cmd.env(
                        "WEBPIPE_CACHE_DIR",
                        std::env::temp_dir().join("webpipe-repo-ingest-cache-private"),
                    );
                    cmd.env("WEBPIPE_GITHUB_API_BASE", &api_base);
                    cmd.env("WEBPIPE_GITHUB_RAW_HOST", &raw_host);
                    cmd.env("WEBPIPE_GITHUB_TOKEN", "test-token");
                }),
            )?)
            .await?;

        let v = call(
            &service,
            "repo_ingest",
            serde_json::json!({
                "repo_url": "http://github.com/owner/repo",
                "reference": "main",
                "max_files": 10,
                "max_total_bytes": 200000,
                "max_file_bytes": 200000,
                "timeout_ms": 10000,
                "include_combined_text": true
            }),
        )
        .await;

        assert_eq!(v["ok"].as_bool(), Some(true));
        assert_eq!(v["request"]["github_token_configured"].as_bool(), Some(true));

        let files = v["files"].as_array().expect("files");
        // Ensure our fallback was used (api_contents_raw should appear in attempts).
        let mut saw_api = false;
        for f in files {
            if let Some(atts) = f.get("attempts").and_then(|x| x.as_array()) {
                if atts.iter().any(|a| a.get("kind").and_then(|k| k.as_str()) == Some("api_contents_raw")) {
                    saw_api = true;
                }
            }
        }
        assert!(saw_api, "expected api_contents_raw fallback attempts in files");

        let combined = v["combined_text"].as_str().unwrap_or("");
        assert!(combined.contains("Repository: owner/repo"));
        assert!(combined.contains("Directory structure:"));
        assert!(
            combined.contains("FILE: README.md"),
            "missing README file delimiter; combined_text was:\n{combined}\n\nfiles:\n{}\n\nwarnings:\n{}",
            v["files"],
            v["warnings"]
        );
        assert!(
            combined.contains("FILE: src/lib.rs"),
            "missing lib.rs file delimiter; combined_text was:\n{combined}"
        );
        assert!(combined.contains("This is the README"));
        assert!(combined.contains("pub fn hello"));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("repo ingest private-like contract");
}
