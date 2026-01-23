use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn webpipe_eval_matrix_contract_stubbed_localhost() {
    // Local stub server for:
    // - fetch targets (HTML pages)
    // - SearXNG provider (/search)
    use axum::{routing::get, Router};
    use std::net::SocketAddr;

    let rt = tokio::runtime::Runtime::new().expect("rt");
    let addr: SocketAddr = rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        let base_for_search = base.clone();

        let app = Router::new()
            .route(
                "/docs",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                        "<html><body><h1>Docs</h1><p>See route handlers.</p></body></html>",
                    )
                }),
            )
            .route(
                "/docs/app/getting-started/route-handlers",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                        "<html><body><h1>Route handlers</h1><p>Use GET/POST handlers.</p></body></html>",
                    )
                }),
            )
            .route(
                "/install",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                        "<html><body><h1>Install</h1><p>Linux install instructions.</p></body></html>",
                    )
                }),
            )
            .route(
                "/install/linux",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                        "<html><body><h1>Install on Linux</h1><p>Use apt.</p></body></html>",
                    )
                }),
            )
            // SearXNG-like: GET /searxng/search?format=json -> { results: [{url,title,content}] }
            .route(
                "/searxng/search",
                get(move || {
                    let base = base_for_search.clone();
                    async move {
                        axum::Json(serde_json::json!({
                            "results": [
                                {"url": format!("{}/docs/app/getting-started/route-handlers", base), "title":"Route handlers", "content":"GET/POST handlers"},
                                {"url": format!("{}/install/linux", base), "title":"Install on Linux", "content":"Use apt."}
                            ]
                        }))
                    }
                }),
            );

        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });
        addr
    });

    let base = format!("http://{addr}");
    let searxng_endpoint = format!("{base}/searxng");

    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache");
    let out = tmp.path().join("matrix.jsonl");

    let queries_json =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/e2e_queries_v1.json");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_webpipe"));
    cmd.args([
        "eval-matrix",
        "--queries-json",
        queries_json.to_str().unwrap(),
        "--base-url",
        &base,
        "--provider",
        "searxng",
        "--max-results",
        "1",
        "--max-urls",
        "1",
        "--top-chunks",
        "2",
        "--max-chunk-chars",
        "200",
        "--out",
        out.to_str().unwrap(),
        "--now-epoch-s",
        "1700000000",
    ]);
    cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
    cmd.env("WEBPIPE_SEARXNG_ENDPOINT", &searxng_endpoint);
    // Ensure we don't accidentally use real keys on dev machines.
    cmd.env_remove("WEBPIPE_BRAVE_API_KEY");
    cmd.env_remove("BRAVE_SEARCH_API_KEY");
    cmd.env_remove("WEBPIPE_TAVILY_API_KEY");
    cmd.env_remove("TAVILY_API_KEY");
    cmd.env_remove("WEBPIPE_FIRECRAWL_API_KEY");
    cmd.env_remove("FIRECRAWL_API_KEY");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("matrix.jsonl"));

    let raw = std::fs::read_to_string(&out).expect("out exists");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(lines.len() >= 3, "expected multiple JSONL rows");
    let v0: serde_json::Value = serde_json::from_str(lines[0]).expect("first row json");
    assert_eq!(v0["schema_version"].as_u64(), Some(1));
    assert_eq!(v0["kind"].as_str(), Some("webpipe_eval_matrix_row"));
    assert!(v0["result"].is_object());
    assert_eq!(v0["result"]["kind"].as_str(), Some("web_search_extract"));
}

