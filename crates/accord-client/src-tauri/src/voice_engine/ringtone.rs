//! The incoming-call sound: a set of ringtones compiled into the binary, played
//! on the user's chosen output device.
//!
//! Ringtones come from `assets/ringtones/` and are embedded by `build.rs`
//! (see the README there), so adding one is a matter of dropping in a file -
//! and there is no runtime path to resolve, which is what makes it behave the
//! same from `cargo run` as from an installed app.
//!
//! Decoding is symphonia (pure Rust) and playback is cpal, deliberately not the
//! webview: the webview cannot route audio to the speaker chosen in settings,
//! and its media stack is exactly what proved unreliable here (WebKitGTK on
//! Linux has no WebRTC at all).

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

use super::audio::{SAMPLE_RATE, pick_config, pick_device, resample};

/// One ringtone compiled in from `assets/ringtones/`.
pub struct BundledRingtone {
    pub id: &'static str,
    pub name: &'static str,
    pub bytes: &'static [u8],
}

// `BUNDLED: &[BundledRingtone]`, generated from the assets folder.
include!(concat!(env!("OUT_DIR"), "/ringtones.rs"));

/// Id of the always-present synthesised ringtone. Also what an unset (or
/// stale) preference falls back to, so there is always something to play.
pub const DEFAULT_ID: &str = "classic";

/// A ringtone as offered in settings.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RingtoneInfo {
    pub id: String,
    pub name: String,
}

/// Every selectable ringtone: the built-in one first, then the bundled files.
#[must_use]
pub fn list() -> Vec<RingtoneInfo> {
    let mut out = vec![RingtoneInfo {
        id: DEFAULT_ID.to_owned(),
        name: "Classic".to_owned(),
    }];
    out.extend(BUNDLED.iter().map(|r| RingtoneInfo {
        id: r.id.to_owned(),
        name: r.name.to_owned(),
    }));
    out
}

/// Decoded ringtones (48 kHz mono), kept so a repeat call does not re-decode.
fn cache() -> &'static Mutex<HashMap<String, Arc<Vec<f32>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Vec<f32>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 48 kHz mono samples for `id`, falling back to the built-in tone when the id
/// is unknown - a ringtone that was removed from the assets folder must not
/// leave someone with a silent phone.
fn samples(id: &str) -> Arc<Vec<f32>> {
    if let Some(hit) = cache().lock().expect("ringtone cache poisoned").get(id) {
        return hit.clone();
    }
    let decoded = BUNDLED
        .iter()
        .find(|r| r.id == id)
        .map(|r| match decode(r.bytes) {
            Ok(samples) => samples,
            Err(e) => {
                tracing::warn!(ringtone = id, error = %e, "could not decode ringtone; using default");
                synthesise()
            }
        })
        .unwrap_or_else(synthesise);
    let decoded = Arc::new(decoded);
    cache()
        .lock()
        .expect("ringtone cache poisoned")
        .insert(id.to_owned(), decoded.clone());
    decoded
}

/// Decode an embedded audio file to 48 kHz mono.
fn decode(bytes: &'static [u8]) -> Result<Vec<f32>, String> {
    // Cursor<&[u8]> is a MediaSource directly.
    let mss = MediaSourceStream::new(
        Box::new(Cursor::new(bytes)),
        MediaSourceStreamOptions::default(),
    );
    let mut format = symphonia::default::get_probe()
        .probe(
            &Hint::new(),
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| format!("unrecognised audio file: {e}"))?;

    let (track_id, params) = audio_track(format.as_ref())?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| format!("no decoder for this audio: {e}"))?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();
    let mut channels = params.channels.as_ref().map_or(0, |c| c.count()) as u16;
    let mut rate = params.sample_rate.unwrap_or(0);

    loop {
        let packet = match format.next_packet() {
            // End of stream in symphonia 0.6 is Ok(None), not an error.
            Ok(None) => break,
            Ok(Some(packet)) => packet,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(format!("could not read audio: {e}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buffer) => {
                if buffer.frames() == 0 {
                    continue;
                }
                let spec = buffer.spec();
                rate = spec.rate();
                channels = spec.channels().count() as u16;
                // copy_to_vec_interleaved RESIZES its destination to this
                // packet, so it has to go through scratch and be appended.
                buffer.copy_to_vec_interleaved(&mut scratch);
                interleaved.extend_from_slice(&scratch);
            }
            // A damaged packet is recoverable; skip it rather than give up.
            Err(SymphoniaError::DecodeError(_) | SymphoniaError::IoError(_)) => continue,
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => return Err(format!("could not decode audio: {e}")),
        }
    }

    if interleaved.is_empty() || channels == 0 || rate == 0 {
        return Err("file contained no audio".to_owned());
    }
    let mono: Vec<f32> = interleaved
        .chunks(channels as usize)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    Ok(resample(&mono, rate, SAMPLE_RATE))
}

fn audio_track(format: &(dyn FormatReader + '_)) -> Result<(u32, AudioCodecParameters), String> {
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("file has no audio track")?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("audio track has no codec parameters")?
        .clone();
    Ok((track.id, params))
}

/// The built-in "Classic" ring: two short double-beeps then silence, so looping
/// it sounds like a phone rather than a continuous tone. Synthesised rather
/// than shipped as a file so there is always a ringtone, whatever is (or isn't)
/// in the assets folder.
fn synthesise() -> Vec<f32> {
    const BEEP_MS: usize = 380;
    const GAP_MS: usize = 200;
    const TAIL_MS: usize = 2200;
    let per_ms = SAMPLE_RATE as usize / 1000;

    let mut out = Vec::with_capacity((2 * (BEEP_MS + GAP_MS) + TAIL_MS) * per_ms);
    for _ in 0..2 {
        let beep = BEEP_MS * per_ms;
        for i in 0..beep {
            let t = i as f32 / SAMPLE_RATE as f32;
            // A major third (660 + 880 Hz) reads as a "ring" rather than an alarm.
            let tone = (std::f32::consts::TAU * 660.0 * t).sin() * 0.5
                + (std::f32::consts::TAU * 880.0 * t).sin() * 0.3;
            // Short fades top and tail; a hard edge on a sine is an audible click.
            let fade = (i as f32 / (20.0 * per_ms as f32))
                .min((beep - i) as f32 / (20.0 * per_ms as f32))
                .clamp(0.0, 1.0);
            out.push(tone * fade * 0.6);
        }
        out.extend(std::iter::repeat_n(0.0, GAP_MS * per_ms));
    }
    out.extend(std::iter::repeat_n(0.0, TAIL_MS * per_ms));
    out
}

/// A ringtone playing on an output device. Dropping it stops the sound.
pub struct Ringing {
    _stream: Stream,
}

/// Start looping `id` on `device_id` (empty = system default) at `volume`
/// (1.0 = unity). Keep the returned value alive for as long as it should ring.
///
/// # Errors
/// Returns a message when the output device cannot be opened.
pub fn start(id: &str, device_id: Option<&str>, volume: f32) -> Result<Ringing, String> {
    let samples = samples(id);
    let device = pick_device(device_id, false).ok_or("no speakers available")?;
    let supported = pick_config(&device, false)?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.config();
    let channels = config.channels as usize;
    let rate = config.sample_rate;

    // Match the device rate once, up front, so the callback only has to copy.
    let ready: Arc<Vec<f32>> = if rate == SAMPLE_RATE {
        samples
    } else {
        Arc::new(resample(&samples, SAMPLE_RATE, rate))
    };
    let cursor = Arc::new(AtomicUsize::new(0));
    let err_fn = |e: cpal::Error| tracing::warn!(error = %e, "ringtone stream error");

    // Wrap around at the end: the ringtone repeats until the call is answered.
    let next = move |out_frames: usize, buf: &Arc<Vec<f32>>, at: &Arc<AtomicUsize>| -> Vec<f32> {
        let start = at.load(Ordering::Relaxed);
        let mut frame = Vec::with_capacity(out_frames);
        for i in 0..out_frames {
            frame.push(buf[(start + i) % buf.len()]);
        }
        at.store((start + out_frames) % buf.len(), Ordering::Relaxed);
        frame
    };

    let stream = match format {
        SampleFormat::F32 => {
            let (buf, at, next) = (ready.clone(), cursor.clone(), next.clone());
            device.build_output_stream(
                config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let frames = next(out.len() / channels, &buf, &at);
                    for (i, frame) in out.chunks_mut(channels).enumerate() {
                        frame.fill(frames.get(i).copied().unwrap_or(0.0) * volume);
                    }
                },
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let (buf, at, next) = (ready.clone(), cursor.clone(), next.clone());
            device.build_output_stream(
                config,
                move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let frames = next(out.len() / channels, &buf, &at);
                    for (i, frame) in out.chunks_mut(channels).enumerate() {
                        let v = (frames.get(i).copied().unwrap_or(0.0) * volume).clamp(-1.0, 1.0);
                        frame.fill((v * 32767.0) as i16);
                    }
                },
                err_fn,
                None,
            )
        }
        other => return Err(format!("unsupported speaker sample format: {other}")),
    }
    .map_err(|e| format!("could not open speakers: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("could not start ringtone: {e}"))?;
    Ok(Ringing { _stream: stream })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_ringtone_is_always_offered_first() {
        let all = list();
        assert_eq!(all[0].id, DEFAULT_ID, "there is always something to play");
        assert_eq!(all.len(), 1 + BUNDLED.len());
    }

    #[test]
    fn bundled_ids_are_unique_so_a_saved_choice_is_unambiguous() {
        let mut ids: Vec<&str> = BUNDLED.iter().map(|r| r.id).collect();
        ids.push(DEFAULT_ID);
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "ringtone ids collide: {ids:?}");
    }

    #[test]
    fn an_unknown_id_falls_back_rather_than_going_silent() {
        let fallback = samples("no-such-ringtone");
        assert!(!fallback.is_empty());
    }

    #[test]
    fn the_synthesised_ring_has_sound_then_silence_so_it_loops_as_a_ring() {
        let ring = synthesise();
        assert!(!ring.is_empty());
        let peak = ring.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.1, "audible");
        assert!(peak <= 1.0, "not clipping");
        // The tail is silent, which is what makes a loop sound like ringing.
        let tail = &ring[ring.len() - SAMPLE_RATE as usize..];
        assert!(tail.iter().all(|s| *s == 0.0), "ends on silence");
    }

    /// Ignored because it opens the real output device and makes a brief sound.
    /// Run with `cargo test -p accord-client ringtone_opens -- --ignored`.
    #[test]
    #[ignore = "opens the speakers and is briefly audible; run explicitly"]
    fn ringtone_opens_the_output_device() {
        let ringing = start(DEFAULT_ID, None, 0.2).expect("ringtone should start");
        std::thread::sleep(std::time::Duration::from_millis(60));
        drop(ringing); // dropping the stream is what stops the sound
    }

    #[test]
    fn every_bundled_ringtone_decodes() {
        // Guards against dropping an unsupported or corrupt file into assets/.
        for tone in BUNDLED {
            let decoded = decode(tone.bytes)
                .unwrap_or_else(|e| panic!("bundled ringtone {} failed: {e}", tone.id));
            assert!(!decoded.is_empty(), "{} decoded to nothing", tone.id);
        }
    }
}
