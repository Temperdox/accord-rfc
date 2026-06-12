//! Verifies the vault service end-to-end (headless):
//! 1. Get on a missing name returns NotFound.
//! 2. Put then Get returns the same opaque bytes.
//! 3. ListBlobs filters by prefix.
//! 4. Delete removes a blob.
//!
//! Blobs are arbitrary bytes here: the server stores ciphertext, so no crypto
//! dependency is needed.
//!
//! ```text
//! cargo run -p accord-server --example vault_smoke
//! ```

use std::time::Duration;

use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::vault_service_client::VaultServiceClient;
use accord_proto::{
    DeleteBlobRequest, GetBlobRequest, ListBlobsRequest, LoginRequest, PutBlobRequest,
    RegisterRequest,
};
use tokio::sync::oneshot;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request};

const PORT: u16 = 50067;

fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let v: MetadataValue<_> = format!("Bearer {token}").parse().unwrap();
    req.metadata_mut().insert("authorization", v);
    req
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = accord_server::Config {
        bind_addr: format!("127.0.0.1:{PORT}").parse()?,
        database_url: "sqlite:vault-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "vault-smoke-secret".to_owned(),
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
        username: "erin".into(),
        password: "vaultpw12345".into(),
        display_name: "Erin".into(),
        invite_token: String::new(),
        identity_pubkey: Vec::new(),
    })
    .await?;
    let token = auth
        .login(LoginRequest {
            username: "erin".into(),
            password: "vaultpw12345".into(),
            device_name: "dev".into(),
        })
        .await?
        .into_inner()
        .access_token;
    println!("[ok] account ready");

    let mut vault = VaultServiceClient::new(channel);

    // 1. Missing.
    match vault
        .get_blob(authed(
            Request::new(GetBlobRequest {
                name: "mls-state".into(),
            }),
            &token,
        ))
        .await
    {
        Err(s) if s.code() == Code::NotFound => println!("[ok] get missing -> NotFound"),
        other => anyhow::bail!("expected NotFound, got {other:?}"),
    }

    // 2. Put + get.
    vault
        .put_blob(authed(
            Request::new(PutBlobRequest {
                name: "mls-state".into(),
                blob: vec![1, 2, 3, 4, 5],
            }),
            &token,
        ))
        .await?;
    let got = vault
        .get_blob(authed(
            Request::new(GetBlobRequest {
                name: "mls-state".into(),
            }),
            &token,
        ))
        .await?
        .into_inner()
        .blob;
    anyhow::ensure!(got == vec![1, 2, 3, 4, 5], "blob round trip mismatch");
    println!("[ok] put then get returns the same blob");

    // 3. List by prefix.
    for g in ["hist/g1", "hist/g2"] {
        vault
            .put_blob(authed(
                Request::new(PutBlobRequest {
                    name: g.into(),
                    blob: vec![9],
                }),
                &token,
            ))
            .await?;
    }
    let names = vault
        .list_blobs(authed(
            Request::new(ListBlobsRequest {
                prefix: "hist/".into(),
            }),
            &token,
        ))
        .await?
        .into_inner()
        .names;
    anyhow::ensure!(
        names == vec!["hist/g1", "hist/g2"],
        "prefix list wrong: {names:?}"
    );
    println!("[ok] list by prefix returns only matching names");

    // 4. Delete.
    vault
        .delete_blob(authed(
            Request::new(DeleteBlobRequest {
                name: "hist/g1".into(),
            }),
            &token,
        ))
        .await?;
    let after = vault
        .list_blobs(authed(
            Request::new(ListBlobsRequest {
                prefix: "hist/".into(),
            }),
            &token,
        ))
        .await?
        .into_inner()
        .names;
    anyhow::ensure!(after == vec!["hist/g2"], "delete did not remove: {after:?}");
    println!("[ok] delete removes a blob");

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("\nVAULT WORKS");
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
