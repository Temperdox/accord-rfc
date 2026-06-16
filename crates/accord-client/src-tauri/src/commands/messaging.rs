//! Messaging commands + the resilient, per-server live streams.
//!
//! Each connected server has its own `start_session` supervisor that keeps a
//! bidirectional `MessageStream` open (reconnecting on drops/sleep) plus a token-
//! refresh loop. All of them run concurrently, and every event they emit is
//! tagged with its `server_id` so the UI can route it to the right server. The
//! send/fetch commands act on the *active* server.

use std::time::Duration;

use accord_mls::DecryptOutcome;
use accord_proto::auth_service_client::AuthServiceClient;
use accord_proto::client_message::Payload as ClientPayload;
use accord_proto::group_service_client::GroupServiceClient;
use accord_proto::messaging_service_client::MessagingServiceClient;
use accord_proto::role_service_client::RoleServiceClient;
use accord_proto::server_message::Payload as ServerPayload;
use accord_proto::{
    BanMemberRequest, ClientMessage, CreatePublicGroupRequest, DeleteGroupRequest,
    FetchHistoryRequest, GetMyPermissionsRequest, GetTavernRequest, GroupId, KickMemberRequest,
    ListBansRequest, ListGroupsRequest, ListMembersRequest, MessageId, RefreshTokenRequest,
    SendPrivateMessage, SendPublicMessage, UnbanMemberRequest, UpdateTavernRequest, UserId,
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tonic::{Code, Request};

use crate::commands::dto::{
    BanDto, ConnectionStatus, DecryptedHistory, GroupDto, HistoryEntry, JoinedGroup, MemberDto,
    MessageDto, ModAlertDto, MyPermsDto, PrivateMessageDto, TavernDto, VoiceParticipantDto,
    VoiceSignalDto,
};
use crate::grpc::{authed, require_session, status_to_string};
use crate::state::{SharedEngine, SharedSessions};

const CONNECTION_STATUS: &str = "connection-status";
/// Refresh the access token well before the (default 1h) expiry. A tokio timer's
/// deadline elapses during OS sleep, so this also fires promptly on wake.
const TOKEN_REFRESH_EVERY: Duration = Duration::from_secs(45 * 60);

const INCOMING_PUBLIC: &str = "incoming-message";
const INCOMING_PRIVATE: &str = "incoming-private-message";
const HISTORY_DECRYPTED: &str = "private-history-decrypted";
const JOINED_GROUP: &str = "joined-group";
const VOICE_PARTICIPANT: &str = "voice-participant";
const VOICE_SIGNAL: &str = "voice-signal";
const MOD_ALERT: &str = "mod-alert";
const OUTBOUND_BUFFER: usize = 32;

/// Convert an archived message into the UI DTO, tagged with its server and
/// whether this session's own user sent it.
fn to_private_dto(
    m: crate::history::ArchivedMessage,
    server_id: &str,
    own_user_id: &str,
) -> PrivateMessageDto {
    PrivateMessageDto {
        server_id: server_id.to_owned(),
        group_id: m.group_id,
        mine: m.sender_id == own_user_id,
        sender_id: m.sender_id,
        content: m.content,
        timestamp_ms: m.timestamp_ms,
    }
}

/// Convert a server group id (UUID string) to the MLS group id (its 16 bytes).
pub fn mls_id(group_id: &str) -> Vec<u8> {
    uuid::Uuid::parse_str(group_id)
        .map(|u| u.as_bytes().to_vec())
        .unwrap_or_default()
}

/// Start (or restart) the resilient session for `server_id`: a self-reconnecting
/// message stream plus a periodic token refresh. Returns once the first
/// connection is up. Requires that server's session (channel/token/refresh_token)
/// to already be set.
pub async fn start_session(
    app: AppHandle,
    server_id: String,
    channel: Channel,
    user_id: String,
    engine: SharedEngine,
) -> Result<(), String> {
    // Cancel this server's previous tasks (e.g. a reconnect/re-login).
    {
        let state = app.state::<SharedSessions>();
        let mut sessions = state.lock().await;
        if let Some(s) = sessions.map.get_mut(&server_id) {
            for handle in s.session_tasks.drain(..) {
                handle.abort();
            }
        }
    }

    let (ready_tx, ready_rx) = oneshot::channel::<()>();
    let supervisor = tokio::spawn(stream_supervisor(
        app.clone(),
        server_id.clone(),
        channel.clone(),
        user_id,
        engine,
        Some(ready_tx),
    ));
    let refresher = tokio::spawn(token_refresh_loop(app.clone(), server_id.clone(), channel));

    {
        let state = app.state::<SharedSessions>();
        let mut sessions = state.lock().await;
        if let Some(s) = sessions.map.get_mut(&server_id) {
            s.session_tasks = vec![supervisor, refresher];
        }
    }

    match tokio::time::timeout(Duration::from_secs(20), ready_rx).await {
        Ok(Ok(())) => Ok(()),
        _ => Err("could not open the message stream".to_owned()),
    }
}

/// A server session's current access token, if any.
async fn current_token(app: &AppHandle, server_id: &str) -> Option<String> {
    let state = app.state::<SharedSessions>();
    let sessions = state.lock().await;
    sessions.map.get(server_id).and_then(|s| s.token.clone())
}

/// Exchange a server session's refresh token for a fresh access token. Best-effort.
async fn refresh_access_token(app: &AppHandle, server_id: &str, channel: &Channel) {
    let refresh = {
        let state = app.state::<SharedSessions>();
        let sessions = state.lock().await;
        sessions
            .map
            .get(server_id)
            .and_then(|s| s.refresh_token.clone())
    };
    let Some(refresh) = refresh else {
        return;
    };
    match AuthServiceClient::new(channel.clone())
        .refresh_token(Request::new(RefreshTokenRequest {
            refresh_token: refresh,
        }))
        .await
    {
        Ok(resp) => {
            let resp = resp.into_inner();
            let state = app.state::<SharedSessions>();
            let mut sessions = state.lock().await;
            if let Some(s) = sessions.map.get_mut(server_id) {
                s.token = Some(resp.access_token);
                // The server rotates the refresh token on every use; store the
                // replacement so the session never hard-expires while active.
                if !resp.refresh_token.is_empty() {
                    s.refresh_token = Some(resp.refresh_token);
                }
            }
        }
        Err(e) => tracing::warn!("token refresh failed: {}", e.message()),
    }
}

/// Periodically refresh a session's access token (also fires on wake from sleep).
async fn token_refresh_loop(app: AppHandle, server_id: String, channel: Channel) {
    loop {
        tokio::time::sleep(TOKEN_REFRESH_EVERY).await;
        refresh_access_token(&app, &server_id, &channel).await;
    }
}

/// Keep `server_id`'s stream open, reconnecting with backoff; refresh the token on
/// an auth failure. Emits `connection-status` (tagged with the server) on each
/// transition.
async fn stream_supervisor(
    app: AppHandle,
    server_id: String,
    channel: Channel,
    user_id: String,
    engine: SharedEngine,
    mut ready: Option<oneshot::Sender<()>>,
) {
    let mut attempt: u32 = 0;
    loop {
        let Some(token) = current_token(&app, &server_id).await else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };

        // Fresh outbound channel for this connection; send commands read the
        // current sender from the session, so updating it here is the handoff.
        let (tx, rx) = mpsc::channel::<ClientMessage>(OUTBOUND_BUFFER);
        {
            let state = app.state::<SharedSessions>();
            let mut sessions = state.lock().await;
            if let Some(s) = sessions.map.get_mut(&server_id) {
                s.outbound = Some(tx);
            }
        }

        let request = match authed(Request::new(ReceiverStream::new(rx)), &token) {
            Ok(req) => req,
            Err(_) => break,
        };

        match MessagingServiceClient::new(channel.clone())
            .message_stream(request)
            .await
        {
            Ok(resp) => {
                attempt = 0;
                if let Some(tx) = ready.take() {
                    let _ = tx.send(());
                }
                let _ = app.emit(
                    CONNECTION_STATUS,
                    ConnectionStatus {
                        server_id: server_id.clone(),
                        connected: true,
                    },
                );

                let mut inbound = resp.into_inner();
                let mut closed_by_ui = false;
                loop {
                    match inbound.message().await {
                        Ok(Some(server_msg)) => {
                            if handle_server_message(
                                &app, &server_id, &engine, &user_id, server_msg,
                            )
                            .await
                            .is_break()
                            {
                                closed_by_ui = true;
                                break;
                            }
                        }
                        _ => break, // stream ended or errored -> reconnect
                    }
                }
                if closed_by_ui {
                    return; // webview gone (app closing); stop supervising
                }
            }
            Err(status) => {
                if status.code() == Code::Unauthenticated {
                    refresh_access_token(&app, &server_id, &channel).await;
                }
            }
        }

        let _ = app.emit(
            CONNECTION_STATUS,
            ConnectionStatus {
                server_id: server_id.clone(),
                connected: false,
            },
        );

        attempt = attempt.saturating_add(1);
        let secs = (1u64 << attempt.min(5)).min(30);
        tokio::time::sleep(Duration::from_secs(secs)).await;
    }
}

/// Process one inbound server message from `server_id`. Returns Break when the
/// webview is gone.
async fn handle_server_message(
    app: &AppHandle,
    server_id: &str,
    engine: &SharedEngine,
    user_id: &str,
    msg: accord_proto::ServerMessage,
) -> std::ops::ControlFlow<()> {
    use std::ops::ControlFlow::{Break, Continue};
    let Some(payload) = msg.payload else {
        return Continue(());
    };

    match payload {
        ServerPayload::PublicMessage(m) => {
            let mut dto = MessageDto::from_incoming_public(m);
            dto.server_id = server_id.to_owned();
            if app.emit(INCOMING_PUBLIC, dto).is_err() {
                return Break(());
            }
        }
        ServerPayload::PrivateMessage(m) => {
            let group_id = m.group_id.clone().map(|g| g.value).unwrap_or_default();
            let sender_id = m.sender_id.clone().map(|s| s.value).unwrap_or_default();
            let decrypted = {
                let mut eng = engine.lock().await;
                eng.process_incoming(&mls_id(&group_id), &m.mls_ciphertext)
            };
            crate::mls_persist::persist(app, engine, user_id).await;
            match decrypted {
                Ok(DecryptOutcome::Application(plaintext)) => {
                    let timestamp_ms = m
                        .timestamp
                        .map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
                        .unwrap_or(0);
                    let content = String::from_utf8_lossy(&plaintext).into_owned();
                    crate::history::record(
                        app,
                        user_id,
                        &group_id,
                        &sender_id,
                        &content,
                        timestamp_ms,
                    )
                    .await;
                    let dto = PrivateMessageDto {
                        server_id: server_id.to_owned(),
                        group_id,
                        mine: sender_id == user_id,
                        sender_id,
                        content,
                        timestamp_ms,
                    };
                    if app.emit(INCOMING_PRIVATE, dto).is_err() {
                        return Break(());
                    }
                }
                Ok(_) => {}
                Err(e) => tracing_warn(&format!("could not decrypt private message: {e}")),
            }
        }
        ServerPayload::WelcomeNotification(w) => {
            let group_id = w.group_id.map(|g| g.value).unwrap_or_default();
            let result = {
                let mut eng = engine.lock().await;
                eng.join_from_welcome(&w.welcome)
            };
            match result {
                Ok(_) => {
                    crate::mls_persist::persist(app, engine, user_id).await;
                    let _ = app.emit(
                        JOINED_GROUP,
                        JoinedGroup {
                            server_id: server_id.to_owned(),
                            group_id,
                        },
                    );
                }
                Err(e) => tracing_warn(&format!("ignoring welcome: {e}")),
            }
        }
        ServerPayload::CommitNotification(c) => {
            let group_id = c.group_id.map(|g| g.value).unwrap_or_default();
            {
                let mut eng = engine.lock().await;
                let _ = eng.process_incoming(&mls_id(&group_id), &c.commit);
            }
            crate::mls_persist::persist(app, engine, user_id).await;
        }
        ServerPayload::VoiceParticipant(p) => {
            let dto = VoiceParticipantDto {
                server_id: server_id.to_owned(),
                group_id: p.group_id.map(|g| g.value).unwrap_or_default(),
                user_id: p.user_id.map(|u| u.value).unwrap_or_default(),
                device_id: p.device_id.map(|d| d.value).unwrap_or_default(),
                joined: p.joined,
                muted: p.muted,
                camera_on: p.camera_on,
                screen_on: p.screen_on,
            };
            if app.emit(VOICE_PARTICIPANT, dto).is_err() {
                return Break(());
            }
        }
        ServerPayload::VoiceSignal(s) => {
            // SignalKind: 1=offer, 2=answer, 3=ice (see messaging.proto).
            let kind = match s.kind {
                1 => "offer",
                2 => "answer",
                3 => "ice",
                _ => "unknown",
            }
            .to_owned();
            let dto = VoiceSignalDto {
                server_id: server_id.to_owned(),
                group_id: s.group_id.map(|g| g.value).unwrap_or_default(),
                from_device: s.from_device.map(|d| d.value).unwrap_or_default(),
                kind,
                data: s.data,
            };
            if app.emit(VOICE_SIGNAL, dto).is_err() {
                return Break(());
            }
        }
        ServerPayload::ModAlert(a) => {
            // Severity: 1=info, 2=warn, 3=hostile (see messaging.proto).
            let severity = match a.severity {
                1 => "info",
                2 => "warn",
                3 => "hostile",
                _ => "info",
            }
            .to_owned();
            let timestamp_ms = a
                .timestamp
                .map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
                .unwrap_or(0);
            let dto = ModAlertDto {
                server_id: server_id.to_owned(),
                actor_id: a.actor_id.map(|u| u.value).unwrap_or_default(),
                action: a.action,
                target: a.target,
                reason: a.reason,
                severity,
                timestamp_ms,
            };
            if app.emit(MOD_ALERT, dto).is_err() {
                return Break(());
            }
        }
        ServerPayload::Typing(_) | ServerPayload::Presence(_) => {}
    }
    Continue(())
}

/// Set which connected server the UI is viewing (instant; no reconnect).
#[tauri::command]
pub async fn set_active_server(
    state: State<'_, SharedSessions>,
    server_id: String,
) -> Result<(), String> {
    let mut sessions = state.lock().await;
    if sessions.map.contains_key(&server_id) {
        sessions.active = Some(server_id);
        Ok(())
    } else {
        Err("not connected to that server".to_owned())
    }
}

/// List the channels the active server's user belongs to.
#[tauri::command]
pub async fn list_groups(state: State<'_, SharedSessions>) -> Result<Vec<GroupDto>, String> {
    let (channel, token) = require_session(&state).await?;
    let mut client = GroupServiceClient::new(channel);
    let resp = client
        .list_groups(authed(Request::new(ListGroupsRequest {}), &token)?)
        .await
        .map_err(status_to_string)?
        .into_inner();
    Ok(resp
        .groups
        .into_iter()
        .map(GroupDto::from_summary)
        .collect())
}

/// Create a public channel on the active server (gated server-side by
/// MANAGE_CHANNELS + the guardrail layer). `channel_kind` is "text" or "voice".
#[tauri::command]
pub async fn create_channel(
    state: State<'_, SharedSessions>,
    name: String,
    description: Option<String>,
    channel_kind: Option<String>,
) -> Result<GroupDto, String> {
    let (channel, token) = require_session(&state).await?;
    let mut client = GroupServiceClient::new(channel);
    let resp = client
        .create_public_group(authed(
            Request::new(CreatePublicGroupRequest {
                name,
                description: description.unwrap_or_default(),
                channel_kind: channel_kind.unwrap_or_else(|| "text".into()),
            }),
            &token,
        )?)
        .await
        .map_err(status_to_string)?
        .into_inner();
    let id = resp.group_id.map(|g| g.value).unwrap_or_default();
    // Return a minimal summary; the UI also refreshes via list_groups.
    Ok(GroupDto {
        id,
        name: String::new(),
        kind: "public".into(),
        channel_kind: "text".into(),
        member_count: 1,
    })
}

/// Delete a public channel (gated by MANAGE_CHANNELS + guardrails).
#[tauri::command]
pub async fn delete_channel(state: State<'_, SharedSessions>, group_id: String) -> Result<(), String> {
    let (channel, token) = require_session(&state).await?;
    GroupServiceClient::new(channel)
        .delete_group(authed(
            Request::new(DeleteGroupRequest {
                group_id: Some(GroupId { value: group_id }),
            }),
            &token,
        )?)
        .await
        .map_err(status_to_string)?;
    Ok(())
}

/// List the members of a channel/server (for the member-list panel).
#[tauri::command]
pub async fn list_members(
    state: State<'_, SharedSessions>,
    group_id: String,
) -> Result<Vec<MemberDto>, String> {
    let (channel, token) = require_session(&state).await?;
    let resp = GroupServiceClient::new(channel)
        .list_members(authed(
            Request::new(ListMembersRequest {
                group_id: Some(GroupId { value: group_id }),
            }),
            &token,
        )?)
        .await
        .map_err(status_to_string)?
        .into_inner();
    Ok(resp
        .members
        .into_iter()
        .map(|m| MemberDto {
            user_id: m.user_id.map(|u| u.value).unwrap_or_default(),
            username: m.username,
            display_name: m.display_name,
            is_owner: m.is_owner,
            online: m.online,
            role_ids: m.role_ids,
        })
        .collect())
}

/// The caller's effective permissions on the active server (UI gating).
#[tauri::command]
pub async fn get_my_permissions(state: State<'_, SharedSessions>) -> Result<MyPermsDto, String> {
    let (channel, token) = require_session(&state).await?;
    let resp = RoleServiceClient::new(channel)
        .get_my_permissions(authed(Request::new(GetMyPermissionsRequest {}), &token)?)
        .await
        .map_err(status_to_string)?
        .into_inner();
    Ok(MyPermsDto {
        permissions: resp.permissions,
        is_owner: resp.is_owner,
    })
}

/// Fetch the tavern (server) identity.
#[tauri::command]
pub async fn get_tavern(state: State<'_, SharedSessions>) -> Result<TavernDto, String> {
    let (channel, token) = require_session(&state).await?;
    let t = GroupServiceClient::new(channel)
        .get_tavern(authed(Request::new(GetTavernRequest {}), &token)?)
        .await
        .map_err(status_to_string)?
        .into_inner();
    Ok(TavernDto {
        name: t.name,
        icon_url: t.icon_url,
        description: t.description,
        linking_enabled: t.linking_enabled,
    })
}

/// Update the tavern identity (gated by MANAGE_SERVER + guardrails).
#[tauri::command]
pub async fn update_tavern(
    state: State<'_, SharedSessions>,
    name: String,
    icon_url: Option<String>,
    description: Option<String>,
) -> Result<TavernDto, String> {
    let (channel, token) = require_session(&state).await?;
    let t = GroupServiceClient::new(channel)
        .update_tavern(authed(
            Request::new(UpdateTavernRequest {
                name,
                icon_url: icon_url.unwrap_or_default(),
                description: description.unwrap_or_default(),
            }),
            &token,
        )?)
        .await
        .map_err(status_to_string)?
        .into_inner();
    Ok(TavernDto {
        name: t.name,
        icon_url: t.icon_url,
        description: t.description,
        linking_enabled: t.linking_enabled,
    })
}

/// Kick a member from a channel (gated by KICK_MEMBERS + guardrails).
#[tauri::command]
pub async fn kick_member(
    state: State<'_, SharedSessions>,
    group_id: String,
    user_id: String,
) -> Result<(), String> {
    let (channel, token) = require_session(&state).await?;
    GroupServiceClient::new(channel)
        .kick_member(authed(
            Request::new(KickMemberRequest {
                group_id: Some(GroupId { value: group_id }),
                user_id: Some(UserId { value: user_id }),
            }),
            &token,
        )?)
        .await
        .map_err(status_to_string)?;
    Ok(())
}

/// Ban an account from the server (gated by BAN_MEMBERS + guardrails).
#[tauri::command]
pub async fn ban_member(
    state: State<'_, SharedSessions>,
    user_id: String,
    reason: Option<String>,
) -> Result<(), String> {
    let (channel, token) = require_session(&state).await?;
    GroupServiceClient::new(channel)
        .ban_member(authed(
            Request::new(BanMemberRequest {
                user_id: Some(UserId { value: user_id }),
                reason: reason.unwrap_or_default(),
            }),
            &token,
        )?)
        .await
        .map_err(status_to_string)?;
    Ok(())
}

/// Lift a ban (gated by BAN_MEMBERS).
#[tauri::command]
pub async fn unban_member(state: State<'_, SharedSessions>, user_id: String) -> Result<(), String> {
    let (channel, token) = require_session(&state).await?;
    GroupServiceClient::new(channel)
        .unban_member(authed(
            Request::new(UnbanMemberRequest {
                user_id: Some(UserId { value: user_id }),
            }),
            &token,
        )?)
        .await
        .map_err(status_to_string)?;
    Ok(())
}

/// List the server's bans (gated by BAN_MEMBERS).
#[tauri::command]
pub async fn list_bans(state: State<'_, SharedSessions>) -> Result<Vec<BanDto>, String> {
    let (channel, token) = require_session(&state).await?;
    let resp = GroupServiceClient::new(channel)
        .list_bans(authed(Request::new(ListBansRequest {}), &token)?)
        .await
        .map_err(status_to_string)?
        .into_inner();
    Ok(resp
        .bans
        .into_iter()
        .map(|b| BanDto {
            user_id: b.user_id.map(|u| u.value).unwrap_or_default(),
            reason: b.reason,
            banned_by: b.banned_by.map(|u| u.value).unwrap_or_default(),
            created_at_ms: b.created_at_ms,
        })
        .collect())
}

/// Send a plaintext message to a public channel on the active server.
#[tauri::command]
pub async fn send_public_message(
    state: State<'_, SharedSessions>,
    group_id: String,
    content: String,
) -> Result<(), String> {
    let tx = outbound(&state).await?;
    let message = ClientMessage {
        payload: Some(ClientPayload::PublicMessage(SendPublicMessage {
            group_id: Some(GroupId { value: group_id }),
            content,
            client_message_id: Some(MessageId {
                value: uuid::Uuid::now_v7().to_string(),
            }),
        })),
    };
    tx.send(message)
        .await
        .map_err(|_| "stream closed".to_string())
}

/// Encrypt and send a message to a private (MLS) group on the active server.
#[tauri::command]
pub async fn send_private_message(
    app: AppHandle,
    state: State<'_, SharedSessions>,
    group_id: String,
    content: String,
) -> Result<(), String> {
    let (engine, tx, user_id) = {
        let sessions = state.lock().await;
        let s = sessions.active().ok_or("not connected")?;
        let engine = s.engine.clone().ok_or("MLS engine not initialized")?;
        let tx = s.outbound.clone().ok_or("message stream is not open")?;
        (engine, tx, s.user_id.clone().unwrap_or_default())
    };

    let ciphertext = {
        let mut eng = engine.lock().await;
        eng.encrypt(&mls_id(&group_id), content.as_bytes())
            .map_err(|e| e.to_string())?
    };
    crate::mls_persist::persist(&app, &engine, &user_id).await;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    crate::history::record(&app, &user_id, &group_id, &user_id, &content, now_ms).await;

    let message = ClientMessage {
        payload: Some(ClientPayload::PrivateMessage(SendPrivateMessage {
            group_id: Some(GroupId { value: group_id }),
            mls_ciphertext: ciphertext,
            epoch: 0,
            client_message_id: Some(MessageId {
                value: uuid::Uuid::now_v7().to_string(),
            }),
        })),
    };
    tx.send(message)
        .await
        .map_err(|_| "stream closed".to_string())
}

/// Fetch a private group's recent history from the local archive (active server).
#[tauri::command]
pub async fn fetch_private_history(
    app: AppHandle,
    state: State<'_, SharedSessions>,
    group_id: String,
    limit: Option<u32>,
) -> Result<Vec<HistoryEntry>, String> {
    let (user_id, server_id) = {
        let sessions = state.lock().await;
        let server_id = sessions.active.clone().unwrap_or_default();
        let user_id = sessions
            .active()
            .and_then(|s| s.user_id.clone())
            .unwrap_or_default();
        (user_id, server_id)
    };
    let limit = limit.unwrap_or(200) as usize;

    let raws = crate::history::tail_raw(&app, &user_id, &group_id, limit);
    let mut entries = Vec::with_capacity(raws.len());
    let mut pending: Vec<(String, Vec<u8>)> = Vec::new();
    for r in raws {
        if r.flag == 0 {
            if let Some(m) = crate::history::decode_entry(r.flag, &r.payload) {
                entries.push(HistoryEntry {
                    id: r.id,
                    message: Some(to_private_dto(m, &server_id, &user_id)),
                });
            }
        } else {
            entries.push(HistoryEntry {
                id: r.id.clone(),
                message: None,
            });
            pending.push((r.id, r.payload));
        }
    }

    if !pending.is_empty() {
        let app = app.clone();
        tokio::spawn(async move {
            for (id, payload) in pending {
                if let Some(m) = crate::history::decode_entry(1, &payload) {
                    let _ = app.emit(
                        HISTORY_DECRYPTED,
                        DecryptedHistory {
                            id,
                            message: to_private_dto(m, &server_id, &user_id),
                        },
                    );
                }
                tokio::task::yield_now().await;
            }
        });
    }

    Ok(entries)
}

/// Fetch recent public history for a channel on the active server (newest-first).
#[tauri::command]
pub async fn fetch_public_history(
    state: State<'_, SharedSessions>,
    group_id: String,
) -> Result<Vec<MessageDto>, String> {
    let (channel, token) = require_session(&state).await?;
    let mut client = MessagingServiceClient::new(channel);
    let resp = client
        .fetch_public_history(authed(
            Request::new(FetchHistoryRequest {
                group_id: Some(GroupId { value: group_id }),
                before_sequence: 0,
                limit: 50,
            }),
            &token,
        )?)
        .await
        .map_err(status_to_string)?
        .into_inner();
    Ok(resp
        .messages
        .into_iter()
        .map(MessageDto::from_incoming_public)
        .collect())
}

// --- helpers ----------------------------------------------------------------

async fn outbound(
    state: &State<'_, SharedSessions>,
) -> Result<mpsc::Sender<ClientMessage>, String> {
    state
        .lock()
        .await
        .active()
        .and_then(|s| s.outbound.clone())
        .ok_or_else(|| "message stream is not open".to_string())
}

fn tracing_warn(msg: &str) {
    eprintln!("[accord-client] {msg}");
}
