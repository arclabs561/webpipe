fn get_env_any(keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            if !v.trim().is_empty() {
                return Some(v);
            }
        }
    }
    None
}

#[test]
fn webpipe_live_arxiv_pdf_smoke_opt_in() {
    // This test makes a real network call (arXiv PDF). Opt-in only.
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping: set WEBPIPE_LIVE=1 to run live arxiv pdf smoke");
        return;
    }

    // Ensure we *don't* accidentally rely on paid search keys here.
    if get_env_any(&[
        "WEBPIPE_TAVILY_API_KEY",
        "TAVILY_API_KEY",
        "WEBPIPE_BRAVE_API_KEY",
        "BRAVE_SEARCH_API_KEY",
    ])
    .is_some()
    {
        eprintln!("note: search keys are set, but this test only uses local fetch");
    }

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let out = tempfile::NamedTempFile::new().expect("tmp out");
    let out_path = out.path().to_path_buf();

    // A stable, well-known paper PDF.
    let url = "https://arxiv.org/pdf/1706.03762.pdf";

    let mut cmd = assert_cmd::Command::new(bin);
    cmd.args([
        "eval-fetch",
        "--fetchers",
        "local",
        "--url",
        url,
        "--timeout-ms",
        "30000",
        "--max-bytes",
        "10000000",
        "--max-text-chars",
        "4000",
        "--extract",
        "--extract-width",
        "80",
        "--out",
    ]);
    cmd.arg(&out_path);

    cmd.assert().success();

    let s = std::fs::read_to_string(&out_path).expect("read output artifact");
    let v: serde_json::Value = serde_json::from_str(&s).expect("parse output json");

    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert_eq!(v["kind"].as_str(), Some("eval_fetch"));

    let runs = v["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["url"].as_str(), Some(url));

    let fetchers = runs[0]["fetchers"].as_array().expect("fetchers array");
    assert_eq!(fetchers.len(), 1);
    let f = &fetchers[0];
    assert_eq!(f["name"].as_str(), Some("local"));
    assert_eq!(f["ok"].as_bool(), Some(true));

    let extracted = f["extracted"].as_object().expect("extracted obj");
    assert_eq!(
        extracted.get("engine").and_then(|v| v.as_str()),
        Some("pdf-extract")
    );
    let text = extracted.get("text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(text.chars().count() >= 500);
}
