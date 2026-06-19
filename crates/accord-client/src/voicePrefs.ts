/**
 * Client-only voice/video preferences (device selection + input processing).
 * These never leave the device, so they live in localStorage rather than the
 * server. `voice.ts` reads them when capturing the mic / routing output.
 */
/** Noise suppression mode:
 * - "none": no suppression
 * - "standard": the browser's built-in WebRTC noise suppression
 * - "rnnoise": RNNoise WASM (stronger, the free Krisp equivalent) */
export type NoiseSuppression = "none" | "standard" | "rnnoise";

export interface VoicePrefs {
  /** Preferred mic input deviceId ("" = system default). */
  micDeviceId: string;
  /** Preferred audio output deviceId ("" = system default). */
  speakerDeviceId: string;
  noiseSuppression: NoiseSuppression;
  /** Echo cancellation. */
  echoCancellation: boolean;
  /** Automatic gain control. */
  autoGain: boolean;
  /** Mic input gain sent to peers, percent (0-200; 100 = unchanged). */
  micGain: number;
  /** Playback volume for other people's audio, percent (0-200; 100 = unchanged). */
  outputVolume: number;
}

const KEY = "accord.voicePrefs";

export const DEFAULT_VOICE_PREFS: VoicePrefs = {
  micDeviceId: "",
  speakerDeviceId: "",
  noiseSuppression: "rnnoise",
  echoCancellation: true,
  autoGain: true,
  micGain: 100,
  outputVolume: 100,
};

export function loadVoicePrefs(): VoicePrefs {
  try {
    const p = { ...DEFAULT_VOICE_PREFS, ...JSON.parse(localStorage.getItem(KEY) ?? "{}") };
    // Migrate the old boolean form to the new mode enum.
    if (typeof (p as { noiseSuppression: unknown }).noiseSuppression === "boolean") {
      p.noiseSuppression = (p as unknown as { noiseSuppression: boolean }).noiseSuppression
        ? "standard"
        : "none";
    }
    return p;
  } catch {
    return { ...DEFAULT_VOICE_PREFS };
  }
}

export function saveVoicePrefs(prefs: VoicePrefs): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs));
  } catch {
    /* storage unavailable - keep in-memory only */
  }
}
