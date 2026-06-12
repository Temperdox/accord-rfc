//! Embedded Yggdrasil mesh networking (Phase C) - **opt-in `mesh` feature**.
//!
//! Brings up a Yggdrasil node *in-process*, giving this machine a stable,
//! end-to-end-encrypted IPv6 mesh address (`200:…`) that is reachable across the
//! internet **without port-forwarding** and **hides the real IP from remote
//! peers** (traffic routes through the mesh; only direct peers see the underlay
//! IP). Accord then runs over that address unchanged.
//!
//! Requires admin/root (it creates a TUN interface), so it's exposed only via
//! the dev menu for now. Build/run with:
//! ```text
//! cargo run -p accord-client --features mesh
//! ```
//!
//! The heavy, experimental `yggdrasil` dependency is gated by the `mesh` feature
//! so default builds stay light; the command shims below compile either way.

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

/// Bundled public Yggdrasil bootstrap peers, used when the user hasn't set any
/// (`ACCORD_MESH_PEERS` / `mesh-peers.txt` override these). Peering to any of
/// these joins the global mesh, so internet hosting works with **zero config**.
///
/// Public peers change over time - verify/refresh against
/// <https://github.com/yggdrasil-network/public-peers> before a release. Having
/// several improves the odds at least one is reachable.
pub const DEFAULT_MESH_PEERS: &[&str] = &[
    "tls://ygg.mkg20001.io:443",
    "tls://yggdrasil.su:62586",
    "tls://s2.i2pd.xyz:39575",
    "tls://ygg-nl.incognet.io:8884",
];

/// The bundled default peers as owned strings.
#[must_use]
pub fn default_peers() -> Vec<String> {
    DEFAULT_MESH_PEERS.iter().map(|s| (*s).to_owned()).collect()
}

/// Holds the running node so it isn't dropped (dropping would tear it down).
#[derive(Default)]
#[cfg_attr(not(feature = "mesh"), allow(dead_code))]
pub struct MeshState {
    #[cfg(feature = "mesh")]
    core: Option<std::sync::Arc<yggdrasil::core::Core>>,
    #[cfg(feature = "mesh")]
    tun: Option<yggdrasil::tun::TunAdapter>,
    /// The node's mesh IPv6 address (when running).
    address: Option<String>,
    /// The peer URIs the running node was started with (watchdog probes these).
    current_peers: Vec<String>,
    /// True while a start is in flight (guards against concurrent starts; the
    /// slow startup work runs without holding this state's lock).
    starting: bool,
    /// Bumped on every connect/disconnect; a watchdog exits when it no longer
    /// matches, so only the newest one runs.
    watchdog_gen: u64,
}

/// Managed-state alias.
#[cfg_attr(not(feature = "mesh"), allow(dead_code))]
pub type SharedMesh = Mutex<MeshState>;

/// Status pushed to the frontend as a `dev-mesh` event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshStatus {
    pub running: bool,
    pub address: Option<String>,
    /// Whether mesh is compiled into this build (`--features mesh`). When false,
    /// the toggle can't do anything.
    pub available: bool,
    /// The user's persisted preference. `enabled && !running` means the start
    /// failed (e.g. missing admin privileges) - the UI shows that distinctly
    /// instead of silently flipping the toggle off.
    pub enabled: bool,
}

/// Whether mesh networking is compiled into this build.
#[must_use]
pub fn is_available() -> bool {
    cfg!(feature = "mesh")
}

/// Current mesh status (for the Settings UI).
#[tauri::command]
pub async fn get_mesh_status(app: AppHandle) -> MeshStatus {
    let address = current_address(&app).await;
    MeshStatus {
        running: address.is_some(),
        address,
        available: is_available(),
        enabled: crate::settings::is_mesh_enabled(&app),
    }
}

/// Payload of the `mesh-connect-status` event driving the Settings throbber and
/// status line: `connecting` (orange), `connected` (green), `error` (red),
/// `idle` (disconnected).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectStatus {
    pub state: String,
    pub message: String,
}

/// Emit a connect-status line to the frontend (and the log, so a stuck connect
/// is diagnosable from the log file alone).
fn emit_status(app: &AppHandle, state: &str, message: impl Into<String>) {
    use tauri::Emitter;
    let message = message.into();
    tracing::info!(state, %message, "mesh connect status");
    let _ = app.emit(
        "mesh-connect-status",
        ConnectStatus {
            state: state.to_owned(),
            message,
        },
    );
}

/// Pull `tcp://` / `tls://` URIs out of the user's private-peers text. Lenient:
/// accepts the `Peers: [ ... ]` block format, one-per-line, or comma-separated.
fn parse_private_peers(text_lines: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for line in text_lines {
        for token in line.split([' ', ',', '\t', '[', ']']) {
            let token = token.trim();
            if token.starts_with("tcp://") || token.starts_with("tls://") {
                out.push(token.to_owned());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Resolve the peer list for a mode, emitting progress along the way.
async fn resolve_peers(
    app: &AppHandle,
    mode: crate::settings::YggPeerMode,
    private_peers: &[String],
) -> Result<Vec<String>, String> {
    use crate::settings::YggPeerMode;
    match mode {
        YggPeerMode::Authorized => {
            if crate::peers::AUTHORIZED_PEERS.is_empty() {
                return Err(
                    "no authorized peers are published yet - they arrive with our hosted \
                     infrastructure; use Public or Private peers for now"
                        .to_owned(),
                );
            }
            Ok(crate::peers::AUTHORIZED_PEERS
                .iter()
                .map(|s| (*s).to_owned())
                .collect())
        }
        YggPeerMode::Private => {
            let parsed = parse_private_peers(private_peers);
            if parsed.is_empty() {
                return Err(
                    "no peer URIs found - enter at least one tcp:// or tls:// peer".to_owned(),
                );
            }
            Ok(parsed)
        }
        YggPeerMode::Public => {
            emit_status(
                app,
                "connecting",
                "Finding the best public peers for your region...",
            );
            crate::peers::select_public_peers().await
        }
    }
}

/// Connect the mesh using a peer mode (persisted): resolve peers, restart the
/// node, start the watchdog. Drives the Settings connect flow.
#[tauri::command]
pub async fn mesh_connect(
    app: AppHandle,
    mode: crate::settings::YggPeerMode,
    private_peers: Vec<String>,
) -> Result<MeshStatus, String> {
    crate::settings::store_ygg_config(&app, mode, private_peers.clone())?;
    if !is_available() {
        let msg = "mesh networking is not compiled into this build";
        emit_status(&app, "error", msg);
        return Err(msg.to_owned());
    }

    emit_status(&app, "connecting", "Resolving peers...");
    let peers = match resolve_peers(&app, mode, &private_peers).await {
        Ok(p) => p,
        Err(e) => {
            emit_status(&app, "error", e.clone());
            return Err(e);
        }
    };

    emit_status(
        &app,
        "connecting",
        format!("Connecting via {} peer(s)...", peers.len()),
    );
    let _ = stop(&app).await; // restart cleanly if already running
    match start_with_peers(&app, Some(peers)).await {
        Ok(address) => {
            let _ = crate::settings::store_mesh_enabled(&app, true);
            spawn_watchdog(&app).await;
            emit_status(
                &app,
                "connected",
                format!("Connected - mesh address {address}"),
            );
            Ok(get_mesh_status(app).await)
        }
        Err(e) => {
            emit_status(&app, "error", e.clone());
            Err(e)
        }
    }
}

/// Disconnect the mesh and turn auto-start off.
#[tauri::command]
pub async fn mesh_disconnect(app: AppHandle) -> Result<MeshStatus, String> {
    let _ = crate::settings::store_mesh_enabled(&app, false);
    {
        let state = app.state::<SharedMesh>();
        state.lock().await.watchdog_gen += 1; // retire the watchdog
    }
    stop(&app).await?;
    emit_status(&app, "idle", "Disconnected");
    Ok(get_mesh_status(app).await)
}

/// Legacy toggle (kept for compatibility): routes through the connect flow with
/// the persisted peer mode.
#[tauri::command]
pub async fn set_mesh_enabled(app: AppHandle, enabled: bool) -> Result<MeshStatus, String> {
    if enabled {
        let (mode, private) = crate::settings::ygg_config(&app);
        mesh_connect(app, mode, private).await
    } else {
        mesh_disconnect(app).await
    }
}

/// Start the mesh on launch if the user enabled it, using the persisted peer
/// mode. Best-effort: a failure (e.g. missing privileges) is logged, not fatal.
pub async fn auto_start_if_enabled(app: &AppHandle) {
    if crate::settings::is_mesh_enabled(app) {
        let (mode, private) = crate::settings::ygg_config(app);
        if let Err(e) = mesh_connect(app.clone(), mode, private).await {
            tracing::warn!("mesh auto-start failed: {e}");
        }
    }
}

/// How often the watchdog re-probes the configured peers.
const WATCHDOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(240);

/// Watch the configured peers; when ALL become unreachable, migrate: public mode
/// re-selects fresh peers from the lists, other modes retry their fixed set.
async fn spawn_watchdog(app: &AppHandle) {
    let generation = {
        let state = app.state::<SharedMesh>();
        let mut guard = state.lock().await;
        guard.watchdog_gen += 1;
        guard.watchdog_gen
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(WATCHDOG_INTERVAL).await;
            let (still_current, running, peers) = {
                let state = app.state::<SharedMesh>();
                let guard = state.lock().await;
                (
                    guard.watchdog_gen == generation,
                    guard.address.is_some(),
                    guard.current_peers.clone(),
                )
            };
            if !still_current || !running {
                return;
            }
            if peers.is_empty() {
                return; // LAN-multicast-only node; nothing to watch
            }
            if crate::peers::any_reachable(&peers).await {
                continue;
            }

            // Every configured peer is down - migrate.
            tracing::warn!("all mesh peers unreachable; migrating");
            emit_status(&app, "connecting", "Mesh peers unreachable - migrating...");
            let (mode, private) = crate::settings::ygg_config(&app);
            let next = match resolve_peers(&app, mode, &private).await {
                Ok(p) => p,
                Err(e) => {
                    emit_status(&app, "error", format!("could not find new peers: {e}"));
                    continue; // keep watching; the network may come back
                }
            };
            let _ = stop(&app).await;
            match start_with_peers(&app, Some(next)).await {
                Ok(address) => {
                    emit_status(
                        &app,
                        "connected",
                        format!("Reconnected - mesh address {address}"),
                    );
                }
                Err(e) => emit_status(&app, "error", format!("mesh restart failed: {e}")),
            }
        }
    });
}

/// Start the mesh node with the env/file/bundled peer resolution (dev path);
/// returns this node's mesh IPv6 address.
pub async fn start(app: &AppHandle) -> Result<String, String> {
    start_with_peers(app, None).await
}

/// Start the mesh node, peering through `peers` when given (Settings > Network
/// connect path); `None` falls back to env/file/bundled resolution.
pub async fn start_with_peers(
    app: &AppHandle,
    peers: Option<Vec<String>>,
) -> Result<String, String> {
    #[cfg(feature = "mesh")]
    {
        imp::start(app, peers).await
    }
    #[cfg(not(feature = "mesh"))]
    {
        let _ = (app, peers);
        Err("mesh networking not compiled in - rebuild with `--features mesh`".to_owned())
    }
}

/// This node's current mesh address, if the mesh is running.
pub async fn current_address(app: &AppHandle) -> Option<String> {
    app.state::<SharedMesh>().lock().await.address.clone()
}

/// Stop the mesh node.
pub async fn stop(app: &AppHandle) -> Result<(), String> {
    #[cfg(feature = "mesh")]
    {
        imp::stop(app).await
    }
    #[cfg(not(feature = "mesh"))]
    {
        let _ = app;
        Ok(())
    }
}

/// Dev command: start mesh networking.
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn dev_start_mesh(app: AppHandle) -> Result<String, String> {
    start(&app).await
}

/// Dev command: stop mesh networking.
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn dev_stop_mesh(app: AppHandle) -> Result<(), String> {
    stop(&app).await
}

#[cfg(feature = "mesh")]
mod imp {
    use super::{MeshStatus, SharedMesh};
    use std::sync::Arc;

    use ed25519_dalek::SigningKey;
    use tauri::{AppHandle, Emitter, Manager};
    use yggdrasil::config::Config;
    use yggdrasil::core::Core;
    use yggdrasil::ipv6rwc::ReadWriteCloser;
    use yggdrasil::tun::TunAdapter;

    /// How long TUN-device creation may take before we give up. The underlying
    /// wintun call is synchronous and has been observed to stall indefinitely
    /// (no admin rights, or a stale adapter), so it runs on its own task with
    /// this escape hatch.
    const TUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    pub async fn start(
        app: &AppHandle,
        peers_override: Option<Vec<String>>,
    ) -> Result<String, String> {
        // Short lock: bail when already running or mid-start. The slow work
        // below runs UNLOCKED so a stalled start can never freeze status
        // queries (get_mesh_status / current_address / contact codes).
        {
            let state = app.state::<SharedMesh>();
            let mut guard = state.lock().await;
            if let Some(addr) = &guard.address {
                return Ok(addr.clone()); // already running
            }
            if guard.starting {
                return Err("the mesh is already starting".to_owned());
            }
            guard.starting = true;
        }

        let result = start_inner(app, peers_override).await;

        let state = app.state::<SharedMesh>();
        let mut guard = state.lock().await;
        guard.starting = false;
        match result {
            Ok((core, tun, address, peers)) => {
                guard.core = Some(core);
                guard.tun = Some(tun);
                guard.address = Some(address.clone());
                guard.current_peers = peers;
                drop(guard);
                let _ = app.emit(
                    "dev-mesh",
                    MeshStatus {
                        running: true,
                        address: Some(address.clone()),
                        available: true,
                        enabled: crate::settings::is_mesh_enabled(app),
                    },
                );
                tracing::info!(%address, "mesh node started");
                Ok(address)
            }
            Err(e) => Err(e),
        }
    }

    /// The slow part of startup (network + TUN), run without holding the lock.
    /// Returns everything `start` stores on success.
    async fn start_inner(
        app: &AppHandle,
        peers_override: Option<Vec<String>>,
    ) -> Result<(Arc<Core>, TunAdapter, String, Vec<String>), String> {
        let signing = load_or_create_key(app)?;

        let mut config = Config::default();
        // Explicit peers (Settings connect flow) win; otherwise the dev
        // resolution: env var, peers file, bundled defaults.
        config.peers = peers_override.unwrap_or_else(|| load_peers(app));

        // Build + start the node (no admin needed up to here).
        let core = Core::new(signing, config.clone());
        let address = core.address().to_string();
        core.init_links().await;
        core.start().await;

        // IPv6 bridge between the OS TUN and the router (firewall off, ckr off).
        let mtu = core.mtu();
        let rwc: Arc<ReadWriteCloser> = ReadWriteCloser::new(core.clone(), mtu, None);
        core.set_path_notify(rwc.clone());

        // Bring up the TUN interface (THIS needs admin/root). The wintun call
        // inside is synchronous and can stall, so it runs on its own task and we
        // await it with a timeout - a stall becomes a clear error instead of a
        // forever-stuck "connecting".
        tracing::info!("creating TUN interface (needs admin)...");
        let addr_str = address.clone();
        let subnet_str = core.subnet().to_string();
        let tun_mtu = config.if_mtu.min(mtu).min(65535) as u16;
        let if_name = config.if_name.clone();
        #[cfg(windows)]
        let dns = config.if_dns_servers.clone();
        let rwc_for_tun = rwc.clone();
        let tun_task = tauri::async_runtime::spawn(async move {
            TunAdapter::new(
                &if_name,
                rwc_for_tun,
                &addr_str,
                &subnet_str,
                tun_mtu,
                #[cfg(windows)]
                &dns,
            )
            .await
        });
        let tun = match tokio::time::timeout(TUN_TIMEOUT, tun_task).await {
            Ok(Ok(Ok(tun))) => tun,
            Ok(Ok(Err(e))) => {
                tracing::warn!("TUN creation failed (leaking started core): {e}");
                return Err(format!(
                    "could not create TUN interface (run as admin?): {e}"
                ));
            }
            Ok(Err(join_err)) => {
                tracing::warn!("TUN task crashed (leaking started core): {join_err}");
                return Err(format!("TUN setup crashed: {join_err}"));
            }
            Err(_) => {
                tracing::warn!("TUN creation timed out (leaking started core)");
                return Err(format!(
                    "TUN setup timed out after {}s - make sure the app is running as \
                     administrator; if it keeps happening, remove the stale 'Yggdrasil' \
                     network adapter in Device Manager and try again",
                    TUN_TIMEOUT.as_secs()
                ));
            }
        };

        let _ = core.start_multicast().await; // best-effort LAN discovery
        let peers = config.peers;
        Ok((core, tun, address, peers))
    }

    pub async fn stop(app: &AppHandle) -> Result<(), String> {
        let state = app.state::<SharedMesh>();
        let mut guard = state.lock().await;
        if let Some(tun) = guard.tun.take() {
            tun.close().await;
        }
        guard.core = None;
        guard.address = None;
        guard.current_peers.clear();
        let _ = app.emit(
            "dev-mesh",
            MeshStatus {
                running: false,
                address: None,
                available: true,
                enabled: crate::settings::is_mesh_enabled(app),
            },
        );
        tracing::info!("mesh node stopped");
        Ok(())
    }

    /// Load the persisted mesh signing key, or generate + persist one so this
    /// install keeps a stable mesh address across restarts. Stored encrypted at
    /// rest (whoever lifts the file could otherwise assume this node's mesh
    /// address); an older plaintext key is read once and re-saved encrypted.
    fn load_or_create_key(app: &AppHandle) -> Result<SigningKey, String> {
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("mesh.key");
        if let Ok(bytes) = std::fs::read(&path) {
            // Current format: encrypted at rest.
            if let Some(plaintext) = crate::at_rest::open_bytes(&bytes) {
                if let Ok(arr) = <[u8; 32]>::try_from(plaintext.as_slice()) {
                    return Ok(SigningKey::from_bytes(&arr));
                }
            }
            // Migration fallback: an older raw 32-byte key. Re-save it encrypted.
            if let Ok(arr) = <[u8; 32]>::try_from(bytes.as_slice()) {
                let key = SigningKey::from_bytes(&arr);
                let blob = crate::at_rest::seal_bytes(&key.to_bytes())?;
                std::fs::write(&path, blob).map_err(|e| e.to_string())?;
                return Ok(key);
            }
        }
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let blob = crate::at_rest::seal_bytes(&key.to_bytes())?;
        std::fs::write(&path, blob).map_err(|e| e.to_string())?;
        Ok(key)
    }

    /// Bootstrap peers for internet reachability, from `ACCORD_MESH_PEERS`
    /// (comma-separated) or `mesh-peers.txt` (one per line) in the app-data dir.
    /// Empty is fine on a LAN - multicast discovery finds local peers.
    fn load_peers(app: &AppHandle) -> Vec<String> {
        if let Ok(env) = std::env::var("ACCORD_MESH_PEERS") {
            let peers: Vec<String> = env
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            if !peers.is_empty() {
                return peers;
            }
        }
        if let Ok(dir) = app.path().app_data_dir() {
            if let Ok(text) = std::fs::read_to_string(dir.join("mesh-peers.txt")) {
                let from_file: Vec<String> = text
                    .lines()
                    .map(|l| l.trim().to_owned())
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .collect();
                if !from_file.is_empty() {
                    return from_file;
                }
            }
        }
        // Nothing configured -> fall back to the bundled public peers so internet
        // hosting works out of the box.
        super::default_peers()
    }
}
