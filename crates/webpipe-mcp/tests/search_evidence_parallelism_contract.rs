use axum::{routing::get, Router};
use rmcp::{
    model::CallToolRequestParam,
    service::{RoleClient, RunningService, ServiceExt},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
async fn search_evidence_can_hydrate_urls_in_parallel_when_enabled() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_in_flight = Arc::new(AtomicUsize::new(0));

    let mk = |label: &'static str, in_flight: Arc<AtomicUsize>, max_in_flight: Arc<AtomicUsize>| {
        get(move || {
            let in_flight = in_flight.clone();
            let max_in_flight = max_in_flight.clone();
            async move {
                let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                // Track max concurrently in-flight handlers.
                let mut prev = max_in_flight.load(Ordering::SeqCst);
                while cur > prev {
                    match max_in_flight.compare_exchange(
                        prev,
                        cur,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => break,
                        Err(p) => prev = p,
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                (
                    [("content-type", "text/html")],
                    format!("<h1>{label}</h1> paralleltoken"),
                )
            }
        })
    };

    let app = Router::new()
        .route("/one", mk("one", in_flight.clone(), max_in_flight.clone()))
        .route("/two", mk("two", in_flight.clone(), max_in_flight.clone()));
    let addr = serve(app).await;

    let u1 = format!("http://127.0.0.1:{}/one", addr.port());
    let u2 = format!("http://127.0.0.1:{}/two", addr.port());

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
            }))
            .expect("spawn mcp child"),
        )
        .await
        .expect("serve mcp child");

    let v = call(
        &service,
        "search_evidence",
        serde_json::json!({
            "query": "paralleltoken",
            "urls": [u1, u2],
            "url_selection_mode": "preserve",
            "fetch_backend": "local",
            "agentic": false,
            "max_urls": 2,
            "max_parallel_urls": 2,
            "top_chunks": 3,
            "max_chunk_chars": 200,
            "include_text": false,
            "include_links": false,
            "compact": true,
            "timeout_ms": 10_000,
            "deadline_ms": 30_000
        }),
    )
    .await;

    assert_eq!(v["ok"].as_bool(), Some(true));
    let rs = v["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(rs.len(), 2);

    // This is the key assertion: we should see at least 2 concurrent in-flight HTTP handlers.
    assert!(
        max_in_flight.load(Ordering::SeqCst) >= 2,
        "expected parallel hydration; max_in_flight={}",
        max_in_flight.load(Ordering::SeqCst)
    );

    service.cancel().await.expect("cancel");
}
