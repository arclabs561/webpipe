use axum::{routing::get, Router};
use flate2::{write::GzEncoder, Compression};
use rmcp::{
    model::CallToolRequestParam,
    service::{RoleClient, RunningService, ServiceExt},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use std::collections::BTreeSet;
use std::net::SocketAddr;

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
fn web_sitemap_extract_discovers_urls_from_robots_and_sitemap() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let robots = "User-agent: *\n";
        let robots_s = robots.to_string();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();

        let sitemap = format!(
            r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://{addr}/docs/a</loc></url>
  <url><loc>http://{addr}/docs/b</loc></url>
</urlset>"#
        );
        let sitemap_s = sitemap.to_string();

        let app = Router::new()
            .route(
                "/robots.txt",
                get(move || {
                    let body = robots_s.clone();
                    async move { ([("content-type", "text/plain")], body) }
                }),
            )
            .route(
                "/sitemap.xml",
                get(move || {
                    let body = sitemap_s.clone();
                    async move { ([("content-type", "application/xml")], body) }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env(
                        "WEBPIPE_CACHE_DIR",
                        std::env::temp_dir().join("webpipe-sitemap-cache"),
                    );
                }),
            )?)
            .await?;

        let tools = service.list_tools(Default::default()).await?;
        let names: BTreeSet<String> = tools
            .tools
            .iter()
            .map(|t| t.name.clone().into_owned())
            .collect();
        assert!(
            names.contains("web_sitemap_extract"),
            "missing web_sitemap_extract tool"
        );

        let site = format!("http://{addr}/docs/");
        let v = call(
            &service,
            "web_sitemap_extract",
            serde_json::json!({
                "site_url": site,
                "max_sitemaps": 1,
                "max_urls": 10,
                "try_default_sitemap": true,
                "restrict_prefix": false,
                "extract": false
            }),
        )
        .await;

        assert_eq!(v["ok"].as_bool(), Some(true));
        let urls = v["urls"].as_array().expect("urls");
        assert!(urls
            .iter()
            .any(|u| u.as_str().unwrap_or("").contains("/docs/a")));
        assert!(urls
            .iter()
            .any(|u| u.as_str().unwrap_or("").contains("/docs/b")));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("sitemap extract contract");
}

#[test]
fn web_sitemap_extract_handles_sitemapindex_and_gz() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let robots = "User-agent: *\n";
        let robots_s = robots.to_string();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();

        let sitemap_index = format!(
            r#"<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <sitemap><loc>http://{addr}/sitemap-pages.xml.gz</loc></sitemap>
</sitemapindex>"#
        );

        let sitemap_pages = format!(
            r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://{addr}/docs/x</loc></url>
</urlset>"#
        );
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        use std::io::Write as _;
        enc.write_all(sitemap_pages.as_bytes()).unwrap();
        let gz = enc.finish().unwrap();

        let index_s = sitemap_index.to_string();
        let gz0 = gz.clone();

        let app = Router::new()
            .route(
                "/robots.txt",
                get(move || {
                    let body = robots_s.clone();
                    async move { ([("content-type", "text/plain")], body) }
                }),
            )
            .route(
                "/sitemap.xml",
                get(move || {
                    let body = index_s.clone();
                    async move { ([("content-type", "application/xml")], body) }
                }),
            )
            .route(
                "/sitemap-pages.xml.gz",
                get(move || {
                    let body = gz0.clone();
                    async move { ([("content-type", "application/x-gzip")], body) }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bin = assert_cmd::cargo::cargo_bin!("webpipe");
        let service = ()
            .serve(TokioChildProcess::new(
                tokio::process::Command::new(bin).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env(
                        "WEBPIPE_CACHE_DIR",
                        std::env::temp_dir().join("webpipe-sitemap-gz-cache"),
                    );
                }),
            )?)
            .await?;

        let tools = service.list_tools(Default::default()).await?;
        let names: BTreeSet<String> = tools
            .tools
            .iter()
            .map(|t| t.name.clone().into_owned())
            .collect();
        assert!(
            names.contains("web_sitemap_extract"),
            "missing web_sitemap_extract tool"
        );

        let site = format!("http://{addr}/docs/");
        let v = call(
            &service,
            "web_sitemap_extract",
            serde_json::json!({
                "site_url": site,
                "max_sitemaps": 3,
                "max_urls": 10,
                "try_default_sitemap": true,
                "restrict_prefix": false,
                "extract": false
            }),
        )
        .await;

        assert_eq!(v["ok"].as_bool(), Some(true));
        let urls = v["urls"].as_array().expect("urls");
        assert!(urls
            .iter()
            .any(|u| u.as_str().unwrap_or("").contains("/docs/x")));

        service.cancel().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .expect("sitemap index/gz contract");
}
