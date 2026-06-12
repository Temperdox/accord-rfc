//! Client side of the server vault: back up state encrypted under a key derived
//! from the master key, so it survives a reinstall (and is wiped with the
//! account). Blobs are sealed locally; the server only ever holds opaque bytes.

use accord_crypto::backup::{open, seal};
use accord_crypto::identity::IdentityKeyPair;
use accord_proto::vault_service_client::VaultServiceClient;
use accord_proto::{GetBlobRequest, ListBlobsRequest, PutBlobRequest};
use tauri::{AppHandle, Manager};
use tonic::Request;
use tonic::transport::Channel;

use crate::grpc::authed;
use crate::state::SharedSessions;

/// Vault name for the MLS session-state snapshot.
pub const MLS_STATE: &str = "mls-state";
/// Vault name prefix for per-group message-history archives.
pub const HISTORY_PREFIX: &str = "hist/";

/// Channel + token for the session owning `user_id` (so background sessions
/// upload to their own server's vault, not whichever is active).
async fn creds_for_user(app: &AppHandle, user_id: &str) -> Option<(Channel, String)> {
    let state = app.state::<SharedSessions>();
    let sessions = state.lock().await;
    sessions.creds_for_user(user_id)
}

/// Seal `plaintext` under the master-derived key for `name` and upload it to the
/// vault of the server owning `user_id`. Best-effort: failures are logged, never
/// fatal (the local copy is the source of truth for the live session).
pub async fn put_sealed(app: &AppHandle, user_id: &str, name: &str, plaintext: &[u8]) {
    let Some((channel, token)) = creds_for_user(app, user_id).await else {
        return;
    };
    let Some(master) = crate::identity::load_master(app) else {
        return;
    };
    let key = master.derive_symmetric(name.as_bytes());
    let blob = match seal(&key, plaintext) {
        Ok(blob) => blob,
        Err(e) => {
            tracing::warn!("vault: could not seal '{name}': {e}");
            return;
        }
    };
    let req = match authed(
        Request::new(PutBlobRequest {
            name: name.to_owned(),
            blob,
        }),
        &token,
    ) {
        Ok(req) => req,
        Err(_) => return,
    };
    if let Err(e) = VaultServiceClient::new(channel).put_blob(req).await {
        tracing::warn!("vault: could not upload '{name}': {e}");
    }
}

/// Download and decrypt the blob named `name`. Uses an explicit channel/token/
/// master so it can run during login before the session is fully set up. Returns
/// `None` if absent or undecryptable.
pub async fn get_sealed(
    app: &AppHandle,
    channel: &Channel,
    token: &str,
    master: &IdentityKeyPair,
    name: &str,
) -> Option<Vec<u8>> {
    let _ = app;
    let req = authed(
        Request::new(GetBlobRequest {
            name: name.to_owned(),
        }),
        token,
    )
    .ok()?;
    let blob = VaultServiceClient::new(channel.clone())
        .get_blob(req)
        .await
        .ok()?
        .into_inner()
        .blob;
    let key = master.derive_symmetric(name.as_bytes());
    match open(&key, &blob) {
        Ok(plaintext) => Some(plaintext.to_vec()),
        Err(e) => {
            tracing::warn!("vault: could not open '{name}': {e}");
            None
        }
    }
}

/// List blob names under `prefix`, using an explicit channel/token (login path).
pub async fn list_names(channel: &Channel, token: &str, prefix: &str) -> Vec<String> {
    let Ok(req) = authed(
        Request::new(ListBlobsRequest {
            prefix: prefix.to_owned(),
        }),
        token,
    ) else {
        return Vec::new();
    };
    match VaultServiceClient::new(channel.clone())
        .list_blobs(req)
        .await
    {
        Ok(resp) => resp.into_inner().names,
        Err(_) => Vec::new(),
    }
}
