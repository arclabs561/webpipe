use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn url_selection_mode_query_rank_is_accepted() {
    // This is a CLI-level contract test: we don't assert which URLs get picked (that can change
    // as heuristics evolve), only that the flag is accepted and the artifact is well-formed.
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("eval.json");
    let urls_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("urls.txt");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("webpipe"));
    cmd.args([
        "eval-search-extract",
        "--urls-file",
        urls_file.to_str().unwrap(),
        "--query",
        "mcp stdio transport",
        "--url-selection-mode",
        "query_rank",
        "--max-urls",
        "3",
        "--top-chunks",
        "3",
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
    assert_eq!(
        v["inputs"]["url_selection_mode"].as_str(),
        Some("query_rank")
    );
}
