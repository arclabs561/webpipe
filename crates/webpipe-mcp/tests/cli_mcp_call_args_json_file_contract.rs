use assert_cmd::cargo::cargo_bin_cmd;

#[test]
fn webpipe_mcp_call_supports_args_json_file_array_and_prints_timing() {
    // This is a pure local/self-contained contract test:
    // - tool is webpipe_meta (no network)
    // - args are empty objects
    // - file contains an array => two calls in one spawned MCP server
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("args.json");
    std::fs::write(&p, "[{},{}]\n").unwrap();

    let mut cmd = cargo_bin_cmd!("webpipe");
    cmd.args([
        "mcp-call",
        "--tool",
        "webpipe_meta",
        "--args-json-file",
        p.to_str().unwrap(),
        "--timeout-ms",
        "5000",
    ]);
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("## Timing"))
        .stdout(predicates::str::contains("calls: 2"));
}
