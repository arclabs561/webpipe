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

fn payload_from_result(r: &rmcp::model::CallToolResult) -> serde_json::Value {
    if let Some(v) = r.structured_content.clone() {
        return v;
    }
    for c in &r.content {
        if let Some(t) = c.as_text() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t.text) {
                return v;
            }
        }
    }
    serde_json::json!({})
}

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
    payload_from_result(&r)
}

#[tokio::test]
async fn search_evidence_text_preview_prefers_top_chunk_when_query_present() {
    // Build a page where the first ~700 chars are irrelevant “nav-like” filler,
    // and the query token occurs only in a later paragraph.
    let nav = "nav ".repeat(250); // ~1000 chars, no query token
    let body = format!(
        "{nav}\n\nRelevant: tardigrade is the query token; this paragraph should be previewed.\n"
    );

    let app = Router::new().route(
        "/doc",
        get({
            let b = body.clone();
            move || {
                let b2 = b.clone();
                async move { ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], b2) }
            }
        }),
    );
    let addr = serve(app).await;
    let url = format!("http://{addr}/doc");

    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache");
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                // Deterministic keyless behavior.
                cmd.env_remove("WEBPIPE_BRAVE_API_KEY");
                cmd.env_remove("BRAVE_SEARCH_API_KEY");
                cmd.env_remove("WEBPIPE_TAVILY_API_KEY");
                cmd.env_remove("TAVILY_API_KEY");
                cmd.env_remove("WEBPIPE_FIRECRAWL_API_KEY");
                cmd.env_remove("FIRECRAWL_API_KEY");
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let v = call(
        &service,
        "search_evidence",
        serde_json::json!({
            "query": "tardigrade",
            "urls": [url],
            "fetch_backend": "local",
            "agentic": false,
            "max_urls": 1,
            "top_chunks": 3,
            "max_chunk_chars": 300,
            "include_text": false,
            "include_links": false,
            "include_structure": false,
            "compact": true,
            "timeout_ms": 5_000,
            "deadline_ms": 10_000,
            "cache_read": false,
            "cache_write": false
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true), "payload={v}");
    let rs = v["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(rs.len(), 1, "results={rs:?} payload={v}");
    let one = &rs[0];
    let preview = one
        .pointer("/extract/text_preview")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let src = one
        .pointer("/extract/text_preview_source")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    assert!(
        preview.to_ascii_lowercase().contains("tardigrade"),
        "expected query token in preview; src={src} preview={preview:?} payload={v}"
    );
    assert_eq!(
        src,
        "top_chunk",
        "expected preview source top_chunk; preview={preview:?} payload={v}"
    );

    service.cancel().await.expect("cancel");
}

#[tokio::test]
async fn search_evidence_text_preview_uses_fallback_chunk_when_query_has_no_overlap() {
    // Build a page where the first ~700 chars are irrelevant “nav-like” filler,
    // and a later paragraph is the only substantive content (but does NOT contain the query).
    let nav = "nav ".repeat(250); // ~1000 chars, no query token
    let content = "Relevant content starts here. This paragraph is long enough to be selected as evidence for humans, even when the query has no overlap.";
    let body = format!("{nav}\n\n{content}\n");

    let app = Router::new().route(
        "/doc",
        get({
            let b = body.clone();
            move || {
                let b2 = b.clone();
                async move { ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], b2) }
            }
        }),
    );
    let addr = serve(app).await;
    let url = format!("http://{addr}/doc");

    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache");
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                // Deterministic keyless behavior.
                cmd.env_remove("WEBPIPE_BRAVE_API_KEY");
                cmd.env_remove("BRAVE_SEARCH_API_KEY");
                cmd.env_remove("WEBPIPE_TAVILY_API_KEY");
                cmd.env_remove("TAVILY_API_KEY");
                cmd.env_remove("WEBPIPE_FIRECRAWL_API_KEY");
                cmd.env_remove("FIRECRAWL_API_KEY");
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let v = call(
        &service,
        "search_evidence",
        serde_json::json!({
            "query": "platypus", // not present in the page
            "urls": [url],
            "fetch_backend": "local",
            "agentic": false,
            "max_urls": 1,
            "top_chunks": 3,
            "max_chunk_chars": 300,
            "include_text": false,
            "include_links": false,
            "include_structure": false,
            "compact": true,
            "timeout_ms": 5_000,
            "deadline_ms": 10_000,
            "cache_read": false,
            "cache_write": false
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true), "payload={v}");
    let rs = v["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(rs.len(), 1, "results={rs:?} payload={v}");
    let one = &rs[0];
    let preview = one
        .pointer("/extract/text_preview")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let src = one
        .pointer("/extract/text_preview_source")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    assert!(
        preview.to_ascii_lowercase().contains("relevant content starts here"),
        "expected substantive content in preview; src={src} preview={preview:?} payload={v}"
    );
    assert_eq!(
        src,
        "top_chunk_fallback",
        "expected preview source top_chunk_fallback; preview={preview:?} payload={v}"
    );

    service.cancel().await.expect("cancel");
}

