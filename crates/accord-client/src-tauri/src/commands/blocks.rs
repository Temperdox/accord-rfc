//! Blocklist (scaffold): who this user has blocked.
//!
//! Blocking is the user-to-user analogue of a server ban (BAN-PLAN.md): a block
//! should cover the blocked person's alts and should **sink silently** so the
//! blocked user can't tell they're blocked or use the block to correlate the
//! blocker's accounts. That enforcement lives at the mailbox + needs the per-user
//! anchor tags from the ban plan, which aren't built yet - so this stores the
//! list and exposes the commands; the silent-sink enforcement plugs in later.
//!
//! Stored locally, encrypted at rest ([`crate::at_rest`]). A block is keyed by the
//! contact's id (hex of their contact-identity public key), the same id contacts
//! use.

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

/// A stored block.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredBlock {
    /// Hex of the blocked contact-identity public key.
    id: String,
    name: String,
    blocked_at_ms: i64,
}

/// A block as shown in the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockDto {
    pub id: String,
    pub name: String,
}

fn path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("blocks.bin"))
}

fn load(app: &AppHandle) -> Vec<StoredBlock> {
    let Ok(path) = path(app) else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    crate::at_rest::open_bytes(&bytes)
        .and_then(|pt| serde_json::from_slice(&pt).ok())
        .unwrap_or_default()
}

fn save(app: &AppHandle, blocks: &[StoredBlock]) -> Result<(), String> {
    let json = serde_json::to_vec(blocks).map_err(|e| e.to_string())?;
    let blob = crate::at_rest::seal_bytes(&json)?;
    std::fs::write(path(app)?, blob).map_err(|e| e.to_string())
}

/// Block a contact by id. No-op if already blocked.
#[tauri::command]
pub fn block_contact(app: AppHandle, id: String, name: String) -> Result<(), String> {
    let mut blocks = load(&app);
    if blocks.iter().any(|b| b.id == id) {
        return Ok(());
    }
    let blocked_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    blocks.push(StoredBlock {
        id,
        name,
        blocked_at_ms,
    });
    save(&app, &blocks)
}

/// Unblock a contact by id.
#[tauri::command]
pub fn unblock_contact(app: AppHandle, id: String) -> Result<(), String> {
    let mut blocks = load(&app);
    blocks.retain(|b| b.id != id);
    save(&app, &blocks)
}

/// List the blocked contacts.
#[tauri::command]
pub fn list_blocks(app: AppHandle) -> Result<Vec<BlockDto>, String> {
    Ok(load(&app)
        .into_iter()
        .map(|b| BlockDto {
            id: b.id,
            name: b.name,
        })
        .collect())
}
