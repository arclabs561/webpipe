#[test]
fn webpipe_doctor_contract_json_and_bool_flags() {
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");

    // Critical contract: allow explicit `--check-stdio=false` (clap ArgAction::Set),
    // and still emit well-formed JSON with stable keys.
    let out = std::process::Command::new(bin)
        .args(["doctor", "--check-stdio=false", "--timeout-ms", "1"])
        // Ensure we don't accidentally inherit keys from the environment.
        .env_remove("WEBPIPE_BRAVE_API_KEY")
        .env_remove("BRAVE_SEARCH_API_KEY")
        .env_remove("WEBPIPE_TAVILY_API_KEY")
        .env_remove("TAVILY_API_KEY")
        .env_remove("WEBPIPE_FIRECRAWL_API_KEY")
        .env_remove("FIRECRAWL_API_KEY")
        .env_remove("WEBPIPE_PERPLEXITY_API_KEY")
        .env_remove("PERPLEXITY_API_KEY")
        .output()
        .expect("run webpipe doctor");

    assert!(out.status.success(), "webpipe doctor failed");
    let s = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&s).expect("parse doctor json");

    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert_eq!(v["name"].as_str(), Some("webpipe"));
    assert!(!v["version"].as_str().unwrap_or("").is_empty());
    assert!(v.get("elapsed_ms").is_some());
    assert_eq!(
        v["features"]["stdio"].as_bool(),
        Some(cfg!(feature = "stdio"))
    );

    // Config surface should be present and booleans-only for secrets.
    assert!(v["configured"]["providers"]["brave"].is_boolean());
    assert!(v["configured"]["providers"]["tavily"].is_boolean());
    assert!(v["configured"]["remote_fetch"]["firecrawl"].is_boolean());
    assert!(v["configured"]["llm"]["perplexity"].is_boolean());
    assert!(!v["configured"]["cache_dir"].as_str().unwrap_or("").is_empty());

    // Check list should exist and include the stdio handshake check with skipped=true.
    let checks = v["checks"].as_array().expect("checks array");
    let handshake = checks
        .iter()
        .find(|c| c["name"].as_str() == Some("mcp_stdio_handshake"))
        .expect("mcp_stdio_handshake check");
    assert_eq!(handshake["skipped"].as_bool(), Some(true));
    assert_eq!(handshake["ok"].as_bool(), Some(true));
    assert!(handshake.get("elapsed_ms").is_some());
    assert!(handshake.get("error").is_some());
}
