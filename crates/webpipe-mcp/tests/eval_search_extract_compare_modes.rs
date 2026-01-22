use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn eval_search_extract_compare_modes_requires_query_in_url_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("eval.json");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_webpipe"));
    cmd.args([
        "eval-search-extract",
        "--url",
        "https://example.com/",
        "--compare-selection-modes",
        "score,pareto",
        "--out",
        out.to_str().unwrap(),
        "--now-epoch-s",
        "1700000000",
    ]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("requires a non-empty query"));
}

#[test]
fn eval_search_extract_compare_modes_is_capped_to_two() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("eval.json");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_webpipe"));
    cmd.args([
        "eval-search-extract",
        "--query",
        "alpha beta",
        "--compare-selection-modes",
        "score,pareto,score",
        "--out",
        out.to_str().unwrap(),
        "--now-epoch-s",
        "1700000000",
    ]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("at most 2 modes"));
}
