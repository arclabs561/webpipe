#[test]
fn webpipe_mcp_stdio_render_fallback_low_signal_offline_marks_not_supported_warning() {
    // End-to-end (spawns child process) and strictly offline:
    // - uses local fixture server (localhost)
    // - forces WEBPIPE_OFFLINE_ONLY=1, so render fallback is not supported
    //
    // Contract:
    // - When local extraction looks low-signal and render_fallback_on_low_signal=true,
    //   web_search_extract should keep the local result but emit render_fallback_not_supported
    //   (with warning_codes + hint).
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use axum::{http::header, http::HeaderMap, http::StatusCode, routing::get, Router};
        use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
        use rmcp::{model::CallToolRequestParam, service::ServiceExt};
        use std::net::SocketAddr;

        // Fixture returns “bundle gunk” tokens that should trip looks_like_bundle_gunk().
        let app = Router::new().route(
            "/",
            get(|headers: HeaderMap| async move {
                if headers.contains_key(header::AUTHORIZATION)
                    || headers.contains_key(header::COOKIE)
                    || headers.contains_key(header::PROXY_AUTHORIZATION)
                {
                    return (
                        StatusCode::BAD_REQUEST,
                        [(header::CONTENT_TYPE, "text/plain")],
                        "sensitive header forwarded".to_string(),
                    );
                }
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/html")],
                    r#"<html><head><title>x</title></head>
<body>
  <h1>Portal</h1>
  <pre>
self.__next_s = self.__next_s || [];
webpackChunk = webpackChunk || [];
__webpack_require__ = function(a,b,c,d){};
window.__NUXT__ = {"suppressHydrationWarning":true};
.push([0,{"suppressHydrationWarning":true}]);
  </pre>
</body></html>"#
                        .to_string(),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });
        let fixture_url = format!("http://{}/", addr);

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    cmd.env("WEBPIPE_OFFLINE_ONLY", "1");
                    cmd.env_remove("WEBPIPE_RENDER_DISABLE");
                }),
            )?)
            .await?;

        let r = service
            .call_tool(CallToolRequestParam {
                name: "search_evidence".into(),
                arguments: Some(
                    serde_json::json!({
                        "query":"portal",
                        "urls":[fixture_url],
                        "url_selection_mode":"preserve",
                        "provider":"auto",
                        "auto_mode":"fallback",
                        "selection_mode":"pareto",
                        "fetch_backend":"local",
                        "no_network": false,
                        "render_fallback_on_low_signal": true,
                        "render_fallback_on_empty_extraction": false,
                        "firecrawl_fallback_on_low_signal": false,
                        "firecrawl_fallback_on_empty_extraction": false,
                        "include_text": false,
                        "include_links": false,
                        "include_structure": false,
                        "max_results": 1,
                        "max_urls": 1,
                        "timeout_ms": 2000,
                        "max_bytes": 200000,
                        "top_chunks": 3,
                        "max_chunk_chars": 200
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        assert_eq!(r.is_error, Some(false));
        let v: serde_json::Value = r
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;

        assert_eq!(v["kind"].as_str(), Some("web_search_extract"));
        assert_eq!(v["ok"].as_bool(), Some(true));

        let one = v["results"]
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| format!("expected results[0], got payload={}", v))?;

        let warnings_v = one
            .get("warnings")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let warnings = warnings_v
            .as_array()
            .ok_or_else(|| format!("expected results[0].warnings array, got payload={}", v))?
            .iter()
            .filter_map(|x| x.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            warnings.contains("render_fallback_not_supported"),
            "expected render_fallback_not_supported warning"
        );

        let codes = one
            .get("warning_codes")
            .and_then(|x| x.as_array())
            .ok_or_else(|| format!("expected results[0].warning_codes array, got payload={}", v))?
            .iter()
            .filter_map(|x| x.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            codes.contains("render_fallback_not_supported"),
            "expected render_fallback_not_supported warning code"
        );

        let hint = one
            .pointer("/warning_hints/render_fallback_not_supported")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        assert!(
            hint.contains("Render fallback is not supported"),
            "expected actionable hint for render_fallback_not_supported"
        );

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio render fallback offline warning contract");
}

#[test]
fn webpipe_mcp_stdio_render_fallback_low_signal_disabled_marks_disabled_warning() {
    // End-to-end (spawns child process), offline-safe (localhost fixture).
    //
    // Contract:
    // - When local extraction looks low-signal and render_fallback_on_low_signal=true,
    //   WEBPIPE_RENDER_DISABLE=1 should cause render_fallback_disabled warning (no node spawn).
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        use axum::{http::header, http::HeaderMap, http::StatusCode, routing::get, Router};
        use rmcp::{model::CallToolRequestParam, service::ServiceExt};
        use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
        use std::net::SocketAddr;

        let app = Router::new().route(
            "/",
            get(|_headers: HeaderMap| async move {
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/html")],
                    "<html><body><pre>webpackChunk .push([0,{\"suppressHydrationWarning\":true}]);</pre></body></html>"
                        .to_string(),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });
        let fixture_url = format!("http://{}/", addr);

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let cache_dir = tempfile::TempDir::new()?;
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_DOTENV", "0");
                    cmd.env("WEBPIPE_CACHE_DIR", cache_dir.path());
                    cmd.env("WEBPIPE_RENDER_DISABLE", "1");
                    cmd.env_remove("WEBPIPE_OFFLINE_ONLY");
                }),
            )?)
            .await?;

        let r = service
            .call_tool(CallToolRequestParam {
                        name: "search_evidence".into(),
                arguments: Some(
                    serde_json::json!({
                        "query":"portal",
                        "urls":[fixture_url],
                        "url_selection_mode":"preserve",
                        "provider":"auto",
                        "auto_mode":"fallback",
                        "selection_mode":"pareto",
                        "fetch_backend":"local",
                        "no_network": false,
                        "render_fallback_on_low_signal": true,
                        "render_fallback_on_empty_extraction": false,
                        "firecrawl_fallback_on_low_signal": false,
                        "firecrawl_fallback_on_empty_extraction": false,
                        "include_text": false,
                        "include_links": false,
                        "include_structure": false,
                        "max_results": 1,
                        "max_urls": 1,
                        "timeout_ms": 2000,
                        "max_bytes": 200000,
                        "top_chunks": 3,
                        "max_chunk_chars": 200
                    })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ),
            })
            .await?;

        assert_eq!(r.is_error, Some(false));
        let v: serde_json::Value = r
            .structured_content
            .clone()
            .ok_or("expected structured_content")?;
        assert_eq!(v["kind"].as_str(), Some("web_search_extract"));
        assert_eq!(v["ok"].as_bool(), Some(true));

        let one = v["results"]
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| format!("expected results[0], got payload={}", v))?;

        let warnings_v = one.get("warnings").cloned().unwrap_or(serde_json::Value::Null);
        let warnings = warnings_v
            .as_array()
            .ok_or_else(|| format!("expected results[0].warnings array, got payload={}", v))?
            .iter()
            .filter_map(|x| x.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            warnings.contains("render_fallback_disabled"),
            "expected render_fallback_disabled warning"
        );

        let codes = one
            .get("warning_codes")
            .and_then(|x| x.as_array())
            .ok_or_else(|| format!("expected results[0].warning_codes array, got payload={}", v))?
            .iter()
            .filter_map(|x| x.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            codes.contains("render_fallback_disabled"),
            "expected render_fallback_disabled warning code"
        );

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("mcp stdio render fallback disabled warning contract");
}
