//! # accord-server (binary)
//!
//! Thin entrypoint: load config, initialize logging, and hand off to the
//! [`accord_server`] library. All service wiring lives in the library so the
//! desktop client can embed and run the exact same server in-process for
//! self-hosting.

use accord_server::Config;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let config = Config::load()?;
    tracing::info!(bind = %config.bind_addr, "starting accord-server");
    accord_server::run(config).await
}

/// Initialize structured logging. Honors `RUST_LOG`; defaults to info (with
/// debug for our own crate) when unset.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,accord_server=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}
