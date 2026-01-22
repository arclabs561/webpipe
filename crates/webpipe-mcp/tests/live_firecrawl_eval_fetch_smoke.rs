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
fn webpipe_live_firecrawl_eval_fetch_smoke_opt_in() {
    // This test makes real paid network calls. Opt-in only.
    if std::env::var("WEBPIPE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skipping: set WEBPIPE_LIVE=1 to run live firecrawl eval-fetch smoke");
        return;
    }

    let firecrawl_key = match get_env_any(&["WEBPIPE_FIRECRAWL_API_KEY", "FIRECRAWL_API_KEY"]) {
        Some(k) => k,
        None => {
            eprintln!("skipping: missing WEBPIPE_FIRECRAWL_API_KEY (or FIRECRAWL_API_KEY)");
            return;
        }
    };

    // Keep Tavily/Brave irrelevant here; this is fetch-only.
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let out = tempfile::NamedTempFile::new().expect("tmp out");
    let out_path = out.path().to_path_buf();

    // Use a stable URL with predictable content. (Still a live network call.)
    let url = "https://example.com/";

    let mut cmd = assert_cmd::Command::new(bin);
    cmd.env("WEBPIPE_FIRECRAWL_API_KEY", firecrawl_key);
    // Keep artifacts bounded and exercise truncation + extraction paths.
    cmd.args([
        "eval-fetch",
        "--fetchers",
        "local,firecrawl",
        "--url",
        url,
        "--timeout-ms",
        "30000",
        "--max-bytes",
        "500000",
        "--max-text-chars",
        "200",
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
    assert!(v["inputs"]["fetchers"].is_array());
    assert_eq!(v["inputs"]["extract"].as_bool(), Some(true));
    assert_eq!(v["inputs"]["max_text_chars"].as_u64(), Some(200));

    let runs = v["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["url"].as_str(), Some(url));

    let fetchers = runs[0]["fetchers"].as_array().expect("fetchers array");
    assert!(fetchers.iter().any(|f| f["name"].as_str() == Some("local")));
    assert!(fetchers
        .iter()
        .any(|f| f["name"].as_str() == Some("firecrawl")));

    // At least one should succeed; ideally both.
    assert!(fetchers.iter().any(|f| f["ok"].as_bool() == Some(true)));

    // Since we forced max_text_chars=200, any successful entry should be <= 200 chars.
    for f in fetchers.iter().filter(|f| f["ok"].as_bool() == Some(true)) {
        let text = f["text"].as_str().unwrap_or("");
        assert!(text.chars().count() <= 200);
    }
}
