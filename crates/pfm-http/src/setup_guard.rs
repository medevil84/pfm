//! Startup guard: when the database is NOT configured and the process was NOT
//! started with `--setup`, the server serves ONLY a minimal "setup required"
//! page. No application routes, no static assets from the main frontend.
//!
//! This is implemented as a tower `Service` that intercepts all requests and
//! returns the guard page HTML, regardless of path.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

/// A tower service that returns the "setup required" page for ALL requests.
/// This replaces the entire router when the DB is not configured and `--setup`
/// is not set.
#[derive(Clone, Default)]
pub struct SetupGuardService {
    html: String,
}

impl SetupGuardService {
    pub fn new() -> Self {
        Self { html: guard_html() }
    }
}

impl tower::Service<Request<Body>> for SetupGuardService {
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: Request<Body>) -> Self::Future {
        let html = self.html.clone();
        Box::pin(async move {
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(Body::from(html))
                .unwrap())
        })
    }
}

/// The minimal HTML page shown when the app requires initial setup but
/// `--setup` was not passed.
fn guard_html() -> String {
    r#"<!DOCTYPE html>
<html lang="it">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>PFM — Configurazione richiesta</title>
<style>
:root { color-scheme: light; }
* { box-sizing: border-box; margin: 0; padding: 0; }
body {
  min-height: 100vh; display: flex; align-items: center; justify-content: center;
  background: #161b2e; color: #e6e9f0;
  font-family: 'Inter', system-ui, -apple-system, 'Segoe UI', sans-serif;
  padding: 1.5rem;
}
.card {
  background: #1e2432; border: 1px solid #2b3346; border-radius: 14px;
  padding: 2.5rem 2rem; max-width: 480px; width: 100%; text-align: center;
}
.icon {
  width: 56px; height: 56px; border-radius: 50%; background: #1d2740;
  display: flex; align-items: center; justify-content: center; margin: 0 auto 1.2rem;
  font-size: 1.5rem; color: #7aa2ff;
}
h1 { font-size: 1.2rem; font-weight: 700; margin-bottom: .8rem; color: #e6e9f0; }
p { font-size: .92rem; color: #9aa5b8; line-height: 1.6; margin-bottom: 1rem; }
code {
  background: #252c3c; border: 1px solid #2b3346; border-radius: 6px;
  padding: .2em .5em; font-size: .88rem;
  font-family: 'Cascadia Code', 'JetBrains Mono', monospace; color: #7aa2ff;
}
.cmd {
  background: #252c3c; border: 1px solid #2b3346; border-radius: 10px;
  padding: .8rem 1rem; margin-top: 1.2rem; text-align: left;
  font-family: 'Cascadia Code', 'JetBrains Mono', monospace; font-size: .9rem;
  color: #e6e9f0;
}
.cmd span { color: #7aa2ff; }
</style>
</head>
<body>
<div class="card">
  <div class="icon">&#9881;</div>
  <h1>Configurazione iniziale non completata</h1>
  <p>
    L'applicazione non &egrave; ancora stata configurata.
    Avvia il processo di primo avvio aggiungendo il flag <code>--setup</code>:
  </p>
  <div class="cmd"><span>$</span> pfm-app --setup</div>
</div>
</body>
</html>"#
        .to_string()
}
