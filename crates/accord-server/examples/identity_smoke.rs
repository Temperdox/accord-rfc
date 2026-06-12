//! Verifies key-based account identity on the server (headless):
//! 1. Registering with a public identity key succeeds.
//! 2. Registering a different username with the SAME key is rejected.
//! 3. A different key is accepted.
//! 4. A wrong-length key is rejected.
//! 5. Multiple accounts may register with NO key (stored as NULL).
//!
//! The server only checks length and uniqueness, so this uses raw byte arrays as
//! stand-in keys and keeps the server example free of any crypto dependency.
//!
//! ```text
//! cargo run -p accord-server --example identity_smoke
//! ```

use std::time::Duration;

use accord_proto::RegisterRequest;
use accord_proto::auth_service_client::AuthServiceClient;
use tokio::sync::oneshot;
use tonic::Code;
use tonic::transport::Channel;

const PORT: u16 = 50065;

async fn try_register(ch: &Channel, username: &str, key: Vec<u8>) -> Result<(), Code> {
    AuthServiceClient::new(ch.clone())
        .register(RegisterRequest {
            username: username.to_owned(),
            password: "identitypw1".to_owned(),
            display_name: username.to_owned(),
            invite_token: String::new(),
            identity_pubkey: key,
        })
        .await
        .map(|_| ())
        .map_err(|s| s.code())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = accord_server::Config {
        bind_addr: format!("127.0.0.1:{PORT}").parse()?,
        database_url: "sqlite:identity-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "identity-smoke-secret".to_owned(),
        access_token_ttl_secs: 3600,
        db_max_connections: 5,
        require_invite: false,
        open_dms: true,
        tls_cert_pem: None,
        tls_key_pem: None,
    };
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        accord_server::run_with_shutdown(config, shutdown_rx)
            .await
            .expect("server ran");
    });

    let endpoint = format!("http://127.0.0.1:{PORT}");
    let channel = connect_with_retry(&endpoint).await?;
    println!("[ok] server up");

    let key_a = vec![1u8; 32];
    let key_c = vec![2u8; 32];

    try_register(&channel, "alice", key_a.clone())
        .await
        .map_err(err)?;
    println!("[ok] registered with an identity key");

    match try_register(&channel, "mallory", key_a.clone()).await {
        Err(Code::AlreadyExists) => println!("[ok] duplicate identity key rejected"),
        other => anyhow::bail!("expected AlreadyExists, got {other:?}"),
    }

    try_register(&channel, "carol", key_c).await.map_err(err)?;
    println!("[ok] a different identity key is accepted");

    match try_register(&channel, "badlen", vec![1u8; 10]).await {
        Err(Code::InvalidArgument) => println!("[ok] wrong-length key rejected"),
        other => anyhow::bail!("expected InvalidArgument, got {other:?}"),
    }

    try_register(&channel, "nokey1", Vec::new())
        .await
        .map_err(err)?;
    try_register(&channel, "nokey2", Vec::new())
        .await
        .map_err(err)?;
    println!("[ok] multiple accounts with no key are allowed (NULL)");

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("\nKEY-BASED IDENTITY WORKS");
    Ok(())
}

fn err(code: Code) -> anyhow::Error {
    anyhow::anyhow!("unexpected gRPC error: {code:?}")
}

async fn connect_with_retry(endpoint: &str) -> anyhow::Result<Channel> {
    for _ in 0..50 {
        if let Ok(ch) = Channel::from_shared(endpoint.to_owned())?.connect().await {
            return Ok(ch);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("server did not start in time")
}
