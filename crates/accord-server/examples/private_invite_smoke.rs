//! Verifies the **private-server invite gate** end-to-end (headless):
//! 1. Start an embedded server with `require_invite = true`, `open_dms = false`.
//! 2. First account registers with NO invite -> succeeds (becomes owner).
//! 3. A non-owner registering with NO invite -> REJECTED.
//! 4. Owner mints an invite token (CreateInvite).
//! 5. A new account registers WITH that invite -> succeeds.
//! 6. After the owner revokes it, the same token is rejected.
//!
//! Connects over the machine's LAN address, not loopback: loopback registrations
//! are treated as the device owner (multi-account) and open_dms is off here, so
//! the invite gate is what's under test.
//!
//! ```text
//! cargo run -p accord-server --example private_invite_smoke
//! ```

use std::time::Duration;

use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::{CreateInviteRequest, LoginRequest, RegisterRequest, RevokeInviteRequest};
use tokio::sync::oneshot;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request};

const PORT: u16 = 50062;

fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let v: MetadataValue<_> = format!("Bearer {token}").parse().unwrap();
    req.metadata_mut().insert("authorization", v);
    req
}

async fn register(ch: &Channel, username: &str, invite: &str) -> Result<(), tonic::Status> {
    AuthServiceClient::new(ch.clone())
        .register(RegisterRequest {
            username: username.into(),
            password: "invitepw123".into(),
            display_name: username.into(),
            invite_token: invite.into(),
            identity_pubkey: Vec::new(),
        })
        .await
        .map(|_| ())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Private server, bound on all interfaces so we can reach it over the LAN
    //    address (loopback would be treated as the device owner and skip the gate).
    let config = accord_server::Config {
        bind_addr: format!("0.0.0.0:{PORT}").parse()?,
        database_url: "sqlite:invite-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "invite-smoke-secret".to_owned(),
        access_token_ttl_secs: 3600,
        db_max_connections: 5,
        require_invite: true,
        open_dms: false,
        tls_cert_pem: None,
        tls_key_pem: None,
    };
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        accord_server::run_with_shutdown(config, shutdown_rx)
            .await
            .expect("server ran");
    });

    let lan_ip = local_ip_address::local_ip()
        .map_err(|e| anyhow::anyhow!("need a non-loopback LAN address for this test: {e}"))?;
    let endpoint = format!("http://{lan_ip}:{PORT}");
    let channel = connect_with_retry(&endpoint).await?;
    println!("[ok] private server up on {endpoint}");

    // 2. First user = owner, no invite needed.
    register(&channel, "owner", "").await?;
    println!("[ok] first account registered (owner), no invite needed");

    // 3. Non-owner without invite -> rejected.
    match register(&channel, "intruder", "").await {
        Err(s) if s.code() == Code::PermissionDenied => {
            println!("[ok] registration without invite correctly rejected");
        }
        other => anyhow::bail!("expected PermissionDenied, got {other:?}"),
    }

    // 4. Owner logs in and mints an invite.
    let owner_token = AuthServiceClient::new(channel.clone())
        .login(LoginRequest {
            username: "owner".into(),
            password: "invitepw123".into(),
            device_name: "owner-dev".into(),
        })
        .await?
        .into_inner()
        .access_token;
    let invite = AuthServiceClient::new(channel.clone())
        .create_invite(authed(Request::new(CreateInviteRequest {}), &owner_token))
        .await?
        .into_inner()
        .token;
    println!("[ok] owner minted an invite token ({} chars)", invite.len());

    // 5. New account WITH the invite -> succeeds.
    register(&channel, "guest", &invite).await?;
    println!("[ok] account registered using the invite");

    // 6. Revoke -> same token now rejected.
    AuthServiceClient::new(channel.clone())
        .revoke_invite(authed(
            Request::new(RevokeInviteRequest {
                token: invite.clone(),
            }),
            &owner_token,
        ))
        .await?;
    match register(&channel, "latecomer", &invite).await {
        Err(s) if s.code() == Code::PermissionDenied => {
            println!("[ok] revoked invite correctly rejected");
        }
        other => anyhow::bail!("expected PermissionDenied after revoke, got {other:?}"),
    }

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("\nPRIVATE-SERVER INVITE GATE WORKS ");
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
