//! pfm-config
//!
//! Typed, layered application configuration.
//!
//! Values are resolved with the following precedence (highest wins):
//!
//!   1. Environment variables prefixed with `PFM_`, nested via `__`
//!      (e.g. `PFM_SERVER__PORT=9000` -> `server.port`).
//!   2. A TOML file, located via the `PFM_CONFIG` environment variable, or
//!      `config.toml` in the current working directory if present.
//!   3. Built-in defaults ([`Config::default`]).
//!
//! Use [`Config::load`] to obtain a fully-resolved [`Config`].

use std::net::IpAddr;
use std::path::PathBuf;

use figment::providers::{Env, Format, Serialized, Toml};
use pfm_i18n::Locale;
use serde::{Deserialize, Serialize};

/// Top-level application configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub i18n: I18nConfig,
    #[serde(rename = "static")]
    pub static_: StaticConfig,
    pub security: SecurityConfig,
    pub logging: LoggingConfig,
    pub scheduler: SchedulerConfig,
}

/// Background scheduler configuration.
///
/// When `enabled = true` the application spawns an in-process tokio task that
/// materializes due fee-rule rows for every account. The task runs once at
/// startup (catch-up pass) and then every `interval_secs` seconds.
///
/// Example TOML:
/// ```toml
/// [scheduler]
/// enabled       = true
/// interval_secs = 3600   # re-check every hour
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SchedulerConfig {
    /// Whether the scheduler is active. Defaults to `false` (opt-in).
    pub enabled: bool,
    /// How often to run the materialization pass, in seconds. Defaults to
    /// 3600 (one hour).
    pub interval_secs: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 3600,
        }
    }
}

impl Config {
    /// Load configuration using the default discovery rules
    /// (env > `PFM_CONFIG`/`config.toml` file > defaults).
    #[allow(clippy::result_large_err)]
    pub fn load() -> Result<Self, figment::Error> {
        Self::load_from(config_file_path())
    }

    /// Load configuration, optionally merging the TOML file at `path`
    /// (when present) between the defaults and the environment layer.
    #[allow(clippy::result_large_err)]
    pub fn load_from(path: Option<PathBuf>) -> Result<Self, figment::Error> {
        // Layer 1 (lowest priority): built-in defaults, serialized to a dict.
        let mut figment = figment::Figment::from(Serialized::defaults(Config::default()));

        // Layer 2: optional TOML file.
        if let Some(file) = path {
            if file.exists() {
                figment = figment.merge(Toml::file(file));
            }
        }

        // Layer 3 (highest priority): PFM_-prefixed environment variables.
        figment = figment.merge(Env::prefixed("PFM_").split("__"));

        figment.extract()
    }
}

/// Resolve the configuration file path: `PFM_CONFIG` env var, else `config.toml`
/// in the current working directory if it exists, else `None`.
pub fn config_file_path() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("PFM_CONFIG") {
        return Some(PathBuf::from(explicit));
    }
    let fallback = PathBuf::from("config.toml");
    fallback.exists().then_some(fallback)
}

/// HTTP listener settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
        }
    }
}

/// Storage engine selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Sqlite,
    MariaDb,
}

/// Database / persistence settings for the selected engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub engine: Engine,
    /// SQLite: database file path (`:memory:` for an in-memory database).
    pub sqlite_path: Option<String>,
    /// SQLite: apply WAL / synchronous / busy_timeout / foreign_keys on connect.
    pub sqlite_apply_pragmas: bool,
    /// SQLite: maximum pooled connections (keep at 1 for a single process).
    pub sqlite_max_connections: u32,
    /// MariaDB: full connection URL, e.g. `mysql://user:pass@host:3306/db`.
    pub mariadb_url: Option<String>,
    /// MariaDB: maximum pooled connections.
    pub mariadb_max_connections: u32,
    /// MariaDB: minimum pooled connections kept alive.
    pub mariadb_min_connections: u32,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            engine: Engine::Sqlite,
            sqlite_path: Some("data/pfm.db".to_string()),
            sqlite_apply_pragmas: true,
            sqlite_max_connections: 1,
            mariadb_url: None,
            mariadb_max_connections: 10,
            mariadb_min_connections: 1,
        }
    }
}

/// Internationalization / locale settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct I18nConfig {
    pub default_locale: Locale,
    pub available_locales: Vec<Locale>,
}

impl Default for I18nConfig {
    fn default() -> Self {
        Self {
            default_locale: Locale::En,
            available_locales: vec![Locale::En, Locale::It],
        }
    }
}

/// Static asset serving settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaticSource {
    /// Serve files from `dir` on the local filesystem.
    Disk,
    /// Serve from an embedded bundle (falls back to `Disk` in this step).
    Embedded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StaticConfig {
    pub source: StaticSource,
    pub dir: String,
}

impl Default for StaticConfig {
    fn default() -> Self {
        Self {
            source: StaticSource::Disk,
            dir: "web".to_string(),
        }
    }
}

/// Logging / observability settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// RUST_LOG-style filter expression.
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
        }
    }
}

/// Security-related settings: password hashing, session cookies, login rate
/// limiting and proxy trust.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Argon2id parameters used to hash / verify passwords.
    pub argon2: Argon2Config,
    /// Lifetime of the session cookie, in seconds.
    pub session_max_age_s: u64,
    /// Name of the (HttpOnly) encrypted session cookie.
    pub session_cookie_name: String,
    /// Name of the (JS-readable) CSRF double-submit cookie.
    pub csrf_cookie_name: String,
    /// Optional secret for encrypting the stateless session cookie. When empty,
    /// the application generates an ephemeral key at startup (issuing new
    /// cookies that do not survive a restart).
    pub session_secret: Option<String>,
    /// When true, `X-Forwarded-For` / `X-Forwarded-Proto` headers are trusted to
    /// recover the real client address/protocol (i.e. the app sits behind a
    /// TLS-terminating reverse proxy). Drives the `Secure` flag on cookies.
    ///
    /// The actual IP from the header is only used when the *source* address of
    /// the TCP connection is in [`trusted_proxies`]. When `trust_proxy` is
    /// false, `X-Forwarded-*` headers are ignored entirely.
    pub trust_proxy: bool,
    /// Whether session / CSRF cookies carry the `Secure` flag (and responses
    /// carry `Strict-Transport-Security`). `None` follows `trust_proxy`; set
    /// it explicitly to `true` when TLS is terminated in front of the app even
    /// if `X-Forwarded-For` is not trusted, or to `false` for plain-HTTP
    /// development behind a proxy.
    pub secure_cookies: Option<bool>,
    /// List of IP addresses / CIDR ranges that are trusted to forward
    /// `X-Forwarded-For` / `X-Forwarded-Proto` headers. Only when the
    /// source address of the incoming TCP connection matches one of these
    /// values will the forwarded headers be consulted. When the list is
    /// empty and `trust_proxy` is true, the `X-Forwarded-For` header is
    /// trusted unconditionally (useful in development behind a single
    /// localhost proxy).
    pub trusted_proxies: Vec<String>,
    /// Content-Security-Policy header value sent on every response.
    /// Default: `default-src 'self'; script-src 'self'; style-src 'self'
    /// 'unsafe-inline'; img-src 'self' data:`.
    pub csp: String,
    /// Login brute-force throttling.
    pub rate_limit_login: RateLimitConfig,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            argon2: Argon2Config::default(),
            session_max_age_s: 60 * 60 * 24, // 24h
            session_cookie_name: "pfm_session".to_string(),
            csrf_cookie_name: "pfm_csrf".to_string(),
            session_secret: None,
            trust_proxy: false,
            secure_cookies: None,
            trusted_proxies: Vec::new(),
            csp: "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:".to_string(),
            rate_limit_login: RateLimitConfig::default(),
        }
    }
}

/// Argon2id cost parameters (OWASP-recommended defaults).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Argon2Config {
    /// Memory cost in KiB.
    pub memory_kib: u32,
    /// Number of iterations.
    pub iterations: u32,
    /// Degree of parallelism.
    pub parallelism: u32,
}

impl Default for Argon2Config {
    fn default() -> Self {
        Self {
            memory_kib: 64 * 1024, // 64 MiB
            iterations: 3,
            parallelism: 4,
        }
    }
}

/// Login rate-limit window.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitConfig {
    /// Maximum login attempts allowed within a window, per (ip, username).
    /// The same window also caps one client address across *all* usernames at
    /// six times this value, which stops password spraying.
    pub per_min: u32,
    /// Length of the counting window, in seconds.
    pub window_s: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            per_min: 5,
            window_s: 60,
        }
    }
}

/// Directives every deployment must enforce regardless of what an (older)
/// config file carries in `csp`: clickjacking, form hijacking, `<base>` and
/// plugin injection. They are appended only when the configured policy does
/// not already set them.
const MANDATORY_CSP_DIRECTIVES: &[(&str, &str)] = &[
    ("frame-ancestors", "'none'"),
    ("form-action", "'self'"),
    ("base-uri", "'self'"),
    ("object-src", "'none'"),
];

impl SecurityConfig {
    /// Whether cookies are issued with `Secure` (and HSTS is sent).
    pub fn secure_cookies_effective(&self) -> bool {
        self.secure_cookies.unwrap_or(self.trust_proxy)
    }

    /// The configured CSP plus the mandatory anti-framing / anti-hijack
    /// directives that it does not already define.
    pub fn effective_csp(&self) -> String {
        let mut csp = self.csp.trim().trim_end_matches(';').to_string();
        for (name, value) in MANDATORY_CSP_DIRECTIVES {
            let present = csp
                .split(';')
                .any(|d| d.split_whitespace().next() == Some(*name));
            if !present {
                if !csp.is_empty() {
                    csp.push_str("; ");
                }
                csp.push_str(name);
                csp.push(' ');
                csp.push_str(value);
            }
        }
        csp
    }

    /// Human-readable configuration problems worth logging at startup.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for entry in &self.trusted_proxies {
            if parse_proxy_entry(entry).is_none() {
                out.push(format!(
                    "security.trusted_proxies: '{entry}' is not a valid IP address or CIDR \
                     range and is ignored"
                ));
            }
        }
        if self.trust_proxy && self.trusted_proxies.is_empty() {
            out.push(
                "security.trust_proxy is on but security.trusted_proxies is empty: \
                 X-Forwarded-For is accepted from ANY peer. Anyone able to reach the \
                 listener directly can spoof their address and dodge login throttling; \
                 list the proxy address(es) or firewall the port."
                    .to_string(),
            );
        }
        if self.session_secret.is_none() {
            out.push(
                "security.session_secret is not set: a random key is generated at each \
                 start and all sessions are lost on restart"
                    .to_string(),
            );
        }
        out
    }
}

/// Parse a `trusted_proxies` entry: a bare IP (`10.0.0.1`) or a CIDR block
/// (`10.0.0.0/8`). The address is canonicalised so an IPv4-mapped IPv6 entry
/// matches the IPv4 peer.
fn parse_proxy_entry(entry: &str) -> Option<(IpAddr, u8)> {
    let entry = entry.trim();
    let (addr, prefix) = match entry.split_once('/') {
        Some((a, p)) => (a.trim(), Some(p.trim())),
        None => (entry, None),
    };
    let ip: IpAddr = addr.parse().ok()?;
    let ip = ip.to_canonical();
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let prefix = match prefix {
        Some(p) => {
            let n: u8 = p.parse().ok()?;
            if n > max {
                return None;
            }
            n
        }
        None => max,
    };
    Some((ip, prefix))
}

/// Whether `ip` lies inside `net/prefix` (families must match).
fn ip_in_net(ip: IpAddr, net: IpAddr, prefix: u8) -> bool {
    fn bits(ip: IpAddr) -> u128 {
        match ip {
            IpAddr::V4(v4) => u32::from(v4) as u128,
            IpAddr::V6(v6) => u128::from(v6),
        }
    }
    match (ip, net) {
        (IpAddr::V4(_), IpAddr::V4(_)) => {
            let shift = 32 - u32::from(prefix);
            shift == 32 || (bits(ip) >> shift) == (bits(net) >> shift)
        }
        (IpAddr::V6(_), IpAddr::V6(_)) => {
            let shift = 128 - u32::from(prefix);
            shift == 128 || (bits(ip) >> shift) == (bits(net) >> shift)
        }
        _ => false,
    }
}

fn is_trusted_proxy(ip: IpAddr, security: &SecurityConfig) -> bool {
    security
        .trusted_proxies
        .iter()
        .filter_map(|e| parse_proxy_entry(e))
        .any(|(net, prefix)| ip_in_net(ip, net, prefix))
}

/// Resolve the effective client IP address of a request.
///
/// `source` is the address of the TCP peer (never the `ip:port` string: the
/// port changes on every connection and would defeat per-client throttling).
///
/// With `trust_proxy` off, `X-Forwarded-For` is ignored. With it on, the
/// header is consulted only when the peer is a trusted proxy (any peer if the
/// list is empty, which [`SecurityConfig::warnings`] flags). The header is
/// read **right to left**: every proxy appends the address it saw, so the
/// rightmost entry that is not itself a trusted proxy is the last address a
/// trusted hop vouches for, while the leftmost one is whatever the client
/// chose to send.
///
/// This is the single place where `X-Forwarded-For` is consulted, so that rate
/// limiting and audit logging always agree on the client IP.
pub fn resolve_client_ip(
    source: IpAddr,
    x_forwarded_for: Option<&str>,
    security: &SecurityConfig,
) -> String {
    let source = source.to_canonical();
    if !security.trust_proxy {
        return source.to_string();
    }

    let peer_trusted = security.trusted_proxies.is_empty() || is_trusted_proxy(source, security);
    if !peer_trusted {
        return source.to_string();
    }

    if let Some(xff) = x_forwarded_for {
        for part in xff.rsplit(',') {
            let Ok(ip) = part.trim().parse::<IpAddr>() else {
                // Garbage in the chain: stop trusting it from here on.
                break;
            };
            let ip = ip.to_canonical();
            if !security.trusted_proxies.is_empty() && is_trusted_proxy(ip, security) {
                continue;
            }
            return ip.to_string();
        }
    }

    source.to_string()
}

#[cfg(test)]
mod ip_tests {
    use super::*;

    fn cfg(trust: bool, proxies: &[&str]) -> SecurityConfig {
        SecurityConfig {
            trust_proxy: trust,
            trusted_proxies: proxies.iter().map(|s| s.to_string()).collect(),
            ..SecurityConfig::default()
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn untrusted_mode_ignores_header_and_has_no_port() {
        let c = cfg(false, &[]);
        assert_eq!(
            resolve_client_ip(ip("192.168.1.9"), Some("6.6.6.6"), &c),
            "192.168.1.9"
        );
    }

    #[test]
    fn rightmost_untrusted_entry_wins_over_spoofed_leftmost() {
        let c = cfg(true, &["10.0.0.1"]);
        // client sends "6.6.6.6"; nginx appends the real peer 203.0.113.7.
        assert_eq!(
            resolve_client_ip(ip("10.0.0.1"), Some("6.6.6.6, 203.0.113.7"), &c),
            "203.0.113.7"
        );
    }

    #[test]
    fn trusted_hops_are_skipped_from_the_right() {
        let c = cfg(true, &["10.0.0.0/8"]);
        assert_eq!(
            resolve_client_ip(ip("10.1.2.3"), Some("198.51.100.4, 10.9.9.9"), &c),
            "198.51.100.4"
        );
    }

    #[test]
    fn peer_outside_trusted_list_cannot_set_the_address() {
        let c = cfg(true, &["10.0.0.1"]);
        assert_eq!(
            resolve_client_ip(ip("192.168.1.100"), Some("10.0.0.42"), &c),
            "192.168.1.100"
        );
    }

    #[test]
    fn cidr_is_matched_by_prefix_not_by_string() {
        let c = cfg(true, &["10.0.0.0/8"]);
        assert_eq!(
            resolve_client_ip(ip("10.200.3.4"), Some("203.0.113.1"), &c),
            "203.0.113.1"
        );
        // "10.0.0.1" must not match "10.0.0.10" as a string prefix would.
        let c = cfg(true, &["10.0.0.1"]);
        assert_eq!(
            resolve_client_ip(ip("10.0.0.10"), Some("1.2.3.4"), &c),
            "10.0.0.10"
        );
    }

    #[test]
    fn empty_list_trusts_any_peer_but_still_reads_from_the_right() {
        let c = cfg(true, &[]);
        assert_eq!(
            resolve_client_ip(ip("127.0.0.1"), Some("6.6.6.6, 203.0.113.7"), &c),
            "203.0.113.7"
        );
    }

    #[test]
    fn mapped_ipv6_peer_matches_ipv4_entry() {
        let c = cfg(true, &["127.0.0.1"]);
        assert_eq!(
            resolve_client_ip(ip("::ffff:127.0.0.1"), Some("203.0.113.9"), &c),
            "203.0.113.9"
        );
    }

    #[test]
    fn garbage_header_falls_back_to_peer() {
        let c = cfg(true, &["127.0.0.1"]);
        assert_eq!(
            resolve_client_ip(ip("127.0.0.1"), Some("not-an-ip"), &c),
            "127.0.0.1"
        );
    }

    #[test]
    fn csp_gets_mandatory_directives_once() {
        let c = SecurityConfig::default();
        let csp = c.effective_csp();
        for d in [
            "frame-ancestors 'none'",
            "form-action 'self'",
            "base-uri 'self'",
            "object-src 'none'",
        ] {
            assert_eq!(csp.matches(d).count(), 1, "{d} in {csp}");
        }
        let c = SecurityConfig {
            csp: "default-src 'self'; frame-ancestors 'self'".into(),
            ..SecurityConfig::default()
        };
        let csp = c.effective_csp();
        assert!(csp.contains("frame-ancestors 'self'"));
        assert!(!csp.contains("frame-ancestors 'none'"));
    }
}
