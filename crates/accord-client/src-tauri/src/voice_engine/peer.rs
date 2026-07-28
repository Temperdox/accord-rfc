//! One WebRTC peer connection to one remote device.
//!
//! The server relays our SDP/ICE envelopes verbatim ([`crate::commands::voice`])
//! and never sees the media: audio is DTLS-SRTP peer-to-peer, which is what
//! keeps the ARCHITECTURE section 5 boundary intact now that the media stack is
//! native rather than in the webview.
//!
//! Glare is avoided the same way the old webview mesh did it: of any two
//! devices, the lexicographically smaller device id offers and the other
//! answers, so a simultaneous join never produces two offers.

use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::Mutex;
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MIME_TYPE_OPUS, MediaEngine};
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_remote::TrackRemote;

use super::audio::{FRAME_SAMPLES, Playback, SAMPLE_RATE};

/// A relayed signaling envelope this peer wants delivered to its remote device.
/// The engine owns the transport; the peer only says what to send.
pub trait Signaler: Send + Sync + 'static {
    /// Send `payload` (JSON) of kind "offer" | "answer" | "ice" to `device`.
    fn send(&self, device: String, kind: &'static str, payload: String);
}

/// STUN helps across NAT on the open internet. LAN and Yggdrasil-mesh peers
/// connect on host candidates, so a call still works with no reachable STUN.
/// (No TURN: a relay would have to be run somewhere, and self-hosting is the
/// point. Symmetric-NAT pairs need the mesh.)
fn ice_servers() -> Vec<RTCIceServer> {
    vec![RTCIceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_owned()],
        ..Default::default()
    }]
}

/// A live connection to one remote device.
pub struct PeerLink {
    pub remote_device: String,
    pc: Arc<RTCPeerConnection>,
    track: Arc<TrackLocalStaticSample>,
    /// Candidates that arrived before the remote description was applied.
    /// `add_ice_candidate` rejects those, and an out-of-band relay delivers
    /// them out of order routinely, so they wait here instead of being lost.
    pending_ice: Mutex<Vec<RTCIceCandidateInit>>,
    has_remote: Mutex<bool>,
}

impl PeerLink {
    /// Build a peer connection to `remote_device` and start pumping its inbound
    /// audio into `playback`.
    ///
    /// # Errors
    /// Returns a message if the WebRTC stack cannot be initialised.
    pub async fn new(
        remote_device: String,
        signaler: Arc<dyn Signaler>,
        playback: Arc<Playback>,
    ) -> Result<Arc<Self>, String> {
        let mut media_engine = MediaEngine::default();
        media_engine
            .register_default_codecs()
            .map_err(|e| format!("codec registration failed: {e}"))?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)
            .map_err(|e| format!("interceptor registration failed: {e}"))?;
        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();

        let pc = Arc::new(
            api.new_peer_connection(RTCConfiguration {
                ice_servers: ice_servers(),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("could not create peer connection: {e}"))?,
        );

        // Our outgoing audio. Opus at 48 kHz is what `register_default_codecs`
        // negotiates, so the encoder settings and this capability must agree.
        let track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: SAMPLE_RATE,
                channels: 1,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: vec![],
            },
            "audio".to_owned(),
            "accord".to_owned(),
        ));
        pc.add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|e| format!("could not add audio track: {e}"))?;

        let link = Arc::new(Self {
            remote_device: remote_device.clone(),
            pc: pc.clone(),
            track,
            pending_ice: Mutex::new(Vec::new()),
            has_remote: Mutex::new(false),
        });

        // Trickle our candidates out as they are discovered. `None` means
        // gathering finished and needs no relay.
        {
            let signaler = signaler.clone();
            let target = remote_device.clone();
            pc.on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
                let signaler = signaler.clone();
                let target = target.clone();
                Box::pin(async move {
                    let Some(candidate) = candidate else { return };
                    match candidate.to_json() {
                        Ok(init) => match serde_json::to_string(&init) {
                            Ok(json) => signaler.send(target, "ice", json),
                            Err(e) => tracing::warn!(error = %e, "could not encode ICE candidate"),
                        },
                        Err(e) => tracing::warn!(error = %e, "could not serialise ICE candidate"),
                    }
                })
            }));
        }

        {
            let device = remote_device.clone();
            pc.on_peer_connection_state_change(Box::new(move |state: RTCPeerConnectionState| {
                let device = device.clone();
                Box::pin(async move {
                    match state {
                        RTCPeerConnectionState::Connected => {
                            tracing::info!(peer = %device, "voice peer connected");
                        }
                        RTCPeerConnectionState::Failed
                        | RTCPeerConnectionState::Disconnected
                        | RTCPeerConnectionState::Closed => {
                            tracing::warn!(peer = %device, ?state, "voice peer lost");
                        }
                        _ => {}
                    }
                })
            }));
        }

        // Inbound audio: decode each Opus payload and hand it to the mixer.
        {
            let device = remote_device.clone();
            let playback = playback.clone();
            pc.on_track(Box::new(
                move |track: Arc<TrackRemote>, _receiver, _transceiver| {
                    let device = device.clone();
                    let playback = playback.clone();
                    Box::pin(async move {
                        tokio::spawn(async move {
                            pump_remote_audio(track, device, playback).await;
                        });
                    })
                },
            ));
        }

        Ok(link)
    }

    /// Create an offer and hand it to the relay. Only the elected offerer calls
    /// this; see the glare rule in the module docs.
    ///
    /// # Errors
    /// Returns a message if SDP negotiation fails.
    pub async fn start_offer(&self, signaler: &Arc<dyn Signaler>) -> Result<(), String> {
        let offer = self
            .pc
            .create_offer(None)
            .await
            .map_err(|e| format!("could not create offer: {e}"))?;
        self.pc
            .set_local_description(offer)
            .await
            .map_err(|e| format!("could not set local offer: {e}"))?;
        let local = self
            .pc
            .local_description()
            .await
            .ok_or("local description missing after set")?;
        let json =
            serde_json::to_string(&local).map_err(|e| format!("could not encode offer: {e}"))?;
        signaler.send(self.remote_device.clone(), "offer", json);
        Ok(())
    }

    /// Apply a remote offer and answer it through the relay.
    ///
    /// # Errors
    /// Returns a message if the offer is malformed or answering fails.
    pub async fn accept_offer(
        &self,
        sdp_json: &str,
        signaler: &Arc<dyn Signaler>,
    ) -> Result<(), String> {
        let offer: RTCSessionDescription =
            serde_json::from_str(sdp_json).map_err(|e| format!("malformed offer: {e}"))?;
        self.pc
            .set_remote_description(offer)
            .await
            .map_err(|e| format!("could not apply offer: {e}"))?;
        self.flush_pending_ice().await;

        let answer = self
            .pc
            .create_answer(None)
            .await
            .map_err(|e| format!("could not create answer: {e}"))?;
        self.pc
            .set_local_description(answer)
            .await
            .map_err(|e| format!("could not set local answer: {e}"))?;
        let local = self
            .pc
            .local_description()
            .await
            .ok_or("local description missing after set")?;
        let json =
            serde_json::to_string(&local).map_err(|e| format!("could not encode answer: {e}"))?;
        signaler.send(self.remote_device.clone(), "answer", json);
        Ok(())
    }

    /// Apply the answer to an offer we sent.
    ///
    /// # Errors
    /// Returns a message if the answer is malformed or cannot be applied.
    pub async fn accept_answer(&self, sdp_json: &str) -> Result<(), String> {
        let answer: RTCSessionDescription =
            serde_json::from_str(sdp_json).map_err(|e| format!("malformed answer: {e}"))?;
        self.pc
            .set_remote_description(answer)
            .await
            .map_err(|e| format!("could not apply answer: {e}"))?;
        self.flush_pending_ice().await;
        Ok(())
    }

    /// Add a relayed ICE candidate, buffering it when the remote description
    /// has not arrived yet.
    ///
    /// # Errors
    /// Returns a message if the candidate is malformed or rejected.
    pub async fn add_ice(&self, candidate_json: &str) -> Result<(), String> {
        let init: RTCIceCandidateInit = serde_json::from_str(candidate_json)
            .map_err(|e| format!("malformed ICE candidate: {e}"))?;
        if !*self.has_remote.lock().await {
            self.pending_ice.lock().await.push(init);
            return Ok(());
        }
        self.pc
            .add_ice_candidate(init)
            .await
            .map_err(|e| format!("could not add ICE candidate: {e}"))
    }

    /// Apply everything that arrived before the remote description did.
    async fn flush_pending_ice(&self) {
        *self.has_remote.lock().await = true;
        let queued: Vec<_> = self.pending_ice.lock().await.drain(..).collect();
        for init in queued {
            if let Err(e) = self.pc.add_ice_candidate(init).await {
                tracing::warn!(error = %e, "queued ICE candidate rejected");
            }
        }
    }

    /// Send one encoded 20 ms Opus frame. Before negotiation completes this is
    /// a silent no-op inside webrtc-rs, which is fine - the first frames of a
    /// call are simply dropped rather than erroring.
    pub async fn send_frame(&self, frame: Bytes) {
        let sample = Sample {
            data: frame,
            // Drives the RTP timestamp advance; zero here would stall the
            // receiver's jitter buffer with no error anywhere.
            duration: std::time::Duration::from_millis(20),
            ..Default::default()
        };
        if let Err(e) = self.track.write_sample(&sample).await {
            tracing::debug!(peer = %self.remote_device, error = %e, "dropped voice frame");
        }
    }

    /// Close the connection. `close` is async, so this cannot live in `Drop`.
    pub async fn close(&self) {
        if let Err(e) = self.pc.close().await {
            tracing::debug!(peer = %self.remote_device, error = %e, "peer close failed");
        }
    }
}

/// Read RTP from a remote track, decode Opus, and feed the playback mixer.
/// For Opus the RTP payload is the codec frame verbatim (the depacketizer is a
/// pass-through), so no reassembly is needed.
async fn pump_remote_audio(track: Arc<TrackRemote>, device: String, playback: Arc<Playback>) {
    let mut decoder = match opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "could not create Opus decoder");
            return;
        }
    };
    // Sized to exactly one frame: PLC infers the concealed duration from the
    // buffer length, so an oversized buffer would invent extra audio.
    let mut pcm = vec![0.0f32; FRAME_SAMPLES];
    tracing::info!(peer = %device, "receiving voice");

    // read_rtp is not cancel-safe, so it is awaited directly rather than inside
    // a select!; the loop ends when the track closes.
    while let Ok((packet, _)) = track.read_rtp().await {
        if packet.payload.is_empty() {
            continue;
        }
        match decoder.decode_float(&packet.payload, &mut pcm, false) {
            Ok(decoded) => playback.push(&device, &pcm[..decoded]),
            Err(e) => tracing::debug!(peer = %device, error = %e, "Opus decode failed"),
        }
    }
    playback.remove(&device);
    tracing::info!(peer = %device, "remote voice track ended");
}
