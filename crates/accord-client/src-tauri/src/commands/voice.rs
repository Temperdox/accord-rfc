//! Voice/video channel commands.
//!
//! Two halves:
//! * **presence** - `VoiceStateUpdate` messages on the session's `MessageStream`
//!   telling the server who is in a channel (it fans the roster out to peers),
//! * **media** - delegated to [`crate::voice_engine`], which owns the
//!   microphone, Opus, and the WebRTC peer mesh natively.
//!
//! Signaling (offer/answer/ICE) never reaches the webview: the engine both
//! produces and consumes those envelopes, so a call cannot be broken by the UI
//! switching servers. Presence is likewise addressed to the session hosting the
//! call rather than to whichever session is active.

use accord_proto::client_message::Payload as ClientPayload;
use accord_proto::{ClientMessage, GroupId, VoiceStateUpdate};
use tauri::State;
use tokio::sync::mpsc;

use crate::state::SharedSessions;
use crate::voice_engine::{AudioDevices, RingtoneInfo, VoiceEngine, VoicePrefs};

/// The outbound `MessageStream` sender for a specific server, or an error.
async fn outbound(
    state: &State<'_, SharedSessions>,
    server_id: &str,
) -> Result<mpsc::Sender<ClientMessage>, String> {
    state
        .lock()
        .await
        .outbound_for(server_id)
        .ok_or_else(|| "message stream is not open".to_string())
}

/// The id of the session a call runs on. A DM call is hosted by the friend's
/// node, so this is captured when the call starts and used for its whole life -
/// resolving "active" later would follow the UI to another server.
async fn active_server_id(state: &State<'_, SharedSessions>) -> Result<String, String> {
    state
        .lock()
        .await
        .active
        .clone()
        .ok_or_else(|| "not connected to a server".to_string())
}

/// Join a voice channel: announce presence, then start the native media engine.
#[tauri::command]
pub async fn join_voice(
    state: State<'_, SharedSessions>,
    engine: State<'_, VoiceEngine>,
    group_id: String,
    device_id: String,
) -> Result<(), String> {
    let server_id = active_server_id(&state).await?;
    engine
        .join(server_id.clone(), group_id.clone(), device_id)
        .await?;
    // Announce only once the media side is up, so a peer that reacts instantly
    // finds us ready to negotiate.
    if let Err(e) = send_state(&state, &server_id, group_id, true, false, false, false).await {
        engine.leave().await;
        return Err(e);
    }
    Ok(())
}

/// Leave a voice channel.
#[tauri::command]
pub async fn leave_voice(
    state: State<'_, SharedSessions>,
    engine: State<'_, VoiceEngine>,
    group_id: String,
) -> Result<(), String> {
    let server_id = active_server_id(&state).await?;
    engine.leave().await;
    send_state(&state, &server_id, group_id, false, false, false, false).await
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
    let server_id = active_server_id(&state).await?;
    send_state(
        &state, &server_id, group_id, true, muted, camera_on, screen_on,
    )
    .await
}

/// Stop/resume transmitting the microphone.
#[tauri::command]
pub async fn set_voice_muted(engine: State<'_, VoiceEngine>, muted: bool) -> Result<(), String> {
    engine.set_muted(muted).await;
    Ok(())
}

/// Stop/resume playing everyone else's audio.
#[tauri::command]
pub async fn set_voice_deafened(
    engine: State<'_, VoiceEngine>,
    deafened: bool,
) -> Result<(), String> {
    engine.set_deafened(deafened).await;
    Ok(())
}

/// Apply audio preferences (devices, gain, volume).
#[tauri::command]
pub async fn set_voice_prefs(
    engine: State<'_, VoiceEngine>,
    prefs: VoicePrefs,
) -> Result<(), String> {
    engine.set_prefs(prefs).await;
    Ok(())
}

/// Microphones and speakers to choose from in settings.
#[tauri::command]
pub fn list_audio_devices() -> AudioDevices {
    VoiceEngine::devices()
}

/// The ringtones offered in settings (built-in plus everything bundled from
/// `assets/ringtones/`).
#[tauri::command]
pub fn list_ringtones() -> Vec<RingtoneInfo> {
    VoiceEngine::ringtones()
}

/// Start ringing for an incoming call.
#[tauri::command]
pub async fn start_ringtone(engine: State<'_, VoiceEngine>) -> Result<(), String> {
    engine.start_ring().await
}

/// Stop ringing (answered, declined, or the caller gave up).
#[tauri::command]
pub async fn stop_ringtone(engine: State<'_, VoiceEngine>) -> Result<(), String> {
    engine.stop_ring().await;
    Ok(())
}

/// Play a ringtone once so the user can hear it while choosing.
#[tauri::command]
pub async fn preview_ringtone(engine: State<'_, VoiceEngine>, id: String) -> Result<(), String> {
    engine.preview_ring(&id).await
}

/// Start metering the microphone for the settings Mic Test (transmits nothing).
#[tauri::command]
pub async fn start_mic_test(engine: State<'_, VoiceEngine>) -> Result<(), String> {
    engine.start_mic_test().await
}

/// Stop the Mic Test and release the microphone.
#[tauri::command]
pub async fn stop_mic_test(engine: State<'_, VoiceEngine>) -> Result<(), String> {
    engine.stop_mic_test().await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn send_state(
    state: &State<'_, SharedSessions>,
    server_id: &str,
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
    outbound(state, server_id)
        .await?
        .send(msg)
        .await
        .map_err(|_| "stream closed".to_string())
}
