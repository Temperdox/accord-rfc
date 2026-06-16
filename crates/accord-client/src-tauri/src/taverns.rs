//! Multi-tavern hosting: stand up additional in-process `accord-server`
//! instances, one per **private** tavern the user owns, separate from the single
//! home node in [`crate::hosting`].
//!
//! Each tavern gets its own port (50052+), SQLite DB, TLS cert, and JWT secret
//! under `app_data_dir/taverns/<id>/`, and is persisted to
//! `app_data_dir/hosted-taverns.json` so it re-spawns on launch
//! ([`resume_hosted_taverns`]). The owner connects + registers (first account =
//! owner) + logs in, and the frontend adds it to the server rail.
//!
//! Scope: **private taverns only.** Public taverns need centralized hosting/
//! registration nodes that don't exist yet, so the UI offers them as an inert
//! scaffold. Trusted-user *migratory* failover (HOSTING-PLAN.md) layers on top of
//! this substrate later.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Manager, State};
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

/// Port scan window for hosted taverns. The home node owns
/// [`crate::hosting::DEFAULT_HOST_PORT`] (50051), so taverns start at 50052. The
/// window is wide enough to find a free port even with fragmentation and a
/// user-raised tavern cap; the *count* is gated separately by the
/// `max_hosted_taverns` setting (default 16), not by the window size.
const FIRST_TAVERN_PORT: u16 = 50052;
const LAST_TAVERN_PORT: u16 = 50307;

/// A running hosted tavern instance.
struct RunningTavern {
    /// Dropping/sending triggers graceful shutdown.
    shutdown: Option<oneshot::Sender<()>>,
    handle: JoinHandle<()>,
    port: u16,
}

/// Registry of this client's hosted taverns. Tauri-managed state.
#[derive(Default)]
pub struct HostedTaverns {
    running: std::collections::HashMap<String, RunningTavern>,
}

pub type SharedHostedTaverns = Mutex<HostedTaverns>;

/// Persisted metadata for a hosted tavern (so it re-spawns on launch).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TavernMeta {
    id: String,
    name: String,
    port: u16,
}

/// Connect info handed to the frontend to connect + register/login + add to rail.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TavernConnect {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub cert: Option<String>,
}

fn taverns_root(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?
        .join("taverns");
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create taverns dir: {e}"))?;
    Ok(dir)
}

fn meta_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?
        .join("hosted-taverns.json"))
}

fn load_meta(app: &AppHandle) -> Vec<TavernMeta> {
    let Ok(path) = meta_path(app) else {
        return Vec::new();
    };
    std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_meta(app: &AppHandle, metas: &[TavernMeta]) -> Result<(), String> {
    let path = meta_path(app)?;
    let json = serde_json::to_vec_pretty(metas).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("could not persist taverns: {e}"))
}

/// First port in `[FIRST_TAVERN_PORT, LAST_TAVERN_PORT]` that is neither in
/// `taken` nor rejected by `is_free`. Pure + injectable for testing.
fn first_free_port(taken: &HashSet<u16>, is_free: impl Fn(u16) -> bool) -> Option<u16> {
    (FIRST_TAVERN_PORT..=LAST_TAVERN_PORT).find(|p| !taken.contains(p) && is_free(*p))
}

/// Whether `port` can be bound on both IPv4 loopback and the dual-stack wildcard
/// (mirrors the host's bind checks).
fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).is_ok()
        && std::net::TcpListener::bind(
            format!("[::]:{port}").parse::<SocketAddr>().expect("valid addr"),
        )
        .is_ok()
}

/// Spawn one private tavern server instance (does NOT touch the registry — the
/// caller stores the returned [`RunningTavern`]).
async fn spawn_tavern(
    app: &AppHandle,
    meta: &TavernMeta,
) -> Result<(RunningTavern, TavernConnect), String> {
    let dir = taverns_root(app)?.join(&meta.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create tavern dir: {e}"))?;

    if !port_is_free(meta.port) {
        return Err(format!("port {} is already in use", meta.port));
    }

    let db_path = dir.join("accord-host.db");
    let database_url = format!("sqlite:{}", db_path.to_string_lossy().replace('\\', "/"));
    // Private taverns serve over TLS with a persisted self-signed cert (pinned via
    // the invite key), and a per-tavern JWT secret. Reuses the home host's helpers.
    let (cert, key) = crate::hosting::load_or_create_cert(&dir)?;
    let jwt_secret = crate::hosting::load_or_create_jwt_secret(&dir)?;

    let bind: SocketAddr = format!("[::]:{}", meta.port)
        .parse()
        .map_err(|e| format!("invalid port: {e}"))?;
    let config = accord_server::Config {
        bind_addr: bind,
        database_url,
        redis_url: String::new(), // in-process bus
        jwt_secret,
        access_token_ttl_secs: 3600,
        db_max_connections: 5,
        require_invite: true, // private tavern: invite-only registration
        open_dms: true,
        tls_cert_pem: Some(cert.clone()),
        tls_key_pem: Some(key),
    };

    let (tx, rx) = oneshot::channel();
    let (err_tx, err_rx) = oneshot::channel::<String>();
    let handle = tauri::async_runtime::spawn(async move {
        if let Err(e) = accord_server::run_with_shutdown(config, rx).await {
            tracing::error!("hosted tavern stopped: {e}");
            let _ = err_tx.send(e.to_string());
        }
    });
    if let Err(e) = crate::hosting::wait_until_listening(meta.port, err_rx).await {
        handle.abort();
        return Err(e);
    }

    let connect = TavernConnect {
        id: meta.id.clone(),
        name: meta.name.clone(),
        endpoint: format!("https://127.0.0.1:{}", meta.port),
        cert: Some(cert),
    };
    let running = RunningTavern {
        shutdown: Some(tx),
        handle,
        port: meta.port,
    };
    Ok((running, connect))
}

/// Create a new **private** tavern: pick a free port, spawn the instance, persist
/// it, and return the connect info (the frontend then connects + registers the
/// owner + logs in + adds it to the rail).
#[tauri::command]
pub async fn create_tavern(
    app: AppHandle,
    state: State<'_, SharedHostedTaverns>,
    name: String,
) -> Result<TavernConnect, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("tavern name is required".to_owned());
    }

    let mut guard = state.lock().await;
    let mut metas = load_meta(&app);

    // Enforce the user-configurable hosted-tavern cap (default 16). Counts the
    // distinct taverns we know about (persisted ∪ running). This is the DoS
    // backstop: a runaway loop can't spin up unbounded servers/ports.
    let mut ids: HashSet<String> = metas.iter().map(|m| m.id.clone()).collect();
    ids.extend(guard.running.keys().cloned());
    let max = crate::settings::max_hosted_taverns(&app);
    if ids.len() as u32 >= max {
        return Err(format!(
            "hosted-tavern limit reached ({max}). Raise it in Settings → Network \
             (each hosted tavern uses a port from 50052 up)."
        ));
    }

    // Ports already taken: running instances + everything persisted.
    let mut taken: HashSet<u16> = guard.running.values().map(|r| r.port).collect();
    taken.extend(metas.iter().map(|m| m.port));
    let port = first_free_port(&taken, port_is_free)
        .ok_or("no free port available for a new tavern")?;

    let meta = TavernMeta {
        id: Uuid::now_v7().to_string(),
        name: name.to_owned(),
        port,
    };
    let (running, connect) = spawn_tavern(&app, &meta).await?;
    guard.running.insert(meta.id.clone(), running);

    // Persist (append) so the tavern re-spawns on next launch.
    metas.push(meta);
    if let Err(e) = save_meta(&app, &metas) {
        tracing::warn!("could not persist hosted tavern: {e}");
    }

    tracing::info!(id = %connect.id, name, port, "created private tavern");
    Ok(connect)
}

/// Re-spawn all persisted hosted taverns that aren't already running, returning
/// the connect info for each that came up (the frontend logs into each + adds it
/// to the rail). Best-effort: a tavern that fails to start is skipped + logged.
#[tauri::command]
pub async fn resume_hosted_taverns(
    app: AppHandle,
    state: State<'_, SharedHostedTaverns>,
) -> Result<Vec<TavernConnect>, String> {
    let metas = load_meta(&app);
    let mut guard = state.lock().await;
    let mut out = Vec::new();
    for meta in metas {
        if guard.running.contains_key(&meta.id) {
            continue;
        }
        match spawn_tavern(&app, &meta).await {
            Ok((running, connect)) => {
                guard.running.insert(meta.id.clone(), running);
                out.push(connect);
            }
            Err(e) => tracing::warn!(id = %meta.id, "could not resume tavern: {e}"),
        }
    }
    Ok(out)
}

/// Stop all hosted taverns (graceful). Called on app shutdown.
pub async fn stop_all(app: &AppHandle) {
    let state = app.state::<SharedHostedTaverns>();
    let mut guard = state.lock().await;
    for (_, mut running) in guard.running.drain() {
        if let Some(tx) = running.shutdown.take() {
            let _ = tx.send(());
        }
        running.handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_first_free_port_skipping_taken() {
        let taken: HashSet<u16> = [FIRST_TAVERN_PORT, FIRST_TAVERN_PORT + 1].into_iter().collect();
        // Everything "free" at the OS level: should skip the two taken ports.
        let picked = first_free_port(&taken, |_| true);
        assert_eq!(picked, Some(FIRST_TAVERN_PORT + 2));
    }

    #[test]
    fn respects_os_level_busy_ports() {
        let taken = HashSet::new();
        // Pretend only the 5th port in range binds.
        let target = FIRST_TAVERN_PORT + 4;
        let picked = first_free_port(&taken, |p| p == target);
        assert_eq!(picked, Some(target));
    }

    #[test]
    fn none_when_range_exhausted() {
        let taken = HashSet::new();
        assert_eq!(first_free_port(&taken, |_| false), None);
    }
}
