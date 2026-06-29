import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// Vite config for the Tauri 2 desktop+mobile shell. Tauri serves this build output (dist/)
// in the native webview; in dev it points the webview at the vite dev server on :1420.
// The `TAURI_DEV_HOST` env is set by `tauri dev` for the mobile target (the webview on a
// device/emulator must reach the dev server over the LAN, not localhost).
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [solid()],
  // Tauri owns the terminal output; don't let vite clear it.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    // src-tauri churns (the Rust build); don't trigger HMR on it.
    watch: { ignored: ["**/src-tauri/**"] },
  },
  // Modern webviews only (Tauri ships a current WebKit/WebView2) — no legacy transpile.
  build: { target: "esnext" },
});
