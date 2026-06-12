//! Verifies **TLS with invite-style cert pinning** (headless):
//! 1. Start an embedded server with a self-signed cert.
//! 2. A client that **pins that cert** connects over TLS and registers.
//! 3. A client that does NOT pin it (default roots) is **rejected** at the
//! TLS handshake - proving the channel is actually authenticated.
//!
//! ```text
//! cargo run -p accord-server --example tls_smoke
//! ```

use std::time::Duration;

use accord_proto::RegisterRequest;
use accord_proto::auth_service_client::AuthServiceClient;
use accord_server::tls::{PINNED_DOMAIN, generate_self_signed};
use tokio::sync::oneshot;
use tonic::transport::{Certificate, Channel, ClientTlsConfig};

const PORT: u16 = 50064;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Self-signed cert + TLS server.
    let (cert_pem, key_pem) = generate_self_signed().map_err(|e| anyhow::anyhow!(e))?;
    let config = accord_server::Config {
        bind_addr: format!("127.0.0.1:{PORT}").parse()?,
        database_url: "sqlite:tls-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "tls-smoke-secret".to_owned(),
        access_token_ttl_secs: 3600,
        db_max_connections: 5,
        require_invite: false,
        open_dms: true,
        tls_cert_pem: Some(cert_pem.clone()),
        tls_key_pem: Some(key_pem),
    };
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        accord_server::run_with_shutdown(config, shutdown_rx)
            .await
            .expect("server ran");
    });
    tokio::time::sleep(Duration::from_millis(600)).await;
    let endpoint = format!("https://127.0.0.1:{PORT}");

    // 2. Pinned client connects + registers.
    let pinned = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(cert_pem))
        .domain_name(PINNED_DOMAIN);
    let channel = Channel::from_shared(endpoint.clone())?
        .tls_config(pinned)?
        .connect()
        .await?;
    AuthServiceClient::new(channel)
        .register(RegisterRequest {
            username: format!("tls-{}", uuid::Uuid::now_v7()),
            password: "tlspw12345".into(),
            display_name: "TLS".into(),
            invite_token: String::new(),
            identity_pubkey: Vec::new(),
        })
        .await?;
    println!("[ok] pinned client connected over TLS and registered");

    // 3. Un-pinned client (default roots) must be rejected.
    let unpinned = Channel::from_shared(endpoint.clone())?.tls_config(ClientTlsConfig::new())?;
    match unpinned.connect().await {
        Err(_) => println!("[ok] un-pinned client correctly rejected (cert not trusted)"),
        Ok(_) => anyhow::bail!("un-pinned client should NOT have connected"),
    }

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("\nTLS + CERT PINNING WORKS ");
    Ok(())
}
