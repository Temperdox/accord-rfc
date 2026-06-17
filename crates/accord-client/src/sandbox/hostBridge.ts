/**
 * Host SDK bridge - the ONLY surface sandboxed content (channel presets, and the
 * future screen-share/embedded media frame) may call, over `postMessage`.
 *
 * SECURITY MODEL (see BOT-API-PLAN.md "The two layers"):
 *   • Sandboxed content runs in an <iframe sandbox="allow-scripts"> WITHOUT
 *     `allow-same-origin`, so it cannot reach this app's origin, `window.__TAURI__`,
 *     `invoke`, cookies, or the session. It is a separate, opaque origin.
 *   • This bridge exposes ONLY capabilities the viewing user already has, scoped
 *     to the hosting channel - never `invoke`, never the Bot API, never anything
 *     privileged. A code-injection inside a preset therefore gains, at most, the
 *     user's own authority in that one channel; it can never escalate to admin or
 *     bot power. (Bots are separate authenticated principals outside the webview.)
 *
 * THIS IS A SCAFFOLD: no preset host calls it yet. It exists now so the boundary
 * is established before any rich/embedded content ships. Keep the allowlist tight
 * and NEVER import or reference `invoke`/Tauri APIs from anything reachable here.
 */

/** Capabilities a sandboxed frame is granted for its hosting channel. All are
 * scoped to the viewing user's existing authority - nothing privileged. */
export interface HostCapabilities {
  /** Channel this frame is hosted in (everything is scoped to it). */
  groupId: string;
  /** Send a message AS THE USER to the hosting channel (same as typing it). */
  sendMessage?: (content: string) => Promise<void>;
  /** Read recent messages of the hosting channel (what the user can already see). */
  readRecentMessages?: () => Promise<unknown[]>;
  /** Read/write the preset's own private key/value storage (namespaced). */
  getPresetData?: (key: string) => Promise<string | null>;
  setPresetData?: (key: string, value: string) => Promise<void>;
}

/** Allowlisted request verbs. Anything not here is rejected - there is no
 * passthrough and no privileged verb. */
type HostRequest =
  | { id: number; verb: "sendMessage"; content: string }
  | { id: number; verb: "readRecentMessages" }
  | { id: number; verb: "getPresetData"; key: string }
  | { id: number; verb: "setPresetData"; key: string; value: string };

/**
 * Install the bridge for one sandboxed iframe. Returns a disposer that removes
 * the listener. The bridge:
 *   • only accepts messages whose `source` is exactly this frame's window,
 *   • only honors allowlisted verbs (capabilities the user already has),
 *   • replies to the frame with the result keyed by request id.
 * It deliberately has no access path to `invoke` or any privileged API.
 */
export function installHostBridge(
  frame: HTMLIFrameElement,
  caps: HostCapabilities
): () => void {
  const onMessage = async (ev: MessageEvent) => {
    // Only messages from THIS sandboxed frame's window are honored.
    if (ev.source !== frame.contentWindow) return;
    const req = ev.data as HostRequest | undefined;
    if (!req || typeof req.id !== "number" || typeof (req as { verb?: unknown }).verb !== "string") {
      return;
    }

    const reply = (ok: boolean, result?: unknown, error?: string) =>
      frame.contentWindow?.postMessage({ id: req.id, ok, result, error }, "*");

    try {
      switch (req.verb) {
        case "sendMessage":
          if (!caps.sendMessage) return reply(false, undefined, "not permitted");
          await caps.sendMessage(String(req.content ?? ""));
          return reply(true);
        case "readRecentMessages":
          if (!caps.readRecentMessages) return reply(false, undefined, "not permitted");
          return reply(true, await caps.readRecentMessages());
        case "getPresetData":
          if (!caps.getPresetData) return reply(false, undefined, "not permitted");
          return reply(true, await caps.getPresetData(String(req.key)));
        case "setPresetData":
          if (!caps.setPresetData) return reply(false, undefined, "not permitted");
          await caps.setPresetData(String(req.key), String(req.value));
          return reply(true);
        default:
          // Unknown/privileged verb: refuse. No passthrough to invoke exists.
          return reply(false, undefined, "unknown verb");
      }
    } catch (e) {
      return reply(false, undefined, String(e));
    }
  };

  window.addEventListener("message", onMessage);
  return () => window.removeEventListener("message", onMessage);
}
