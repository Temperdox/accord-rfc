//! Factory reset (dev tooling): wipe all local state and relaunch.
//!
//! Returns the app to a fresh-install state so new features can be tested
//! without stale accounts, identities, or databases. It deletes **everything in
//! the app-data directory**: the embedded host DB (accounts live there), the
//! master identity key, the per-install JWT secret and TLS cert, settings,
//! contacts/blocks/account pills, DM targets, MLS state, history archives, the
//! mesh key, and the install id. Logs (a separate directory) are kept for
//! diagnostics, and the OS-keyring data key is kept so the wipe/restart cycle
//! stays self-consistent (it identifies the machine, not the user).
//!
//! Debug builds only (gated at the module level in `main.rs`), reachable only
//! from the Dev menu - a factory reset never ships in release.

use std::time::Duration;

use tauri::{AppHandle, Manager};

/// Stop everything holding files open, wipe the app-data directory, and
/// relaunch the app.
///
/// # Errors
/// Returns an error string when some state could not be removed (e.g. a file
/// still locked); nothing is relaunched in that case so the failure is visible.
pub async fn factory_reset(app: &AppHandle) -> Result<(), String> {
    tracing::warn!("FACTORY RESET requested - wiping all local state");

    // Every preparatory phase runs under a timeout: a reset must never deadlock
    // behind a stuck login or hosting call (it is the recovery tool for exactly
    // those states). A skipped phase only risks a locked file, which the wipe
    // loop below already retries.
    const PHASE: Duration = Duration::from_secs(5);

    // Release file handles: the mesh holds mesh.key only at startup, but the
    // embedded servers hold their SQLite databases open - both the home node and
    // every hosted tavern (each keeps `taverns/<id>/accord-host.db` open, which
    // otherwise blocks deleting the `taverns/` directory on Windows).
    if tokio::time::timeout(PHASE, crate::mesh::stop(app)).await.is_err() {
        tracing::warn!("mesh stop timed out; continuing reset");
    }
    if tokio::time::timeout(PHASE, crate::hosting::stop(app)).await.is_err() {
        tracing::warn!("embedded server stop timed out; continuing reset");
    }
    if tokio::time::timeout(PHASE, crate::taverns::stop_all(app)).await.is_err() {
        tracing::warn!("hosted-tavern stop timed out; continuing reset");
    }

    // Abort session supervisors so nothing reconnects mid-wipe.
    {
        let state = app.state::<crate::state::SharedSessions>();
        match tokio::time::timeout(PHASE, state.lock()).await {
            Ok(mut sessions) => {
                for session in sessions.map.values_mut() {
                    for handle in session.session_tasks.drain(..) {
                        handle.abort();
                    }
                }
                sessions.map.clear();
                sessions.active = None;
            }
            Err(_) => {
                tracing::warn!("session state busy; skipping supervisor abort");
            }
        }
    }

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?;

    // SQLite releases its lock shortly after the server's pool drops; retry the
    // wipe a few times instead of failing on the first locked file (Windows).
    let mut last_err = String::new();
    for attempt in 0..10 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        match wipe_dir_contents(&dir) {
            Ok(()) => {
                tracing::warn!("factory reset complete; relaunching");
                app.restart();
            }
            Err(e) => last_err = e,
        }
    }
    Err(format!("could not wipe all local state: {last_err}"))
}

/// Delete every entry inside `dir` (but not `dir` itself).
fn wipe_dir_contents(dir: &std::path::Path) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut failures = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = result {
            failures.push(format!("{}: {e}", path.display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}
