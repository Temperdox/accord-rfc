//! Local persistence of the per-device MLS engine.
//!
//! MLS group state (epochs, ratchets, the device's key store) lives in the
//! `MlsEngine`. Without saving it, restarting the client loses every private
//! group and DM. I serialize the engine after each state change and restore it on
//! login, so private chats survive restarts.
//!
//! The state is stored per account (keyed by user id) in the app-data dir.
//!
//! NOTE: like the local identity-key cache, this file is currently stored
//! unencrypted at rest. Encrypting it with a key derived from the master key is a
//! planned hardening follow-up; it does not change the persistence model here.

use accord_mls::MlsEngine;
use tauri::{AppHandle, Manager};

use crate::state::SharedEngine;

/// Path to the account's serialized MLS state.
fn path(app: &AppHandle, user_id: &str) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("mls");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(format!("{user_id}.bin")))
}

/// Restore the account's MLS engine from disk, if a valid saved state exists. The
/// file is encrypted at rest; an older plaintext file is still read (and will be
/// re-saved encrypted on the next persist).
#[must_use]
pub fn load(app: &AppHandle, user_id: &str) -> Option<MlsEngine> {
    let bytes = std::fs::read(path(app, user_id).ok()?).ok()?;
    // Current format: encrypted at rest. Fall back to an older plaintext file.
    let serialized = crate::at_rest::open_bytes(&bytes).unwrap_or(bytes);
    match MlsEngine::from_serialized(&serialized) {
        Ok(engine) => Some(engine),
        Err(e) => {
            tracing::warn!("could not restore MLS state, starting fresh: {e}");
            None
        }
    }
}

/// Serialize and write the engine's current state (encrypted at rest).
/// Best-effort: a failure is logged but does not abort the operation that
/// triggered it (the in-memory state is still correct for this session).
pub async fn persist(app: &AppHandle, engine: &SharedEngine, user_id: &str) {
    let bytes = match { engine.lock().await.serialize() } {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!("could not serialize MLS state: {e}");
            return;
        }
    };
    match (path(app, user_id), crate::at_rest::seal_bytes(&bytes)) {
        (Ok(p), Ok(blob)) => {
            if let Err(e) = std::fs::write(p, blob) {
                tracing::warn!("could not persist MLS state: {e}");
            }
        }
        (Err(e), _) => tracing::warn!("could not resolve MLS state path: {e}"),
        (_, Err(e)) => tracing::warn!("could not encrypt MLS state: {e}"),
    }
    // Mark the (plaintext) snapshot for debounced upload to this user's server
    // vault, where it is sealed under a master-derived key (survives reinstall).
    crate::sync::mark_mls(app, user_id, bytes).await;
}
