#[test]
fn webpipe_eval_search_contract_no_keys_no_network() {
    // This test is designed to run without any API keys configured:
    // it should still produce a well-formed JSON artifact with ok=false
    // per provider and structured error objects.

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let out = tempfile::NamedTempFile::new().expect("tmp out");
    let out_path = out.path().to_path_buf();

    let status = std::process::Command::new(bin)
        .args([
            "eval-search",
            "--providers",
            "brave,tavily",
            "--query",
            "contract test query",
            "--max-results",
            "3",
            "--now-epoch-s",
            "1700000000",
            "--out",
        ])
        .arg(&out_path)
        // Ensure we don't accidentally inherit keys from the environment.
        .env_remove("WEBPIPE_BRAVE_API_KEY")
        .env_remove("BRAVE_SEARCH_API_KEY")
        .env_remove("WEBPIPE_TAVILY_API_KEY")
        .env_remove("TAVILY_API_KEY")
        .status()
        .expect("run eval-search");

    assert!(
        status.success(),
        "eval-search should succeed even without keys"
    );

    let s = std::fs::read_to_string(&out_path).expect("read artifact");
    let v: serde_json::Value = serde_json::from_str(&s).expect("parse artifact json");

    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert_eq!(v["kind"].as_str(), Some("eval_search"));
    assert_eq!(v["generated_at_epoch_s"].as_u64(), Some(1700000000));
    assert_eq!(v["inputs"]["max_results"].as_u64(), Some(3));
    assert_eq!(v["inputs"]["query_count"].as_u64(), Some(1));

    let runs = v["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["query"].as_str(), Some("contract test query"));

    let per = runs[0]["providers"].as_array().expect("providers array");
    assert_eq!(per.len(), 2);

    for p in per {
        let name = p["name"].as_str().unwrap_or("");
        assert!(name == "brave" || name == "tavily");
        let result = &p["result"];
        assert_eq!(result["schema_version"].as_u64(), Some(1));
        assert_eq!(result["ok"].as_bool(), Some(false));
        assert_eq!(result["provider"].as_str(), Some(name));
        assert_eq!(result["query"].as_str(), Some("contract test query"));
        assert_eq!(result["max_results"].as_u64(), Some(3));
        assert_eq!(result["error"]["code"].as_str(), Some("not_configured"));
        assert!(result["error"]["message"].as_str().unwrap_or("").len() > 0);
    }
}
