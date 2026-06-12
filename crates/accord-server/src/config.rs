//! Server configuration.
//!
//! Values are layered with [`figment`] (ARCHITECTURE.md section 8.1) in this order
//! (later overrides earlier):
//! 1. Built-in defaults (point at the local `docker-compose` services).
//! 2. An optional `accord-server.toml` file in the working directory.
//! 3. Environment variables prefixed `ACCORD_` (e.g. `ACCORD_DATABASE_URL`).
//!
//! This means `cargo run` "just works" against the bundled Docker stack, while
//! deployments override via env vars without code changes.

use std::net::SocketAddr;

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

/// Top-level server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Address the gRPC server listens on.
    pub bind_addr: SocketAddr,
    /// PostgreSQL connection string.
    pub database_url: String,
    /// Redis (or KeyDB) connection string, used for real-time pub/sub fan-out.
    pub redis_url: String,
    /// HMAC secret used to sign/verify JWT access tokens.
    ///
    /// MUST be overridden in production via `ACCORD_JWT_SECRET`. The default is
    /// for local development only.
    pub jwt_secret: String,
    /// Access-token lifetime in seconds.
    pub access_token_ttl_secs: u64,
    /// Max PostgreSQL connections in the pool.
    pub db_max_connections: u32,
    /// When true, this is a **private** server: registration requires a valid
    /// invite token (except the first account, which becomes the owner).
    #[serde(default)]
    pub require_invite: bool,
    /// When true (default), a contact may register on this host to open a DM with
    /// a user here, even on a private server - registration is allowed but server
    /// channels still require an invite. This is what makes cross-user DMs work
    /// (the DM lives on the recipient's host; the initiator joins as a guest).
    /// Set false to refuse DMs from non-members. See BAN-PLAN.md for abuse
    /// controls (blocking, future proof-of-work).
    #[serde(default = "default_true")]
    pub open_dms: bool,
    /// PEM-encoded TLS certificate. When both this and `tls_key_pem` are set,
    /// the server serves over TLS (self-signed + pinned via the invite key).
    #[serde(default)]
    pub tls_cert_pem: Option<String>,
    /// PEM-encoded TLS private key (paired with `tls_cert_pem`).
    #[serde(default)]
    pub tls_key_pem: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:50051".parse().expect("valid default bind addr"),
            // Matches docker-compose.yml service credentials + remapped host
            // ports (55432/56379 avoid clashing with local Postgres/Redis).
            database_url: "postgres://accord:accord@localhost:55432/accord".to_owned(),
            redis_url: "redis://localhost:56379".to_owned(),
            jwt_secret: "dev-only-insecure-secret-change-me".to_owned(),
            access_token_ttl_secs: 3600,
            db_max_connections: 10,
            require_invite: false,
            open_dms: true,
            tls_cert_pem: None,
            tls_key_pem: None,
        }
    }
}

/// Serde default for [`Config::open_dms`] (defaults to true).
fn default_true() -> bool {
    true
}

impl Config {
    /// Load configuration from defaults + optional TOML + `ACCORD_` env vars.
    ///
    /// # Errors
    /// Returns an error if the TOML file or env vars cannot be parsed into the
    /// [`Config`] shape.
    pub fn load() -> Result<Self, figment::Error> {
        Figment::new()
            .merge(Serialized::defaults(Config::default()))
            .merge(Toml::file("accord-server.toml"))
            .merge(Env::prefixed("ACCORD_"))
            .extract()
    }
}
