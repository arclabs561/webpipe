#[test]
fn webpipe_version_text_output_contract() {
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let out = std::process::Command::new(bin)
        .args(["version", "--output", "text"])
        .output()
        .expect("run webpipe version --output text");

    assert!(out.status.success(), "webpipe version failed");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.trim_start().starts_with("webpipe "),
        "expected text output to start with `webpipe `"
    );
}

#[test]
fn webpipe_doctor_text_output_contract() {
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let out = std::process::Command::new(bin)
        .args([
            "doctor",
            "--output",
            "text",
            "--check-stdio=false",
            "--timeout-ms",
            "1",
        ])
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
        .expect("run webpipe doctor --output text");

    assert!(out.status.success(), "webpipe doctor failed");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("webpipe "),
        "expected doctor text output to mention webpipe"
    );
    assert!(s.contains("checks:"), "expected checks summary");
}
