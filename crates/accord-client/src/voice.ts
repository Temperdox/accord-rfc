/**
 * Voice/video media layer (SCAFFOLD - no live media yet).
 *
 * The transport decision (see plan + ARCHITECTURE): WebRTC P2P in this webview.
 * Each participant opens an `RTCPeerConnection` to every other participant; the
 * server only relays SDP/ICE via `api.sendVoiceSignal` / `onVoiceSignal` and
 * never sees media (DTLS-SRTP P2P), preserving the §5 crypto boundary.
 *
 * THIS FILE IS THE SEAM. The functions below manage capture/track wiring; today
 * they only flip local state and log a TODO. To make voice live, implement:
 *   1. getUserMedia / getDisplayMedia capture in start{Mic,Camera,Screen}.
 *   2. An RTCPeerConnection per remote device (keyed by deviceId), created on the
 *      `voice-participant` join event; exchange offers/answers/candidates through
 *      `api.sendVoiceSignal` and `onVoiceSignal`.
 *   3. Attach remote tracks to <audio>/<video> sinks; render screen/camera video
 *      inside a SandboxedFrame (src/sandbox) so embedded media stays isolated.
 * None of that runs yet - joining a voice channel exchanges presence only.
 */
import * as api from "./api";
import { dismissKey, notify, notifyTransient } from "./notifications";

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

const TODO = (what: string) =>
  console.warn(
    `[voice] TODO: ${what} - RTCPeerConnection/track wiring is not implemented yet ` +
      `(scaffold). See src/voice.ts.`
  );

/** Join a voice channel: announce presence (media negotiation is the TODO). */
export async function join(groupId: string): Promise<void> {
  await api.joinVoice(groupId);
  TODO("create RTCPeerConnections for existing participants");
}

/** Leave the current voice channel and tear down any (future) peer connections. */
export async function leave(groupId: string): Promise<void> {
  await api.leaveVoice(groupId);
  TODO("close RTCPeerConnections and stop local tracks");
}

/** Toggle mic. Stub for capture, but unmuting does a real check that an input
 * device exists and surfaces a header notification if none is found. */
export async function setMuted(s: VoiceLocalState, muted: boolean): Promise<void> {
  if (!s.groupId) return;
  if (!muted) {
    TODO("getUserMedia({audio:true}) and add the track");
    if (!(await hasMicrophone())) {
      notify({
        key: "no-mic",
        severity: "issue",
        message: "No microphone detected - others won't hear you.",
      });
    } else {
      dismissKey("no-mic");
    }
  }
  await api.setVoiceState(s.groupId, muted, s.cameraOn, s.screenOn);
}

/** Whether the system has at least one audio input device. Uses the webview's
 * mediaDevices API (device presence is visible without mic permission). */
async function hasMicrophone(): Promise<boolean> {
  try {
    const devices = await navigator.mediaDevices?.enumerateDevices?.();
    return !!devices?.some((d) => d.kind === "audioinput");
  } catch {
    return true; // can't tell - don't nag
  }
}

/** Call when the user tries to speak while personally muted. The future voice-
 * activity detector (once real capture lands) invokes this; it surfaces a brief
 * header reminder rather than silently dropping their audio. */
export function warnMutedSpeaking(): void {
  notifyTransient(
    {
      key: "muted-speaking",
      severity: "warn",
      message: "You're muted - unmute to talk.",
    },
    3000
  );
}

/** Toggle camera. Stub: only updates announced state. */
export async function setCamera(s: VoiceLocalState, on: boolean): Promise<void> {
  if (!s.groupId) return;
  if (on) TODO("getUserMedia({video:true}) and add/replace the track");
  await api.setVoiceState(s.groupId, s.muted, on, s.screenOn);
}

/** Toggle screen share. Stub: only updates announced state. */
export async function setScreen(s: VoiceLocalState, on: boolean): Promise<void> {
  if (!s.groupId) return;
  if (on) TODO("getDisplayMedia() and add the screen track");
  await api.setVoiceState(s.groupId, s.muted, s.cameraOn, on);
}

/** Handle a relayed signaling envelope (no-op until WebRTC is wired). */
export function handleSignal(_s: api.VoiceSignal): void {
  TODO("apply remote SDP/ICE to the matching RTCPeerConnection");
}
