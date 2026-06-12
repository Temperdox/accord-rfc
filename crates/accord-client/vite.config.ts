import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// Vite config tuned for Tauri (see https://tauri.app).
// - Fixed port 1420 so tauri.conf.json's devUrl is stable.
// - clearScreen:false keeps Rust compiler output visible alongside Vite.
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  // tauri.conf.json points frontendDist at this `dist/`.
  build: {
    outDir: "dist",
    target: "esnext",
  },
});
