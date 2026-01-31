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
fn web_extract_rewrites_github_pr_to_patch() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let patch = "From 0000000000000000000000000000000000000000 Mon Sep 17 00:00:00 2001\nSubject: [PATCH] test\n";
        let patch_s = patch.to_string();
        let app = Router::new().route(
            "/owner/repo/pull/1.patch",
            get(move || {
                let body = patch_s.clone();
                async move { ([("content-type", "text/plain")], body) }
            }),
        );
        let addr = serve(app).await;

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_CACHE_DIR", std::env::temp_dir().join("webpipe-github-pr-cache"));
                    cmd.env("WEBPIPE_GITHUB_REWRITE_HOSTS", "127.0.0.1");
                }),
            )?)
            .await?;

        let tools = service.list_tools(Default::default()).await?;
        let names: BTreeSet<String> = tools.tools.iter().map(|t| t.name.clone().into_owned()).collect();
        assert!(names.contains("web_extract"), "missing web_extract tool");

        let pr_url = format!("http://{addr}/owner/repo/pull/1");
        let v = call(
            &service,
            "web_extract",
            serde_json::json!({
                "url": pr_url,
                "fetch_backend": "local",
                "timeout_ms": 10_000,
                "max_bytes": 200_000,
                "max_chars": 10_000,
                "include_text": true,
                "include_links": false,
                "include_structure": false
            }),
        )
        .await;

        assert_eq!(v["ok"].as_bool(), Some(true));
        assert!(
            v["final_url"].as_str().unwrap_or("").contains("/pull/1.patch"),
            "expected final_url to be .patch: {}",
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
            codes.contains(&"github_pr_rewritten_to_patch"),
            "expected github PR rewrite warning; got {codes:?}"
        );
        assert!(v["extract"]["text"].as_str().unwrap_or("").contains("[PATCH]"));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("github pr rewrite contract");
}
