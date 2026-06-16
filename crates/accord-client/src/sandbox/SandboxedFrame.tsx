/**
 * SandboxedFrame — isolation wrapper for ALL embedded/rich content (future
 * channel presets, and the natural home for screen-share / camera video).
 *
 * The `sandbox` attribute deliberately OMITS `allow-same-origin`, so the framed
 * content runs in an opaque origin with no access to this app's origin,
 * `window.__TAURI__`, `invoke`, cookies, or the session. Its only channel to the
 * host is the capability-scoped `postMessage` bridge (see hostBridge.ts), which
 * exposes nothing privileged. This is the structural guarantee that a malicious
 * or injected preset can never escalate to admin or bot power.
 *
 * Scaffold: nothing privileged renders through this yet. Use it for any embedded
 * content from here on so the boundary holds by construction.
 */
import { onCleanup, onMount } from "solid-js";

import { type HostCapabilities, installHostBridge } from "./hostBridge";

export interface SandboxedFrameProps {
  /** Frame document source (srcdoc for inline content, or a blob/url). */
  srcdoc?: string;
  src?: string;
  /** Capabilities granted to this frame (scoped to the user's own authority). */
  capabilities: HostCapabilities;
  class?: string;
  title?: string;
}

export default function SandboxedFrame(props: SandboxedFrameProps) {
  let frame: HTMLIFrameElement | undefined;

  onMount(() => {
    if (!frame) return;
    const dispose = installHostBridge(frame, props.capabilities);
    onCleanup(dispose);
  });

  return (
    <iframe
      ref={frame}
      class={props.class ?? "sandboxed-frame"}
      title={props.title ?? "embedded content"}
      // NOTE: no `allow-same-origin` — embedded content is a separate opaque
      // origin and can never reach the app, Tauri invoke, or the session.
      sandbox="allow-scripts"
      srcdoc={props.srcdoc}
      src={props.src}
    />
  );
}
