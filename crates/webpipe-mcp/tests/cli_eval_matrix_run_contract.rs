use assert_cmd::prelude::*;
use std::process::Command;

#[test]
fn webpipe_eval_matrix_run_contract_stubbed_localhost() {
    // Local stub server for fetch targets + SearXNG.
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
                "/docs/app/getting-started/route-handlers",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                        "<html><body><h1>Route handlers</h1><p>Use GET/POST handlers.</p></body></html>",
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
    let out_dir = tmp.path().join("out");

    let queries_json =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/e2e_queries_v1.json");
    let qrels_json =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/e2e_qrels_v1.json");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_webpipe"));
    cmd.args([
        "eval-matrix-run",
        "--queries-json",
        queries_json.to_str().unwrap(),
        "--qrels",
        qrels_json.to_str().unwrap(),
        "--base-url",
        &base,
        "--provider",
        "searxng",
        "--out-dir",
        out_dir.to_str().unwrap(),
        "--now-epoch-s",
        "1700000000",
    ]);
    cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
    cmd.env("WEBPIPE_SEARXNG_ENDPOINT", &searxng_endpoint);

    let assert = cmd.assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(stdout.contains("webpipe-eval-matrix-judge-run-1700000000.json"));

    let judge_path = out_dir.join("webpipe-eval-matrix-judge-run-1700000000.json");
    let raw = std::fs::read_to_string(&judge_path).expect("judge exists");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(v["kind"].as_str(), Some("webpipe_eval_matrix_judge"));

    let manifest_path = out_dir.join("webpipe-eval-matrix-manifest-run-1700000000.json");
    let raw = std::fs::read_to_string(&manifest_path).expect("manifest exists");
    let mv: serde_json::Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(
        mv["kind"].as_str(),
        Some("webpipe_eval_matrix_run_manifest")
    );
    assert_eq!(
        mv["artifacts"]["judge"].as_str(),
        Some(judge_path.to_str().unwrap())
    );
}

