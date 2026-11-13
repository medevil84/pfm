//! pfm-app
//!
//! Application entry point. Wires together configuration, the database pool
//! (with migrations), and the axum HTTP server with cross-platform graceful
//! shutdown.
//!
//! ## Command-line options
//!
//! Run `pfm-app --help` for the full list. The most common flags are:
//!
//! | Flag | Description |
//! |------|-------------|
//! | `--help` / `-h` | Show help and exit |
//! | `--version` / `-v` | Show version and exit |
//! | `--config <PATH>` | Use an explicit configuration file |
//! | `--host <ADDR>` | Override the listen address from the config file |
//! | `--port <PORT>` | Override the listen port from the config file |
//! | `--log-level <LEVEL>` | Override the log filter (e.g. `debug`, `warn`) |
//! | `--migrate` | Run database migrations and exit |
//! | `--setup` | Run the first-time setup wizard |

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use clap::Parser;
use rust_embed::RustEmbed;
use tracing_subscriber::EnvFilter;

use pfm_config::{Config, DatabaseConfig, Engine as ConfigEngine, StaticSource};
use pfm_http::{build_router, setup_guard::SetupGuardService, AppMode};
use pfm_repo::{DbSettings, Engine as RepoEngine, SetupRepo, Storage};

/// Embedded static assets for the `static_source = "embedded"` mode.
/// The `web/` directory at the workspace root is baked into the binary at
/// compile time.
#[derive(RustEmbed)]
#[folder = "../../web/"]
struct EmbeddedAssets;

/// Determine the application mode based on CLI flags and database state.
async fn determine_mode(cli_setup: bool, storage: &Storage) -> AppMode {
    if cli_setup {
        let repo = pfm_repo::SqlxSetupRepo::new(storage);
        let configured = {
            let marker = repo.is_initialized().await.unwrap_or(false);
            if marker {
                repo.has_admin_user().await.unwrap_or(false)
            } else {
                false
            }
        };
        AppMode::Setup { configured }
    } else {
        // No --setup: check if the DB is configured.
        let repo = pfm_repo::SqlxSetupRepo::new(storage);
        let configured = {
            let marker = repo.is_initialized().await.unwrap_or(false);
            if marker {
                repo.has_admin_user().await.unwrap_or(false)
            } else {
                false
            }
        };
        if configured {
            AppMode::Normal
        } else {
            AppMode::Guard
        }
    }
}

/// Return the correct fallback service for static assets based on the
/// configured source and the app mode.
fn static_fallback(config: &Arc<Config>, mode: AppMode) -> Router {
    let dir = config.static_.dir.clone();

    // In Guard mode, serve the guard page for ALL paths (no static assets).
    if matches!(mode, AppMode::Guard) {
        return Router::new().fallback_service(SetupGuardService::new());
    }

    // In Setup mode, serve static assets (wizard, reset page, vendor files).
    // The appropriate page is served at `/` based on the mode's `configured`
    // flag (handled by a special root handler below).
    match config.static_.source {
        StaticSource::Disk => {
            tracing::info!("static source: disk ('{dir}')");
            Router::new().fallback_service(DiskFallback {
                dir: PathBuf::from(dir),
            })
        }
        StaticSource::Embedded => {
            tracing::info!("static source: embedded (rust-embed)");
            Router::new().fallback_service(EmbeddedFallback)
        }
    }
}

/// In setup mode, the root path `/` should serve the appropriate page:
///   - `configured = false` → `/setup_wizard.html`
///   - `configured = true`  → `/setup_reset.html`
///
/// This is handled by a special root handler that returns a redirect or
/// serves the page directly.
fn setup_root_route(configured: bool) -> Router {
    let page = if configured {
        "/setup_reset.html"
    } else {
        "/setup_wizard.html"
    };
    Router::new().route(
        "/",
        axum::routing::get(move || {
            let page = page.to_string();
            async move {
                (
                    axum::http::StatusCode::FOUND,
                    [(axum::http::header::LOCATION, page)],
                )
            }
        }),
    )
}

/// An unmatched `/api/...` path must never be answered with the SPA shell.
///
/// The static fallback exists so that client-side routes such as `/statistiche`
/// survive a browser refresh, but it used to swallow API paths too: a typo or a
/// missing route returned `index.html` with status 200, so callers saw a
/// successful response carrying HTML. That is what let the offline queue
/// discard movements it had never actually delivered. Anything under `/api/`
/// gets an honest JSON 404 instead.
fn api_not_found() -> axum::response::Response {
    axum::response::Response::builder()
        .status(axum::http::StatusCode::NOT_FOUND)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            r#"{"type":"about:blank","title":"Not Found","status":404,"detail":"Endpoint non trovato","code":"errors.not_found"}"#,
        ))
        .unwrap()
}

/// Axum-compatible service that serves files from the `EmbeddedAssets`
/// rust-embed bundle. Falls back to `index.html` for SPA-style routing.
#[derive(Clone, Default)]
struct EmbeddedFallback;

impl tower::Service<axum::http::Request<axum::body::Body>> for EmbeddedFallback {
    type Response = axum::response::Response;
    type Error = std::convert::Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: axum::http::Request<axum::body::Body>) -> Self::Future {
        let path = req.uri().path().to_string();
        let file_key = if path == "/" {
            "index.html".to_string()
        } else {
            path.trim_start_matches('/').to_string()
        };

        Box::pin(async move {
            if let Some(file) = EmbeddedAssets::get(&file_key) {
                let content_type = mime_guess::from_path(&file_key)
                    .first_or(mime_guess::mime::TEXT_HTML_UTF_8)
                    .to_string();
                let data = file.data.clone();
                Ok(axum::response::Response::builder()
                    .status(axum::http::StatusCode::OK)
                    .header(axum::http::header::CONTENT_TYPE, content_type)
                    .body(axum::body::Body::from(data))
                    .unwrap())
            } else if path.starts_with("/api/") || path == "/api" {
                Ok(api_not_found())
            } else {
                // SPA fallback: serve index.html for any unmatched path
                match EmbeddedAssets::get("index.html") {
                    Some(file) => {
                        let data = file.data.clone();
                        Ok(axum::response::Response::builder()
                            .status(axum::http::StatusCode::OK)
                            .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                            .body(axum::body::Body::from(data))
                            .unwrap())
                    }
                    None => Ok(axum::response::Response::builder()
                        .status(axum::http::StatusCode::NOT_FOUND)
                        .body(axum::body::Body::from("Not Found"))
                        .unwrap()),
                }
            }
        })
    }
}

/// Axum-compatible service that serves static files from a directory on
/// disk. When a file is not found, serves `index.html` so that client-side
/// routes (e.g. /statistiche, /conti) work on direct load / browser refresh.
#[derive(Clone)]
struct DiskFallback {
    dir: PathBuf,
}

impl tower::Service<axum::http::Request<axum::body::Body>> for DiskFallback {
    type Response = axum::response::Response;
    type Error = std::convert::Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: axum::http::Request<axum::body::Body>) -> Self::Future {
        let dir = self.dir.clone();
        let path = req.uri().path().to_string();
        let rel = if path == "/" {
            "index.html".to_string()
        } else {
            // Strip leading slash; PathBuf handles both / and \ on Windows.
            path.trim_start_matches('/').to_string()
        };

        Box::pin(async move {
            // The request path is attacker-controlled: resolve it strictly
            // inside `dir` (no `..`, no absolute / drive-prefixed component, no
            // symlink that leads outside). Anything else is treated exactly
            // like a missing file.
            let file_path = safe_join(&dir, &rel);
            if let Some(file_path) = file_path.filter(|p| p.is_file()) {
                match tokio::fs::read(&file_path).await {
                    Ok(data) => {
                        let content_type = mime_guess::from_path(&rel)
                            .first_or(mime_guess::mime::TEXT_HTML_UTF_8)
                            .to_string();
                        Ok(axum::response::Response::builder()
                            .status(axum::http::StatusCode::OK)
                            .header(axum::http::header::CONTENT_TYPE, content_type)
                            .body(axum::body::Body::from(data))
                            .unwrap())
                    }
                    Err(e) => {
                        tracing::warn!("failed to read static file {file_path:?}: {e}");
                        Ok(axum::response::Response::builder()
                            .status(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
                            .body(axum::body::Body::from("Internal Server Error"))
                            .unwrap())
                    }
                }
            } else if path.starts_with("/api/") || path == "/api" {
                Ok(api_not_found())
            } else {
                // SPA fallback: serve index.html for any unmatched path
                let index_path = dir.join("index.html");
                match tokio::fs::read(&index_path).await {
                    Ok(data) => Ok(axum::response::Response::builder()
                        .status(axum::http::StatusCode::OK)
                        .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                        .body(axum::body::Body::from(data))
                        .unwrap()),
                    Err(e) => {
                        tracing::error!("SPA fallback: cannot read index.html: {e}");
                        Ok(axum::response::Response::builder()
                            .status(axum::http::StatusCode::NOT_FOUND)
                            .body(axum::body::Body::from("Not Found"))
                            .unwrap())
                    }
                }
            }
        })
    }
}

/// Join the URL-derived relative path `rel` onto `dir`, refusing anything that
/// could leave `dir`.
///
/// `rel` is the raw request path with the leading slash removed. It is rejected
/// when it contains a backslash, a drive colon or a NUL, or any component other
/// than a plain name (`..`, `.`, a root or a Windows prefix such as `C:`: the
/// latter would even *replace* the base path in `Path::join`). The joined path
/// is then canonicalised and must still sit under the canonical `dir`, which
/// also closes symlinks pointing outside the web root.
fn safe_join(dir: &std::path::Path, rel: &str) -> Option<PathBuf> {
    use std::path::Component;

    if rel.is_empty() || rel.contains(['\\', ':', '\0']) {
        return None;
    }
    let rel_path = std::path::Path::new(rel);
    if !rel_path
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let base = dir.canonicalize().ok()?;
    let candidate = base.join(rel_path).canonicalize().ok()?;
    candidate.starts_with(&base).then_some(candidate)
}

/// PFM – Personal Finance Manager.
///
/// Starts the PFM HTTP server. Configuration is loaded from the file
/// specified by `--config`, the `PFM_CONFIG` environment variable, or
/// `config.toml` in the current directory (in that order of precedence).
/// Individual settings can be overridden with the flags below.
#[derive(Debug, Parser)]
#[command(name = "pfm-app", about = "PFM – Personal Finance Manager HTTP server")]
struct Cli {
    /// Path to an explicit configuration file (TOML).
    /// Overrides the `PFM_CONFIG` environment variable and the default
    /// `config.toml` discovery.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Listen address (e.g. `127.0.0.1`, `0.0.0.0`).
    /// Overrides `server.host` in the configuration file.
    #[arg(long, value_name = "ADDR")]
    host: Option<String>,

    /// Listen port (e.g. `8080`).
    /// Overrides `server.port` in the configuration file.
    #[arg(long, value_name = "PORT")]
    port: Option<u16>,

    /// Log filter expression (e.g. `info`, `debug`, `pfm_http=trace,tokio=warn`).
    /// Overrides the `RUST_LOG` environment variable and `logging.level`
    /// in the configuration file.
    #[arg(long, value_name = "LEVEL")]
    log_level: Option<String>,

    /// Run database migrations and exit (useful for deploying or fixing
    /// schema drift without starting the server).
    #[arg(long)]
    migrate: bool,

    /// Run the first-time setup wizard. After successful completion the
    /// process exits. If the database is already configured, a reset
    /// confirmation page is shown first.
    #[arg(long)]
    setup: bool,

    /// Print version information and exit.
    #[arg(long = "version", short = 'v')]
    version: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.version {
        println!("pfm-app {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // --log-level takes highest priority, then RUST_LOG, then "info".
    init_tracing(cli.log_level.as_deref());

    let config = load_config(&cli).context("loading configuration")?;
    tracing::info!("configuration loaded");

    sqlx::any::install_default_drivers();
    let db_settings = to_db_settings(&config.database);
    let storage = match Storage::connect(&db_settings).await {
        Ok(storage) => {
            let migrator = sqlx::migrate!("../../migrations");
            match storage.migrate(&migrator).await {
                Ok(()) => {
                    tracing::info!("database ready");
                    Some(storage)
                }
                Err(err) => {
                    tracing::error!("database migration failed: {err}");
                    None
                }
            }
        }
        Err(err) => {
            tracing::error!("database connection failed: {err}");
            None
        }
    };

    let Some(storage) = storage else {
        anyhow::bail!("application requires a database connection");
    };

    // --migrate: we already ran migrations above; exit cleanly.
    if cli.migrate {
        tracing::info!("migrations complete – exiting");
        return Ok(());
    }

    // Determine the application mode.
    let mode = determine_mode(cli.setup, &storage).await;
    tracing::info!(?mode, "application mode determined");

    // Apply CLI overrides for host / port on top of the loaded config.
    let host_str = cli.host.unwrap_or_else(|| config.server.host.clone());
    let host: IpAddr = host_str
        .parse()
        .with_context(|| format!("invalid server address: '{host_str}'"))?;
    let port = cli.port.unwrap_or(config.server.port);
    let addr = SocketAddr::new(host, port);

    let config_arc = Arc::new(config.clone());
    let shutdown_notify = Arc::new(tokio::sync::Notify::new());
    let shutdown_notify_clone = Arc::clone(&shutdown_notify);

    // Build the static fallback first and hand it to `build_router`, which
    // merges it *inside* the middleware stack. Merging it here, after the
    // router had already been layered, left every HTML page and script served
    // without security headers — `Router::layer` only wraps the routes that
    // exist when it is called.
    //
    // In Guard mode this is the guard page service; in Setup/Normal mode it
    // serves the web/ directory (disk or embedded).
    let mut static_router = static_fallback(&config_arc, mode);

    // In setup mode, add a root redirect to the appropriate page.
    if let AppMode::Setup { configured } = mode {
        static_router = static_router.merge(setup_root_route(configured));
    }

    let app = build_router(
        Some(Arc::new(storage)),
        config_arc.clone(),
        mode,
        Some(shutdown_notify_clone),
        Some(static_router),
    )?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding TCP listener to {addr}"))?;
    tracing::info!("listening on http://{addr}");

    // `into_make_service_with_connect_info` (called on the Router) injects the
    // peer `SocketAddr` into each request as `ConnectInfo`, which the auth
    // handlers (login/logout/change-password) use for IP-based rate limiting
    // and audit logging. Without it those handlers fail with a 500
    // "Missing request extension: ConnectInfo".
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(shutdown_notify))
    .await
    .context("HTTP server error")?;

    tracing::info!("server stopped");
    Ok(())
}

/// Load configuration, honouring the `--config` flag if present.
fn load_config(cli: &Cli) -> anyhow::Result<Config> {
    match &cli.config {
        Some(path) => {
            tracing::debug!("loading config from explicit path: {path:?}");
            Config::load_from(Some(path.clone())).map_err(anyhow::Error::msg)
        }
        None => Config::load().map_err(anyhow::Error::msg),
    }
}

/// Graceful shutdown: waits for OS signal OR the in-process notify (used by
/// the setup-completion endpoint to trigger exit).
async fn shutdown_signal(notify: Arc<tokio::sync::Notify>) {
    let notified = notify.notified();

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut terminate =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");

        tokio::select! {
            _ = tokio::signal::ctrl_c() => tracing::info!("received ctrl_c, shutting down"),
            _ = terminate.recv() => tracing::info!("received SIGTERM, shutting down"),
            _ = notified => tracing::info!("shutdown triggered programmatically"),
        }
    }

    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received ctrl_c, shutting down");
            }
            _ = notified => tracing::info!("shutdown triggered programmatically"),
        }
    }
}

fn to_db_settings(cfg: &DatabaseConfig) -> DbSettings {
    DbSettings {
        engine: match cfg.engine {
            ConfigEngine::Sqlite => RepoEngine::Sqlite,
            ConfigEngine::MariaDb => RepoEngine::MariaDb,
        },
        sqlite_path: cfg.sqlite_path.clone(),
        sqlite_apply_pragmas: cfg.sqlite_apply_pragmas,
        sqlite_max_connections: cfg.sqlite_max_connections,
        mariadb_url: cfg.mariadb_url.clone(),
        mariadb_max_connections: cfg.mariadb_max_connections,
        mariadb_min_connections: cfg.mariadb_min_connections,
    }
}

fn init_tracing(log_level: Option<&str>) {
    let filter = match log_level {
        Some(level) => EnvFilter::new(level),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    use super::safe_join;
    use std::fs;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pfm-safe-join-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("web/sub")).unwrap();
        fs::write(d.join("web/index.html"), "x").unwrap();
        fs::write(d.join("web/sub/a.js"), "x").unwrap();
        fs::write(d.join("secret.txt"), "top secret").unwrap();
        d
    }

    #[test]
    fn serves_files_inside_the_root() {
        let d = scratch("ok");
        let web = d.join("web");
        assert!(safe_join(&web, "index.html").is_some());
        assert!(safe_join(&web, "sub/a.js").is_some());
        assert!(safe_join(&web, "missing.js").is_none());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_traversal_and_foreign_paths() {
        let d = scratch("evil");
        let web = d.join("web");
        for evil in [
            "../secret.txt",
            "sub/../../secret.txt",
            "../../../../etc/passwd",
            "/etc/passwd",
            "..\\secret.txt",
            "C:/Windows/win.ini",
            "C:\\Windows\\win.ini",
            "index.html\0.png",
        ] {
            assert!(safe_join(&web, evil).is_none(), "{evil} must be refused");
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escaping_the_root() {
        let d = scratch("link");
        let web = d.join("web");
        std::os::unix::fs::symlink(d.join("secret.txt"), web.join("link.txt")).unwrap();
        assert!(safe_join(&web, "link.txt").is_none());
        let _ = fs::remove_dir_all(&d);
    }
}
