//! Verifies the token-refresh path the client's session resilience relies on:
//! 1. Log in -> access token + refresh token.
//! 2. RefreshToken(refresh) -> a fresh access token.
//! 3. The fresh access token authorizes a normal RPC (ListGroups).
//! 4. A bogus token is rejected (Unauthenticated).
//!
//! ```text
//! cargo run -p accord-server --example refresh_smoke
//! ```

use std::time::Duration;

use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::group_service_client::GroupServiceClient;
use accord_proto::{ListGroupsRequest, LoginRequest, RefreshTokenRequest, RegisterRequest};
use tokio::sync::oneshot;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request};

const PORT: u16 = 50068;

fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let v: MetadataValue<_> = format!("Bearer {token}").parse().unwrap();
    req.metadata_mut().insert("authorization", v);
    req
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = accord_server::Config {
        bind_addr: format!("127.0.0.1:{PORT}").parse()?,
        database_url: "sqlite:refresh-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "refresh-smoke-secret".to_owned(),
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

    let mut auth = AuthServiceClient::new(channel.clone());
    auth.register(RegisterRequest {
        username: "frank".into(),
        password: "refreshpw123".into(),
        display_name: "Frank".into(),
        invite_token: String::new(),
        identity_pubkey: Vec::new(),
    })
    .await?;
    let login = auth
        .login(LoginRequest {
            username: "frank".into(),
            password: "refreshpw123".into(),
            device_name: "dev".into(),
        })
        .await?
        .into_inner();
    anyhow::ensure!(
        !login.refresh_token.is_empty(),
        "login returned no refresh token"
    );
    println!("[ok] login returned access + refresh tokens");

    // Exchange the refresh token for a fresh access token (and rotation).
    let refreshed = auth
        .refresh_token(Request::new(RefreshTokenRequest {
            refresh_token: login.refresh_token.clone(),
        }))
        .await?
        .into_inner();
    anyhow::ensure!(
        !refreshed.refresh_token.is_empty() && refreshed.refresh_token != login.refresh_token,
        "refresh should rotate the refresh token"
    );
    println!("[ok] refresh token rotated");

    // The OLD refresh token must now be dead.
    match auth
        .refresh_token(Request::new(RefreshTokenRequest {
            refresh_token: login.refresh_token.clone(),
        }))
        .await
    {
        Err(s) if s.code() == Code::Unauthenticated => {
            println!("[ok] consumed refresh token rejected");
        }
        other => anyhow::bail!("expected Unauthenticated for old refresh token, got {other:?}"),
    }

    // The rotated refresh token works.
    let new_access = auth
        .refresh_token(Request::new(RefreshTokenRequest {
            refresh_token: refreshed.refresh_token.clone(),
        }))
        .await?
        .into_inner()
        .access_token;
    anyhow::ensure!(!new_access.is_empty(), "refresh returned no access token");
    println!("[ok] refresh token minted a fresh access token");

    // The fresh token authorizes a normal RPC.
    let mut groups = GroupServiceClient::new(channel);
    groups
        .list_groups(authed(Request::new(ListGroupsRequest {}), &new_access))
        .await?;
    println!("[ok] refreshed access token authorizes RPCs");

    // A bogus token is rejected.
    match groups
        .list_groups(authed(
            Request::new(ListGroupsRequest {}),
            "not-a-real-token",
        ))
        .await
    {
        Err(s) if s.code() == Code::Unauthenticated => println!("[ok] bogus token rejected"),
        other => anyhow::bail!("expected Unauthenticated, got {other:?}"),
    }

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("\nTOKEN REFRESH WORKS");
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
