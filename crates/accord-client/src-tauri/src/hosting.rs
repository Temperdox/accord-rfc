//! Self-hosting: run an `accord-server` **in-process** so the client can host.
//!
//! This is the mechanism behind "the client can also be a server". The embedded
//! server uses SQLite + an in-process bus (zero external services) and binds to
//! `0.0.0.0:<port>` so other devices on the LAN can connect.
//!
//! For now this is exposed only through a **dev-only menu** (see `main.rs`,
//! gated by `#[cfg(debug_assertions)]`) for testing - a polished end-user
//! hosting UI comes later. The start/stop logic here is always compiled (it is
//! the future production capability); only the dev *commands* and *menu* are
//! gated out of release builds.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use serde::Serialize;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;
use tokio::sync::oneshot;

/// Default port the embedded server listens on.
pub const DEFAULT_HOST_PORT: u16 = 50051;

/// Handle to the running embedded server (if any).
#[derive(Default)]
pub struct LocalServer {
    shutdown: Option<oneshot::Sender<()>>,
    handle: Option<JoinHandle<()>>,
    /// The LAN URL other devices should connect to.
    addr: Option<String>,
    /// The shareable host (LAN IP) + port, for building invite keys.
    host: Option<String>,
    port: Option<u16>,
    /// Whether the running server is private (invite-only).
    private: bool,
    /// The server's self-signed TLS cert (PEM), when serving over TLS.
    cert: Option<String>,
}

/// The host's TLS cert (PEM), if it is serving over TLS.
pub async fn host_cert(app: &AppHandle) -> Option<String> {
    let state = app.state::<SharedLocalServer>();
    let guard = state.lock().await;
    guard.cert.clone()
}

/// `(endpoint, cert)` the host itself should connect with (localhost + its cert).
pub async fn local_connect_info(app: &AppHandle) -> Option<(String, Option<String>)> {
    let state = app.state::<SharedLocalServer>();
    let guard = state.lock().await;
    let port = guard.port?;
    let scheme = if guard.cert.is_some() {
        "https"
    } else {
        "http"
    };
    Some((format!("{scheme}://127.0.0.1:{port}"), guard.cert.clone()))
}

/// The running server's shareable host + port (for invite keys), if hosting.
pub async fn shareable(app: &AppHandle) -> Option<(String, u16, bool)> {
    let state = app.state::<SharedLocalServer>();
    let guard = state.lock().await;
    match (&guard.host, guard.port) {
        (Some(h), Some(p)) => Some((h.clone(), p, guard.private)),
        _ => None,
    }
}

/// Managed-state alias.
pub type SharedLocalServer = Mutex<LocalServer>;

/// Status pushed to the frontend as a `dev-server` event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevServerStatus {
    pub running: bool,
    pub addr: Option<String>,
}

/// Start the embedded server (idempotent). Returns the LAN URL.
///
/// `require_invite` makes it a **private** server (invite-only registration).
/// `tls` serves over TLS with a persisted self-signed cert (pinned via the
/// invite key); `false` serves plaintext (dev / manual-URL testing).
pub async fn start(
    app: &AppHandle,
    port: u16,
    require_invite: bool,
    tls: bool,
) -> Result<String, String> {
    let state = app.state::<SharedLocalServer>();
    let mut guard = state.lock().await;
    if let Some(addr) = &guard.addr {
        return Ok(addr.clone()); // already running
    }

    // Bind the IPv6 wildcard; the server makes it dual-stack, so this one
    // listener serves LAN IPv4 *and* the Yggdrasil mesh IPv6 address.
    let bind: SocketAddr = format!("[::]:{port}")
        .parse()
        .map_err(|e| format!("invalid port: {e}"))?;
    // Fail fast with a clear message if the port is taken. Check IPv4 loopback
    // explicitly: a dual-stack `[::]` bind can otherwise succeed even while a
    // `127.0.0.1`-specific listener (e.g. a standalone accord-server) owns the
    // port, and then loopback connections would silently hit that other server.
    std::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).map_err(|_| {
        format!(
            "port {port} is already in use - is another Accord server running \
             (e.g. the standalone Server)? Stop it and try again."
        )
    })?;
    std::net::TcpListener::bind(bind).map_err(|e| format!("port {port} unavailable: {e}"))?;

    // Store the SQLite file in the OS app-data directory.
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create data dir: {e}"))?;
    let db_path = dir.join("accord-host.db");
    // sqlx wants forward slashes in the sqlite URL, even on Windows.
    let database_url = format!("sqlite:{}", db_path.to_string_lossy().replace('\\', "/"));

    // For TLS, load-or-generate a persisted self-signed cert so the pinned cert
    // stays stable across restarts (joiners already have it in their invite key).
    let cert = if tls {
        Some(load_or_create_cert(&dir)?)
    } else {
        None
    };

    let config = accord_server::Config {
        bind_addr: bind,
        database_url,
        redis_url: String::new(), // empty => in-process bus
        jwt_secret: load_or_create_jwt_secret(&dir)?,
        access_token_ttl_secs: 3600,
        db_max_connections: 5,
        require_invite,
        // Allow contacts to open DMs with this device's user without an invite
        // (channel membership still requires one). See BAN-PLAN.md for abuse
        // controls.
        open_dms: true,
        tls_cert_pem: cert.as_ref().map(|(c, _)| c.clone()),
        tls_key_pem: cert.as_ref().map(|(_, k)| k.clone()),
    };

    let (tx, rx) = oneshot::channel();
    let handle = tauri::async_runtime::spawn(async move {
        if let Err(e) = accord_server::run_with_shutdown(config, rx).await {
            eprintln!("[accord-client] embedded server stopped: {e}");
        }
    });

    // The server binds its real listener only after it connects the DB and runs
    // migrations, so it isn't accepting connections the instant this returns.
    // Wait until it does, or the immediate `connect` would hit a transport error.
    wait_until_listening(port).await?;

    let lan_ip = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "<your-LAN-ip>".to_owned());
    let scheme = if tls { "https" } else { "http" };
    let addr = format!("{scheme}://{lan_ip}:{port}");

    guard.shutdown = Some(tx);
    guard.handle = Some(handle);
    guard.addr = Some(addr.clone());
    guard.host = Some(lan_ip);
    guard.port = Some(port);
    guard.private = require_invite;
    guard.cert = cert.map(|(c, _)| c);
    let _ = app.emit(
        "dev-server",
        DevServerStatus {
            running: true,
            addr: Some(addr.clone()),
        },
    );
    Ok(addr)
}

/// Poll until the embedded server is accepting TCP connections on `port` (its
/// listener is bound once migrations finish), or time out. Connect over IPv4
/// loopback - the dual-stack `[::]` listener accepts it.
async fn wait_until_listening(port: u16) -> Result<(), String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err("embedded server did not start in time".to_owned())
}

/// Stop the embedded server (idempotent).
pub async fn stop(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<SharedLocalServer>();
    let mut guard = state.lock().await;
    if let Some(tx) = guard.shutdown.take() {
        let _ = tx.send(()); // triggers graceful shutdown
    }
    guard.handle = None;
    guard.addr = None;
    guard.host = None;
    guard.port = None;
    guard.private = false;
    guard.cert = None;
    let _ = app.emit(
        "dev-server",
        DevServerStatus {
            running: false,
            addr: None,
        },
    );
    Ok(())
}

/// Load the persisted per-install JWT secret, or generate + persist one
/// (encrypted at rest). Every install MUST have its own random secret: a shared
/// or hardcoded secret would let anyone forge access tokens for any user on any
/// reachable home server.
fn load_or_create_jwt_secret(dir: &std::path::Path) -> Result<String, String> {
    let path = dir.join("host-jwt.secret");
    if let Ok(bytes) = std::fs::read(&path) {
        if let Some(plaintext) = crate::at_rest::open_bytes(&bytes) {
            if let Ok(secret) = String::from_utf8(plaintext) {
                if !secret.is_empty() {
                    return Ok(secret);
                }
            }
        }
    }
    // 32 cryptographically-random bytes, hex-encoded. (An Ed25519 secret seed is
    // OS-CSPRNG output; we use the generator the client already links.)
    let random = accord_crypto::identity::IdentityKeyPair::generate();
    let secret: String = random
        .secret_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let blob = crate::at_rest::seal_bytes(secret.as_bytes())?;
    std::fs::write(&path, blob).map_err(|e| format!("could not persist JWT secret: {e}"))?;
    Ok(secret)
}

/// Load the persisted self-signed cert (PEM cert, PEM key), or generate + persist
/// one. Stable across restarts so a pinned cert in someone's invite key keeps
/// working.
fn load_or_create_cert(dir: &std::path::Path) -> Result<(String, String), String> {
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    if let (Ok(cert), Ok(key)) = (
        std::fs::read_to_string(&cert_path),
        std::fs::read_to_string(&key_path),
    ) {
        return Ok((cert, key));
    }
    let (cert, key) = accord_server::tls::generate_self_signed()?;
    std::fs::write(&cert_path, &cert).map_err(|e| format!("could not write cert: {e}"))?;
    std::fs::write(&key_path, &key).map_err(|e| format!("could not write key: {e}"))?;
    Ok((cert, key))
}

/// Whether this is a development build. Drives the dev-only UI (returns `false`
/// in release, so the dev banner never renders in production).
#[tauri::command]
#[must_use]
pub fn is_dev_build() -> bool {
    cfg!(debug_assertions)
}

/// Dev command: start the embedded local server.
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn dev_start_local_server(app: AppHandle, port: Option<u16>) -> Result<String, String> {
    // Dev quick-host is a public/open, plaintext server (easy manual-URL testing).
    start(&app, port.unwrap_or(DEFAULT_HOST_PORT), false, false).await
}

/// Dev command: stop the embedded local server.
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn dev_stop_local_server(app: AppHandle) -> Result<(), String> {
    stop(&app).await
}
