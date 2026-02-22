use axum::{routing::get, Router};
use rmcp::{
    model::CallToolRequestParam,
    service::{RoleClient, RunningService, ServiceExt},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::collections::BTreeSet;
use std::net::SocketAddr;

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
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

#[test]
fn web_extract_rewrites_arxiv_abs_to_pdf_on_same_host() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        // Keep this contract focused on URL rewrite behavior, not PDF extraction robustness.
        //
        // Some environments have slow or flaky PDF backends (shellouts like `pdftotext`), which
        // can make this test *look* hung while it is actually waiting on PDF parsing/extraction.
        // Serving non-PDF bytes avoids that whole class of nondeterminism.
        let app = Router::new()
            .route(
                "/pdf/1234.5678.pdf",
                get(|| async { ([("content-type", "text/plain; charset=utf-8")], "ok") }),
            )
            // Legacy arXiv IDs include a category prefix.
            .route(
                "/pdf/hep-th/9901001.pdf",
                get(|| async { ([("content-type", "text/plain; charset=utf-8")], "ok") }),
            );
        let addr = serve(app).await;

        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("webpipe-cache");

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                    cmd.env("WEBPIPE_ARXIV_REWRITE_HOSTS", "127.0.0.1");
                    cmd.env("WEBPIPE_PDF_SHELLOUT", "off");
                    // Deterministic keyless behavior.
                    cmd.env_remove("WEBPIPE_BRAVE_API_KEY");
                    cmd.env_remove("BRAVE_SEARCH_API_KEY");
                    cmd.env_remove("WEBPIPE_TAVILY_API_KEY");
                    cmd.env_remove("TAVILY_API_KEY");
                    cmd.env_remove("WEBPIPE_FIRECRAWL_API_KEY");
                    cmd.env_remove("FIRECRAWL_API_KEY");
                }),
            )?)
            .await?;

        let tools = service.list_tools(Default::default()).await?;
        let names: BTreeSet<String> = tools
            .tools
            .iter()
            .map(|t| t.name.clone().into_owned())
            .collect();
        assert!(names.contains("web_extract"), "missing web_extract tool");

        let abs_url = format!("http://{addr}/abs/1234.5678");
        let v = call(
            &service,
            "web_extract",
            serde_json::json!({
                "url": abs_url,
                "fetch_backend": "local",
                "timeout_ms": 10_000,
                "max_bytes": 200_000,
                "max_chars": 10_000,
                "include_text": false,
                "include_links": false,
                "include_structure": false
            }),
        )
        .await;

        assert_eq!(v["ok"].as_bool(), Some(true));
        assert!(
            v["final_url"]
                .as_str()
                .unwrap_or("")
                .contains("/pdf/1234.5678.pdf"),
            "expected final_url to be pdf: {}",
            v["final_url"]
        );
        let empty = vec![];
        let codes: Vec<&str> = v["warning_codes"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|x| x.as_str())
            .collect();
        assert!(
            codes.contains(&"arxiv_abs_rewritten_to_pdf"),
            "expected arxiv rewrite warning; got {codes:?}"
        );

        // Legacy IDs should preserve the full path segment after /abs/.
        let abs_url2 = format!("http://{addr}/abs/hep-th/9901001");
        let v2 = call(
            &service,
            "web_extract",
            serde_json::json!({
                "url": abs_url2,
                "fetch_backend": "local",
                "timeout_ms": 10_000,
                "max_bytes": 200_000,
                "max_chars": 10_000,
                "include_text": false,
                "include_links": false,
                "include_structure": false
            }),
        )
        .await;
        assert_eq!(v2["ok"].as_bool(), Some(true));
        assert!(
            v2["final_url"]
                .as_str()
                .unwrap_or("")
                .contains("/pdf/hep-th/9901001.pdf"),
            "expected final_url to be legacy pdf: {}",
            v2["final_url"]
        );

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("arxiv rewrite contract");
}
