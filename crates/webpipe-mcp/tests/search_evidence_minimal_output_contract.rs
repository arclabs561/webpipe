/// Contract: search_evidence minimal_output field.
///
/// - minimal_output=false (default): full response tree (mode, results, request, provider, …)
/// - minimal_output=true: compact set only (ok, kind, schema_version, elapsed_ms, top_chunks,
///   and optionally warning_codes/warning_hints when present)
///
/// This test runs a local HTTP server so it requires no external network.
#[test]
fn search_evidence_minimal_output_contract() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use axum::{http::header, routing::get, Router};
        use rmcp::{
            model::CallToolRequestParam,
            service::ServiceExt,
            transport::{ConfigureCommandExt, TokioChildProcess},
        };
        use std::{collections::BTreeSet, net::SocketAddr};

        // Minimal local HTTP server so we can fetch + extract without touching the network.
        let app = Router::new().route(
            "/doc",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html")],
                    "<html><body><main><h1>Test</h1><p>minimal output contract token xyz</p></main></body></html>",
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move { axum::serve(listener, app).await.expect("axum serve") });
        let doc_url = format!("http://{addr}/doc");

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                }),
            )?)
            .await?;

        let base_args = serde_json::json!({
            "urls": [doc_url],
            "query": "minimal output contract",
            "url_selection_mode": "preserve",
            "fetch_backend": "local",
            "max_urls": 1,
            "timeout_ms": 5000,
            "top_chunks": 2,
            "max_chunk_chars": 150,
            "compact": true,
            "agentic": false,
            "include_text": false,
            "include_links": false,
            "include_structure": false,
        });

        // ── Test 1: minimal_output=false (default) ────────────────────────────────
        let full_args = {
            let mut m = base_args.as_object().cloned().unwrap();
            m.insert("minimal_output".into(), serde_json::json!(false));
            m
        };
        let full_r = service
            .call_tool(CallToolRequestParam {
                name: "search_evidence".into(),
                arguments: Some(full_args),
            })
            .await?;
        let full_sc = full_r.structured_content.clone().unwrap();
        assert_eq!(full_sc["ok"].as_bool(), Some(true), "full: ok={full_sc}");
        assert_eq!(
            full_sc["kind"].as_str(),
            Some("web_search_extract"),
            "full: kind"
        );
        // Full response must include the verbose fields.
        assert!(
            full_sc.get("results").is_some(),
            "full: expected results[] to be present"
        );
        assert!(
            full_sc.get("request").is_some(),
            "full: expected request to be present"
        );
        assert!(
            full_sc.get("mode").is_some(),
            "full: expected mode to be present"
        );
        assert!(
            full_sc.get("top_chunks").is_some(),
            "full: expected top_chunks to be present"
        );

        // ── Test 2: minimal_output=true ───────────────────────────────────────────
        let minimal_args = {
            let mut m = base_args.as_object().cloned().unwrap();
            m.insert("minimal_output".into(), serde_json::json!(true));
            m
        };
        let min_r = service
            .call_tool(CallToolRequestParam {
                name: "search_evidence".into(),
                arguments: Some(minimal_args),
            })
            .await?;
        let min_sc = min_r.structured_content.clone().unwrap();
        assert_eq!(min_sc["ok"].as_bool(), Some(true), "minimal: ok={min_sc}");
        assert_eq!(
            min_sc["kind"].as_str(),
            Some("web_search_extract"),
            "minimal: kind must be preserved"
        );
        assert!(
            min_sc.get("top_chunks").is_some(),
            "minimal: top_chunks must be present"
        );

        // Verbose fields must be absent.
        let stripped_fields = ["results", "request", "mode", "provider", "query",
                               "backend_provider", "url_count_in", "url_count_used",
                               "search", "agentic"];
        for field in stripped_fields {
            assert!(
                min_sc.get(field).is_none(),
                "minimal: field '{field}' should be stripped but is present; sc={min_sc}"
            );
        }

        // Only the known allowed keys should be present.
        let allowed: BTreeSet<&str> = [
            "ok", "top_chunks", "warning_codes", "warning_hints",
            "schema_version", "kind", "elapsed_ms", "error",
        ].iter().copied().collect();
        if let Some(obj) = min_sc.as_object() {
            for key in obj.keys() {
                assert!(
                    allowed.contains(key.as_str()),
                    "minimal: unexpected key '{key}' in compact response"
                );
            }
        }

        // ── Test 3: response size sanity ──────────────────────────────────────────
        let full_size = serde_json::to_string(&full_sc).unwrap().len();
        let min_size = serde_json::to_string(&min_sc).unwrap().len();
        assert!(
            min_size < full_size / 2,
            "minimal response ({min_size}B) should be significantly smaller than full ({full_size}B)"
        );

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("minimal_output contract");
}
