//! Local private (MLS) message-history archive, with per-message at-rest encryption.
//!
//! MLS has forward secrecy, so old ciphertext can't be decrypted again later. To
//! still show DM history after a reinstall, I keep my own plaintext copy of each
//! private message I send or receive, store it locally per group, and mirror it
//! to the server vault sealed under a master-derived key.
//!
//! At rest, each message is encrypted **based on who sent it**:
//! * messages I received from others are ALWAYS encrypted (a theft of my machine
//!   must not leak the people who messaged me);
//! * my own messages are encrypted only if I enabled "encrypt at rest" in
//!   settings (off by default, for faster loads).
//!
//! On-disk format (app-data/history/<user_id>/<group_id>.dmlog): an append-only
//! log of records, each `[u8 flag][u32 LE len][payload]`. flag 1 => payload is
//! at-rest-sealed JSON; flag 0 => payload is plaintext JSON. The server vault
//! instead stores the full plaintext JSONL sealed under the master-derived key.

use std::io::Write as _;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tonic::transport::Channel;

use accord_crypto::identity::IdentityKeyPair;

use crate::vault::HISTORY_PREFIX;

/// One archived private message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedMessage {
    pub group_id: String,
    pub sender_id: String,
    pub content: String,
    pub timestamp_ms: i64,
}

fn dir(app: &AppHandle, user_id: &str) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("history")
        .join(user_id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn group_path(
    app: &AppHandle,
    user_id: &str,
    group_id: &str,
) -> Result<std::path::PathBuf, String> {
    Ok(dir(app, user_id)?.join(format!("{group_id}.dmlog")))
}

/// Split a `.dmlog` file into its `(flag, payload)` records.
fn read_records(path: &std::path::Path) -> Vec<(u8, Vec<u8>)> {
    let Ok(data) = std::fs::read(path) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    let mut i = 0;
    while i + 5 <= data.len() {
        let flag = data[i];
        let len = u32::from_le_bytes([data[i + 1], data[i + 2], data[i + 3], data[i + 4]]) as usize;
        i += 5;
        if i + len > data.len() {
            break;
        }
        records.push((flag, data[i..i + len].to_vec()));
        i += len;
    }
    records
}

/// Encode one record as `[flag][len][payload]` (a single write keeps it atomic).
fn frame(flag: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(flag);
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Decode one record into a message (decrypting if it was sealed).
fn decode(flag: u8, payload: &[u8]) -> Option<ArchivedMessage> {
    let json = if flag == 1 {
        crate::at_rest::open_bytes(payload)?
    } else {
        payload.to_vec()
    };
    serde_json::from_slice(&json).ok()
}

/// Build a record for `msg`, encrypting it iff it's a received message or the
/// user enabled at-rest encryption for their own messages.
fn encode(app: &AppHandle, user_id: &str, msg: &ArchivedMessage) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
    let mine = msg.sender_id == user_id;
    let encrypt = !mine || crate::settings::is_encrypt_at_rest(app);
    if encrypt {
        Ok(frame(1, &crate::at_rest::seal_bytes(&json)?))
    } else {
        Ok(frame(0, &json))
    }
}

/// Load all of a group's archived messages (oldest-first).
#[must_use]
pub fn load(app: &AppHandle, user_id: &str, group_id: &str) -> Vec<ArchivedMessage> {
    let Ok(path) = group_path(app, user_id, group_id) else {
        return Vec::new();
    };
    read_records(&path)
        .into_iter()
        .filter_map(|(f, p)| decode(f, &p))
        .collect()
}

/// An undecoded record from the tail of a group's archive, with a stable id.
pub struct RawEntry {
    /// Stable id (`<group_id>#<record-index>`) for matching decrypt events.
    pub id: String,
    /// 1 = sealed payload, 0 = plaintext payload.
    pub flag: u8,
    pub payload: Vec<u8>,
}

/// Read the most recent `limit` records WITHOUT decrypting them, so the UI can
/// render placeholders for the sealed ones immediately and fill them in as they
/// decrypt. Framing is parsed cheaply; nothing is decrypted here.
#[must_use]
pub fn tail_raw(app: &AppHandle, user_id: &str, group_id: &str, limit: usize) -> Vec<RawEntry> {
    let Ok(path) = group_path(app, user_id, group_id) else {
        return Vec::new();
    };
    let records = read_records(&path);
    let start = records.len().saturating_sub(limit);
    records
        .into_iter()
        .enumerate()
        .skip(start)
        .map(|(index, (flag, payload))| RawEntry {
            id: format!("{group_id}#{index}"),
            flag,
            payload,
        })
        .collect()
}

/// Decode one record (decrypting if sealed). Public so the command layer can
/// decrypt pending records off the UI thread.
#[must_use]
pub fn decode_entry(flag: u8, payload: &[u8]) -> Option<ArchivedMessage> {
    decode(flag, payload)
}

/// The full plaintext JSONL for a group, for sealing to the vault under the
/// master-derived key. `None` if the archive is empty.
#[must_use]
pub fn plaintext_jsonl(app: &AppHandle, user_id: &str, group_id: &str) -> Option<Vec<u8>> {
    let messages = load(app, user_id, group_id);
    if messages.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for m in &messages {
        if let Ok(mut line) = serde_json::to_vec(m) {
            line.push(b'\n');
            out.extend_from_slice(&line);
        }
    }
    Some(out)
}

/// Append one message to a group's archive (O(1)) and mark it dirty so the
/// background sync mirrors it to the vault. Best-effort.
pub async fn record(
    app: &AppHandle,
    user_id: &str,
    group_id: &str,
    sender_id: &str,
    content: &str,
    timestamp_ms: i64,
) {
    let msg = ArchivedMessage {
        group_id: group_id.to_owned(),
        sender_id: sender_id.to_owned(),
        content: content.to_owned(),
        timestamp_ms,
    };
    let written = (|| -> Result<(), String> {
        let path = group_path(app, user_id, group_id)?;
        let record = encode(app, user_id, &msg)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(&record).map_err(|e| e.to_string())
    })();
    if let Err(e) = written {
        tracing::warn!("history: could not append: {e}");
    }
    crate::sync::mark_group(app, user_id, group_id).await;
}

/// Overwrite a group's archive from plaintext JSONL (applying the per-message
/// encryption rule). Used when restoring from the vault.
fn write_from_jsonl(app: &AppHandle, user_id: &str, group_id: &str, jsonl: &[u8]) {
    let mut buf = Vec::new();
    for line in jsonl.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_slice::<ArchivedMessage>(line) else {
            continue;
        };
        match encode(app, user_id, &msg) {
            Ok(record) => buf.extend_from_slice(&record),
            Err(e) => {
                tracing::warn!("history: could not encode restored message: {e}");
                return;
            }
        }
    }
    if let Ok(path) = group_path(app, user_id, group_id) {
        if let Err(e) = std::fs::write(path, buf) {
            tracing::warn!("history: could not write restored archive for {group_id}: {e}");
        }
    }
}

/// On a fresh install, pull every per-group archive from the vault into local
/// files (only where there is no local copy). Best-effort.
pub async fn restore_all(
    app: &AppHandle,
    channel: &Channel,
    token: &str,
    master: &IdentityKeyPair,
    user_id: &str,
) {
    let names = crate::vault::list_names(channel, token, HISTORY_PREFIX).await;
    for name in names {
        let Some(group_id) = name.strip_prefix(HISTORY_PREFIX) else {
            continue;
        };
        if group_path(app, user_id, group_id)
            .map(|p| p.exists())
            .unwrap_or(false)
        {
            continue;
        }
        if let Some(jsonl) = crate::vault::get_sealed(app, channel, token, master, &name).await {
            write_from_jsonl(app, user_id, group_id, &jsonl);
        }
    }
}

/// Mark every local archive dirty so the next flush brings the vault up to date.
pub async fn mark_all_local_dirty(app: &AppHandle, user_id: &str) {
    let Ok(dir) = dir(app, user_id) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("dmlog") {
            if let Some(group_id) = path.file_stem().and_then(|s| s.to_str()) {
                crate::sync::mark_group(app, user_id, group_id).await;
            }
        }
    }
}
