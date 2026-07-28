/**
 * Client-only voice preferences (device selection + levels). These never leave
 * the device, so they live in localStorage rather than the server; the native
 * engine (`src-tauri/src/voice_engine`) reads them via `set_voice_prefs`.
 */
/** Noise suppression mode. NOT currently applied: it was implemented in the
 * browser audio stack, which the media path no longer goes through. Kept so
 * saved settings survive until native processing lands. */
export type NoiseSuppression = "none" | "standard" | "rnnoise";

export interface VoicePrefs {
  /** Preferred mic device id from `list_audio_devices` ("" = system default). */
  micDeviceId: string;
  /** Preferred output device id ("" = system default). */
  speakerDeviceId: string;
  /** Unused pending native input processing (see the type's note). */
  noiseSuppression: NoiseSuppression;
  /** Unused pending native input processing. */
  echoCancellation: boolean;
  /** Unused pending native input processing. */
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
