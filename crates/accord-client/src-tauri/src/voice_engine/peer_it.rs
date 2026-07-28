//! In-process integration test for the native voice transport.
//!
//! Two [`PeerLink`]s are wired to each other through a fake relay that mimics
//! what the server does (deliver offer/answer/ICE envelopes to the other side),
//! then real Opus frames are pushed through. This exercises the parts that have
//! no hardware dependency - negotiation, ICE, DTLS-SRTP, RTP, and the decode
//! path into the playback mixer - so a regression in the call setup shows up in
//! CI rather than on a phone call with a friend.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;

use super::audio::{FRAME_SAMPLES, Playback, SAMPLE_RATE};
use super::peer::{PeerLink, Signaler};

/// One relayed envelope: (kind, payload).
type Envelope = (&'static str, String);

/// Stands in for the server relay: whatever a peer sends is handed to the other
/// peer's queue, exactly as `MessagingService` would.
struct FakeRelay {
    tx: mpsc::UnboundedSender<Envelope>,
    sent: Arc<AtomicUsize>,
}

impl Signaler for FakeRelay {
    fn send(&self, _device: String, kind: &'static str, payload: String) {
        self.sent.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send((kind, payload));
    }
}

/// Drive one side's inbox until the channel closes.
async fn pump(
    mut rx: mpsc::UnboundedReceiver<Envelope>,
    peer: Arc<PeerLink>,
    signaler: Arc<dyn Signaler>,
) {
    while let Some((kind, payload)) = rx.recv().await {
        let result = match kind {
            "offer" => peer.accept_offer(&payload, &signaler).await,
            "answer" => peer.accept_answer(&payload).await,
            "ice" => peer.add_ice(&payload).await,
            other => Err(format!("unexpected kind {other}")),
        };
        if let Err(e) = result {
            eprintln!("relay apply failed ({kind}): {e}");
        }
    }
}

/// Encode `count` frames of a 440 Hz tone - real Opus payloads, so the decode
/// side is exercised for real rather than with dummy bytes.
fn tone_frames(count: usize) -> Vec<Bytes> {
    let mut encoder =
        opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)
            .expect("opus encoder");
    let mut phase = 0.0f32;
    let step = std::f32::consts::TAU * 440.0 / SAMPLE_RATE as f32;
    (0..count)
        .map(|_| {
            let pcm: Vec<f32> = (0..FRAME_SAMPLES)
                .map(|_| {
                    phase += step;
                    (phase.sin()) * 0.5
                })
                .collect();
            Bytes::from(encoder.encode_vec_float(&pcm, 1500).expect("encode"))
        })
        .collect()
}

/// Two peers negotiate through the relay and audio flows from one to the other.
///
/// Ignored by default: it opens real UDP sockets and needs a working local
/// network stack, and the `Playback` sink wants an audio device. Run with
/// `cargo test -p accord-client voice_call_round_trip -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opens UDP sockets and an audio device; run explicitly"]
async fn voice_call_round_trip() {
    // Each sink receives what ITS side hears, keyed by the far device's id.
    let sink_a = Arc::new(Playback::start(None, 1.0).expect("playback for A"));
    let sink_b = Arc::new(Playback::start(None, 1.0).expect("playback for B"));

    let (tx_to_a, rx_a) = mpsc::unbounded_channel::<Envelope>();
    let (tx_to_b, rx_b) = mpsc::unbounded_channel::<Envelope>();
    let sent_a = Arc::new(AtomicUsize::new(0));
    let sent_b = Arc::new(AtomicUsize::new(0));

    // A's signaler delivers into B's inbox and vice versa.
    let sig_a: Arc<dyn Signaler> = Arc::new(FakeRelay {
        tx: tx_to_b,
        sent: sent_a.clone(),
    });
    let sig_b: Arc<dyn Signaler> = Arc::new(FakeRelay {
        tx: tx_to_a,
        sent: sent_b.clone(),
    });

    // A talks to device-b; anything A hears lands in sink_a under "device-b".
    let peer_a = PeerLink::new("device-b".to_owned(), sig_a.clone(), sink_a.clone())
        .await
        .expect("peer A");
    let peer_b = PeerLink::new("device-a".to_owned(), sig_b.clone(), sink_b.clone())
        .await
        .expect("peer B");

    tokio::spawn(pump(rx_a, peer_a.clone(), sig_a.clone()));
    tokio::spawn(pump(rx_b, peer_b.clone(), sig_b.clone()));

    // "device-a" < "device-b", so A is the elected offerer.
    peer_a.start_offer(&sig_a).await.expect("offer");

    // Give ICE/DTLS a moment, then push a second of audio from A.
    tokio::time::sleep(Duration::from_secs(2)).await;
    for frame in tone_frames(50) {
        peer_a.send_frame(frame).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert!(
        sent_a.load(Ordering::Relaxed) > 0,
        "the offerer must have signalled"
    );
    assert!(
        sent_b.load(Ordering::Relaxed) > 0,
        "the answerer must have replied"
    );
    // A sent, so B's sink is the one that should have heard "device-a".
    let heard = sink_b.peer_levels();
    println!("B heard: {heard:?}");
    assert!(
        heard.contains_key("device-a"),
        "A's audio should have reached B's mixer; got {heard:?}"
    );

    peer_a.close().await;
    peer_b.close().await;
}

/// The negotiation halves are ordered correctly even without media: an answer
/// cannot be produced before the offer is applied. Runs anywhere (no sockets
/// carry media, though ICE gathering still binds locally).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "binds local UDP sockets for ICE gathering; run explicitly"]
async fn answer_requires_an_offer_first() {
    let playback = Arc::new(Playback::start(None, 1.0).expect("playback"));
    let collected: Arc<AsyncMutex<Vec<Envelope>>> = Arc::new(AsyncMutex::new(Vec::new()));

    struct Collector(Arc<AsyncMutex<Vec<Envelope>>>);
    impl Signaler for Collector {
        fn send(&self, _device: String, kind: &'static str, payload: String) {
            let store = self.0.clone();
            tokio::spawn(async move { store.lock().await.push((kind, payload)) });
        }
    }

    let signaler: Arc<dyn Signaler> = Arc::new(Collector(collected.clone()));
    let peer = PeerLink::new("device-z".to_owned(), signaler.clone(), playback)
        .await
        .expect("peer");

    // An answer with no preceding offer must be refused, not silently accepted.
    let bogus = r#"{"type":"answer","sdp":"v=0\r\n"}"#;
    assert!(
        peer.accept_answer(bogus).await.is_err(),
        "an answer without a local offer is invalid"
    );
    peer.close().await;
}
