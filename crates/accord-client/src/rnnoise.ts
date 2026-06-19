/**
 * RNNoise mic noise suppression - the free, open-source equivalent of Discord's
 * Krisp. Runs RNNoise compiled to WASM inside an AudioWorklet (off the main
 * thread) and returns a denoised MediaStream that we send over WebRTC.
 *
 * Uses @sapphi-red/web-noise-suppressor (MIT), which ships the worklet + wasm.
 * Vite bundles the worklet/wasm as hashed assets via the `?url` imports below;
 * RNNoise assumes a 48 kHz AudioContext (voice.ts creates one).
 */
import { loadRnnoise, RnnoiseWorkletNode } from "@sapphi-red/web-noise-suppressor";
import rnnoiseWorkletUrl from "@sapphi-red/web-noise-suppressor/rnnoiseWorklet.js?url";
import rnnoiseWasmUrl from "@sapphi-red/web-noise-suppressor/rnnoise.wasm?url";
import rnnoiseSimdWasmUrl from "@sapphi-red/web-noise-suppressor/rnnoise_simd.wasm?url";

let wasmBinary: ArrayBuffer | null = null;
let moduleCtx: AudioContext | null = null;

/** Load the wasm (once) + register the worklet module on `ctx` (once per ctx). */
async function ensureLoaded(ctx: AudioContext): Promise<void> {
  if (!wasmBinary) {
    wasmBinary = await loadRnnoise({ url: rnnoiseWasmUrl, simdUrl: rnnoiseSimdWasmUrl });
  }
  if (moduleCtx !== ctx) {
    await ctx.audioWorklet.addModule(rnnoiseWorkletUrl);
    moduleCtx = ctx;
  }
}

/** An RNNoise worklet node (not yet connected) plus its disposer. The caller
 * wires it into its own graph: `source.connect(node)` then chain onward. */
export interface RnnoiseNode {
  node: AudioWorkletNode;
  dispose: () => void;
}

/** Create an RNNoise AudioWorklet node on `ctx`. Caller connects it. */
export async function createRnnoiseNode(ctx: AudioContext): Promise<RnnoiseNode> {
  await ensureLoaded(ctx);
  const node = new RnnoiseWorkletNode(ctx, { maxChannels: 1, wasmBinary: wasmBinary! });
  return {
    node,
    dispose: () => {
      try {
        node.disconnect();
        node.destroy();
      } catch {
        /* ignore teardown errors */
      }
    },
  };
}
