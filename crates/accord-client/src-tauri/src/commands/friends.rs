//! Friend requests: a real request/accept flow over the federation transport.
//!
//! Pasting someone's fr code no longer adds them unilaterally - it **sends a
//! friend request**: we connect to their home node as our guest identity and
//! park a request there carrying OUR code. Their client lists it, applies their
//! friend-request policy, and accepts or declines. Accepting parks an `accept`
//! back on our node carrying THEIR code, which we consume to add them - so both
//! sides end up with each other's contact (mutual naming) after one paste + one
//! accept.
//!
//! Persistence and restarts: incoming requests live in the recipient's home-node
//! database (they wait through restarts and logouts while the node is up), and
//! our outgoing requests/accepts live in a local encrypted outbox that is
//! retried on login and whenever the requests view syncs - so an unreachable
//! friend just delays delivery, never loses it.

use accord_proto::friend_service_client::FriendServiceClient;
use accord_proto::{
    DeleteFriendRequestRequest, GetPublicProfileRequest, ListFriendRequestsRequest,
    SendFriendRequestRequest, UserId,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tonic::Request;
use tonic::transport::Channel;

use accord_types::contact::ContactCode;

use crate::commands::contacts::{self, ContactTarget};
use crate::grpc::{authed, status_to_string};
use crate::settings::FriendRequestPolicy;

/// An outgoing request or acceptance awaiting (or retrying) delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OutboxEntry {
    /// Hex of the peer's contact-identity key.
    peer_id: String,
    /// The peer's fr code (how we reach their node).
    peer_code: String,
    peer_name: String,
    /// "request" or "accept".
    kind: String,
    /// Display name we introduce ourselves with.
    my_display: String,
    sent_at_ms: i64,
    delivered: bool,
    /// The peer's live account data, fetched from their home node on delivery
    /// (the code only carries a snapshot). `None` until a fetch succeeds;
    /// `serde(default)` keeps outboxes from before these fields loading.
    #[serde(default)]
    peer_username: Option<String>,
    #[serde(default)]
    peer_display: Option<String>,
}

/// An incoming friend request for the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingRequestDto {
    /// Server-side row id (used to respond).
    pub id: String,
    pub name: String,
    pub fingerprint: String,
    /// The requester's fr code (added on accept).
    pub code: String,
    pub created_at_ms: i64,
}

/// An outgoing request as shown in "Pending sent" (and as a placeholder row in
/// the Friends list until accepted).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSentDto {
    pub peer_id: String,
    /// Name from the pasted code (placeholder until the profile fetch).
    pub name: String,
    pub fingerprint: String,
    pub delivered: bool,
    pub sent_at_ms: i64,
    /// Live account data from their home node, once delivery succeeded.
    pub username: Option<String>,
    pub display_name: Option<String>,
}

/// What a pasted code identifies, without sending anything (drives the
/// send-button state: gray when the pasted code is already pending).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodePeek {
    pub peer_id: String,
    pub name: String,
    pub fingerprint: String,
}

/// Result of a sync: what to render in the Friend Requests view.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendsSync {
    pub incoming: Vec<IncomingRequestDto>,
    pub pending: Vec<PendingSentDto>,
}

// --- outbox persistence (encrypted at rest) ----------------------------------

fn outbox_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("friend-outbox.bin"))
}

fn load_outbox(app: &AppHandle) -> Vec<OutboxEntry> {
    let Ok(path) = outbox_path(app) else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    crate::at_rest::open_bytes(&bytes)
        .and_then(|pt| serde_json::from_slice(&pt).ok())
        .unwrap_or_default()
}

fn save_outbox(app: &AppHandle, entries: &[OutboxEntry]) -> Result<(), String> {
    let json = serde_json::to_vec(entries).map_err(|e| e.to_string())?;
    let blob = crate::at_rest::seal_bytes(&json)?;
    std::fs::write(outbox_path(app)?, blob).map_err(|e| e.to_string())
}

/// Serializes read-modify-write cycles on the outbox file. Tauri runs commands
/// concurrently and deliveries take seconds (a connect timeout per unreachable
/// address), so every mutation goes through [`update_outbox`] - which re-reads
/// under this lock instead of writing back a snapshot taken before the network
/// work, which would silently erase whatever the user did meanwhile.
static OUTBOX_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Apply `mutate` to a freshly-read outbox and persist it when the closure
/// reports a change. Returns the outbox as it now stands on disk.
async fn update_outbox<F>(app: &AppHandle, mutate: F) -> Result<Vec<OutboxEntry>, String>
where
    F: FnOnce(&mut Vec<OutboxEntry>) -> bool,
{
    let _guard = OUTBOX_LOCK.lock().await;
    let mut outbox = load_outbox(app);
    if mutate(&mut outbox) {
        save_outbox(app, &outbox)?;
    }
    Ok(outbox)
}

/// Read the outbox under the lock (no mutation).
async fn read_outbox(app: &AppHandle) -> Vec<OutboxEntry> {
    let _guard = OUTBOX_LOCK.lock().await;
    load_outbox(app)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Build a delivery target from a peer's fr code.
fn target_from_code(code: &str) -> Result<(ContactTarget, Vec<u8>), String> {
    let parsed = ContactCode::decode(code).map_err(|e| e.to_string())?;
    if parsed.identity_pubkey.len() != 32 {
        return Err("contact code has an invalid identity key".to_owned());
    }
    let identity = parsed.identity_pubkey.clone();
    Ok((
        ContactTarget {
            name: parsed.name.unwrap_or_else(|| "Unknown".to_owned()),
            addresses: parsed.addresses,
            cert: parsed.cert,
            host_user_id: parsed.host_user_id,
        },
        identity,
    ))
}

/// Deliver one outbox entry to the peer's home node. Errors mean "retry later".
///
/// On success this also fetches the peer's live public profile (username,
/// display name; avatar/banner once those ship) over the same guest session and
/// stores it on the entry, so the pending UI shows real account data instead of
/// the snapshot baked into the pasted code. The fetch is best-effort: a profile
/// failure never fails the delivery (sync backfills it later).
async fn deliver(app: &AppHandle, entry: &mut OutboxEntry) -> Result<(), String> {
    let (target, _identity) = target_from_code(&entry.peer_code)?;
    let recipient = target
        .host_user_id
        .clone()
        .ok_or("peer's code has no host info")?;
    let guest = crate::commands::mls::guest_login(app, &target, &entry.my_display).await?;
    // A fresh code each delivery so the peer gets our current addresses.
    let my_code = contacts::my_contact_code(app.clone(), Some(entry.my_display.clone())).await?;
    let mut client = FriendServiceClient::new(guest.channel);
    client
        .send_friend_request(authed(
            Request::new(SendFriendRequestRequest {
                recipient: Some(UserId {
                    value: recipient.clone(),
                }),
                contact_code: my_code,
                kind: entry.kind.clone(),
            }),
            &guest.token,
        )?)
        .await
        .map_err(status_to_string)?;

    let profile_req = authed(
        Request::new(GetPublicProfileRequest {
            user_id: Some(UserId { value: recipient }),
        }),
        &guest.token,
    );
    if let Ok(req) = profile_req
        && let Ok(profile) = client.get_public_profile(req).await
    {
        let profile = profile.into_inner();
        entry.peer_username = Some(profile.username);
        entry.peer_display = Some(profile.display_name);
    }
    Ok(())
}

/// The home session's channel + token (where MY incoming requests are parked).
async fn home_creds(app: &AppHandle) -> Result<(Channel, String), String> {
    let state = app.state::<crate::state::SharedSessions>();
    let sessions = state.lock().await;
    sessions
        .map
        .get("home")
        .and_then(|s| Some((s.channel.clone()?, s.token.clone()?)))
        .ok_or_else(|| "not signed in to your home server".to_owned())
}

// --- commands -----------------------------------------------------------------

/// Send a friend request from a pasted fr code. Stores it in the outbox first,
/// so an unreachable peer just means "delivered later", then attempts delivery.
#[tauri::command]
pub async fn send_friend_request(
    app: AppHandle,
    code: String,
    my_display: String,
) -> Result<PendingSentDto, String> {
    let (target, identity) = target_from_code(code.trim())?;
    if target.host_user_id.is_none() {
        return Err(
            "this code has no host info - ask your friend for a freshly generated code".to_owned(),
        );
    }
    let peer_id = contacts::to_hex(&identity);

    let mut entry = OutboxEntry {
        peer_id: peer_id.clone(),
        peer_code: code.trim().to_owned(),
        peer_name: target.name.clone(),
        kind: "request".to_owned(),
        my_display,
        sent_at_ms: now_ms(),
        delivered: false,
        peer_username: None,
        peer_display: None,
    };
    entry.delivered = deliver(&app, &mut entry).await.is_ok();

    let dto = pending_dto(&entry);
    update_outbox(&app, |outbox| {
        outbox.retain(|e| !(e.peer_id == peer_id && e.kind == "request"));
        outbox.push(entry);
        true
    })
    .await?;
    let _ = app.emit("friends-changed", ());

    Ok(dto)
}

/// Re-attempt delivery of a pending request right now (their node upserts, so
/// re-sending an already-delivered request is harmless - it refreshes the code
/// they hold and the profile data we hold).
#[tauri::command]
pub async fn resend_friend_request(
    app: AppHandle,
    peer_id: String,
    my_display: String,
) -> Result<PendingSentDto, String> {
    let mut entry = read_outbox(&app)
        .await
        .into_iter()
        .find(|e| e.peer_id == peer_id && e.kind == "request")
        .ok_or("no pending request for this contact")?;
    if entry.my_display.is_empty() {
        entry.my_display = my_display;
    }
    entry.delivered = deliver(&app, &mut entry).await.is_ok();
    entry.sent_at_ms = now_ms();
    let dto = pending_dto(&entry);
    let delivered = entry.delivered;
    update_outbox(&app, |outbox| {
        outbox.retain(|e| !(e.peer_id == peer_id && e.kind == "request"));
        outbox.push(entry);
        true
    })
    .await?;
    let _ = app.emit("friends-changed", ());
    if delivered {
        Ok(dto)
    } else {
        Err("still unreachable - the request stays queued and retries automatically".to_owned())
    }
}

/// Decode a pasted code locally (nothing is sent). Drives the send-button
/// state: the UI grays the button when the pasted code's peer already has a
/// pending request.
#[tauri::command]
pub fn peek_contact_code(code: String) -> Result<CodePeek, String> {
    let (target, identity) = target_from_code(code.trim())?;
    Ok(CodePeek {
        peer_id: contacts::to_hex(&identity),
        name: target.name,
        fingerprint: contacts::fingerprint(&identity),
    })
}

/// Sync friend requests: retry undelivered outbox entries, consume acceptances
/// (adding the new friend), apply the friend-request policy to incoming
/// requests, and return what the UI should show.
#[tauri::command]
pub async fn sync_friends(app: AppHandle, my_display: String) -> Result<FriendsSync, String> {
    // 1. Retry the outbox (requests AND acceptances) on a snapshot. Delivered
    // requests that are still missing the peer's profile (delivered by an older
    // build, or the profile fetch failed) get a backfill attempt the same way.
    // The file is deliberately not held across these deliveries - each one can
    // burn a connect timeout - so the results are merged back at the end.
    let mut snapshot = read_outbox(&app).await;
    let mut delivered_now: Vec<OutboxEntry> = Vec::new();
    for entry in &mut snapshot {
        if entry.delivered && (entry.peer_display.is_some() || entry.kind != "request") {
            continue;
        }
        if entry.my_display.is_empty() {
            entry.my_display = my_display.clone();
        }
        if deliver(&app, entry).await.is_ok() {
            entry.delivered = true;
            delivered_now.push(entry.clone());
        }
    }

    // 2. Fetch what's parked for me on my home node.
    let (channel, token) = home_creds(&app).await?;
    let parked = FriendServiceClient::new(channel.clone())
        .list_friend_requests(authed(Request::new(ListFriendRequestsRequest {}), &token)?)
        .await
        .map_err(status_to_string)?
        .into_inner()
        .requests;

    let policy = crate::settings::friend_request_policy(&app);
    let mut incoming = Vec::new();
    let mut friends_changed = false;
    // Peers whose acceptance we consumed: their pending "request" entry is done.
    let mut accepted_peers: Vec<String> = Vec::new();
    for entry in parked {
        let Ok((target, identity)) = target_from_code(&entry.contact_code) else {
            // Garbage row; clear it.
            delete_parked(&app, &channel, &token, &entry.id).await;
            continue;
        };
        let peer_id = contacts::to_hex(&identity);

        if entry.kind == "accept" {
            // They accepted us: add them, complete the pending-sent, clear row.
            // Only drop the parked row once the contact is actually stored, so a
            // failed write leaves the acceptance to be consumed on the next sync
            // instead of losing the friendship on this side.
            match contacts::add_contact(app.clone(), entry.contact_code.clone()) {
                Ok(_) => {
                    accepted_peers.push(peer_id);
                    friends_changed = true;
                    delete_parked(&app, &channel, &token, &entry.id).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "could not add accepted friend; retrying next sync")
                }
            }
            continue;
        }

        // kind == "request": apply my policy. "No one" sinks silently (the
        // requester sees no difference, per the blocking design). The
        // tavern-members / friends-of-friends policies need relationship proofs
        // that arrive with the trust work; until then those requests are shown
        // for manual review like "everyone".
        if policy == FriendRequestPolicy::NoOne {
            delete_parked(&app, &channel, &token, &entry.id).await;
            continue;
        }
        incoming.push(IncomingRequestDto {
            id: entry.id,
            name: target.name,
            fingerprint: contacts::fingerprint(&identity),
            code: entry.contact_code,
            created_at_ms: entry.created_at_ms,
        });
    }

    // 3. Merge the delivery results into the outbox as it stands *now* (the user
    // may have sent, cancelled, or accepted something while we were on the
    // network), rather than writing back the pre-delivery snapshot.
    let outbox = update_outbox(&app, |outbox| {
        let mut changed = false;
        for done in &delivered_now {
            if let Some(e) = outbox
                .iter_mut()
                .find(|e| e.peer_id == done.peer_id && e.kind == done.kind)
            {
                e.delivered = true;
                if done.peer_username.is_some() {
                    e.peer_username.clone_from(&done.peer_username);
                }
                if done.peer_display.is_some() {
                    e.peer_display.clone_from(&done.peer_display);
                }
                changed = true;
            }
        }
        let before = outbox.len();
        // A consumed acceptance completes its pending request, and delivered
        // acceptances are one-shot - nothing further arrives for either.
        outbox.retain(|e| {
            !(e.kind == "request" && accepted_peers.contains(&e.peer_id))
                && !(e.kind == "accept" && e.delivered)
        });
        changed || outbox.len() != before
    })
    .await?;

    if friends_changed {
        let _ = app.emit("friends-changed", ());
    }

    let pending = outbox
        .iter()
        .filter(|e| e.kind == "request")
        .map(pending_dto)
        .collect();
    Ok(FriendsSync { incoming, pending })
}

/// Accept or decline an incoming friend request.
#[tauri::command]
pub async fn respond_friend_request(
    app: AppHandle,
    id: String,
    code: String,
    accept: bool,
    my_display: String,
) -> Result<(), String> {
    let (channel, token) = home_creds(&app).await?;
    if !accept {
        delete_parked(&app, &channel, &token, &id).await;
        return Ok(());
    }

    // Add them now, and queue the acceptance back to their node (so they add
    // us). The parked row is only cleared once the contact is stored - dropping
    // it first would make a failed write unrecoverable (the row is gone, so
    // Accept can never be retried).
    contacts::add_contact(app.clone(), code.clone())?;
    delete_parked(&app, &channel, &token, &id).await;
    let (target, identity) = target_from_code(&code)?;
    let mut entry = OutboxEntry {
        peer_id: contacts::to_hex(&identity),
        peer_code: code,
        peer_name: target.name,
        kind: "accept".to_owned(),
        my_display,
        sent_at_ms: now_ms(),
        delivered: false,
        peer_username: None,
        peer_display: None,
    };
    entry.delivered = deliver(&app, &mut entry).await.is_ok();

    let peer_id = entry.peer_id.clone();
    update_outbox(&app, |outbox| {
        outbox.retain(|e| !(e.peer_id == peer_id && e.kind == "accept"));
        if !entry.delivered {
            outbox.push(entry);
        }
        true
    })
    .await?;
    let _ = app.emit("friends-changed", ());
    Ok(())
}

/// Withdraw a pending request locally (their node's copy can't be recalled, but
/// the pending row goes away here).
#[tauri::command]
pub async fn cancel_friend_request(app: AppHandle, peer_id: String) -> Result<(), String> {
    update_outbox(&app, |outbox| {
        let before = outbox.len();
        outbox.retain(|e| !(e.peer_id == peer_id && e.kind == "request"));
        outbox.len() != before
    })
    .await
    .map(|_| ())
}

/// Background sync after login: deliver queued requests/acceptances and consume
/// any acceptances waiting on the home node. Best-effort.
pub async fn background_sync(app: &AppHandle, my_display: &str) {
    if let Err(e) = sync_friends(app.clone(), my_display.to_owned()).await {
        tracing::debug!("friend sync skipped: {e}");
    }
}

// --- helpers -------------------------------------------------------------------

async fn delete_parked(app: &AppHandle, channel: &Channel, token: &str, id: &str) {
    let _ = app; // reserved for richer error surfacing
    if let Ok(req) = authed(
        Request::new(DeleteFriendRequestRequest { id: id.to_owned() }),
        token,
    ) {
        let _ = FriendServiceClient::new(channel.clone())
            .delete_friend_request(req)
            .await;
    }
}

/// Map an outbox entry to its UI shape.
fn pending_dto(e: &OutboxEntry) -> PendingSentDto {
    PendingSentDto {
        peer_id: e.peer_id.clone(),
        name: e.peer_name.clone(),
        fingerprint: hex_fingerprint(&e.peer_id),
        delivered: e.delivered,
        sent_at_ms: e.sent_at_ms,
        username: e.peer_username.clone(),
        display_name: e.peer_display.clone(),
    }
}

/// Fingerprint from a stored hex identity (outbox entries store the hex form).
fn hex_fingerprint(peer_id_hex: &str) -> String {
    let bytes: Option<Vec<u8>> = (0..peer_id_hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(peer_id_hex.get(i..i + 2)?, 16).ok())
        .collect();
    bytes.map(|b| contacts::fingerprint(&b)).unwrap_or_default()
}
