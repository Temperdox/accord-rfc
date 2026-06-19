//! Local account registry: the accounts created on this device, shown as pills on
//! the login screen so a user can pick one and just enter its password.
//!
//! Accounts live on this device's embedded home server (there is no recovery yet),
//! so we keep a small encrypted-at-rest list of them. The first account created is
//! the "main"; later ones are sub-accounts (alts). The server-visible linkage used
//! for moderation (and the persistent-ban / hardware-tag system) is designed in
//! BAN-PLAN.md and not built here.

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

/// A stored local account.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredAccount {
    username: String,
    /// True for the first account created on this device (the main account).
    main: bool,
    created_at_ms: i64,
    /// Cached avatar (base64 data URL) so the login picker can show a face before
    /// any session exists. Updated on login + when the home profile changes.
    #[serde(default)]
    avatar: String,
}

/// An account as shown on the login screen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPill {
    pub username: String,
    pub is_main: bool,
    pub avatar: String,
}

fn path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("accounts.bin"))
}

fn load(app: &AppHandle) -> Vec<StoredAccount> {
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

fn save(app: &AppHandle, accounts: &[StoredAccount]) -> Result<(), String> {
    let json = serde_json::to_vec(accounts).map_err(|e| e.to_string())?;
    let blob = crate::at_rest::seal_bytes(&json)?;
    std::fs::write(path(app)?, blob).map_err(|e| e.to_string())
}

/// Remember an account created on this device. The first one becomes the main
/// account; later ones are sub-accounts. No-op if already known. Best-effort.
pub fn record(app: &AppHandle, username: &str) {
    let mut accounts = load(app);
    if accounts.iter().any(|a| a.username == username) {
        return;
    }
    let created_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    accounts.push(StoredAccount {
        username: username.to_owned(),
        main: accounts.is_empty(),
        created_at_ms,
        avatar: String::new(),
    });
    if let Err(e) = save(app, &accounts) {
        tracing::warn!("could not record account: {e}");
    }
}

/// Cache an account's avatar so the login picker can show it. Best-effort.
#[tauri::command]
pub fn set_account_avatar(app: AppHandle, username: String, avatar: String) -> Result<(), String> {
    let mut accounts = load(&app);
    let Some(acct) = accounts.iter_mut().find(|a| a.username == username) else {
        return Ok(());
    };
    if acct.avatar == avatar {
        return Ok(());
    }
    acct.avatar = avatar;
    save(&app, &accounts)
}

/// List the accounts known on this device, oldest first (main account first).
#[tauri::command]
pub fn list_accounts(app: AppHandle) -> Result<Vec<AccountPill>, String> {
    Ok(load(&app)
        .into_iter()
        .map(|a| AccountPill {
            username: a.username,
            is_main: a.main,
            avatar: a.avatar,
        })
        .collect())
}
