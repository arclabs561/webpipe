use axum::{http::header, routing::get, Router};
use rmcp::{
    model::CallToolRequestParam,
    service::{RoleClient, RunningService, ServiceExt},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::net::SocketAddr;

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn web_cache_search_extract_finds_warmed_cache_entry() {
    // Local server with distinctive content.
    let app = Router::new().route(
        "/doc",
        get(|| async {
            (
                [(header::CONTENT_TYPE, "text/html")],
                "<html><body><h1>Doc</h1><p>quuxword unique token</p></body></html>",
            )
        }),
    );
    let addr = serve(app).await;
    let url = format!("http://{addr}/doc");

    // Spawn MCP server with a temp cache dir.
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache");
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                // Disable `.env` autoload so this test stays hermetic.
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    async fn call(
        service: &RunningService<RoleClient, ()>,
        name: &'static str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        let r = service
            .call_tool(CallToolRequestParam {
                name: name.to_string().into(),
                arguments: Some(args.as_object().cloned().unwrap()),
            })
            .await
            .expect("call_tool");
        r.structured_content
            .clone()
            .expect("expected structured_content")
    }

    // 1) Warm cache via web_fetch.
    let v_fetch = call(
        &service,
        "web_fetch",
        serde_json::json!({
            "url": url,
            "fetch_backend": "local",
            "cache_read": true,
            "cache_write": true,
            "timeout_ms": 5000,
            "max_bytes": 1_000_000,
            "include_text": false
        }),
    )
    .await;
    assert_eq!(v_fetch["ok"].as_bool(), Some(true));

    // 2) Query cache-only search.
    let v = call(
        &service,
        "web_cache_search_extract",
        serde_json::json!({
            "query": "quuxword",
            "max_docs": 50,
            "max_scan_entries": 2000,
            "include_structure": true,
            "semantic_rerank": true,
            "semantic_top_k": 3
        }),
    )
    .await;
    assert_eq!(v["ok"].as_bool(), Some(true));
    // Default mode is compact to reduce duplication.
    assert_eq!(v["request"]["compact"].as_bool(), Some(true));
    let rs = v["results"].as_array().cloned().unwrap_or_default();
    assert!(!rs.is_empty());
    assert!(rs[0]["final_url"].as_str().unwrap_or("").contains("/doc"));
    assert!(rs[0].get("chunks").is_none());
    let chunks = rs[0]["extract"]["chunks"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(!chunks.is_empty());
    assert!(chunks[0]["text"]
        .as_str()
        .unwrap_or("")
        .to_lowercase()
        .contains("quuxword"));
    assert!(rs[0].get("semantic").is_some());
    assert_eq!(
        rs[0]["extract"]["engine"].as_str(),
        rs[0]["extraction_engine"].as_str()
    );
    assert!(rs[0]["extract"]["chunks"].as_array().is_some());
    assert!(rs[0]["extract"]["structure"].as_object().is_some());

    // 3) Verbose mode should retain top-level `chunks` for back-compat / debugging.
    let v_verbose = call(
        &service,
        "web_cache_search_extract",
        serde_json::json!({
            "query": "quuxword",
            "max_docs": 50,
            "max_scan_entries": 2000,
            "include_structure": true,
            "compact": false
        }),
    )
    .await;
    assert_eq!(v_verbose["ok"].as_bool(), Some(true));
    assert_eq!(v_verbose["request"]["compact"].as_bool(), Some(false));
    let rs2 = v_verbose["results"].as_array().cloned().unwrap_or_default();
    assert!(!rs2.is_empty());
    assert!(rs2[0].get("chunks").is_some());
    assert!(rs2[0]["extract"]["chunks"].as_array().is_some());
}

#[tokio::test]
async fn web_cache_search_extract_can_load_persisted_corpus_opt_in() {
    // Local server with distinctive content.
    let app = Router::new().route(
        "/doc",
        get(|| async {
            (
                [(header::CONTENT_TYPE, "text/html")],
                "<html><body><h1>Doc</h1><p>persisttoken unique token</p></body></html>",
            )
        }),
    );
    let addr = serve(app).await;
    let url = format!("http://{addr}/doc");

    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache");
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");

    async fn call(
        service: &RunningService<RoleClient, ()>,
        name: &'static str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        let r = service
            .call_tool(CallToolRequestParam {
                name: name.to_string().into(),
                arguments: Some(args.as_object().cloned().unwrap()),
            })
            .await
            .expect("call_tool");
        r.structured_content
            .clone()
            .expect("expected structured_content")
    }

    // First process: warm cache and write checkpoint.
    {
        let service = ()
            .serve(
                TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                    cmd.env("WEBPIPE_CACHE_SEARCH_PERSIST", "1");
                }))
                .expect("spawn mcp child"),
            )
            .await
            .expect("serve mcp child");

        let v_fetch = call(
            &service,
            "web_fetch",
            serde_json::json!({
                "url": url,
                "fetch_backend": "local",
                "cache_read": true,
                "cache_write": true,
                "timeout_ms": 5000,
                "max_bytes": 1_000_000,
                "include_text": false
            }),
        )
        .await;
        assert_eq!(v_fetch["ok"].as_bool(), Some(true));

        let v = call(
            &service,
            "web_cache_search_extract",
            serde_json::json!({
                "query": "persisttoken",
                "max_docs": 50
            }),
        )
        .await;
        assert_eq!(v["ok"].as_bool(), Some(true));
    }

    // Second process: should load persisted corpus.
    {
        let service = ()
            .serve(
                TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                    cmd.env("WEBPIPE_CACHE_SEARCH_PERSIST", "1");
                }))
                .expect("spawn mcp child"),
            )
            .await
            .expect("serve mcp child");

        let v = call(
            &service,
            "web_cache_search_extract",
            serde_json::json!({
                "query": "persisttoken",
                "max_docs": 50
            }),
        )
        .await;
        assert_eq!(v["ok"].as_bool(), Some(true));
        let warns = v["warnings"].as_array().cloned().unwrap_or_default();
        assert!(warns
            .iter()
            .any(|w| w.as_str() == Some("persisted_corpus_loaded")));
        let rs = v["results"].as_array().cloned().unwrap_or_default();
        assert!(!rs.is_empty());
    }
}
