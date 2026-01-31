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
        // Minimal PDF header bytes; extraction may yield empty, but we only assert the rewrite happened.
        let pdf_bytes: Vec<u8> = b"%PDF-1.4\n%fake\n".to_vec();
        let pdf_s = String::from_utf8_lossy(&pdf_bytes).to_string();

        let app = Router::new().route(
            "/pdf/1234.5678.pdf",
            get(move || {
                let body = pdf_s.clone();
                async move { ([("content-type", "application/pdf")], body) }
            }),
        );
        let addr = serve(app).await;

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env(
                        "WEBPIPE_CACHE_DIR",
                        std::env::temp_dir().join("webpipe-arxiv-rewrite-cache"),
                    );
                    cmd.env("WEBPIPE_ARXIV_REWRITE_HOSTS", "127.0.0.1");
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

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("arxiv rewrite contract");
}
