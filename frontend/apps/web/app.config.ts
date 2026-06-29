import { defineConfig } from "@solidjs/start/config";

// SolidStart app config (MR-019). The web target gets SSR (preset: node) — this is what earns the
// server-side cookie-auth gateway its keep: the session + the Bearer token live ONLY on the server
// (doc 10 §5), so tokens never reach client JS. The same Solid app is wrapped by the MR-018 Tauri
// shell for desktop/mobile; only the web target runs this SolidStart server.
export default defineConfig({
  server: {
    preset: "node-server",
  },
  vite: {
    // The design-system ships .ts/.tsx source (workspace package); let vite transform it.
    ssr: { noExternal: ["@myelin/design-system"] },
  },
});
