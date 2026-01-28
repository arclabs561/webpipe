use axum::{routing::get, routing::post, Json, Router};
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr: SocketAddr = "127.0.0.1:18080".parse().expect("addr");
    let base = Arc::new(format!("http://{addr}"));
    let base_for_search = Arc::clone(&base);

    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(|body: Json<serde_json::Value>| async move {
                // Return strict JSON in message.content, shaped by the prompt.
                let mut user = String::new();
                if let Some(ms) = body.get("messages").and_then(|v| v.as_array()) {
                    if let Some(last) = ms.last() {
                        user = last
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                    }
                }

                let content = if user.contains("Aggregate multiple judge reports") {
                    r#"{"overall_assessment":"ok","cross_judge_agreement":"high","top_systemic_failures":["missing_key_facts"],"top_3_fixes":["increase max_urls","increase max_bytes"],"recommended_next_experiments":["increase max_urls"]}"#
                } else if user.contains("Summarize the judge's findings across queries") {
                    r#"{"overall_assessment":"ok","top_failure_modes":["missing_key_facts"],"recommended_defaults":{"agentic":true},"confidence":0.7}"#
                } else if user.contains("evidence-only task solver")
                    || user.contains("\"best_trial_id\"")
                    || user.contains("Best-trial hint")
                {
                    // Task solver output: always valid JSON per the solver schema.
                    r#"{"best_trial_id":"basic","answer":"","abstain_reason":"stub: evidence not evaluated","evidence_sufficient":false,"confidence":0.2,"citations":[]}"#
                } else {
                    r#"{"overall_score":80,"relevant":true,"answerable":false,"confidence":0.7,"issues":["missing_key_facts"],"notes":"stubbed judge"}"#
                };

                Json(serde_json::json!({
                    "choices": [{
                        "message": { "content": content }
                    }]
                }))
            }),
        )
        .route(
            "/docs",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/html")],
                    "<html><body><h1>Docs</h1><p>See route handlers.</p></body></html>",
                )
            }),
        )
        .route(
            "/docs/app/getting-started/route-handlers",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/html")],
                    "<html><body><h1>Route handlers</h1><p>Use GET/POST handlers.</p></body></html>",
                )
            }),
        )
        .route(
            "/install",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/html")],
                    "<html><body><h1>Install</h1><p>Linux install instructions.</p></body></html>",
                )
            }),
        )
        .route(
            "/install/linux",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/html")],
                    "<html><body><h1>Install on Linux</h1><p>Use apt.</p></body></html>",
                )
            }),
        )
        .route(
            "/searxng/search",
            get(move || {
                let base = Arc::clone(&base_for_search);
                async move {
                    Json(serde_json::json!({
                        "results": [
                            {"url": format!("{}/docs/app/getting-started/route-handlers", base), "title":"Route handlers", "content":"GET/POST handlers"},
                            {"url": format!("{}/install/linux", base), "title":"Install on Linux", "content":"Use apt."}
                        ]
                    }))
                }
            }),
        );

    eprintln!("stub server listening: {addr}");
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}
