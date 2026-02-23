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

async fn call_extract(service: &RunningService<RoleClient, ()>, url: &str) -> serde_json::Value {
    let r = service
        .call_tool(CallToolRequestParam {
            name: "web_extract".to_string().into(),
            arguments: Some(
                serde_json::json!({
                    "url": url,
                    "fetch_backend": "local",
                    "query": "Fenchel-Young",
                    "timeout_ms": 10_000,
                    "max_bytes": 200_000,
                    "max_chars": 10_000,
                    "include_text": false,
                    "include_links": false,
                    "include_structure": false,
                    "top_chunks": 3,
                    "max_chunk_chars": 500
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await
        .expect("call_tool");
    r.structured_content
        .clone()
        .unwrap_or_else(|| serde_json::json!({}))
}

async fn call_search_evidence(
    service: &RunningService<RoleClient, ()>,
    url: &str,
) -> serde_json::Value {
    let r = service
        .call_tool(CallToolRequestParam {
            name: "search_evidence".to_string().into(),
            arguments: Some(
                serde_json::json!({
                    "urls": [url],
                    "query": "Fenchel-Young",
                    "fetch_backend": "local",
                    "timeout_ms": 10_000,
                    "max_urls": 1,
                    "max_parallel_urls": 1,
                    "max_bytes": 200_000,
                    "max_chars": 10_000,
                    "include_text": false,
                    "include_links": false,
                    "include_structure": false,
                    "top_chunks": 3,
                    "max_chunk_chars": 500,
                    "compact": true
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await
        .expect("call_tool");
    r.structured_content
        .clone()
        .unwrap_or_else(|| serde_json::json!({}))
}

async fn call_search_evidence_markdown(
    service: &RunningService<RoleClient, ()>,
    url: &str,
) -> String {
    let r = service
        .call_tool(CallToolRequestParam {
            name: "search_evidence".to_string().into(),
            arguments: Some(
                serde_json::json!({
                    "urls": [url],
                    "query": "Fenchel-Young",
                    "fetch_backend": "local",
                    "timeout_ms": 10_000,
                    "max_urls": 1,
                    "max_parallel_urls": 1,
                    "max_bytes": 200_000,
                    "max_chars": 10_000,
                    "include_text": false,
                    "include_links": false,
                    "include_structure": false,
                    "top_chunks": 3,
                    "max_chunk_chars": 500,
                    "compact": true
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        })
        .await
        .expect("call_tool");
    r.content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn web_extract_falls_back_from_arxiv_pdf_to_html_when_pdf_extraction_fails(
) -> Result<(), Box<dyn std::error::Error>> {
    // Contract: when an arXiv PDF URL yields degraded extraction (e.g. malformed PDF),
    // web_extract should fall back to an ar5iv-like HTML URL (configured via env)
    // and return higher-signal text evidence.

    let app = Router::new()
        .route(
            "/pdf/2108.01988.pdf",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/pdf")],
                    "%PDF-1.1\nnot actually a real pdf\n",
                )
            }),
        )
        .route(
            "/html/2108.01988",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    "<html><body>\
                     <h1>Sparse Continuous Distributions and Fenchel-Young Losses</h1>\
                     <p>This page is a stand-in for ar5iv HTML conversion.</p>\
                     </body></html>",
                )
            }),
        );
    let addr = serve(app).await;
    let base = format!("http://{addr}");

    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache-arxiv-fallback");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(TokioChildProcess::new(
            tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                // Treat localhost as “arXiv” for this hermetic contract test.
                cmd.env("WEBPIPE_ARXIV_REWRITE_HOSTS", "127.0.0.1");
                // Point the fallback base at our local fixture HTML route.
                cmd.env("WEBPIPE_ARXIV_PDF_FALLBACK_BASE", format!("{base}/html/"));
                // Keep deterministic: avoid relying on external pdf tools.
                cmd.env("WEBPIPE_PDF_SHELLOUT", "off");
            }),
        )?)
        .await?;

    let v = call_extract(&service, &format!("{base}/pdf/2108.01988.pdf")).await;
    assert_eq!(v["ok"].as_bool(), Some(true));
    assert!(
        v["final_url"]
            .as_str()
            .unwrap_or("")
            .contains("/html/2108.01988"),
        "expected final_url to be HTML fallback; got {}",
        v["final_url"]
    );
    let engine = v["extract"]["engine"].as_str().unwrap_or("");
    assert!(
        !engine.starts_with("pdf-"),
        "expected non-pdf extract engine after fallback; got {engine}"
    );
    let empty = vec![];
    let warnings = v["warnings"].as_array().unwrap_or(&empty);
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str() == Some("arxiv_pdf_fallback_to_html")),
        "expected arxiv_pdf_fallback_to_html warning; warnings={warnings:?}"
    );
    assert!(
        v["extract"]["chunks"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "expected non-empty chunks"
    );
    let chunk_text = v["extract"]["chunks"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        chunk_text.to_ascii_lowercase().contains("fenchel"),
        "expected chunk text to contain fenchel; got {chunk_text:?}"
    );

    service.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn search_evidence_urls_mode_falls_back_from_arxiv_pdf_to_html_when_pdf_extraction_fails(
) -> Result<(), Box<dyn std::error::Error>> {
    // Contract: `search_evidence` (aka web_search_extract) should apply the same arXiv PDF→HTML
    // content fallback even in single-URL urls-mode (max_urls=1), which uses the sequential
    // hydration path.

    let app = Router::new()
        .route(
            "/pdf/2108.01988.pdf",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/pdf")],
                    "%PDF-1.1\nnot actually a real pdf\n",
                )
            }),
        )
        .route(
            "/html/2108.01988",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    "<html><body>\
                     <h1>Sparse Continuous Distributions and Fenchel-Young Losses</h1>\
                     <p>This page is a stand-in for ar5iv HTML conversion.</p>\
                     </body></html>",
                )
            }),
        );
    let addr = serve(app).await;
    let base = format!("http://{addr}");

    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache-arxiv-fallback");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(TokioChildProcess::new(
            tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                // Treat localhost as “arXiv” for this hermetic contract test.
                cmd.env("WEBPIPE_ARXIV_REWRITE_HOSTS", "127.0.0.1");
                // Point the fallback base at our local fixture HTML route.
                cmd.env("WEBPIPE_ARXIV_PDF_FALLBACK_BASE", format!("{base}/html/"));
                // Keep deterministic: avoid relying on external pdf tools.
                cmd.env("WEBPIPE_PDF_SHELLOUT", "off");
            }),
        )?)
        .await?;

    let v = call_search_evidence(&service, &format!("{base}/pdf/2108.01988.pdf")).await;
    assert_eq!(v["ok"].as_bool(), Some(true));
    let r0 = &v["results"][0];
    assert!(
        r0["final_url"]
            .as_str()
            .unwrap_or("")
            .contains("/html/2108.01988"),
        "expected per-url final_url to be HTML fallback; got {}",
        r0["final_url"]
    );
    let engine = r0["extract"]["engine"].as_str().unwrap_or("");
    assert!(
        !engine.starts_with("pdf-"),
        "expected non-pdf extract engine after fallback; got {engine}"
    );
    let empty = vec![];
    let warnings = r0["warnings"].as_array().unwrap_or(&empty);
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str() == Some("arxiv_pdf_fallback_to_html")),
        "expected arxiv_pdf_fallback_to_html warning; warnings={warnings:?}"
    );
    assert!(
        v["top_chunks"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "expected non-empty top_chunks"
    );
    let top = v["top_chunks"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        top.to_ascii_lowercase().contains("fenchel"),
        "expected top chunk to mention fenchel; got {top:?}"
    );

    service.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn web_extract_falls_back_from_openreview_pdf_to_api_when_pdf_extraction_fails(
) -> Result<(), Box<dyn std::error::Error>> {
    // Contract: when an OpenReview PDF URL yields degraded extraction (e.g. malformed/truncated PDF),
    // web_extract should fall back to the OpenReview notes API (JSON) and return higher-signal text
    // evidence (title/abstract).

    let app = Router::new()
        .route(
            "/pdf",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/pdf")],
                    "%PDF-1.1\nnot actually a real pdf\n",
                )
            }),
        )
        .route(
            "/api/notes",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                    serde_json::json!({
                        "notes": [{
                            "id": "OPENREVIEW_TEST_ID",
                            "content": {
                                "title": "OpenReview Paper: Fenchel-Young Losses",
                                "abstract": "This is a stand-in API response. Fenchel-Young appears here so query matching works."
                            }
                        }],
                        "count": 1
                    })
                    .to_string(),
                )
            }),
        )
        .route(
            "/forum",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    "<html><body>\
                     <h1>OpenReview Paper: Fenchel-Young Losses</h1>\
                     <p>Abstract: This is a stand-in forum page. Fenchel-Young appears here so query matching works.</p>\
                     </body></html>",
                )
            }),
        );
    let addr = serve(app).await;
    let base = format!("http://{addr}");
    let pdf_url = format!("{base}/pdf?id=OPENREVIEW_TEST_ID");

    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache-openreview-fallback");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(TokioChildProcess::new(
            tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                // Treat localhost as “OpenReview” for this hermetic contract test.
                cmd.env("WEBPIPE_OPENREVIEW_REWRITE_HOSTS", "127.0.0.1");
                // Point the API base at our local fixture JSON route.
                cmd.env("WEBPIPE_OPENREVIEW_API_BASE", format!("{base}/api/notes"));
                // Keep deterministic: avoid relying on external pdf tools.
                cmd.env("WEBPIPE_PDF_SHELLOUT", "off");
            }),
        )?)
        .await?;

    let v = call_extract(&service, &pdf_url).await;
    assert_eq!(v["ok"].as_bool(), Some(true));
    assert!(
        v["final_url"]
            .as_str()
            .unwrap_or("")
            .contains("/api/notes?id=OPENREVIEW_TEST_ID"),
        "expected final_url to be API fallback; got {}",
        v["final_url"]
    );
    let engine = v["extract"]["engine"].as_str().unwrap_or("");
    assert!(
        !engine.starts_with("pdf-"),
        "expected non-pdf extract engine after fallback; got {engine}"
    );
    let empty = vec![];
    let warnings = v["warnings"].as_array().unwrap_or(&empty);
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str() == Some("openreview_pdf_fallback_to_api")),
        "expected openreview_pdf_fallback_to_api warning; warnings={warnings:?}"
    );
    assert!(
        v["extract"]["chunks"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "expected non-empty chunks"
    );
    let chunk_text = v["extract"]["chunks"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        chunk_text.to_ascii_lowercase().contains("fenchel"),
        "expected chunk text to contain fenchel; got {chunk_text:?}"
    );

    service.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn search_evidence_urls_mode_falls_back_from_openreview_pdf_to_api_when_pdf_extraction_fails(
) -> Result<(), Box<dyn std::error::Error>> {
    // Contract: `search_evidence` (aka web_search_extract) should apply the same OpenReview PDF→API
    // fallback even in single-URL urls-mode (max_urls=1), which uses the sequential hydration path.

    let app = Router::new()
        .route(
            "/pdf",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/pdf")],
                    "%PDF-1.1\nnot actually a real pdf\n",
                )
            }),
        )
        .route(
            "/api/notes",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                    serde_json::json!({
                        "notes": [{
                            "id": "OPENREVIEW_TEST_ID",
                            "content": {
                                "title": "OpenReview Paper: Fenchel-Young Losses",
                                "abstract": "This is a stand-in API response. Fenchel-Young appears here so query matching works."
                            }
                        }],
                        "count": 1
                    })
                    .to_string(),
                )
            }),
        )
        .route(
            "/forum",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    "<html><body>\
                     <h1>OpenReview Paper: Fenchel-Young Losses</h1>\
                     <p>Abstract: This is a stand-in forum page. Fenchel-Young appears here so query matching works.</p>\
                     </body></html>",
                )
            }),
        );
    let addr = serve(app).await;
    let base = format!("http://{addr}");
    let pdf_url = format!("{base}/pdf?id=OPENREVIEW_TEST_ID");

    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache-openreview-fallback");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(TokioChildProcess::new(
            tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                // Treat localhost as “OpenReview” for this hermetic contract test.
                cmd.env("WEBPIPE_OPENREVIEW_REWRITE_HOSTS", "127.0.0.1");
                // Point the API base at our local fixture JSON route.
                cmd.env("WEBPIPE_OPENREVIEW_API_BASE", format!("{base}/api/notes"));
                // Keep deterministic: avoid relying on external pdf tools.
                cmd.env("WEBPIPE_PDF_SHELLOUT", "off");
            }),
        )?)
        .await?;

    let v = call_search_evidence(&service, &pdf_url).await;
    assert_eq!(v["ok"].as_bool(), Some(true));
    let r0 = &v["results"][0];
    assert!(
        r0["final_url"]
            .as_str()
            .unwrap_or("")
            .contains("/api/notes?id=OPENREVIEW_TEST_ID"),
        "expected per-url final_url to be API fallback; got {}",
        r0["final_url"]
    );
    let engine = r0["extract"]["engine"].as_str().unwrap_or("");
    assert!(
        !engine.starts_with("pdf-"),
        "expected non-pdf extract engine after fallback; got {engine}"
    );
    let empty = vec![];
    let warnings = r0["warnings"].as_array().unwrap_or(&empty);
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str() == Some("openreview_pdf_fallback_to_api")),
        "expected openreview_pdf_fallback_to_api warning; warnings={warnings:?}"
    );
    assert!(
        v["top_chunks"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "expected non-empty top_chunks"
    );
    let top = v["top_chunks"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        top.to_ascii_lowercase().contains("fenchel"),
        "expected top chunk to mention fenchel; got {top:?}"
    );

    service.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn search_evidence_markdown_surfaces_final_url_when_arxiv_fallback_used(
) -> Result<(), Box<dyn std::error::Error>> {
    // Contract: when arXiv PDF→HTML fallback is used, Markdown should surface the final_url
    // so users can understand where the evidence came from.

    let app = Router::new()
        .route(
            "/pdf/2108.01988.pdf",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/pdf")],
                    "%PDF-1.1\nnot actually a real pdf\n",
                )
            }),
        )
        .route(
            "/html/2108.01988",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    "<html><body>\
                     <h1>Sparse Continuous Distributions and Fenchel-Young Losses</h1>\
                     <p>This page is a stand-in for ar5iv HTML conversion.</p>\
                     </body></html>",
                )
            }),
        );
    let addr = serve(app).await;
    let base = format!("http://{addr}");
    let pdf_url = format!("{base}/pdf/2108.01988.pdf");
    let html_url = format!("{base}/html/2108.01988");

    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("webpipe-cache-arxiv-fallback-md");

    let bin = assert_cmd::cargo::cargo_bin!("webpipe");
    let service = ()
        .serve(TokioChildProcess::new(
            tokio::process::Command::new(bin).configure(|cmd| {
                cmd.args(["mcp-stdio"]);
                cmd.env("WEBPIPE_DOTENV", "0");
                cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                cmd.env("WEBPIPE_MCP_INCLUDE_JSON_TEXT", "0");
                cmd.env("WEBPIPE_MCP_MARKDOWN_CHUNK_EXCERPTS", "0");
                // Treat localhost as “arXiv” for this hermetic contract test.
                cmd.env("WEBPIPE_ARXIV_REWRITE_HOSTS", "127.0.0.1");
                cmd.env("WEBPIPE_ARXIV_PDF_FALLBACK_BASE", format!("{base}/html/"));
                cmd.env("WEBPIPE_PDF_SHELLOUT", "off");
            }),
        )?)
        .await?;

    let md = call_search_evidence_markdown(&service, &pdf_url).await;
    assert!(
        md.contains("## Sources"),
        "expected Sources section in markdown; got: {md:?}"
    );
    assert!(
        md.contains("- final_url:"),
        "expected final_url field in Sources; got: {md:?}"
    );
    assert!(
        md.contains(&html_url),
        "expected markdown to mention fallback html url; got: {md:?}"
    );

    service.cancel().await?;
    Ok(())
}
