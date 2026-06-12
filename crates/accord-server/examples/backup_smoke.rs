//! Verifies the encrypted key-backup service end-to-end (headless):
//! 1. With no backup yet, download returns NotFound.
//! 2. Upload, then download returns the same opaque bytes.
//! 3. Update replaces the blob.
//! 4. Delete removes it (download is NotFound again).
//!
//! The blobs here are arbitrary bytes: the server only stores ciphertext, so this
//! example needs no crypto dependency.
//!
//! ```text
//! cargo run -p accord-server --example backup_smoke
//! ```

use std::time::Duration;

use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::backup_service_client::BackupServiceClient;
use accord_proto::{
    DeleteKeyBackupRequest, DownloadKeyBackupRequest, LoginRequest, RegisterRequest,
    UploadKeyBackupRequest,
};
use tokio::sync::oneshot;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request};

const PORT: u16 = 50066;

fn authed<T>(mut req: Request<T>, token: &str) -> Request<T> {
    let v: MetadataValue<_> = format!("Bearer {token}").parse().unwrap();
    req.metadata_mut().insert("authorization", v);
    req
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = accord_server::Config {
        bind_addr: format!("127.0.0.1:{PORT}").parse()?,
        database_url: "sqlite:backup-smoke.db".to_owned(),
        redis_url: String::new(),
        jwt_secret: "backup-smoke-secret".to_owned(),
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
        username: "dave".into(),
        password: "backuppw123".into(),
        display_name: "Dave".into(),
        invite_token: String::new(),
        identity_pubkey: Vec::new(),
    })
    .await?;
    let token = auth
        .login(LoginRequest {
            username: "dave".into(),
            password: "backuppw123".into(),
            device_name: "dev".into(),
        })
        .await?
        .into_inner()
        .access_token;
    println!("[ok] account ready");

    let mut backup = BackupServiceClient::new(channel);

    // 1. No backup yet.
    let missing = backup
        .download_key_backup(authed(Request::new(DownloadKeyBackupRequest {}), &token))
        .await;
    match missing {
        Err(s) if s.code() == Code::NotFound => {
            println!("[ok] download with no backup -> NotFound")
        }
        other => anyhow::bail!("expected NotFound, got {other:?}"),
    }

    // 2. Upload + download round trip.
    backup
        .upload_key_backup(authed(
            Request::new(UploadKeyBackupRequest {
                encrypted_blob: vec![1, 2, 3, 4],
                salt: vec![9u8; 32],
                argon2_params: vec![0u8; 12],
                version: 1,
            }),
            &token,
        ))
        .await?;
    let got = backup
        .download_key_backup(authed(Request::new(DownloadKeyBackupRequest {}), &token))
        .await?
        .into_inner();
    anyhow::ensure!(
        got.encrypted_blob == vec![1, 2, 3, 4] && got.version == 1,
        "round trip mismatch"
    );
    println!("[ok] upload then download returns the same blob");

    // 3. Update replaces it.
    backup
        .upload_key_backup(authed(
            Request::new(UploadKeyBackupRequest {
                encrypted_blob: vec![5, 6, 7],
                salt: vec![8u8; 32],
                argon2_params: vec![0u8; 12],
                version: 2,
            }),
            &token,
        ))
        .await?;
    let updated = backup
        .download_key_backup(authed(Request::new(DownloadKeyBackupRequest {}), &token))
        .await?
        .into_inner();
    anyhow::ensure!(
        updated.encrypted_blob == vec![5, 6, 7] && updated.version == 2,
        "update failed"
    );
    println!("[ok] re-upload replaces the backup");

    // 4. Delete.
    backup
        .delete_key_backup(authed(Request::new(DeleteKeyBackupRequest {}), &token))
        .await?;
    let gone = backup
        .download_key_backup(authed(Request::new(DownloadKeyBackupRequest {}), &token))
        .await;
    match gone {
        Err(s) if s.code() == Code::NotFound => println!("[ok] delete removes the backup"),
        other => anyhow::bail!("expected NotFound after delete, got {other:?}"),
    }

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    println!("\nENCRYPTED KEY BACKUP WORKS");
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
