//! Native audio I/O for voice calls: microphone capture and speaker playback.
//!
//! Why this is native rather than `getUserMedia` in the webview: the webview
//! cannot carry the media at all on Linux (WebKitGTK ships without
//! `RTCPeerConnection`), so the whole pipeline lives in Rust and every platform
//! takes the same path.
//!
//! Shape of the pipeline:
//! ```text
//!   mic --cpal--> [downmix to mono, resample to 48k] --> 20 ms frames --> Opus
//!                                                                          |
//!                                                        (voice_engine sends)
//!   speakers <--cpal-- [mix all peers] <-- per-peer buffer <-- Opus decode
//! ```
//! cpal's stream callbacks run on a dedicated audio thread and must never
//! block, so they only touch small buffers guarded by their own mutexes -
//! nothing the async side holds across an await. Resampling is linear: voice at
//! 48 kHz mono is forgiving, and it avoids pulling in a resampler.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, DeviceId, SampleFormat, Stream, StreamConfig, SupportedStreamConfig};

/// Opus (and WebRTC) operate at 48 kHz for voice.
pub const SAMPLE_RATE: u32 = 48_000;
/// 20 ms frames: the WebRTC default packetization for Opus.
pub const FRAME_SAMPLES: usize = (SAMPLE_RATE as usize / 1000) * 20;
/// Cap any buffer at one second so a stalled consumer cannot grow it forever.
const MAX_BUFFER: usize = SAMPLE_RATE as usize;

/// A microphone/speaker device as offered to the settings UI.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    /// Stable identifier to persist in prefs (cpal's `host:device` form).
    pub id: String,
    /// Human-readable label.
    pub name: String,
    pub is_default: bool,
}

/// The device lists for the settings UI.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevices {
    pub inputs: Vec<AudioDevice>,
    pub outputs: Vec<AudioDevice>,
}

fn device_entry(device: &Device, default_id: Option<&str>) -> Option<AudioDevice> {
    let id = device.id().ok()?.to_string();
    let name = device
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| id.clone());
    Some(AudioDevice {
        is_default: default_id == Some(id.as_str()),
        id,
        name,
    })
}

/// Enumerate input/output devices. Never fails: a host with no devices (or a
/// backend that errors) yields empty lists so the UI can say so plainly.
#[must_use]
pub fn list_devices() -> AudioDevices {
    let host = cpal::default_host();
    let default_in = host
        .default_input_device()
        .and_then(|d| d.id().ok())
        .map(|id| id.to_string());
    let default_out = host
        .default_output_device()
        .and_then(|d| d.id().ok())
        .map(|id| id.to_string());

    let mut devices = AudioDevices::default();
    let Ok(all) = host.devices() else {
        return devices;
    };
    for device in all {
        // Enumeration opens the PCM on ALSA and can fail per device (busy,
        // weird virtual PCMs); skip those rather than losing the whole list.
        if device.supports_input()
            && let Some(entry) = device_entry(&device, default_in.as_deref())
        {
            devices.inputs.push(entry);
        }
        if device.supports_output()
            && let Some(entry) = device_entry(&device, default_out.as_deref())
        {
            devices.outputs.push(entry);
        }
    }
    devices
}

/// Resolve a saved device id, falling back to the host default when it is gone
/// (unplugged headset, renamed sink). A missing device must degrade to "use the
/// default", never to "no audio".
fn pick_device(saved: Option<&str>, input: bool) -> Option<Device> {
    let host = cpal::default_host();
    if let Some(want) = saved.filter(|s| !s.is_empty()) {
        if let Ok(id) = DeviceId::from_str(want)
            && let Some(device) = host.device_by_id(&id)
        {
            return Some(device);
        }
        tracing::warn!(device = want, "saved audio device not found; using default");
    }
    if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    }
}

/// Choose a stream config, preferring 48 kHz (no resampling) and f32 samples.
fn pick_config(device: &Device, input: bool) -> Result<SupportedStreamConfig, String> {
    let ranges: Vec<_> = if input {
        device
            .supported_input_configs()
            .map_err(|e| format!("no input configs: {e}"))?
            .collect()
    } else {
        device
            .supported_output_configs()
            .map_err(|e| format!("no output configs: {e}"))?
            .collect()
    };

    // Prefer 48 kHz in f32, then 48 kHz in i16, then whatever the device calls
    // its default (we resample in that case).
    for format in [SampleFormat::F32, SampleFormat::I16] {
        if let Some(config) = ranges
            .iter()
            .filter(|r| r.sample_format() == format)
            .find(|r| r.contains_rate(SAMPLE_RATE))
            .and_then(|r| r.try_with_sample_rate(SAMPLE_RATE))
        {
            return Ok(config);
        }
    }
    if input {
        device.default_input_config()
    } else {
        device.default_output_config()
    }
    .map_err(|e| format!("no usable audio config: {e}"))
}

/// Linear resample of mono audio between `from` and `to` rates. Voice-grade and
/// dependency-free; a device already at 48 kHz skips it entirely.
fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = f64::from(to) / f64::from(from);
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = (i as f64) / ratio;
        let idx = src.floor() as usize;
        let frac = (src - src.floor()) as f32;
        let a = input.get(idx).copied().unwrap_or(0.0);
        let b = input.get(idx + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}

/// Shared level readout (0..=1 scaled by 1000) so the UI can draw speaking bars
/// without touching anything the audio callbacks lock.
#[derive(Debug, Default)]
pub struct Level(AtomicU32);

impl Level {
    pub fn set(&self, v: f32) {
        self.0
            .store((v.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
    }
    #[must_use]
    pub fn get(&self) -> f32 {
        self.0.load(Ordering::Relaxed) as f32 / 1000.0
    }
}

/// RMS of a mono frame, gained so ordinary speech fills the bar (same scaling
/// the previous webview meter used, so the UI looks unchanged).
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    ((sum / samples.len() as f32).sqrt() * 2.8).min(1.0)
}

/// Microphone capture: owns the cpal input stream and hands out 20 ms mono
/// 48 kHz frames.
pub struct MicCapture {
    /// Held for the call's lifetime; dropping it stops capture.
    _stream: Stream,
    /// Captured mono 48 kHz samples awaiting framing.
    pending: Arc<Mutex<Vec<f32>>>,
    pub level: Arc<Level>,
    pub muted: Arc<AtomicBool>,
    gain: Arc<Mutex<f32>>,
}

impl MicCapture {
    /// Open the microphone. An empty/unknown `device_id` means the OS default.
    ///
    /// # Errors
    /// Returns a message when no input device exists or the stream cannot start.
    pub fn start(device_id: Option<&str>, gain: f32) -> Result<Self, String> {
        let device = pick_device(device_id, true).ok_or("no microphone available")?;
        let supported = pick_config(&device, true)?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.config();
        let channels = config.channels as usize;
        let rate = config.sample_rate;

        let pending: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let level = Arc::new(Level::default());
        let muted = Arc::new(AtomicBool::new(false));
        let gain = Arc::new(Mutex::new(gain));
        let err_fn = |e: cpal::Error| tracing::warn!(error = %e, "microphone stream error");

        // Downmix to mono, apply gain, meter, resample, park for the encoder.
        // Muted still meters (so the UI can show "you're talking while muted")
        // but stops feeding the encoder, which is what actually silences us.
        let ingest = {
            let (buf, lvl, mute_flag, gain_ref) =
                (pending.clone(), level.clone(), muted.clone(), gain.clone());
            move |mono: Vec<f32>| {
                let g = *gain_ref.lock().expect("mic gain poisoned");
                let mono: Vec<f32> = mono.into_iter().map(|s| s * g).collect();
                lvl.set(rms(&mono));
                if mute_flag.load(Ordering::Relaxed) {
                    return;
                }
                let resampled = resample(&mono, rate, SAMPLE_RATE);
                let mut pending = buf.lock().expect("mic buffer poisoned");
                pending.extend_from_slice(&resampled);
                if pending.len() > MAX_BUFFER {
                    let excess = pending.len() - FRAME_SAMPLES * 2;
                    pending.drain(..excess);
                }
            }
        };

        let stream = match format {
            SampleFormat::F32 => device.build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    ingest(
                        data.chunks(channels)
                            .map(|f| f.iter().sum::<f32>() / f.len() as f32)
                            .collect(),
                    );
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    ingest(
                        data.chunks(channels)
                            .map(|f| {
                                f.iter().map(|s| f32::from(*s) / 32768.0).sum::<f32>()
                                    / f.len() as f32
                            })
                            .collect(),
                    );
                },
                err_fn,
                None,
            ),
            other => return Err(format!("unsupported microphone sample format: {other}")),
        }
        .map_err(|e| format!("could not open microphone: {e}"))?;

        // Streams start paused; without this there is silence and no error.
        stream
            .play()
            .map_err(|e| format!("could not start microphone: {e}"))?;
        tracing::info!(rate, channels, %format, "microphone capture started");

        Ok(Self {
            _stream: stream,
            pending,
            level,
            muted,
            gain,
        })
    }

    /// Take one 20 ms frame once enough audio has accumulated.
    pub fn next_frame(&self) -> Option<Vec<f32>> {
        let mut pending = self.pending.lock().expect("mic buffer poisoned");
        if pending.len() < FRAME_SAMPLES {
            return None;
        }
        Some(pending.drain(..FRAME_SAMPLES).collect())
    }

    pub fn set_gain(&self, gain: f32) {
        *self.gain.lock().expect("mic gain poisoned") = gain;
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }
}

/// Speaker playback: owns the cpal output stream and mixes every peer's decoded
/// audio into it.
pub struct Playback {
    _stream: Stream,
    peers: Arc<Mutex<HashMap<String, Vec<f32>>>>,
    levels: Arc<Mutex<HashMap<String, f32>>>,
    pub deafened: Arc<AtomicBool>,
    volume: Arc<Mutex<f32>>,
}

impl Playback {
    /// Open the speakers. An empty/unknown `device_id` means the OS default.
    ///
    /// # Errors
    /// Returns a message when no output device exists or the stream cannot start.
    pub fn start(device_id: Option<&str>, volume: f32) -> Result<Self, String> {
        let device = pick_device(device_id, false).ok_or("no speakers available")?;
        let supported = pick_config(&device, false)?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.config();
        let channels = config.channels as usize;
        let rate = config.sample_rate;

        let peers: Arc<Mutex<HashMap<String, Vec<f32>>>> = Arc::new(Mutex::new(HashMap::new()));
        let levels = Arc::new(Mutex::new(HashMap::new()));
        let deafened = Arc::new(AtomicBool::new(false));
        let volume = Arc::new(Mutex::new(volume));
        let err_fn = |e: cpal::Error| tracing::warn!(error = %e, "speaker stream error");

        // Produce `frames` mono samples at the device rate by mixing every
        // peer's queued audio; deafened means "mix nothing" (still draining, so
        // un-deafening does not replay a backlog).
        let mix = {
            let (bufs, deaf, vol) = (peers.clone(), deafened.clone(), volume.clone());
            move |frames: usize| -> Vec<f32> {
                let needed = if rate == SAMPLE_RATE {
                    frames
                } else {
                    ((frames as f64) * f64::from(SAMPLE_RATE) / f64::from(rate)).ceil() as usize
                };
                let silent = deaf.load(Ordering::Relaxed);
                let mut mixed = vec![0.0f32; needed];
                {
                    let mut bufs = bufs.lock().expect("playback buffers poisoned");
                    for buf in bufs.values_mut() {
                        let take = buf.len().min(needed);
                        for (slot, sample) in mixed.iter_mut().zip(buf.drain(..take)) {
                            if !silent {
                                *slot += sample;
                            }
                        }
                    }
                }
                let g = *vol.lock().expect("playback volume poisoned");
                resample(&mixed, SAMPLE_RATE, rate)
                    .into_iter()
                    // Several peers at once can exceed full scale; clamp rather
                    // than wrap (which would be audible as harsh distortion).
                    .map(|s| (s * g).clamp(-1.0, 1.0))
                    .collect()
            }
        };

        let stream = match format {
            SampleFormat::F32 => device.build_output_stream(
                config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let ready = mix(out.len() / channels);
                    for (i, frame) in out.chunks_mut(channels).enumerate() {
                        frame.fill(ready.get(i).copied().unwrap_or(0.0));
                    }
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => device.build_output_stream(
                config,
                move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let ready = mix(out.len() / channels);
                    for (i, frame) in out.chunks_mut(channels).enumerate() {
                        let v = ready.get(i).copied().unwrap_or(0.0);
                        frame.fill((v * 32767.0) as i16);
                    }
                },
                err_fn,
                None,
            ),
            other => return Err(format!("unsupported speaker sample format: {other}")),
        }
        .map_err(|e| format!("could not open speakers: {e}"))?;

        stream
            .play()
            .map_err(|e| format!("could not start speakers: {e}"))?;
        tracing::info!(rate, channels, %format, "speaker playback started");

        Ok(Self {
            _stream: stream,
            peers,
            levels,
            deafened,
            volume,
        })
    }

    /// Queue a decoded 48 kHz mono frame from `device_id` for playback.
    pub fn push(&self, device_id: &str, samples: &[f32]) {
        self.levels
            .lock()
            .expect("levels poisoned")
            .insert(device_id.to_owned(), rms(samples));
        let mut peers = self.peers.lock().expect("playback buffers poisoned");
        let buf = peers.entry(device_id.to_owned()).or_default();
        buf.extend_from_slice(samples);
        // Drop the oldest audio rather than drift ever further behind live.
        if buf.len() > MAX_BUFFER {
            let excess = buf.len() - FRAME_SAMPLES * 2;
            buf.drain(..excess);
        }
    }

    /// Forget a peer that left, so its level stops showing in the UI.
    pub fn remove(&self, device_id: &str) {
        self.peers
            .lock()
            .expect("playback buffers poisoned")
            .remove(device_id);
        self.levels
            .lock()
            .expect("levels poisoned")
            .remove(device_id);
    }

    pub fn set_volume(&self, volume: f32) {
        *self.volume.lock().expect("playback volume poisoned") = volume;
    }

    pub fn set_deafened(&self, on: bool) {
        self.deafened.store(on, Ordering::Relaxed);
    }

    /// Current per-peer speaking levels for the UI.
    #[must_use]
    pub fn peer_levels(&self) -> HashMap<String, f32> {
        self.levels.lock().expect("levels poisoned").clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_is_identity_at_the_same_rate() {
        let input = vec![0.1, -0.2, 0.3];
        assert_eq!(resample(&input, SAMPLE_RATE, SAMPLE_RATE), input);
    }

    #[test]
    fn resampling_scales_length_by_the_rate_ratio() {
        // 441 samples at 44.1 kHz is 10 ms, which is 480 samples at 48 kHz.
        let up = resample(&vec![0.0; 441], 44_100, SAMPLE_RATE);
        assert_eq!(up.len(), 480);
        let down = resample(&up, SAMPLE_RATE, 44_100);
        assert_eq!(down.len(), 441, "round trip returns to the source rate");
    }

    #[test]
    fn resampling_preserves_a_constant_signal() {
        let flat = vec![0.5f32; 480];
        for s in resample(&flat, SAMPLE_RATE, 44_100) {
            assert!(
                (s - 0.5).abs() < 0.01,
                "interpolation of a constant is flat"
            );
        }
    }

    #[test]
    fn rms_is_zero_for_silence_and_positive_for_signal() {
        assert_eq!(rms(&[0.0; 128]), 0.0);
        assert!(rms(&[0.5; 128]) > 0.0);
        assert_eq!(rms(&[]), 0.0, "no samples means no level");
    }

    #[test]
    fn rms_saturates_rather_than_exceeding_one() {
        assert_eq!(rms(&[1.0; 64]), 1.0, "loud input clamps to a full bar");
    }

    #[test]
    fn level_round_trips_through_the_atomic() {
        let level = Level::default();
        level.set(0.25);
        assert!((level.get() - 0.25).abs() < 0.01);
        level.set(5.0);
        assert_eq!(level.get(), 1.0, "out-of-range input is clamped");
    }

    #[test]
    fn frame_is_twenty_milliseconds() {
        assert_eq!(FRAME_SAMPLES, 960, "Opus/WebRTC standard frame at 48 kHz");
    }
}
