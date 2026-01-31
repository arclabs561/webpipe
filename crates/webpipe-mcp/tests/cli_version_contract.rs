#[test]
fn webpipe_version_contract() {
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let out = std::process::Command::new(bin)
        .args(["version"])
        // Disable `.env` autoload so this contract stays hermetic.
        .env("WEBPIPE_DOTENV", "0")
        .output()
        .expect("run webpipe version");

    assert!(out.status.success(), "webpipe version failed");
    let s = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&s).expect("parse version json");

    assert_eq!(v["schema_version"].as_u64(), Some(2));
    assert_eq!(v["name"].as_str(), Some("webpipe"));
    assert!(!v["version"].as_str().unwrap_or("").is_empty());
}
