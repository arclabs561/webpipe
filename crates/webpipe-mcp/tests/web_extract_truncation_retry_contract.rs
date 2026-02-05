use axum::{routing::get, Router};
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
fn web_extract_retry_on_truncation_recovers_tail_marker() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        // HTML with a marker near the end.
        let head = "<html><body><main><h1>Doc</h1><p>";
        // Keep this moderate: the contract is “retry can recover a tail marker”, not a stress test.
        // Very large bodies can make structure parsing slow on some HTML parser paths.
        let filler = "x".repeat(35_000);
        let tail_marker = "TAIL_MARKER_123";
        let tail = format!("</p><p>{tail_marker}</p></main></body></html>");
        let body = format!("{head}{filler}{tail}");

        let app = Router::new().route(
            "/",
            get(move || {
                let b = body.clone();
                async move { ([("content-type", "text/html")], b) }
            }),
        );
        let addr = serve(app).await;
        let url = format!("http://{addr}/");

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env(
                        "WEBPIPE_CACHE_DIR",
                        std::env::temp_dir().join("webpipe-web-extract-trunc-retry"),
                    );
                }),
            )?)
            .await?;

        // Force truncation: max_bytes small, but enable retry to a larger cap.
        let v = call(
            &service,
            "web_extract",
            serde_json::json!({
                "url": url,
                "fetch_backend": "local",
                "query": "TAIL_MARKER_123",
                "timeout_ms": 10_000,
                "max_bytes": 10_000,
                "max_chars": 200_000,
                "retry_on_truncation": true,
                "truncation_retry_max_bytes": 120_000,
                "include_text": true,
                "include_structure": true
            }),
        )
        .await;

        assert_eq!(v["ok"].as_bool(), Some(true));
        eprintln!(
            "debug: truncated={} attempts={} text_chars={}",
            v.get("truncated")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            v.get("attempts").is_some(),
            v.get("extract")
                .and_then(|e| e.get("text_chars"))
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
        );
        if let Some(a) = v.get("attempts") {
            eprintln!("debug: attempts={a}");
        }
        // The marker should appear in the extracted text after retry.
        let txt = v["extract"]["text"].as_str().unwrap_or("");
        assert!(
            txt.contains(tail_marker),
            "expected tail marker in extracted text after retry"
        );
        // Should include truncation_retry attempt details.
        assert!(
            v.get("attempts")
                .and_then(|a| a.get("truncation_retry"))
                .is_some(),
            "expected attempts.truncation_retry in payload"
        );

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("truncation retry contract");
}
