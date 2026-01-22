use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn webpipe_eval_search_extract_contract_urls_mode_offlineish() {
    // Deterministic local fixture server.
    use axum::{routing::get, Router};
    use std::net::SocketAddr;
    let app = Router::new().route(
        "/",
        get(|| async {
            (
                [(axum::http::header::CONTENT_TYPE, "text/html")],
                "<html><body><h1>Hello</h1><p>world</p></body></html>",
            )
        }),
    );
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let (addr, _guard) = rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });
        (addr, ())
    });
    let url = format!("http://{}/", addr);

    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("eval.json");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_webpipe"));
    cmd.args([
        "eval-search-extract",
        "--provider",
        "auto",
        "--auto-mode",
        "fallback",
        "--selection-mode",
        "pareto",
        "--fetch-backend",
        "local",
        "--url",
        &url,
        "--max-urls",
        "1",
        "--max-results",
        "1",
        "--top-chunks",
        "2",
        "--include-links",
        "false",
        "--include-text",
        "false",
        "--out",
        out.to_str().unwrap(),
        "--now-epoch-s",
        "1700000000",
    ]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("eval.json"));

    let s = std::fs::read_to_string(&out).expect("out exists");
    let v: serde_json::Value = serde_json::from_str(&s).expect("json");
    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert_eq!(v["kind"].as_str(), Some("eval_search_extract"));
    assert_eq!(v["generated_at_epoch_s"].as_u64(), Some(1700000000));
    assert!(!v["runs"].as_array().unwrap().is_empty());
}
