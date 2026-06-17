//! Verifies **password-less key authentication** end-to-end (headless):
//! 1. Register a KEY-ONLY account (empty password + an Ed25519 identity key).
//! 2. RequestChallenge -> sign it with the identity key -> LoginWithKey succeeds.
//! 3. A bad signature is rejected.
//! 4. A challenge issued for one key can't be used to log in as a different key.
//! 5. Password login on a key-only account fails (no password set).
//!
//! This is how a user joins a tavern without typing a per-tavern password.
//!
//! ```text
//! cargo run -p accord-server --example key_auth_smoke
//! ```

use std::time::Duration;

use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::{
    ChallengeRequest, KeyLoginRequest, LoginRequest, RegisterRequest,
};
use ed25519_dalek::{Signer, SigningKey};
use tokio::sync::oneshot;
use tonic::transport::Channel;
use tonic::{Code, Request};

const PORT: u16 = 50065;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = std::fs::remove_file("key-auth-smoke.db");
    let config = accord_server::Config {
        bind_addr: format!("127.0.0.1:{PORT}").parse()?,
        database_url: "sqlite:key-auth-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "key-auth-smoke-secret".to_owned(),
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
    println!("[ok] server up on {endpoint}");

    // The client's per-server identity key (a fixed seed keeps the smoke
    // deterministic; the real client derives this from its master key).
    let signing = SigningKey::from_bytes(&[7u8; 32]);
    let pubkey = signing.verifying_key().to_bytes().to_vec();

    let mut auth = AuthServiceClient::new(channel.clone());

    // 1. Register a KEY-ONLY account: empty password + the identity key.
    auth.register(RegisterRequest {
        username: "keyuser".into(),
        password: String::new(),
        display_name: "Key User".into(),
        invite_token: String::new(),
        identity_pubkey: pubkey.clone(),
    })
    .await?;
    println!("[ok] registered a key-only account (no password)");

    // 2. Challenge -> sign -> login.
    let challenge = auth
        .request_challenge(Request::new(ChallengeRequest {
            identity_pubkey: pubkey.clone(),
        }))
        .await?
        .into_inner()
        .challenge;
    let signature = signing.sign(challenge.as_bytes()).to_bytes().to_vec();
    let resp = auth
        .login_with_key(Request::new(KeyLoginRequest {
            identity_pubkey: pubkey.clone(),
            challenge: challenge.clone(),
            signature,
            device_name: "dev".into(),
        }))
        .await?
        .into_inner();
    anyhow::ensure!(!resp.access_token.is_empty(), "key login returned a token");
    println!("[ok] challenge-response key login succeeded");

    // 3. A bad signature is rejected.
    let challenge2 = auth
        .request_challenge(Request::new(ChallengeRequest {
            identity_pubkey: pubkey.clone(),
        }))
        .await?
        .into_inner()
        .challenge;
    let bad_sig = signing.sign(b"not the challenge").to_bytes().to_vec();
    let bad = auth
        .login_with_key(Request::new(KeyLoginRequest {
            identity_pubkey: pubkey.clone(),
            challenge: challenge2,
            signature: bad_sig,
            device_name: "dev".into(),
        }))
        .await;
    anyhow::ensure!(bad.is_err(), "a bad signature must be rejected");
    println!("[ok] bad signature rejected");

    // 4. A challenge issued for a DIFFERENT key can't be used here.
    let other = SigningKey::from_bytes(&[9u8; 32]);
    let other_pubkey = other.verifying_key().to_bytes().to_vec();
    let other_challenge = auth
        .request_challenge(Request::new(ChallengeRequest {
            identity_pubkey: other_pubkey,
        }))
        .await?
        .into_inner()
        .challenge;
    // Sign it correctly with OUR key, but the challenge was bound to the other key.
    let sig = signing.sign(other_challenge.as_bytes()).to_bytes().to_vec();
    let mismatched = auth
        .login_with_key(Request::new(KeyLoginRequest {
            identity_pubkey: pubkey.clone(),
            challenge: other_challenge,
            signature: sig,
            device_name: "dev".into(),
        }))
        .await;
    anyhow::ensure!(
        mismatched.is_err(),
        "a challenge bound to another key must not authenticate this one"
    );
    println!("[ok] challenge is bound to its identity key");

    // 5. Password login on a key-only account fails (no password set).
    let pw = auth
        .login(LoginRequest {
            username: "keyuser".into(),
            password: "anything".into(),
            device_name: "dev".into(),
        })
        .await;
    match pw {
        Err(s) if s.code() == Code::Unauthenticated => {
            println!("[ok] password login refused for a key-only account");
        }
        other => anyhow::bail!("expected Unauthenticated, got {other:?}"),
    }

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    let _ = std::fs::remove_file("key-auth-smoke.db");
    println!("\nKEY-BASED AUTH (password-less tavern join) WORKS");
    Ok(())
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
