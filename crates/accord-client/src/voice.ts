/**
 * Voice media layer - LIVE mic audio (camera/screen still scaffolded).
 *
 * Transport: WebRTC P2P in this webview. Each participant device opens one
 * `RTCPeerConnection` to every other participant device; SDP/ICE is relayed
 * opaquely by the server via `api.sendVoiceSignal` / `onVoiceSignal`, which never
 * sees the media (DTLS-SRTP P2P), preserving the ARCHITECTURE section 5 boundary.
 *
 * Mesh setup uses a DETERMINISTIC offerer to avoid glare: of any two devices,
 * the one with the lexicographically smaller device id sends the offer; the
 * other answers. Mic-only means a single audio track added at connection time,
 * so there is no mid-call renegotiation - muting just toggles `track.enabled`.
 *
 * Still TODO (camera/screen): capture via getUserMedia/getDisplayMedia and
 * renegotiate the peer connections; render remote video inside a SandboxedFrame.
 */
import * as api from "./api";
import { dismissKey, notify, notifyTransient } from "./notifications";
import { DEFAULT_VOICE_PREFS, type VoicePrefs } from "./voicePrefs";
import { createRnnoiseNode } from "./rnnoise";

/** Local capture/announce state for the channel this device is in. */
export interface VoiceLocalState {
  groupId: string | null;
  muted: boolean;
  cameraOn: boolean;
  screenOn: boolean;
}

export const initialVoiceState = (): VoiceLocalState => ({
  groupId: null,
  muted: false,
  cameraOn: false,
  screenOn: false,
});

/** STUN helps NAT traversal on the open internet; LAN + Yggdrasil-mesh peers
 * connect via host candidates, so calls still work with no reachable STUN. */
const ICE_SERVERS: RTCIceServer[] = [{ urls: "stun:stun.l.google.com:19302" }];

interface Peer {
  pc: RTCPeerConnection;
  audio: HTMLAudioElement;
  /** Per-peer output gain (their playback volume); the <audio> plays this node's
   * stream so output-device selection (setSinkId) still works. */
  outGain: GainNode | null;
  /** Analyser over this peer's RECEIVED audio, for their speaking indicator.
   * Only exists for devices we have a live connection with (i.e. only while we
   * are in the same voice channel), so non-participants never get a level. */
  analyser: AnalyserNode | null;
}

// One active voice session at a time; module-level singleton state.
const peers = new Map<string, Peer>(); // remote deviceId -> connection
// rawStream = the mic as captured; localStream = what we SEND/analyse (always the
// Web Audio destination: raw -> [RNNoise] -> gain -> dest).
let rawStream: MediaStream | null = null;
let localStream: MediaStream | null = null;
let micGainNode: GainNode | null = null;
let captureDispose: (() => void) | null = null;
// Live gain nodes for an in-progress Mic Test, so the sliders adjust it live.
let testGainNode: GainNode | null = null;
let testOutGain: GainNode | null = null;
let currentGroup: string | null = null;
let myDevice = "";
let deafened = false;
let audioPrefs: VoicePrefs = { ...DEFAULT_VOICE_PREFS };

/** Apply voice prefs. Volumes + output device take effect live; a mic device or
 * processing change re-captures and swaps the track on peers. */
export async function setAudioPrefs(prefs: VoicePrefs): Promise<void> {
  const micChanged =
    prefs.micDeviceId !== audioPrefs.micDeviceId ||
    prefs.noiseSuppression !== audioPrefs.noiseSuppression ||
    prefs.echoCancellation !== audioPrefs.echoCancellation ||
    prefs.autoGain !== audioPrefs.autoGain;
  audioPrefs = { ...prefs };
  applyOutputDevice();
  applyOutputVolume();
  if (micGainNode) micGainNode.gain.value = audioPrefs.micGain / 100;
  // Keep a running Mic Test in sync with the sliders too.
  if (testGainNode) testGainNode.gain.value = audioPrefs.micGain / 100;
  if (testOutGain) testOutGain.gain.value = audioPrefs.outputVolume / 100;
  if (micChanged && currentGroup) await reapplyMic();
}

/** Route every peer's audio to the chosen speaker (setSinkId; Chromium/WebView2). */
function applyOutputDevice(): void {
  const sink = audioPrefs.speakerDeviceId;
  for (const peer of peers.values()) {
    const el = peer.audio as HTMLAudioElement & { setSinkId?: (id: string) => Promise<void> };
    el.setSinkId?.(sink).catch(() => {});
  }
}

/** Apply the playback volume to every peer's output gain. */
function applyOutputVolume(): void {
  const v = audioPrefs.outputVolume / 100;
  for (const peer of peers.values()) if (peer.outGain) peer.outGain.gain.value = v;
}

/** Re-capture the mic with the current prefs and swap it onto live peers. */
async function reapplyMic(): Promise<void> {
  disposeCapture();
  await captureMic();
  const track = localStream ? (localStream as MediaStream).getAudioTracks()[0] : null;
  for (const peer of peers.values()) {
    const sender = peer.pc.getSenders().find((s) => s.track?.kind === "audio");
    if (sender && track) await sender.replaceTrack(track).catch(() => {});
  }
  localAnalyser = null; // rebuilt by startLevelMeter from the new stream
  startLevelMeter();
}

/** Tear down the current capture graph (RNNoise/gain nodes + mic tracks). Does
 * NOT close the shared AudioContext - that lives for the whole call. */
function disposeCapture(): void {
  captureDispose?.();
  captureDispose = null;
  micGainNode = null;
  rawStream?.getTracks().forEach((t) => t.stop());
  rawStream = null;
  localStream = null;
}

/** Deafen: silence every peer's audio sink (you stop hearing others). New peers
 * inherit this. The caller also mutes the mic separately (Discord convention). */
export function setDeafened(on: boolean): void {
  deafened = on;
  for (const peer of peers.values()) peer.audio.muted = on;
}

const enc = (obj: unknown): number[] =>
  Array.from(new TextEncoder().encode(JSON.stringify(obj)));
const dec = (data: number[]): any =>
  JSON.parse(new TextDecoder().decode(new Uint8Array(data)));

// --- Speaking-level meter ----------------------------------------------------
// One animation-frame loop measures the local mic AND every connected peer's
// RECEIVED audio, so each side shows a live bar for everyone in the channel.
// Peers we don't have a connection to (i.e. when we're not in the VC) simply
// have no analyser, so non-participants never compute or show a level.
const FFT = 512;
let audioCtx: AudioContext | null = null;
let localAnalyser: AnalyserNode | null = null;
let levelRaf = 0;
let levelListener: ((level: number) => void) | null = null;
let levelsListener: ((levels: Record<string, number>) => void) | null = null;
const levelBuf = new Uint8Array(FFT);

/** Subscribe to the LOCAL mic level (0..1) - drives the user-pill indicator. */
export function onLevel(cb: (level: number) => void): void {
  levelListener = cb;
}
/** Subscribe to REMOTE peer levels keyed by device id (0..1) - drives each
 * voice participant tile's indicator. Only populated for connected peers. */
export function onLevels(cb: (levels: Record<string, number>) => void): void {
  levelsListener = cb;
}

function audioContext(): AudioContext {
  // 48 kHz: RNNoise assumes it, and it's the Opus/WebRTC rate anyway.
  if (!audioCtx) audioCtx = new AudioContext({ sampleRate: 48000 });
  return audioCtx;
}

/** Build an analyser fed by `stream` (used for both local mic + remote audio). */
function makeAnalyser(stream: MediaStream): AnalyserNode | null {
  try {
    const a = audioContext().createAnalyser();
    a.fftSize = FFT;
    a.smoothingTimeConstant = 0.6;
    audioContext().createMediaStreamSource(stream).connect(a);
    return a;
  } catch (e) {
    console.warn("[voice] analyser failed", e);
    return null;
  }
}

/** RMS of an analyser's waveform, gained so normal speech fills the bar. A muted
 * (disabled) track feeds silence, so its level naturally falls to 0. */
function levelOf(a: AnalyserNode): number {
  a.getByteTimeDomainData(levelBuf);
  let sum = 0;
  for (let i = 0; i < levelBuf.length; i++) {
    const v = (levelBuf[i] - 128) / 128;
    sum += v * v;
  }
  return Math.min(1, Math.sqrt(sum / levelBuf.length) * 2.8);
}

/** Start the per-frame level loop for the current call (idempotent). */
function startLevelMeter(): void {
  if (localStream && !localAnalyser) localAnalyser = makeAnalyser(localStream);
  if (levelRaf) return;
  const tick = () => {
    levelListener?.(localAnalyser ? levelOf(localAnalyser) : 0);
    const map: Record<string, number> = {};
    for (const [id, peer] of peers) {
      if (peer.analyser) map[id] = levelOf(peer.analyser);
    }
    levelsListener?.(map);
    levelRaf = requestAnimationFrame(tick);
  };
  levelRaf = requestAnimationFrame(tick);
}

/** Stop the meter loop and report empties so all indicators collapse. The shared
 * AudioContext stays open for the call (RNNoise + analysers); leave() closes it. */
function stopLevelMeter(): void {
  if (levelRaf) cancelAnimationFrame(levelRaf);
  levelRaf = 0;
  localAnalyser = null;
  levelListener?.(0);
  levelsListener?.({});
}

/** Mic test: capture the mic with the current prefs (device + processing + gain),
 * send it through a LOCAL loopback `RTCPeerConnection` pair (so it exercises the
 * real WebRTC encode/decode), then play the received audio back to your output
 * device. Reports the post-loopback level via `onLevel`, so you can confirm
 * WebRTC actually carries your audio - and tune the gain - before joining a call.
 * The gain/output sliders adjust it live. Returns a stop function. (Use
 * headphones to avoid feedback.) */
export async function startMicTest(onLevel: (level: number) => void): Promise<() => void> {
  const ctx = new AudioContext({ sampleRate: 48000 });
  let stream: MediaStream;
  try {
    stream = await navigator.mediaDevices.getUserMedia(micConstraints());
  } catch {
    void ctx.close();
    throw new Error("no microphone available");
  }

  // --- send graph: mic -> [RNNoise] -> gain -> destination -> loopback pc1 ---
  const source = ctx.createMediaStreamSource(stream);
  let tail: AudioNode = source;
  let rnDispose: (() => void) | null = null;
  if (audioPrefs.noiseSuppression === "rnnoise") {
    try {
      const rn = await createRnnoiseNode(ctx);
      source.connect(rn.node);
      tail = rn.node;
      rnDispose = rn.dispose;
    } catch {
      /* fall back to raw mic for the test */
    }
  }
  const gain = ctx.createGain();
  gain.gain.value = audioPrefs.micGain / 100;
  tail.connect(gain);
  const sendDest = ctx.createMediaStreamDestination();
  gain.connect(sendDest);
  testGainNode = gain;

  // --- local loopback so the audio actually round-trips through WebRTC ---
  const pc1 = new RTCPeerConnection();
  const pc2 = new RTCPeerConnection();
  pc1.onicecandidate = (e) => e.candidate && void pc2.addIceCandidate(e.candidate).catch(() => {});
  pc2.onicecandidate = (e) => e.candidate && void pc1.addIceCandidate(e.candidate).catch(() => {});

  const audio = new Audio();
  audio.autoplay = true;
  audio.style.display = "none";
  document.body.appendChild(audio);
  const sink = audio as HTMLAudioElement & { setSinkId?: (id: string) => Promise<void> };
  if (audioPrefs.speakerDeviceId) sink.setSinkId?.(audioPrefs.speakerDeviceId).catch(() => {});

  const analyser = ctx.createAnalyser();
  analyser.fftSize = FFT;
  analyser.smoothingTimeConstant = 0.6;

  pc2.ontrack = (e) => {
    const recv = e.streams[0] ?? new MediaStream([e.track]);
    // Meter the POST-loopback audio (proves the round-trip carries sound).
    ctx.createMediaStreamSource(recv).connect(analyser);
    // Playback with the output-volume gain so that slider is live too.
    const recvSrc = ctx.createMediaStreamSource(recv);
    const outGain = ctx.createGain();
    outGain.gain.value = audioPrefs.outputVolume / 100;
    const outDest = ctx.createMediaStreamDestination();
    recvSrc.connect(outGain).connect(outDest);
    testOutGain = outGain;
    audio.srcObject = outDest.stream;
    void audio.play().catch(() => {});
  };

  for (const track of sendDest.stream.getTracks()) pc1.addTrack(track, sendDest.stream);
  try {
    const offer = await pc1.createOffer();
    await pc1.setLocalDescription(offer);
    await pc2.setRemoteDescription(offer);
    const answer = await pc2.createAnswer();
    await pc2.setLocalDescription(answer);
    await pc1.setRemoteDescription(answer);
  } catch (e) {
    console.error("[voice] mic-test loopback failed", e);
  }

  let raf = requestAnimationFrame(function tick() {
    onLevel(levelOf(analyser));
    raf = requestAnimationFrame(tick);
  });
  return () => {
    cancelAnimationFrame(raf);
    testGainNode = null;
    testOutGain = null;
    stream.getTracks().forEach((t) => t.stop());
    rnDispose?.();
    try {
      pc1.close();
      pc2.close();
    } catch {
      /* ignore */
    }
    audio.srcObject = null;
    audio.remove();
    void ctx.close().catch(() => {});
    onLevel(0);
  };
}

/** Join a voice channel: capture the mic (best-effort), announce presence, then
 * let participant events drive the peer mesh. */
export async function join(groupId: string, myDeviceId: string): Promise<void> {
  currentGroup = groupId;
  myDevice = myDeviceId;
  await captureMic();
  startLevelMeter();
  await api.joinVoice(groupId);
}

/** Build the mic constraints from prefs. Browser NS only in "standard" mode
 * (RNNoise does its own; "none" disables both). */
function micConstraints(): MediaStreamConstraints {
  return {
    audio: {
      deviceId: audioPrefs.micDeviceId ? { exact: audioPrefs.micDeviceId } : undefined,
      echoCancellation: audioPrefs.echoCancellation,
      noiseSuppression: audioPrefs.noiseSuppression === "standard",
      autoGainControl: audioPrefs.autoGain,
    },
    video: false,
  };
}

/** Capture the mic into `rawStream` and build the send graph
 * (raw -> [RNNoise] -> gain -> destination); `localStream` is the destination's
 * stream, so the mic-volume gain + denoise apply to what peers receive. */
async function captureMic(): Promise<void> {
  try {
    rawStream = await navigator.mediaDevices.getUserMedia(micConstraints());
    dismissKey("no-mic");
  } catch {
    rawStream = null;
    localStream = null;
    notify({
      key: "no-mic",
      severity: "issue",
      message: "No microphone available - others won't hear you (you can still listen).",
    });
    return;
  }

  const ctx = audioContext();
  const source = ctx.createMediaStreamSource(rawStream);
  let tail: AudioNode = source;
  let rnDispose: (() => void) | null = null;
  if (audioPrefs.noiseSuppression === "rnnoise") {
    try {
      const rn = await createRnnoiseNode(ctx);
      source.connect(rn.node);
      tail = rn.node;
      rnDispose = rn.dispose;
    } catch (e) {
      console.warn("[voice] RNNoise failed, using raw mic", e);
      notifyTransient(
        {
          key: "rnnoise-fail",
          severity: "warn",
          message: "Noise suppression unavailable; using your raw mic.",
        },
        4000
      );
    }
  }
  const gain = ctx.createGain();
  gain.gain.value = audioPrefs.micGain / 100;
  tail.connect(gain);
  const dest = ctx.createMediaStreamDestination();
  gain.connect(dest);
  micGainNode = gain;
  localStream = dest.stream;
  captureDispose = () => {
    try {
      source.disconnect();
      rnDispose?.();
      gain.disconnect();
    } catch {
      /* ignore teardown errors */
    }
  };
}

/** Leave the current channel: tear down peers, capture graph, and the audio ctx. */
export async function leave(groupId: string): Promise<void> {
  stopLevelMeter();
  deafened = false;
  for (const [id] of peers) closePeer(id);
  disposeCapture();
  if (audioCtx) {
    void audioCtx.close().catch(() => {});
    audioCtx = null;
  }
  currentGroup = null;
  try {
    await api.leaveVoice(groupId);
  } catch {
    /* best-effort */
  }
}

/** Open (or reuse) the connection to a remote device. `offerer` true means this
 * device initiates the SDP offer; false means it waits for one. Idempotent. */
function peerFor(remoteDevice: string, offerer: boolean): Peer {
  const existing = peers.get(remoteDevice);
  if (existing) return existing;

  const pc = new RTCPeerConnection({ iceServers: ICE_SERVERS });
  const audio = new Audio();
  audio.autoplay = true;
  audio.muted = deafened; // inherit current deafen state
  // Some engines only start playback for elements attached to the document.
  audio.style.display = "none";
  document.body.appendChild(audio);
  // Route to the chosen output device (best-effort; Chromium/WebView2).
  if (audioPrefs.speakerDeviceId) {
    const el = audio as HTMLAudioElement & { setSinkId?: (id: string) => Promise<void> };
    el.setSinkId?.(audioPrefs.speakerDeviceId).catch(() => {});
  }

  if (localStream) {
    for (const track of localStream.getTracks()) pc.addTrack(track, localStream);
  }
  pc.ontrack = (e) => {
    const stream = e.streams[0] ?? new MediaStream([e.track]);
    const p = peers.get(remoteDevice);
    try {
      // remote -> gain -> destination; the <audio> plays the gained stream so the
      // output device (setSinkId) still applies and volume can exceed 100%.
      const ctx = audioContext();
      const src = ctx.createMediaStreamSource(stream);
      const gain = ctx.createGain();
      gain.gain.value = audioPrefs.outputVolume / 100;
      const dest = ctx.createMediaStreamDestination();
      src.connect(gain).connect(dest);
      audio.srcObject = dest.stream;
      if (p) p.outGain = gain;
    } catch {
      audio.srcObject = stream; // fallback: no volume control
    }
    void audio.play().catch(() => {});
    // Tap this peer's received audio for their speaking indicator.
    if (p && !p.analyser) p.analyser = makeAnalyser(stream);
  };
  pc.onicecandidate = (e) => {
    if (e.candidate && currentGroup) {
      void api.sendVoiceSignal(currentGroup, remoteDevice, "ice", enc(e.candidate.toJSON()));
    }
  };
  pc.onconnectionstatechange = () => {
    if (pc.connectionState === "failed" || pc.connectionState === "closed") {
      closePeer(remoteDevice);
    }
  };

  const peer: Peer = { pc, audio, outGain: null, analyser: null };
  peers.set(remoteDevice, peer);

  if (offerer) {
    void (async () => {
      try {
        const offer = await pc.createOffer();
        await pc.setLocalDescription(offer);
        if (currentGroup) {
          await api.sendVoiceSignal(currentGroup, remoteDevice, "offer", enc(pc.localDescription));
        }
      } catch (err) {
        console.error("[voice] offer failed", err);
      }
    })();
  }
  return peer;
}

/** Close + forget a peer connection and its audio sink. */
function closePeer(remoteDevice: string): void {
  const peer = peers.get(remoteDevice);
  if (!peer) return;
  peers.delete(remoteDevice);
  try {
    peer.pc.close();
  } catch {
    /* ignore */
  }
  peer.audio.srcObject = null;
  peer.audio.remove();
}

/** Drive the peer mesh from a participant event (join/leave of a device). Called
 * for every `voice-participant`; filters to this device's current channel. */
export function onParticipant(p: api.VoiceParticipant): void {
  if (p.groupId !== currentGroup || p.deviceId === myDevice || !myDevice) return;
  if (p.joined) {
    // Deterministic offerer: the smaller device id initiates.
    peerFor(p.deviceId, myDevice < p.deviceId);
  } else {
    closePeer(p.deviceId);
  }
}

/** Apply a relayed signaling envelope (offer / answer / ICE) to the right peer. */
export async function handleSignal(s: api.VoiceSignal): Promise<void> {
  if (s.groupId !== currentGroup || !s.fromDevice) return;
  try {
    if (s.kind === "offer") {
      // We answer; create the peer as the non-offerer if it doesn't exist yet.
      const peer = peerFor(s.fromDevice, false);
      await peer.pc.setRemoteDescription(dec(s.data));
      const answer = await peer.pc.createAnswer();
      await peer.pc.setLocalDescription(answer);
      if (currentGroup) {
        await api.sendVoiceSignal(currentGroup, s.fromDevice, "answer", enc(peer.pc.localDescription));
      }
    } else if (s.kind === "answer") {
      const peer = peers.get(s.fromDevice);
      if (peer) await peer.pc.setRemoteDescription(dec(s.data));
    } else if (s.kind === "ice") {
      const peer = peers.get(s.fromDevice);
      if (peer) await peer.pc.addIceCandidate(dec(s.data)).catch(() => {});
    }
  } catch (err) {
    console.error("[voice] signal handling failed", err);
  }
}

/** Toggle mic. Mute = disable the local audio track (no renegotiation). */
export async function setMuted(s: VoiceLocalState, muted: boolean): Promise<void> {
  if (!s.groupId) return;
  if (localStream) {
    for (const track of localStream.getTracks()) track.enabled = !muted;
  } else if (!muted) {
    // Unmuting with no captured mic: try once more to grab one, then add the
    // track to every existing peer (cast sidesteps control-flow narrowing).
    await captureMic();
    const stream = localStream as MediaStream | null;
    if (stream) {
      for (const peer of peers.values()) {
        for (const track of stream.getTracks()) peer.pc.addTrack(track, stream);
      }
      startLevelMeter();
    }
  }
  await api.setVoiceState(s.groupId, muted, s.cameraOn, s.screenOn);
}

/** Brief reminder shown if the user tries to talk while muted. */
export function warnMutedSpeaking(): void {
  notifyTransient(
    { key: "muted-speaking", severity: "warn", message: "You're muted - unmute to talk." },
    3000
  );
}

const TODO = (what: string) =>
  console.warn(`[voice] TODO: ${what} - camera/screen media not wired yet. See src/voice.ts.`);

/** Toggle camera. Stub: announces state only (live video is the next phase). */
export async function setCamera(s: VoiceLocalState, on: boolean): Promise<void> {
  if (!s.groupId) return;
  if (on) TODO("getUserMedia({video:true}) and renegotiate");
  await api.setVoiceState(s.groupId, s.muted, on, s.screenOn);
}

/** Toggle screen share. Stub: announces state only. */
export async function setScreen(s: VoiceLocalState, on: boolean): Promise<void> {
  if (!s.groupId) return;
  if (on) TODO("getDisplayMedia() and renegotiate");
  await api.setVoiceState(s.groupId, s.muted, s.cameraOn, on);
}
