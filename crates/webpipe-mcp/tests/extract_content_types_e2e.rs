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
async fn web_extract_handles_multiple_content_types() {
    // A small local server with different content-types.
    let app = Router::new()
        .route(
            "/html",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html")],
                    "<html><head><title>T</title></head><body><h1>Hello</h1><a href=\"/x\">x</a></body></html>",
                )
            }),
        )
        .route(
            "/article",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html")],
                    "<html><body>\
                    <nav class=\"nav\">\
                      <a href=\"https://example.com/a\">A</a>\
                      <a href=\"https://example.com/b\">B</a>\
                      <a href=\"https://example.com/c\">C</a>\
                      <a href=\"https://example.com/d\">D</a>\
                      <a href=\"https://example.com/e\">E</a>\
                      <a href=\"https://example.com/f\">F</a>\
                      <a href=\"https://example.com/g\">G</a>\
                      <a href=\"https://example.com/h\">H</a>\
                      <a href=\"https://example.com/i\">I</a>\
                      <a href=\"https://example.com/j\">J</a>\
                    </nav>\
                    <article>\
                      <h1>Article</h1>\
                      <p>The key paragraph mentions transformers and attention.</p>\
                      <p>Another paragraph with details about self-attention and sequence transduction.</p>\
                      <p>More content here so the main block dominates.</p>\
                    </article>\
                    <footer class=\"footer\">\
                      <a href=\"https://example.com/privacy\">Privacy</a>\
                      <a href=\"https://example.com/terms\">Terms</a>\
                      <a href=\"https://example.com/cookies\">Cookies</a>\
                    </footer>\
                    </body></html>",
                )
            }),
        )
        .route(
            "/json",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"a":1,"b":"two"}"#,
                )
            }),
        )
        .route(
            "/md",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/markdown")],
                    "Hello\n\n[pdf](/paper.pdf)\n",
                )
            }),
        )
        .route(
            "/shell",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html")],
                    "<html><head><title>Only Title</title><meta name=\"description\" content=\"Desc\"/></head><body><script>1</script></body></html>",
                )
            }),
        )
        .route(
            "/pdf_bad",
            get(|| async {
                // Not a valid PDF, but content-type says PDF. We should attempt pdf extraction and
                // surface a warning without panicking.
                (
                    [(header::CONTENT_TYPE, "application/pdf")],
                    "%PDF-1.1\nnot actually a real pdf\n",
                )
            }),
        );

    let addr = serve(app).await;
    let base = format!("http://{addr}");

    // Spawn the real MCP stdio server binary and talk to it over stdio.
    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                // Disable cache for determinism.
                cmd.env(
                    "WEBPIPE_CACHE_DIR",
                    std::env::temp_dir().join("webpipe-test-cache-types"),
                );
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    async fn call_extract(
        service: &RunningService<RoleClient, ()>,
        url: &str,
        query: Option<&str>,
    ) -> serde_json::Value {
        let r = service
            .call_tool(CallToolRequestParam {
                name: "web_extract".into(),
                arguments: Some(
                    serde_json::json!({
                        "url": url,
                        "fetch_backend": "local",
                        "cache_read": false,
                        "cache_write": false,
                        "timeout_ms": 5000,
                        "max_bytes": 1_000_000,
                        "max_chars": 5000,
                        "width": 80,
                        "query": query,
                        "include_text": true,
                        "include_links": false,
                        "max_links": 25,
                        "include_structure": true,
                        "max_outline_items": 25,
                        "max_blocks": 20,
                        "max_block_chars": 300,
                        "semantic_rerank": false
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await
            .expect("call_tool web_extract");
        let text = r
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        serde_json::from_str(&text).expect("parse web_extract json")
    }

    // HTML should use html2text.
    let v_html = call_extract(&service, &format!("{base}/html"), None).await;
    assert_eq!(v_html["ok"].as_bool(), Some(true));
    assert_eq!(v_html["content_type"].as_str(), Some("text/html"));
    assert_eq!(v_html["extraction"]["engine"].as_str(), Some("html2text"));
    assert_eq!(v_html["extract"]["engine"].as_str(), Some("html2text"));
    assert!(v_html.get("structure").is_some());

    // Boilerplate-heavy article should prefer html_main.
    let v_article = call_extract(&service, &format!("{base}/article"), None).await;
    assert_eq!(v_article["ok"].as_bool(), Some(true));
    assert_eq!(v_article["content_type"].as_str(), Some("text/html"));
    assert_eq!(
        v_article["extraction"]["engine"].as_str(),
        Some("html_main")
    );
    assert_eq!(v_article["extract"]["engine"].as_str(), Some("html_main"));
    let warnings = v_article["warnings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(warnings
        .iter()
        .any(|w| w.as_str() == Some("boilerplate_reduced")));
    assert!(v_article["structure"]["outline"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .any(|x| x.as_str().unwrap_or("").to_lowercase().contains("article")));

    // Query chunking should use structure and keep heading context.
    let v_article_q = call_extract(
        &service,
        &format!("{base}/article"),
        Some("transformers attention"),
    )
    .await;
    let c0 = v_article_q["chunks"]
        .as_array()
        .and_then(|xs| xs.first())
        .and_then(|x| x.get("text"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    assert!(c0.to_lowercase().contains("article"));
    assert!(c0.to_lowercase().contains("attention"));

    // JSON should be treated as json/text, not html2text.
    let v_json = call_extract(&service, &format!("{base}/json"), None).await;
    assert_eq!(v_json["ok"].as_bool(), Some(true));
    assert_eq!(v_json["content_type"].as_str(), Some("application/json"));
    assert_eq!(v_json["extraction"]["engine"].as_str(), Some("json"));
    assert_eq!(v_json["extract"]["engine"].as_str(), Some("json"));
    assert!(v_json["structure"]["outline"]
        .as_array()
        .map(|a| !a.is_empty())
        .unwrap_or(false));

    // Markdown should be treated as markdown.
    let v_md = call_extract(&service, &format!("{base}/md"), None).await;
    assert_eq!(v_md["ok"].as_bool(), Some(true));
    assert_eq!(v_md["content_type"].as_str(), Some("text/markdown"));
    assert_eq!(v_md["extraction"]["engine"].as_str(), Some("markdown"));
    assert_eq!(v_md["extract"]["engine"].as_str(), Some("markdown"));

    // JS shell HTML should fall back to hint text.
    let v_shell = call_extract(&service, &format!("{base}/shell"), None).await;
    assert_eq!(v_shell["ok"].as_bool(), Some(true));
    assert_eq!(v_shell["extraction"]["engine"].as_str(), Some("html_hint"));
    assert_eq!(v_shell["extract"]["engine"].as_str(), Some("html_hint"));
    let warnings = v_shell["warnings"].as_array().cloned().unwrap_or_default();
    assert!(warnings
        .iter()
        .any(|w| w.as_str() == Some("hint_text_fallback")));

    // Bad PDF should not panic; should warn pdf_extract_failed.
    let v_pdf = call_extract(&service, &format!("{base}/pdf_bad"), None).await;
    assert_eq!(v_pdf["ok"].as_bool(), Some(true));
    assert_eq!(v_pdf["content_type"].as_str(), Some("application/pdf"));
    assert_eq!(v_pdf["extraction"]["engine"].as_str(), Some("pdf-extract"));
    assert_eq!(v_pdf["extract"]["engine"].as_str(), Some("pdf-extract"));
    let warnings = v_pdf["warnings"].as_array().cloned().unwrap_or_default();
    assert!(warnings
        .iter()
        .any(|w| w.as_str() == Some("pdf_extract_failed")));
}
