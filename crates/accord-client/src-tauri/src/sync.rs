//! Debounced background sync of encrypted state to each server's vault.
//!
//! Uploading the MLS snapshot and per-group history blob on every message is
//! wasteful, so mutation sites mark what is dirty (cheap, no network) and a
//! single background flusher coalesces the uploads. Dirty state is keyed by
//! `user_id`, so with several connected servers each blob is uploaded to the
//! vault of the server it belongs to (resolved by user id in [`crate::vault`]).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

/// How often the flusher uploads dirty blobs.
const FLUSH_INTERVAL: Duration = Duration::from_secs(4);

/// Pending-upload state, keyed by the owning account's user id.
#[derive(Default)]
pub struct Sync {
    /// Latest serialized MLS state awaiting upload, per user.
    mls: HashMap<String, Vec<u8>>,
    /// Group ids whose history archive changed, per user.
    groups: HashMap<String, HashSet<String>>,
    /// Whether the flusher task is already running.
    started: bool,
}

/// Managed-state alias.
pub type SharedSync = Mutex<Sync>;

/// Mark a user's MLS snapshot dirty with its latest bytes.
pub async fn mark_mls(app: &AppHandle, user_id: &str, bytes: Vec<u8>) {
    let state = app.state::<SharedSync>();
    state.lock().await.mls.insert(user_id.to_owned(), bytes);
}

/// Mark a user's group-history archive dirty.
pub async fn mark_group(app: &AppHandle, user_id: &str, group_id: &str) {
    let state = app.state::<SharedSync>();
    state
        .lock()
        .await
        .groups
        .entry(user_id.to_owned())
        .or_default()
        .insert(group_id.to_owned());
}

/// Start the background flusher once for the whole app.
pub async fn spawn_flusher(app: &AppHandle) {
    {
        let state = app.state::<SharedSync>();
        let mut sync = state.lock().await;
        if sync.started {
            return;
        }
        sync.started = true;
    }
    let app = app.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
        loop {
            ticker.tick().await;
            flush(&app).await;
        }
    });
}

/// Upload everything currently marked dirty, to each owning server's vault.
pub async fn flush(app: &AppHandle) {
    let (mls, groups) = {
        let state = app.state::<SharedSync>();
        let mut sync = state.lock().await;
        (
            std::mem::take(&mut sync.mls),
            std::mem::take(&mut sync.groups),
        )
    };

    for (user_id, bytes) in mls {
        crate::vault::put_sealed(app, &user_id, crate::vault::MLS_STATE, &bytes).await;
    }
    for (user_id, gids) in groups {
        for group_id in gids {
            if let Some(bytes) = crate::history::plaintext_jsonl(app, &user_id, &group_id) {
                let name = format!("{}{}", crate::vault::HISTORY_PREFIX, group_id);
                crate::vault::put_sealed(app, &user_id, &name, &bytes).await;
            }
        }
    }
}
