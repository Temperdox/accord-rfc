/**
 * Voice media layer - a thin shim over the NATIVE engine in Rust.
 *
 * The media pipeline (microphone capture, Opus, WebRTC peer connections,
 * playback) lives in `src-tauri/src/voice_engine`, not here. That is not a
 * preference: WebKitGTK - the webview Tauri uses on Linux - ships without
 * `RTCPeerConnection` (verified on Fedora 44: the API is absent even with
 * `enable-webrtc` turned on), so a webview-side WebRTC stack cannot work at
 * all there. Keeping one native path means every platform behaves the same.
 *
 * This module therefore only: forwards user intent to the engine, and exposes
 * the speaking levels the engine reports so the UI can draw its indicators.
 * Signaling (offer/answer/ICE) never reaches the webview - the engine consumes
 * the relayed envelopes directly.
 *
 * Still TODO (camera/screen): the engine is audio-only; video would add a
 * second track and a renderer.
 */
import * as api from "./api";
import { notifyTransient } from "./notifications";
import { DEFAULT_VOICE_PREFS, type VoicePrefs } from "./voicePrefs";

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

let audioPrefs: VoicePrefs = { ...DEFAULT_VOICE_PREFS };
let currentGroup: string | null = null;
let levelListener: ((level: number) => void) | null = null;
let levelsListener: ((levels: Record<string, number>) => void) | null = null;
let micTestListener: ((level: number) => void) | null = null;
let unlistenLevels: (() => void) | null = null;

/** Subscribe to the LOCAL mic level (0..1) - drives the user-pill indicator. */
export function onLevel(cb: (level: number) => void): void {
  levelListener = cb;
  void ensureLevelStream();
}

/** Subscribe to REMOTE peer levels keyed by device id (0..1) - drives each
 * voice participant tile's indicator. Only connected peers appear. */
export function onLevels(cb: (levels: Record<string, number>) => void): void {
  levelsListener = cb;
  void ensureLevelStream();
}

/** Attach to the engine's level events once; it emits while a call or mic test
 * is running and goes quiet otherwise. */
async function ensureLevelStream(): Promise<void> {
  if (unlistenLevels) return;
  unlistenLevels = await api.onVoiceLevels((l) => {
    // During a mic test the engine reports the captured level as `local` too,
    // so the test meter and the call meter share one event.
    if (micTestListener) micTestListener(l.local);
    levelListener?.(l.local);
    levelsListener?.(l.peers);
  });
}

/** Apply voice prefs. Everything (device choice, gain, volume) is applied live
 * by the engine; a device change re-opens that stream underneath. */
export async function setAudioPrefs(prefs: VoicePrefs): Promise<void> {
  audioPrefs = { ...prefs };
  await api.setVoicePrefs(audioPrefs);
}

/** Deafen: stop playing everyone else's audio. The caller mutes the mic
 * separately (Discord convention). */
export function setDeafened(on: boolean): void {
  void api.setVoiceDeafened(on);
}

/** Mic test: the engine captures from the configured input and reports its
 * level through the same event stream; nothing is transmitted. Returns a stop
 * function. */
export async function startMicTest(onLevel: (level: number) => void): Promise<() => void> {
  micTestListener = onLevel;
  await ensureLevelStream();
  await api.startMicTest();
  return () => {
    micTestListener = null;
    onLevel(0);
    void api.stopMicTest();
  };
}

/** Join a voice channel: start capture/playback and announce presence. The
 * engine then drives the peer mesh from the relayed participant events. */
export async function join(groupId: string, myDeviceId: string): Promise<void> {
  currentGroup = groupId;
  await api.joinVoice(groupId, myDeviceId);
}

/** Leave the current channel: the engine tears down peers and audio devices. */
export async function leave(groupId: string): Promise<void> {
  currentGroup = null;
  await api.leaveVoice(groupId);
  levelListener?.(0);
  levelsListener?.({});
}

/** Toggle mic. Muting stops transmitting; the level meter keeps working so the
 * user can see they are talking into a muted mic. */
export async function setMuted(s: VoiceLocalState, muted: boolean): Promise<void> {
  if (!s.groupId) return;
  await api.setVoiceMuted(muted);
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

/** Toggle camera. Stub: announces state only (the engine is audio-only). */
export async function setCamera(s: VoiceLocalState, on: boolean): Promise<void> {
  if (!s.groupId) return;
  if (on) TODO("native camera capture + a second track");
  await api.setVoiceState(s.groupId, s.muted, on, s.screenOn);
}

/** Toggle screen share. Stub: announces state only. */
export async function setScreen(s: VoiceLocalState, on: boolean): Promise<void> {
  if (!s.groupId) return;
  if (on) TODO("native screen capture + a second track");
  await api.setVoiceState(s.groupId, s.muted, s.cameraOn, on);
}

/** The channel this device is currently in (or null). */
export const activeGroup = (): string | null => currentGroup;
