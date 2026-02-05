#[test]
fn webpipe_mcp_stdio_batch_listings_contract() {
    // Regression test:
    // Some MCP clients (notably Cursor) may batch multiple JSON-RPC requests into a single
    // JSON array message over stdio. The server must not close the transport on such input,
    // and must return complete listings (tools/prompts/resources).
    //
    // This test intentionally uses a raw line-delimited JSON client (not rmcp's client),
    // so it can send a JSON-RPC batch array.

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;

        let mut child = tokio::process::Command::new(bin)
            .args(["mcp-stdio"])
            .env("WEBPIPE_DOTENV", "0")
            .env("WEBPIPE_CACHE_DIR", cache_dir.path())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let mut stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let mut stdout = BufReader::new(stdout).lines();

        // initialize
        stdin
            .write_all(
                (serde_json::json!({
                    "jsonrpc":"2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {},
                        "clientInfo": {"name":"batch-probe","version":"0"}
                    }
                })
                .to_string()
                    + "\n")
                    .as_bytes(),
            )
            .await?;
        stdin.flush().await?;

        let init_line = stdout
            .next_line()
            .await?
            .ok_or("missing initialize response")?;
        let init_v: serde_json::Value = serde_json::from_str(&init_line)?;
        assert_eq!(init_v["id"].as_i64(), Some(1));

        // rmcp expects this *exact* method string for post-init.
        stdin
            .write_all(
                (serde_json::json!({
                    "jsonrpc":"2.0",
                    "method": "notifications/initialized"
                })
                .to_string()
                    + "\n")
                    .as_bytes(),
            )
            .await?;

        // Send a JSON-RPC batch: tools/list + prompts/list + resources/list.
        stdin
            .write_all(
                (serde_json::json!([
                    {"jsonrpc":"2.0","id": 2,"method":"tools/list","params":{}},
                    {"jsonrpc":"2.0","id": 3,"method":"prompts/list","params":{}},
                    {"jsonrpc":"2.0","id": 4,"method":"resources/list","params":{}}
                ])
                .to_string()
                    + "\n")
                    .as_bytes(),
            )
            .await?;
        stdin.flush().await?;

        let mut got_tools: Option<usize> = None;
        let mut got_prompts: Option<usize> = None;
        let mut got_resources: Option<usize> = None;

        // We expect 3 responses (order not guaranteed).
        for _ in 0..10 {
            if got_tools.is_some() && got_prompts.is_some() && got_resources.is_some() {
                break;
            }
            let line = match stdout.next_line().await? {
                Some(l) => l,
                None => break,
            };
            let v: serde_json::Value = serde_json::from_str(&line)?;
            match v["id"].as_i64() {
                Some(2) => {
                    let n = v["result"]["tools"].as_array().map(|a| a.len());
                    got_tools = n;
                }
                Some(3) => {
                    let n = v["result"]["prompts"].as_array().map(|a| a.len());
                    got_prompts = n;
                }
                Some(4) => {
                    let n = v["result"]["resources"].as_array().map(|a| a.len());
                    got_resources = n;
                }
                _ => {}
            }
        }

        assert!(
            got_tools.unwrap_or(0) > 0,
            "expected tools/list to return >0 tools in batch mode"
        );
        assert_eq!(got_prompts, Some(3), "expected 3 prompts in batch mode");
        assert_eq!(got_resources, Some(2), "expected 2 resources in batch mode");

        // Terminate the child.
        let _ = child.kill().await;

        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio batch listings contract");
}
