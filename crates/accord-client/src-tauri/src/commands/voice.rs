//! Voice/video channel commands (scaffold).
//!
//! Transport plan: WebRTC P2P in the webview (`RTCPeerConnection` +
//! `getUserMedia`/`getDisplayMedia`); these commands only carry **signaling** on
//! the existing `MessageStream` (the server relays it opaquely). The actual
//! peer-connection/track wiring lives in the frontend (`src/voice.ts`) and is the
//! documented TODO seam - these commands are the IPC half of that seam.

use accord_proto::client_message::Payload as ClientPayload;
use accord_proto::{ClientMessage, DeviceId, GroupId, VoiceSignal, VoiceStateUpdate};
use tauri::State;
use tokio::sync::mpsc;

use crate::state::SharedSessions;

/// The active session's outbound `MessageStream` sender, or an error.
async fn outbound(state: &State<'_, SharedSessions>) -> Result<mpsc::Sender<ClientMessage>, String> {
    state
        .lock()
        .await
        .active()
        .and_then(|s| s.outbound.clone())
        .ok_or_else(|| "message stream is not open".to_string())
}

/// Join a voice channel (announce presence). The webview then negotiates WebRTC
/// with the participants reported via the `voice-participant` event.
#[tauri::command]
pub async fn join_voice(state: State<'_, SharedSessions>, group_id: String) -> Result<(), String> {
    send_state(&state, group_id, true, false, false, false).await
}

/// Leave a voice channel.
#[tauri::command]
pub async fn leave_voice(state: State<'_, SharedSessions>, group_id: String) -> Result<(), String> {
    send_state(&state, group_id, false, false, false, false).await
}

/// Update this device's mute / camera / screen-share flags while in a channel.
#[tauri::command]
pub async fn set_voice_state(
    state: State<'_, SharedSessions>,
    group_id: String,
    muted: bool,
    camera_on: bool,
    screen_on: bool,
) -> Result<(), String> {
    send_state(&state, group_id, true, muted, camera_on, screen_on).await
}

/// Relay a WebRTC signaling envelope to a specific peer device. Called by the
/// frontend WebRTC layer; `kind` is "offer" | "answer" | "ice".
#[tauri::command]
pub async fn send_voice_signal(
    state: State<'_, SharedSessions>,
    group_id: String,
    target_device: String,
    kind: String,
    data: Vec<u8>,
) -> Result<(), String> {
    // SignalKind: 1=offer, 2=answer, 3=ice (see messaging.proto).
    let kind = match kind.as_str() {
        "offer" => 1,
        "answer" => 2,
        "ice" => 3,
        _ => 0,
    };
    let msg = ClientMessage {
        payload: Some(ClientPayload::VoiceSignal(VoiceSignal {
            group_id: Some(GroupId { value: group_id }),
            from_device: None, // stamped by the server on relay
            target_device: Some(DeviceId {
                value: target_device,
            }),
            kind,
            data,
        })),
    };
    outbound(&state)
        .await?
        .send(msg)
        .await
        .map_err(|_| "stream closed".to_string())
}

async fn send_state(
    state: &State<'_, SharedSessions>,
    group_id: String,
    joined: bool,
    muted: bool,
    camera_on: bool,
    screen_on: bool,
) -> Result<(), String> {
    let msg = ClientMessage {
        payload: Some(ClientPayload::VoiceState(VoiceStateUpdate {
            group_id: Some(GroupId { value: group_id }),
            joined,
            muted,
            camera_on,
            screen_on,
        })),
    };
    outbound(state)
        .await?
        .send(msg)
        .await
        .map_err(|_| "stream closed".to_string())
}
