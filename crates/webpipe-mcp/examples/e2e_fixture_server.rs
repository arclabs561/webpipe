use axum::{routing::get, Router};
use clap::Parser;
use std::net::SocketAddr;
use tokio::net::TcpListener;

/// Tiny deterministic HTTP server for eval harnesses.
///
/// This exists to support `webpipe eval-critic-run` / other offline-first tests that
/// expect a `base_url` and a small set of `url_paths`.
#[derive(clap::Parser, Debug)]
struct Args {
    /// Port to bind on localhost. Use 0 for an ephemeral port.
    #[arg(long, default_value_t = 8080)]
    port: u16,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Minimal “docs” content. Keep it small but distinct so extract/chunking is testable.
    let docs_landing = r#"
<html><body>
  <main>
    <h1>Docs</h1>
    <p>Welcome to the docs landing page.</p>
    <nav>
      <a href="/docs/app/getting-started/route-handlers">Route Handlers</a>
      <a href="/docs/app/getting-started/routing">Routing</a>
      <a href="/install">Install</a>
    </nav>
  </main>
</body></html>
"#;
    let docs_route_handlers = r#"
<html><body><main>
  <h1>Route Handlers</h1>
  <p>Route handlers let you create custom request handlers for a given route.</p>
  <p>TAIL_SENTINEL: This page is long enough to survive aggressive boilerplate reduction.</p>
</main></body></html>
"#;
    let docs_routing = r#"
<html><body><main>
  <h1>Routing</h1>
  <p>This page is about routing, but does not mention handlers.</p>
</main></body></html>
"#;

    let install = r#"
<html><body><main>
  <h1>Install</h1>
  <p>Choose your OS:</p>
  <ul>
    <li><a href="/install/linux">Linux</a></li>
  </ul>
</main></body></html>
"#;
    let install_linux = r#"
<html><body><main>
  <h1>Install on Linux</h1>
  <p>Steps:</p>
  <ol>
    <li>Download</li>
    <li>Verify</li>
    <li>Run</li>
  </ol>
</main></body></html>
"#;

    let app = Router::new()
        .route(
            "/docs",
            get({
                let body = docs_landing.to_string();
                move || {
                    let body = body.clone();
                    async move { ([(axum::http::header::CONTENT_TYPE, "text/html")], body) }
                }
            }),
        )
        .route(
            "/docs/app/getting-started/route-handlers",
            get({
                let body = docs_route_handlers.to_string();
                move || {
                    let body = body.clone();
                    async move { ([(axum::http::header::CONTENT_TYPE, "text/html")], body) }
                }
            }),
        )
        .route(
            "/docs/app/getting-started/routing",
            get({
                let body = docs_routing.to_string();
                move || {
                    let body = body.clone();
                    async move { ([(axum::http::header::CONTENT_TYPE, "text/html")], body) }
                }
            }),
        )
        .route(
            "/install",
            get({
                let body = install.to_string();
                move || {
                    let body = body.clone();
                    async move { ([(axum::http::header::CONTENT_TYPE, "text/html")], body) }
                }
            }),
        )
        .route(
            "/install/linux",
            get({
                let body = install_linux.to_string();
                move || {
                    let body = body.clone();
                    async move { ([(axum::http::header::CONTENT_TYPE, "text/html")], body) }
                }
            }),
        );

    let bind = SocketAddr::from(([127, 0, 0, 1], args.port));
    let listener = TcpListener::bind(bind)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {bind}: {e}"));
    let addr = listener.local_addr().expect("local_addr");
    eprintln!("fixture_server_listening: http://{addr}");

    axum::serve(listener, app).await.expect("axum serve");
}
