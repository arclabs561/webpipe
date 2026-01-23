use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn webpipe_eval_matrix_score_contract_stubbed_localhost() {
    // Reuse the same kind of localhost stub server as eval-matrix.
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
    let matrix = tmp.path().join("matrix.jsonl");
    let score = tmp.path().join("score.json");

    let queries_json =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/e2e_queries_v1.json");
    let qrels_json =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/e2e_qrels_v1.json");

    // Produce the matrix artifact.
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
        matrix.to_str().unwrap(),
        "--now-epoch-s",
        "1700000000",
    ]);
    cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
    cmd.env("WEBPIPE_SEARXNG_ENDPOINT", &searxng_endpoint);
    cmd.assert().success();

    // Score it.
    let mut score_cmd = Command::new(env!("CARGO_BIN_EXE_webpipe"));
    score_cmd.args([
        "eval-matrix-score",
        "--matrix-artifact",
        matrix.to_str().unwrap(),
        "--qrels",
        qrels_json.to_str().unwrap(),
        "--out",
        score.to_str().unwrap(),
        "--now-epoch-s",
        "1700000001",
    ]);
    score_cmd.assert()
        .success()
        .stdout(predicate::str::contains("score.json"));

    let raw = std::fs::read_to_string(&score).expect("score exists");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert_eq!(v["kind"].as_str(), Some("webpipe_eval_matrix_score"));
    assert!(v["per_query"].is_array());
    assert!(v["totals"]["cases"]["offline_urls"]["ok"].is_number());
}

