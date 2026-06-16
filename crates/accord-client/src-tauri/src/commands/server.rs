//! Create / Join server commands + invite keys.
//!
//! These give end users a no-config experience: **Create** a server (the client
//! hosts it) and get a shareable **invite key**; **Join** by pasting a key (the
//! app extracts the address, mesh peers, and token - the user types nothing
//! technical). The frontend composes these with `connect`/`register`/`login`.

use accord_proto::CreateInviteRequest;
use accord_proto::auth_service_client::AuthServiceClient;
use accord_types::invite::{InviteKey, Transport};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tonic::Request;

use crate::grpc::{authed, status_to_string};
use crate::state::SharedSessions;
use crate::{hosting, mesh};

const DEFAULT_PORT: u16 = 50051;

/// How the owner connects to their freshly-hosted server (localhost endpoint +
/// the server's TLS cert to pin).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub endpoint: String,
    pub cert: Option<String>,
}

/// Host a new **private** (invite-only, TLS) server in-process. Returns the
/// localhost endpoint + cert the owner connects with (then register -> login ->
/// create invite).
#[tauri::command]
pub async fn host_private_server(app: AppHandle) -> Result<HostInfo, String> {
    hosting::start(&app, DEFAULT_PORT, true, true).await?;
    let (endpoint, cert) = hosting::local_connect_info(&app)
        .await
        .ok_or("server did not start")?;
    Ok(HostInfo { endpoint, cert })
}

/// Host a new **public** (open, TLS) server in-process (scaffold).
#[tauri::command]
pub async fn host_public_server(app: AppHandle) -> Result<HostInfo, String> {
    hosting::start(&app, DEFAULT_PORT, false, true).await?;
    let (endpoint, cert) = hosting::local_connect_info(&app)
        .await
        .ok_or("server did not start")?;
    Ok(HostInfo { endpoint, cert })
}

/// Mint an invite token for a tavern the caller hosts (their home node, or a
/// tavern they created — both are local in-process instances on this machine),
/// and wrap it into an opaque, shareable invite key. `server_id` is the rail id
/// of the tavern to invite to (e.g. `"home"` or a created tavern's id). Each call
/// mints a fresh token. The server gates `CreateInvite` on CREATE_INVITE, so a
/// non-owner / guest session is rejected there.
#[tauri::command]
pub async fn create_invite_key(
    app: AppHandle,
    state: State<'_, SharedSessions>,
    server_id: String,
) -> Result<String, String> {
    // Mint on the TARGET tavern's session (channel/token/cert/endpoint), so the
    // key points at that specific tavern's instance — not a hardcoded home node.
    let (channel, token, cert, endpoint) = {
        let sessions = state.lock().await;
        let s = sessions
            .map
            .get(&server_id)
            .ok_or("not signed in to that tavern")?;
        (
            s.channel.clone().ok_or("tavern is not connected")?,
            s.token.clone().ok_or("not signed in to that tavern")?,
            s.cert.clone(),
            s.endpoint.clone().ok_or("tavern has no endpoint")?,
        )
    };
    let invite = AuthServiceClient::new(channel)
        .create_invite(authed(Request::new(CreateInviteRequest {}), &token)?)
        .await
        .map_err(status_to_string)?
        .into_inner()
        .token;

    // The tavern's port is its own (each instance binds a distinct port); the
    // shareable host is this machine's LAN ip (others can't reach 127.0.0.1). The
    // pinned TLS cert (if any) came from this session's connect.
    let port = parse_port(&endpoint).ok_or("could not determine the tavern's port")?;
    let key = if let Some(mesh_addr) = mesh::current_address(&app).await {
        // Same node over the mesh, at this tavern's port.
        InviteKey::mesh(mesh_addr, port, invite, configured_mesh_peers()).with_cert(cert)
    } else {
        let host = local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .map_err(|e| format!("could not determine LAN address: {e}"))?;
        InviteKey::direct(host, port, invite).with_cert(cert)
    };
    Ok(key.encode())
}

/// Extract the port from a `scheme://host:port` endpoint (the host's own
/// loopback endpoint carries its real bind port).
fn parse_port(endpoint: &str) -> Option<u16> {
    endpoint.trim_end_matches('/').rsplit(':').next()?.parse().ok()
}

/// Decoded invite info handed to the frontend to drive the join flow.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteInfo {
    pub endpoint: String,
    pub token: String,
    pub transport: String,
    pub peers: Vec<String>,
    pub name: Option<String>,
    pub cert: Option<String>,
}

/// Decode an invite key into its parts (pure; no network).
#[tauri::command]
pub fn decode_invite(key: String) -> Result<InviteInfo, String> {
    let k = InviteKey::decode(&key).map_err(|e| e.to_string())?;
    let transport = match k.transport {
        Transport::Direct => "direct",
        Transport::Mesh => "mesh",
    }
    .to_owned();
    Ok(InviteInfo {
        endpoint: k.endpoint(),
        token: k.token,
        transport,
        peers: k.peers,
        name: k.name,
        cert: k.cert,
    })
}

/// Prepare mesh transport for a join: persist the invite's bootstrap peers and
/// start the mesh node. Requires the `mesh` feature + admin (otherwise returns a
/// clear error the UI can show).
#[tauri::command]
pub async fn prepare_mesh(app: AppHandle, peers: Vec<String>) -> Result<String, String> {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("mesh-peers.txt"), peers.join("\n"));
    }
    mesh::start(&app).await
}

/// Mesh bootstrap peers for an invite key: `ACCORD_MESH_PEERS` if set, otherwise
/// the bundled public peers (so a joiner needs no configuration).
fn configured_mesh_peers() -> Vec<String> {
    let from_env: Vec<String> = std::env::var("ACCORD_MESH_PEERS")
        .ok()
        .map(|s| {
            s.split(',')
                .map(|p| p.trim().to_owned())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if from_env.is_empty() {
        mesh::default_peers()
    } else {
        from_env
    }
}

#[cfg(test)]
mod tests {
    use super::parse_port;

    #[test]
    fn parses_endpoint_port() {
        assert_eq!(parse_port("https://127.0.0.1:50052"), Some(50052));
        assert_eq!(parse_port("http://127.0.0.1:50051/"), Some(50051));
        assert_eq!(parse_port("https://192.168.1.5:50099"), Some(50099));
        assert_eq!(parse_port("not-an-endpoint"), None);
        assert_eq!(parse_port("https://host:notaport"), None);
    }
}
