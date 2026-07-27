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

/// Development-only default database credentials. Matches docker-compose.yml
/// service credentials + remapped host ports (55432/56379 avoid clashing with
/// local Postgres/Redis). [`Config::validate`] refuses to serve on a
/// non-loopback address while this is still in effect.
const DEV_DATABASE_URL: &str = "postgres://accord:accord@localhost:55432/accord";

/// Development-only default JWT secret; same deal as [`DEV_DATABASE_URL`].
const DEV_JWT_SECRET: &str = "dev-only-insecure-secret-change-me";

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:50051".parse().expect("valid default bind addr"),
            database_url: DEV_DATABASE_URL.to_owned(),
            redis_url: "redis://localhost:56379".to_owned(),
            jwt_secret: DEV_JWT_SECRET.to_owned(),
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

    /// Guard against accidentally exposing a dev-configured server.
    ///
    /// Serving on a non-loopback address while the dev `database_url` or
    /// `jwt_secret` defaults are still in effect would let anyone who can
    /// reach the port forge access tokens or try the well-known database
    /// credentials, so startup refuses instead. Loopback binds (the `cargo
    /// run` dev flow) are exempt, as are deployments that override the
    /// defaults - the embedded client server always generates its own secret
    /// and SQLite URL. Set `ACCORD_ALLOW_DEV_CREDENTIALS=1` to bypass for
    /// deliberate LAN testing.
    ///
    /// # Errors
    /// Returns a message naming the offending fields when the bind address is
    /// non-loopback and dev defaults are still present.
    pub fn validate(&self) -> Result<(), String> {
        if self.bind_addr.ip().is_loopback()
            || std::env::var_os("ACCORD_ALLOW_DEV_CREDENTIALS").is_some_and(|v| v == "1")
        {
            return Ok(());
        }
        let mut dev_fields = Vec::new();
        if self.database_url == DEV_DATABASE_URL {
            dev_fields.push("database_url (ACCORD_DATABASE_URL)");
        }
        if self.jwt_secret == DEV_JWT_SECRET {
            dev_fields.push("jwt_secret (ACCORD_JWT_SECRET)");
        }
        if dev_fields.is_empty() {
            return Ok(());
        }
        Err(format!(
            "refusing to bind {} with development credentials still set: {}. \
             Override them (env vars in parentheses) or set \
             ACCORD_ALLOW_DEV_CREDENTIALS=1 for deliberate LAN testing.",
            self.bind_addr,
            dev_fields.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_loopback_config_is_valid() {
        Config::default()
            .validate()
            .expect("loopback + dev defaults is the supported dev flow");
    }

    #[test]
    fn non_loopback_with_dev_defaults_is_refused() {
        let mut config = Config::default();
        config.bind_addr = "0.0.0.0:50051".parse().expect("valid addr");
        let err = config
            .validate()
            .expect_err("dev credentials must not be exposed");
        assert!(
            err.contains("database_url"),
            "names the leaked field: {err}"
        );
        assert!(err.contains("jwt_secret"), "names the leaked field: {err}");
    }

    #[test]
    fn non_loopback_with_overridden_credentials_is_valid() {
        let mut config = Config::default();
        config.bind_addr = "[::]:50051".parse().expect("valid addr");
        config.database_url = "sqlite:/tmp/test.db".to_owned();
        config.jwt_secret = "a-real-secret".to_owned();
        config
            .validate()
            .expect("overridden credentials may be exposed");
    }
}
