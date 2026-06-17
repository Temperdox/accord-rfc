//! Auth commands: `connect`, `register`, `login`.

use std::sync::Arc;

use accord_mls::MlsEngine;
use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::mls_service_client::MlsServiceClient;
use accord_proto::{LoginRequest, RegisterRequest, UploadKeyPackagesRequest};
use tauri::{AppHandle, State};
use tokio::sync::Mutex;
use tonic::Request;
use tonic::transport::Channel;

use crate::commands::dto::LoginInfo;
use crate::commands::messaging;
use crate::grpc::{authed, require_channel, status_to_string};
use crate::state::SharedSessions;

/// How many KeyPackages to publish on login so peers can add this device offline.
const KEY_PACKAGE_BATCH: usize = 10;

/// Establish the gRPC channel to `endpoint`.
///
/// For `https://` endpoints we use TLS. If `cert` (the server's self-signed PEM
/// from an invite key) is provided, we **pin exactly that cert** and verify
/// against the fixed pinned domain - authenticated with no CA. `http://`
/// endpoints connect in plaintext (dev / manual).
#[tauri::command]
pub async fn connect(
    state: State<'_, SharedSessions>,
    server_id: String,
    endpoint: String,
    cert: Option<String>,
) -> Result<(), String> {
    let channel = crate::grpc::build_channel(&endpoint, cert.as_deref()).await?;
    let mut sessions = state.lock().await;
    {
        let s = sessions.entry(&server_id);
        s.channel = Some(channel);
        s.endpoint = Some(endpoint);
        s.cert = cert;
    }
    sessions.active = Some(server_id);
    Ok(())
}

/// Register a new account. Requires a prior `connect`.
///
/// The account is bound to a per-server public identity key derived from this
/// device's hidden master key, so it is unique without any central authority.
#[tauri::command]
pub async fn register(
    app: AppHandle,
    state: State<'_, SharedSessions>,
    username: String,
    password: String,
    display_name: String,
    invite_token: Option<String>,
) -> Result<(), String> {
    let (channel, endpoint, cert) = {
        let sessions = state.lock().await;
        let s = sessions.active().ok_or("not connected")?;
        let channel = s.channel.clone().ok_or("not connected")?;
        (
            channel,
            s.endpoint.clone().unwrap_or_default(),
            s.cert.clone(),
        )
    };

    let master = crate::identity::load_or_create_master(&app)?;
    let identity_pubkey = crate::identity::derived_pubkey_for(&master, cert.as_deref(), &endpoint);

    let mut client = AuthServiceClient::new(channel);
    client
        .register(RegisterRequest {
            username: username.clone(),
            password,
            display_name,
            invite_token: invite_token.unwrap_or_default(),
            identity_pubkey,
        })
        .await
        .map_err(status_to_string)?;
    Ok(())
}

/// Log in, store the access token, and open the real-time message stream.
#[tauri::command]
pub async fn login(
    app: AppHandle,
    state: State<'_, SharedSessions>,
    username: String,
    password: String,
    device_name: String,
) -> Result<LoginInfo, String> {
    let channel = require_channel(&state).await?;
    let mut client = AuthServiceClient::new(channel.clone());
    let resp = client
        .login(LoginRequest {
            username: username.clone(),
            password: password.clone(),
            // Stable per-install name so the server reuses the same device row
            // (the mailbox/Welcome inbox are keyed by device id).
            device_name: crate::identity::device_name(&app, &device_name),
        })
        .await
        .map_err(status_to_string)?
        .into_inner();

    let token = resp.access_token;
    let refresh_token = resp.refresh_token;
    let user_id = resp.user_id.map(|u| u.value).unwrap_or_default();
    let device_id = resp.device_id.map(|d| d.value).unwrap_or_default();

    // Recover the master identity key from the encrypted backup if this device
    // doesn't have it, and upload one if the server has none yet.
    sync_key_backup(&app, &channel, &token, &password).await?;

    // Shared post-auth setup: MLS engine + KeyPackages, session start, vault sync,
    // and (home only) DM-reopen + friend sync.
    finish_session(&app, &state, channel, &username, &user_id, token, refresh_token).await?;
    Ok(LoginInfo { user_id, device_id })
}

/// Password-less **key login** for taverns: derive this server's identity key
/// from the local master, fetch a one-time challenge, sign it, and authenticate.
/// No password is typed and no key-backup sync runs - key-only accounts have no
/// password, and the master key already exists locally from the home login.
#[tauri::command]
pub async fn login_with_key(
    app: AppHandle,
    state: State<'_, SharedSessions>,
    username: String,
    device_name: String,
) -> Result<LoginInfo, String> {
    use accord_proto::{ChallengeRequest, KeyLoginRequest};

    let channel = require_channel(&state).await?;
    let (endpoint, cert) = {
        let sessions = state.lock().await;
        let s = sessions.active().ok_or("not connected")?;
        (s.endpoint.clone().unwrap_or_default(), s.cert.clone())
    };

    // The per-server derived key IS the credential (different per tavern, so a
    // tavern can't correlate you across servers).
    let master = crate::identity::load_or_create_master(&app)?;
    let derived = crate::identity::derive_for(&master, cert.as_deref(), &endpoint);
    let identity_pubkey = derived.public().to_bytes().to_vec();

    let mut auth = AuthServiceClient::new(channel.clone());
    let challenge = auth
        .request_challenge(Request::new(ChallengeRequest {
            identity_pubkey: identity_pubkey.clone(),
        }))
        .await
        .map_err(status_to_string)?
        .into_inner()
        .challenge;
    let signature = derived.sign(challenge.as_bytes()).to_vec();
    let resp = auth
        .login_with_key(Request::new(KeyLoginRequest {
            identity_pubkey,
            challenge,
            signature,
            device_name: crate::identity::device_name(&app, &device_name),
        }))
        .await
        .map_err(status_to_string)?
        .into_inner();

    let token = resp.access_token;
    let refresh_token = resp.refresh_token;
    let user_id = resp.user_id.map(|u| u.value).unwrap_or_default();
    let device_id = resp.device_id.map(|d| d.value).unwrap_or_default();

    finish_session(&app, &state, channel, &username, &user_id, token, refresh_token).await?;
    Ok(LoginInfo { user_id, device_id })
}

/// Shared post-authentication setup used by both password [`login`] and key
/// [`login_with_key`]: build/restore the MLS engine, publish KeyPackages, attach
/// credentials to the active session and start its resilient stream, sync the
/// vault, and (home only) reopen DMs + run a friend sync.
async fn finish_session(
    app: &AppHandle,
    state: &State<'_, SharedSessions>,
    channel: Channel,
    username: &str,
    user_id: &str,
    token: String,
    refresh_token: String,
) -> Result<(), String> {
    let (endpoint, cert, is_home) = {
        let sessions = state.lock().await;
        let is_home = sessions.active.as_deref() == Some("home");
        let s = sessions.active();
        (
            s.and_then(|s| s.endpoint.clone()).unwrap_or_default(),
            s.and_then(|s| s.cert.clone()),
            is_home,
        )
    };
    // MLS engine in priority order: local cache, encrypted server vault, then a
    // fresh engine. Home signs DMs with the stable contact identity; taverns use
    // the unlinkable per-server derived key.
    let master = crate::identity::load_or_create_master(app)?;
    let mls = if let Some(local) = crate::mls_persist::load(app, user_id) {
        local
    } else if let Some(restored) =
        crate::vault::get_sealed(app, &channel, &token, &master, crate::vault::MLS_STATE)
            .await
            .and_then(|bytes| MlsEngine::from_serialized(&bytes).ok())
    {
        restored
    } else {
        let signer = if is_home {
            crate::identity::contact_identity(&master)
        } else {
            crate::identity::derive_for(&master, cert.as_deref(), &endpoint)
        };
        MlsEngine::new(&signer).map_err(|e| e.to_string())?
    };
    let key_packages = mls
        .generate_key_packages(KEY_PACKAGE_BATCH)
        .map_err(|e| e.to_string())?;
    MlsServiceClient::new(channel.clone())
        .upload_key_packages(authed(
            Request::new(UploadKeyPackagesRequest { key_packages }),
            &token,
        )?)
        .await
        .map_err(status_to_string)?;
    let engine = Arc::new(Mutex::new(mls));

    crate::history::restore_all(app, &channel, &token, &master, user_id).await;
    crate::sync::spawn_flusher(app).await;
    crate::history::mark_all_local_dirty(app, user_id).await;

    let server_id = {
        let mut sessions = state.lock().await;
        let server_id = sessions.active.clone().ok_or("not connected")?;
        if let Some(s) = sessions.active_mut() {
            s.user_id = Some(user_id.to_owned());
            s.token = Some(token);
            s.refresh_token = Some(refresh_token);
            s.engine = Some(engine.clone());
        }
        server_id
    };

    messaging::start_session(
        app.clone(),
        server_id,
        channel.clone(),
        user_id.to_owned(),
        engine.clone(),
    )
    .await?;

    crate::mls_persist::persist(app, &engine, user_id).await;

    if is_home {
        crate::commands::accounts::record(app, username);
        let app2 = app.clone();
        let display = username.to_owned();
        tauri::async_runtime::spawn(async move {
            crate::commands::mls::reopen_dm_targets(&app2, &display).await;
            crate::commands::friends::background_sync(&app2, &display).await;
        });
    }
    Ok(())
}

/// Recover the master identity key from the server's encrypted backup when this
/// device lacks it, and upload one if the server has none yet.
///
/// The backup is encrypted on this device with a key derived from the password
/// (Argon2id + XChaCha20-Poly1305); the server only ever stores opaque bytes.
/// This is what lets the identity survive a reinstall and move to a new device.
async fn sync_key_backup(
    app: &AppHandle,
    channel: &Channel,
    token: &str,
    password: &str,
) -> Result<(), String> {
    use accord_crypto::backup::{Argon2Params, decrypt_backup, encrypt_backup};
    use accord_crypto::identity::IdentityKeyPair;
    use accord_proto::backup_service_client::BackupServiceClient;
    use accord_proto::{DownloadKeyBackupRequest, UploadKeyBackupRequest};

    let mut client = BackupServiceClient::new(channel.clone());
    let downloaded = client
        .download_key_backup(authed(Request::new(DownloadKeyBackupRequest {}), token)?)
        .await
        .ok()
        .map(|r| r.into_inner());

    // No local key: recover it from the backup, or create a fresh one.
    if crate::identity::load_master(app).is_none() {
        if let Some(b) = &downloaded {
            let params = Argon2Params::from_bytes(&b.argon2_params).map_err(|e| e.to_string())?;
            let secret = decrypt_backup(password.as_bytes(), &b.encrypted_blob, &b.salt, params)
                .map_err(|_| "could not decrypt key backup (wrong password?)".to_owned())?;
            let arr = <[u8; 32]>::try_from(secret.as_slice())
                .map_err(|_| "corrupt key backup".to_owned())?;
            crate::identity::save_master(app, &IdentityKeyPair::from_secret_bytes(&arr))?;
        } else {
            crate::identity::load_or_create_master(app)?;
        }
    }

    // Server has no backup yet: upload the current master key.
    if downloaded.is_none() {
        let master = crate::identity::load_or_create_master(app)?;
        let secret = master.secret_bytes();
        let enc = encrypt_backup(password.as_bytes(), &secret, Argon2Params::default())
            .map_err(|e| e.to_string())?;
        client
            .upload_key_backup(authed(
                Request::new(UploadKeyBackupRequest {
                    encrypted_blob: enc.blob,
                    salt: enc.salt.to_vec(),
                    argon2_params: enc.params.to_bytes().to_vec(),
                    version: 1,
                }),
                token,
            )?)
            .await
            .map_err(status_to_string)?;
    }
    Ok(())
}
