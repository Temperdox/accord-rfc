//! Private-chat (MLS) commands: starting a DM.
//!
//! `start_dm` opens a DM with someone on the **active** server (resolve username
//! -> fetch KeyPackage -> create group -> register/relay Welcome).
//!
//! `open_contact_dm` is the cross-user path: given a saved contact, connect to
//! **their** home node as a guest (allowed by `open_dms`), signing with our stable
//! contact identity so they can recognize us, then create the DM there. The DM
//! lives on the recipient's host; messages wait in their mailbox while offline.
//! The server only ever sees opaque bytes.

use std::sync::Arc;

use accord_mls::MlsEngine;
use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::group_service_client::GroupServiceClient;
use accord_proto::mls_service_client::MlsServiceClient;
use accord_proto::{
    CreatePrivateGroupRequest, FetchKeyPackagesRequest, GroupId, ListGroupsRequest, LoginRequest,
    LookupUserRequest, RegisterRequest, UploadKeyPackagesRequest, UserId, WelcomeTarget,
};
use tauri::{AppHandle, State};
use tokio::sync::Mutex;
use tonic::Request;
use tonic::transport::Channel;

use crate::commands::dto::{DmConversation, GroupDto, OpenedDm};
use crate::commands::messaging;
use crate::grpc::{authed, require_session, status_to_string};
use crate::state::{SharedEngine, SharedSessions};

/// List DM conversations across the home + contact-DM sessions, each attributed to
/// the other member (matched to a contact, else "Unknown"). Tavern sessions are
/// excluded - their private groups aren't personal DMs.
#[tauri::command]
pub async fn list_dms(
    app: AppHandle,
    state: State<'_, SharedSessions>,
) -> Result<Vec<DmConversation>, String> {
    // Snapshot the DM-bearing sessions so we don't hold the lock across awaits.
    let dm_sessions: Vec<(String, Channel, String, SharedEngine)> = {
        let sessions = state.lock().await;
        sessions
            .map
            .iter()
            .filter(|(id, _)| id.as_str() == "home" || id.starts_with("dm:"))
            .filter_map(|(id, s)| {
                Some((
                    id.clone(),
                    s.channel.clone()?,
                    s.token.clone()?,
                    s.engine.clone()?,
                ))
            })
            .collect()
    };

    let mut out = Vec::new();
    for (server_id, channel, token, engine) in dm_sessions {
        let groups = match GroupServiceClient::new(channel)
            .list_groups(authed(Request::new(ListGroupsRequest {}), &token)?)
            .await
        {
            Ok(r) => r.into_inner().groups,
            Err(_) => continue,
        };
        let eng = engine.lock().await;
        let own = eng.credential_identity().to_vec();
        for g in groups {
            // ChatKind 2 = private (see common.proto).
            if g.kind != 2 {
                continue;
            }
            let group_id = g.group_id.map(|x| x.value).unwrap_or_default();
            let members = eng
                .group_member_identities(&messaging::mls_id(&group_id))
                .unwrap_or_default();
            let Some(peer) = members.into_iter().find(|m| m != &own) else {
                continue;
            };
            let peer_id = hex_full(&peer);
            let peer_name = crate::commands::contacts::lookup(&app, &peer_id)
                .map(|c| c.name)
                .unwrap_or_else(|| "Unknown".to_owned());
            let fingerprint = <[u8; 32]>::try_from(peer.as_slice())
                .ok()
                .and_then(|a| accord_crypto::identity::IdentityPublicKey::from_bytes(&a).ok())
                .map(|pk| pk.fingerprint())
                .unwrap_or_default();
            out.push(DmConversation {
                server_id: server_id.clone(),
                group_id,
                peer_id,
                peer_name,
                fingerprint,
            });
        }
    }
    Ok(out)
}

/// How many KeyPackages a guest publishes on a contact's host.
const KEY_PACKAGE_BATCH: usize = 5;

/// Start (or re-create) a direct message with `username` on the active server.
///
/// Returns the new group's summary so the UI can select it immediately.
#[tauri::command]
pub async fn start_dm(
    app: AppHandle,
    state: State<'_, SharedSessions>,
    username: String,
) -> Result<GroupDto, String> {
    let (channel, token) = require_session(&state).await?;
    let (engine, user_id) = {
        let sessions = state.lock().await;
        let s = sessions.active().ok_or("not connected")?;
        let engine = s.engine.clone().ok_or("MLS engine not initialized")?;
        (engine, s.user_id.clone().unwrap_or_default())
    };

    // Resolve the peer username -> UserId, then create the DM.
    let peer = AuthServiceClient::new(channel.clone())
        .lookup_user(authed(
            Request::new(LookupUserRequest {
                username: username.clone(),
            }),
            &token,
        )?)
        .await
        .map_err(status_to_string)?
        .into_inner();
    let peer_user_id = peer.user_id.ok_or("server returned no user id")?.value;
    let peer_display = if peer.display_name.is_empty() {
        username
    } else {
        peer.display_name
    };

    create_dm_with(
        &app,
        &channel,
        &token,
        &engine,
        &user_id,
        &peer_user_id,
        &peer_display,
    )
    .await
}

/// Open a DM with a saved contact on the contact's own home node.
///
/// `my_display` is the name the contact will see the DM come from.
#[tauri::command]
pub async fn open_contact_dm(
    app: AppHandle,
    contact_id: String,
    my_display: String,
) -> Result<OpenedDm, String> {
    open_dm_with_contact(&app, &contact_id, &my_display, true).await
}

/// Reconnect every persisted DM target in the background (called after a home
/// login, so the DM list survives restarts). Best-effort: an unreachable contact
/// is skipped; each success notifies the UI via a `dms-changed` event.
pub async fn reopen_dm_targets(app: &AppHandle, my_display: &str) {
    use tauri::Emitter;
    for contact_id in dm_targets(app) {
        match open_dm_with_contact(app, &contact_id, my_display, false).await {
            Ok(_) => {
                let _ = app.emit("dms-changed", ());
            }
            Err(e) => tracing::warn!(contact_id, "could not reopen DM: {e}"),
        }
    }
}

/// The full contact-DM dance: reach the contact's host, log in as our guest
/// identity, reuse the existing DM group when our MLS state still has one, else
/// create it. `make_active` switches the UI's active session (true for the user
/// clicking a contact; false for background reconnects).
/// A logged-in guest session on a contact's host (no UI session registered).
pub struct GuestSession {
    pub channel: Channel,
    pub endpoint: String,
    pub token: String,
    pub refresh_token: String,
    pub user_id: String,
}

/// Reach a contact's host (LAN, then mesh) and log in as our stable guest
/// identity (registering it via open_dms on first contact). Shared by the DM
/// flow and friend-request delivery.
pub async fn guest_login(
    app: &AppHandle,
    target: &crate::commands::contacts::ContactTarget,
    my_display: &str,
) -> Result<GuestSession, String> {
    let host_user_id = target
        .host_user_id
        .clone()
        .ok_or("this contact code has no host info - ask them for an updated code")?;
    let cert = target.cert.clone();

    // Reach the contact's host: their LAN address, falling back to their mesh
    // address across the internet (whichever connects first).
    let (endpoint, channel) =
        crate::grpc::connect_first(&target.addresses, cert.as_deref()).await?;

    // Our stable contact identity: the key the contact has in our fr code, so
    // everything we do is attributable to us (not to a throwaway per-host key).
    let master = crate::identity::load_or_create_master(app)?;
    let contact_identity = crate::identity::contact_identity(&master);
    let identity_pubkey = contact_identity.public().to_bytes().to_vec();
    let guest_user = format!("dm-{}", hex16(&identity_pubkey));
    // Secret, deterministic, and **scoped to this host** (its pinned cert when
    // there is one, else its stable user id). Servers see passwords in plaintext
    // at register/login, so a host-independent password would let one malicious
    // DM host impersonate our guest account on every other host. Deriving from
    // the master means we can log back in later without storing anything.
    let host_context = cert.clone().unwrap_or_else(|| host_user_id.clone());
    let guest_pass = hex_full(
        &master
            .derive_for_context(format!("accord:dm-guest-pw:{host_context}").as_bytes())
            .secret_bytes(),
    );

    // Register the guest account (open_dms permits this without an invite); ignore
    // "already exists" so coming back just logs back in.
    let _ = AuthServiceClient::new(channel.clone())
        .register(RegisterRequest {
            username: guest_user.clone(),
            password: guest_pass.clone(),
            display_name: my_display.to_owned(),
            invite_token: String::new(),
            identity_pubkey,
        })
        .await;
    let login = AuthServiceClient::new(channel.clone())
        .login(LoginRequest {
            username: guest_user,
            password: guest_pass,
            // Stable per-install name -> the host reuses the same device row, so
            // mailbox messages queued while we were away survive restarts.
            device_name: crate::identity::device_name(app, "Desktop"),
        })
        .await
        .map_err(status_to_string)?
        .into_inner();
    Ok(GuestSession {
        channel,
        endpoint,
        token: login.access_token,
        refresh_token: login.refresh_token,
        user_id: login.user_id.map(|u| u.value).unwrap_or_default(),
    })
}

async fn open_dm_with_contact(
    app: &AppHandle,
    contact_id: &str,
    my_display: &str,
    make_active: bool,
) -> Result<OpenedDm, String> {
    let state = {
        use tauri::Manager;
        app.state::<SharedSessions>()
    };
    let target = crate::commands::contacts::lookup(app, contact_id).ok_or("unknown contact")?;
    let host_user_id = target
        .host_user_id
        .clone()
        .ok_or("this contact code has no host info - ask them for an updated code")?;
    let cert = target.cert.clone();

    let guest = guest_login(app, &target, my_display).await?;
    let GuestSession {
        channel,
        endpoint,
        token,
        refresh_token,
        user_id: guest_user_id,
    } = guest;

    // Contact identity for the MLS engine (same derivation as in guest_login).
    let master = crate::identity::load_or_create_master(app)?;
    let contact_identity = crate::identity::contact_identity(&master);

    // MLS engine for this DM, signing with our contact identity. Reuse the local
    // cache if we've talked here before so the ratchet state is preserved.
    let mls = match crate::mls_persist::load(app, &guest_user_id) {
        Some(cached) => cached,
        None => MlsEngine::new(&contact_identity).map_err(|e| e.to_string())?,
    };
    let key_packages = mls
        .generate_key_packages(KEY_PACKAGE_BATCH)
        .map_err(|e| e.to_string())?;
    MlsServiceClient::new(channel.clone())
        .upload_key_packages(authed(
            Request::new(UploadKeyPackagesRequest { key_packages }),
            &token,
        )?)
        .await
        .map_err(status_to_string)?;
    let engine: SharedEngine = Arc::new(Mutex::new(mls));

    // Register the contact's host as a background session (not a tavern in the
    // rail). Keyed by the contact so re-opening reuses it. Background reconnects
    // must not steal the UI's active session.
    let server_id = format!("dm:{contact_id}");
    {
        let mut sessions = state.lock().await;
        let s = sessions.entry(&server_id);
        s.channel = Some(channel.clone());
        s.endpoint = Some(endpoint);
        s.cert = cert;
        s.user_id = Some(guest_user_id.clone());
        s.token = Some(token.clone());
        s.refresh_token = Some(refresh_token);
        s.engine = Some(engine.clone());
        if make_active {
            sessions.active = Some(server_id.clone());
        }
    }
    messaging::start_session(
        app.clone(),
        server_id.clone(),
        channel.clone(),
        guest_user_id.clone(),
        engine.clone(),
    )
    .await?;

    // Reuse the existing DM group with this contact when our MLS state still has
    // it - re-opening must not mint a fresh group per app restart.
    let group = match find_existing_dm(&channel, &token, &engine, contact_id, &target.name).await {
        Some(existing) => existing,
        None => {
            create_dm_with(
                app,
                &channel,
                &token,
                &engine,
                &guest_user_id,
                &host_user_id,
                &target.name,
            )
            .await?
        }
    };

    // Remember this contact so the DM reconnects (and stays listed) after a
    // restart.
    record_dm_target(app, contact_id);
    Ok(OpenedDm { server_id, group })
}

/// Find a private group on this host that our engine still has state for and
/// whose other member is `contact_id` (their contact-identity public key).
async fn find_existing_dm(
    channel: &Channel,
    token: &str,
    engine: &SharedEngine,
    contact_id: &str,
    peer_name: &str,
) -> Option<GroupDto> {
    let peer_pub = hex_decode(contact_id)?;
    let groups = GroupServiceClient::new(channel.clone())
        .list_groups(authed(Request::new(ListGroupsRequest {}), token).ok()?)
        .await
        .ok()?
        .into_inner()
        .groups;
    let eng = engine.lock().await;
    for g in groups {
        // ChatKind 2 = private (see common.proto).
        if g.kind != 2 {
            continue;
        }
        let group_id = g.group_id.map(|x| x.value).unwrap_or_default();
        let Ok(members) = eng.group_member_identities(&messaging::mls_id(&group_id)) else {
            continue; // no local MLS state for this group (e.g. pre-reinstall)
        };
        if members.iter().any(|m| m == &peer_pub) {
            return Some(GroupDto {
                id: group_id,
                name: peer_name.to_owned(),
                kind: "private".into(),
                channel_kind: "text".into(),
                member_count: 2,
            });
        }
    }
    None
}

/// Path of the encrypted persisted DM-target registry (contact ids with open
/// DMs, reconnected on login so the DM list survives restarts).
fn targets_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("dm-targets.bin"))
}

/// The persisted DM targets (contact ids).
fn dm_targets(app: &AppHandle) -> Vec<String> {
    let Ok(path) = targets_path(app) else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    crate::at_rest::open_bytes(&bytes)
        .and_then(|pt| serde_json::from_slice(&pt).ok())
        .unwrap_or_default()
}

/// Remember a contact we have a DM with (no-op if already recorded; best-effort).
fn record_dm_target(app: &AppHandle, contact_id: &str) {
    let mut targets = dm_targets(app);
    if targets.iter().any(|t| t == contact_id) {
        return;
    }
    targets.push(contact_id.to_owned());
    let Ok(json) = serde_json::to_vec(&targets) else {
        return;
    };
    let Ok(blob) = crate::at_rest::seal_bytes(&json) else {
        return;
    };
    let Ok(path) = targets_path(app) else {
        return;
    };
    if let Err(e) = std::fs::write(path, blob) {
        tracing::warn!("could not persist DM target: {e}");
    }
}

/// Decode a lowercase hex string into bytes (contact ids are hex public keys).
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Fetch the peer's KeyPackage(s), create a local MLS group, add the peer, and
/// register the group on the server (which relays the Welcome). Shared by
/// `start_dm` and `open_contact_dm`.
async fn create_dm_with(
    app: &AppHandle,
    channel: &Channel,
    token: &str,
    engine: &SharedEngine,
    user_id: &str,
    peer_user_id: &str,
    peer_display: &str,
) -> Result<GroupDto, String> {
    let fetched = MlsServiceClient::new(channel.clone())
        .fetch_key_packages(authed(
            Request::new(FetchKeyPackagesRequest {
                user_ids: vec![UserId {
                    value: peer_user_id.to_owned(),
                }],
            }),
            token,
        )?)
        .await
        .map_err(status_to_string)?
        .into_inner();
    let bundle = fetched
        .packages
        .get(peer_user_id)
        .ok_or("peer has no published KeyPackages (have they logged in?)")?;
    if bundle.device_packages.is_empty() {
        return Err("peer has no devices to add".into());
    }

    let group_uuid = uuid::Uuid::now_v7();
    let group_id = group_uuid.to_string();
    let mut welcomes = Vec::new();
    let mut last_commit = Vec::new();
    {
        let mut eng = engine.lock().await;
        eng.create_group(group_uuid.as_bytes())
            .map_err(|e| e.to_string())?;
        for dp in &bundle.device_packages {
            let (commit, welcome) = eng
                .add_member(group_uuid.as_bytes(), &dp.key_package)
                .map_err(|e| e.to_string())?;
            last_commit = commit;
            welcomes.push(WelcomeTarget {
                device_id: dp.device_id.clone(),
                welcome,
            });
        }
    }
    crate::mls_persist::persist(app, engine, user_id).await;

    GroupServiceClient::new(channel.clone())
        .create_private_group(authed(
            Request::new(CreatePrivateGroupRequest {
                name: peer_display.to_owned(),
                member_ids: vec![UserId {
                    value: peer_user_id.to_owned(),
                }],
                initial_commit: last_commit,
                welcomes,
                group_id: Some(GroupId {
                    value: group_id.clone(),
                }),
            }),
            token,
        )?)
        .await
        .map_err(status_to_string)?;

    Ok(GroupDto {
        id: group_id,
        name: peer_display.to_owned(),
        kind: "private".into(),
        channel_kind: "text".into(),
        member_count: 2,
    })
}

/// Lowercase hex of the first 8 bytes (16 hex chars).
fn hex16(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for b in bytes.iter().take(8) {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Lowercase hex of all bytes.
fn hex_full(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
