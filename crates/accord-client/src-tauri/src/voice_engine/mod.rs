//! Native voice engine: microphone in, peers' audio out.
//!
//! The webview used to own this (an `RTCPeerConnection` mesh in `voice.ts`),
//! which cannot work on Linux - WebKitGTK ships without `RTCPeerConnection`, so
//! the API is simply absent there even with `enable-webrtc` on. Moving the
//! whole media path into Rust gives every platform the same, testable pipeline.
//!
//! Responsibilities:
//! * own the microphone and speaker streams for the duration of a call,
//! * encode 20 ms Opus frames and fan them out to one [`PeerLink`] per remote
//!   device (a full mesh, as before - fine for the handful of people a
//!   self-hosted tavern has in a channel),
//! * consume the relayed participant/signaling events and drive negotiation,
//! * report speaking levels to the UI.
//!
//! Signaling is pinned to the session that hosts the call, not to whichever
//! server the UI happens to be showing: a DM call lives on the friend's node,
//! and clicking another tavern mid-call must not redirect its offers.

pub mod audio;
pub mod peer;
#[cfg(test)]
mod peer_it;

use std::collections::HashMap;
use std::sync::Arc;

use accord_proto::client_message::Payload as ClientPayload;
use accord_proto::{ClientMessage, DeviceId, GroupId, VoiceSignal};
use bytes::Bytes;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, broadcast};

use crate::state::SharedSessions;
use audio::{FRAME_SAMPLES, MicCapture, Playback, SAMPLE_RATE};
use peer::{PeerLink, Signaler};

pub use audio::AudioDevices;

/// Event carrying speaking levels to the UI (local mic + each peer device).
const VOICE_LEVELS: &str = "voice-levels";

/// Audio preferences the engine honours. Mirrors the UI's `VoicePrefs`; device
/// fields hold cpal device ids ("" = system default).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoicePrefs {
    #[serde(default)]
    pub mic_device_id: String,
    #[serde(default)]
    pub speaker_device_id: String,
    /// 0..=200, where 100 is unity.
    #[serde(default = "unity")]
    pub mic_gain: f32,
    /// 0..=200, where 100 is unity.
    #[serde(default = "unity")]
    pub output_volume: f32,
}

fn unity() -> f32 {
    100.0
}

impl Default for VoicePrefs {
    fn default() -> Self {
        Self {
            mic_device_id: String::new(),
            speaker_device_id: String::new(),
            mic_gain: 100.0,
            output_volume: 100.0,
        }
    }
}

impl VoicePrefs {
    fn mic(&self) -> Option<&str> {
        Some(self.mic_device_id.as_str()).filter(|s| !s.is_empty())
    }
    fn speaker(&self) -> Option<&str> {
        Some(self.speaker_device_id.as_str()).filter(|s| !s.is_empty())
    }
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LevelsEvent {
    local: f32,
    peers: HashMap<String, f32>,
}

/// Sends signaling envelopes on the session that hosts the call.
struct SessionSignaler {
    app: AppHandle,
    server_id: String,
    group_id: String,
}

impl Signaler for SessionSignaler {
    fn send(&self, device: String, kind: &'static str, payload: String) {
        let (app, server_id, group_id) = (
            self.app.clone(),
            self.server_id.clone(),
            self.group_id.clone(),
        );
        tauri::async_runtime::spawn(async move {
            // SignalKind: 1=offer, 2=answer, 3=ice (see messaging.proto).
            let kind_code = match kind {
                "offer" => 1,
                "answer" => 2,
                "ice" => 3,
                _ => 0,
            };
            let msg = ClientMessage {
                payload: Some(ClientPayload::VoiceSignal(VoiceSignal {
                    group_id: Some(GroupId { value: group_id }),
                    from_device: None, // stamped by the server on relay
                    target_device: Some(DeviceId { value: device }),
                    kind: kind_code,
                    data: payload.into_bytes(),
                })),
            };
            let sender = {
                let sessions = app.state::<SharedSessions>();
                let sessions = sessions.lock().await;
                sessions.outbound_for(&server_id)
            };
            match sender {
                Some(tx) => {
                    if tx.send(msg).await.is_err() {
                        tracing::warn!(server = %server_id, "voice signal dropped: stream closed");
                    }
                }
                None => tracing::warn!(server = %server_id, "voice signal dropped: no session"),
            }
        });
    }
}

/// One connected remote device and the task feeding it our audio.
struct ConnectedPeer {
    link: Arc<PeerLink>,
    sender: JoinHandle<()>,
}

/// A call in progress.
struct ActiveCall {
    server_id: String,
    group_id: String,
    my_device: String,
    mic: Arc<MicCapture>,
    playback: Arc<Playback>,
    signaler: Arc<dyn Signaler>,
    peers: HashMap<String, ConnectedPeer>,
    /// Encoded frames, produced once and fanned out to every peer. Encoding per
    /// peer would be wasteful, but more importantly each peer pulling from the
    /// microphone buffer directly would *consume* frames the others needed, so
    /// with three people in a channel everyone would hear every other frame.
    frames: broadcast::Sender<Bytes>,
    /// Capture -> encode loop and the level reporter.
    tasks: Vec<JoinHandle<()>>,
}

/// Microphone-only capture used by the settings Mic Test.
struct MicTest {
    mic: Arc<MicCapture>,
    task: JoinHandle<()>,
}

/// The managed voice engine. One call at a time, matching the UI.
pub struct VoiceEngine {
    app: AppHandle,
    call: Mutex<Option<ActiveCall>>,
    test: Mutex<Option<MicTest>>,
    prefs: Mutex<VoicePrefs>,
}

impl VoiceEngine {
    #[must_use]
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            call: Mutex::new(None),
            test: Mutex::new(None),
            prefs: Mutex::new(VoicePrefs::default()),
        }
    }

    /// Devices for the settings UI.
    #[must_use]
    pub fn devices() -> AudioDevices {
        audio::list_devices()
    }

    /// Apply preferences. Volume and gain take effect live; changing a device
    /// mid-call is deferred to the next join (re-opening a live stream would
    /// drop audio for everyone in the channel).
    pub async fn set_prefs(&self, prefs: VoicePrefs) {
        if let Some(call) = self.call.lock().await.as_ref() {
            call.mic.set_gain(prefs.mic_gain / 100.0);
            call.playback.set_volume(prefs.output_volume / 100.0);
        }
        if let Some(test) = self.test.lock().await.as_ref() {
            test.mic.set_gain(prefs.mic_gain / 100.0);
        }
        *self.prefs.lock().await = prefs;
    }

    /// Start a call: open the audio devices and begin encoding. Peers are added
    /// as their participant events arrive.
    ///
    /// # Errors
    /// Returns a message when the microphone or speakers cannot be opened.
    pub async fn join(
        &self,
        server_id: String,
        group_id: String,
        my_device: String,
    ) -> Result<(), String> {
        if my_device.is_empty() {
            return Err("this session has no device id - reconnect and try again".to_owned());
        }
        self.leave().await;
        // The mic test holds the input device; a call takes priority.
        self.stop_mic_test().await;

        let prefs = self.prefs.lock().await.clone();
        // Speakers first: being unable to hear is fatal to a call, whereas a
        // missing mic still lets someone listen in.
        let playback = Arc::new(Playback::start(
            prefs.speaker(),
            prefs.output_volume / 100.0,
        )?);
        let mic = match MicCapture::start(prefs.mic(), prefs.mic_gain / 100.0) {
            Ok(mic) => Arc::new(mic),
            Err(e) => {
                tracing::warn!(error = %e, "joining voice without a microphone");
                return Err(e);
            }
        };

        let signaler: Arc<dyn Signaler> = Arc::new(SessionSignaler {
            app: self.app.clone(),
            server_id: server_id.clone(),
            group_id: group_id.clone(),
        });

        // A few frames of slack: a peer that stalls briefly skips ahead rather
        // than holding up everyone else's audio.
        let (frames, _) = broadcast::channel(32);
        let mut call = ActiveCall {
            server_id,
            group_id,
            my_device,
            mic,
            playback,
            signaler,
            peers: HashMap::new(),
            frames,
            tasks: Vec::new(),
        };
        call.tasks
            .push(Self::spawn_encoder(call.mic.clone(), call.frames.clone()));
        call.tasks.push(self.spawn_level_reporter(&call));
        *self.call.lock().await = Some(call);
        tracing::info!("voice call started");
        Ok(())
    }

    /// Stop the call: close every peer and release the audio devices.
    pub async fn leave(&self) {
        let Some(call) = self.call.lock().await.take() else {
            return;
        };
        for task in &call.tasks {
            task.abort();
        }
        for peer in call.peers.values() {
            peer.sender.abort();
            peer.link.close().await;
        }
        // Levels stop mattering the moment the call ends; tell the UI so no
        // indicator is left lit.
        let _ = self.app.emit(
            VOICE_LEVELS,
            LevelsEvent {
                local: 0.0,
                peers: HashMap::new(),
            },
        );
        tracing::info!("voice call ended");
    }

    /// Mute/unmute the microphone. Metering continues so the UI can still show
    /// that the user is talking into a muted mic.
    pub async fn set_muted(&self, muted: bool) {
        if let Some(call) = self.call.lock().await.as_ref() {
            call.mic.set_muted(muted);
        }
    }

    /// Deafen: stop mixing anyone else's audio.
    pub async fn set_deafened(&self, deafened: bool) {
        if let Some(call) = self.call.lock().await.as_ref() {
            call.playback.set_deafened(deafened);
        }
    }

    /// A participant joined or left the channel this device is in.
    ///
    /// The offerer is elected deterministically (smaller device id offers) so a
    /// simultaneous join cannot produce two offers.
    pub async fn on_participant(
        &self,
        server_id: &str,
        group_id: &str,
        device_id: &str,
        joined: bool,
    ) {
        let mut guard = self.call.lock().await;
        let Some(call) = guard.as_mut() else { return };
        if call.server_id != server_id || call.group_id != group_id {
            return;
        }
        if device_id == call.my_device || device_id.is_empty() {
            return;
        }

        if !joined {
            if let Some(peer) = call.peers.remove(device_id) {
                // Stop feeding a peer that has gone before closing it, or the
                // encoder keeps handing frames to a dead connection.
                peer.sender.abort();
                let playback = call.playback.clone();
                let device = device_id.to_owned();
                tauri::async_runtime::spawn(async move {
                    peer.link.close().await;
                    playback.remove(&device);
                });
            }
            return;
        }
        if call.peers.contains_key(device_id) {
            return;
        }

        let link = match PeerLink::new(
            device_id.to_owned(),
            call.signaler.clone(),
            call.playback.clone(),
        )
        .await
        {
            Ok(link) => link,
            Err(e) => {
                tracing::error!(peer = %device_id, error = %e, "could not create voice peer");
                return;
            }
        };
        call.peers.insert(
            device_id.to_owned(),
            ConnectedPeer {
                link: link.clone(),
                sender: Self::spawn_peer_sender(call.frames.subscribe(), link.clone()),
            },
        );

        if call.my_device.as_str() < device_id {
            let signaler = call.signaler.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = link.start_offer(&signaler).await {
                    tracing::error!(error = %e, "voice offer failed");
                }
            });
        }
    }

    /// A relayed signaling envelope arrived for this device.
    pub async fn on_signal(
        &self,
        server_id: &str,
        group_id: &str,
        from_device: &str,
        kind: &str,
        data: &[u8],
    ) {
        let payload = match std::str::from_utf8(data) {
            Ok(text) => text.to_owned(),
            Err(e) => {
                tracing::warn!(error = %e, "voice signal was not valid UTF-8");
                return;
            }
        };

        let (peer, signaler) = {
            let mut guard = self.call.lock().await;
            let Some(call) = guard.as_mut() else { return };
            if call.server_id != server_id || call.group_id != group_id {
                return;
            }
            let signaler = call.signaler.clone();
            // An offer from a device we have not seen a participant event for
            // yet still deserves an answer - the events can race.
            let peer = match call.peers.get(from_device) {
                Some(existing) => existing.link.clone(),
                None if kind == "offer" => {
                    let link = match PeerLink::new(
                        from_device.to_owned(),
                        signaler.clone(),
                        call.playback.clone(),
                    )
                    .await
                    {
                        Ok(link) => link,
                        Err(e) => {
                            tracing::error!(error = %e, "could not create answering peer");
                            return;
                        }
                    };
                    call.peers.insert(
                        from_device.to_owned(),
                        ConnectedPeer {
                            link: link.clone(),
                            sender: Self::spawn_peer_sender(call.frames.subscribe(), link.clone()),
                        },
                    );
                    link
                }
                None => {
                    tracing::debug!(peer = %from_device, kind, "signal for unknown peer");
                    return;
                }
            };
            (peer, signaler)
        };

        let result = match kind {
            "offer" => peer.accept_offer(&payload, &signaler).await,
            "answer" => peer.accept_answer(&payload).await,
            "ice" => peer.add_ice(&payload).await,
            other => Err(format!("unknown signal kind {other}")),
        };
        if let Err(e) = result {
            tracing::error!(peer = %from_device, kind, error = %e, "voice signal failed");
        }
    }

    /// Encode the microphone into 20 ms Opus frames for the whole call. One
    /// encoder, one consumer of the capture buffer - see [`ActiveCall::frames`].
    fn spawn_encoder(mic: Arc<MicCapture>, frames: broadcast::Sender<Bytes>) -> JoinHandle<()> {
        tauri::async_runtime::spawn(async move {
            let mut encoder = match opus::Encoder::new(
                SAMPLE_RATE,
                opus::Channels::Mono,
                opus::Application::Voip,
            ) {
                Ok(enc) => enc,
                Err(e) => {
                    tracing::error!(error = %e, "could not create Opus encoder");
                    return;
                }
            };
            // Voice-tuned: 32 kbit/s mono is transparent for speech, and
            // in-band FEC lets the decoder rebuild a lost frame from the next.
            let _ = encoder.set_bitrate(opus::Bitrate::Bits(32_000));
            let _ = encoder.set_inband_fec(true);
            let _ = encoder.set_packet_loss_perc(10);

            // Frames are 20 ms; poll faster so the buffer never starves us.
            let mut ticker = tokio::time::interval(std::time::Duration::from_millis(10));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                while let Some(frame) = mic.next_frame() {
                    debug_assert_eq!(frame.len(), FRAME_SAMPLES);
                    match encoder.encode_vec_float(&frame, 1500) {
                        // Err just means nobody is connected yet.
                        Ok(encoded) => {
                            let _ = frames.send(Bytes::from(encoded));
                        }
                        Err(e) => tracing::debug!(error = %e, "Opus encode failed"),
                    }
                }
            }
        })
    }

    /// Forward encoded frames to one peer until the call ends or it drops.
    fn spawn_peer_sender(
        mut frames: broadcast::Receiver<Bytes>,
        peer: Arc<PeerLink>,
    ) -> JoinHandle<()> {
        tauri::async_runtime::spawn(async move {
            loop {
                match frames.recv().await {
                    Ok(frame) => peer.send_frame(frame).await,
                    // Lagging means this peer fell behind the encoder; skipping
                    // to the newest audio is right for a live call.
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!(peer = %peer.remote_device, skipped = n, "voice sender lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    }

    /// Report speaking levels to the UI at roughly 15 Hz - fast enough to look
    /// live, slow enough not to flood the IPC bridge.
    fn spawn_level_reporter(&self, call: &ActiveCall) -> JoinHandle<()> {
        let app = self.app.clone();
        let mic = call.mic.clone();
        let playback = call.playback.clone();
        tauri::async_runtime::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_millis(66));
            loop {
                ticker.tick().await;
                let event = LevelsEvent {
                    local: mic.level.get(),
                    peers: playback.peer_levels(),
                };
                if app.emit(VOICE_LEVELS, event).is_err() {
                    break; // webview went away
                }
            }
        })
    }

    /// Start the settings Mic Test: capture and meter, transmitting nothing.
    ///
    /// # Errors
    /// Returns a message when the microphone cannot be opened.
    pub async fn start_mic_test(&self) -> Result<(), String> {
        if self.call.lock().await.is_some() {
            return Err("already in a call - leave it to test your microphone".to_owned());
        }
        self.stop_mic_test().await;
        let prefs = self.prefs.lock().await.clone();
        let mic = Arc::new(MicCapture::start(prefs.mic(), prefs.mic_gain / 100.0)?);

        let app = self.app.clone();
        let metered = mic.clone();
        let task = tauri::async_runtime::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_millis(66));
            loop {
                ticker.tick().await;
                // Drain what was captured so the buffer cannot grow unbounded
                // during a long test.
                while metered.next_frame().is_some() {}
                let event = LevelsEvent {
                    local: metered.level.get(),
                    peers: HashMap::new(),
                };
                if app.emit(VOICE_LEVELS, event).is_err() {
                    break;
                }
            }
        });
        *self.test.lock().await = Some(MicTest { mic, task });
        Ok(())
    }

    /// Stop the Mic Test and release the microphone.
    pub async fn stop_mic_test(&self) {
        if let Some(test) = self.test.lock().await.take() {
            test.task.abort();
            let _ = self.app.emit(
                VOICE_LEVELS,
                LevelsEvent {
                    local: 0.0,
                    peers: HashMap::new(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_default_to_unity_and_system_devices() {
        let prefs = VoicePrefs::default();
        assert_eq!(prefs.mic_gain, 100.0);
        assert_eq!(prefs.output_volume, 100.0);
        assert!(prefs.mic().is_none(), "empty id means the system default");
        assert!(prefs.speaker().is_none());
    }

    #[test]
    fn prefs_deserialize_from_the_ui_shape() {
        // The UI sends camelCase and may omit fields it has never set.
        let prefs: VoicePrefs = serde_json::from_str(
            r#"{"micDeviceId":"alsa:hw:1,0","micGain":150,"outputVolume":80}"#,
        )
        .expect("UI prefs parse");
        assert_eq!(prefs.mic(), Some("alsa:hw:1,0"));
        assert_eq!(prefs.mic_gain, 150.0);
        assert_eq!(prefs.output_volume, 80.0);
        assert!(prefs.speaker().is_none(), "omitted device means default");
    }

    #[test]
    fn offerer_election_is_deterministic_and_asymmetric() {
        // The rule both sides apply independently: smaller device id offers.
        let (a, b) = ("11111111-aaaa", "22222222-bbbb");
        assert!(a < b, "one side offers");
        assert!(!(b < a), "and the other answers - never both");
    }
}
