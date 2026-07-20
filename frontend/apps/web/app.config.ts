import { defineConfig } from "@solidjs/start/config";

// SolidStart app config (MR-019). The web target gets SSR (preset: node) — this is what earns the
// server-side cookie-auth gateway its keep: the session + the Bearer token live ONLY on the server
// (doc 10 §5), so tokens never reach client JS. The same Solid app is wrapped by the MR-018 Tauri
// shell for desktop/mobile; only the web target runs this SolidStart server.
export default defineConfig({
  middleware: "src/middleware.ts",
  // The default compact serializer evaluates its payload in the browser. JSON is compatible with
  // the nonce-based production CSP and keeps `unsafe-eval` out of the shipped policy.
  serialization: { mode: "json" },
  server: {
    preset: "node-server",
    routeRules: {
      // Hashed production assets already receive Nitro's immutable one-year cache policy. Add the
      // browser boundary headers here because SolidStart middleware does not run on the static router.
      "/_build/**": {
        headers: {
          "Cross-Origin-Resource-Policy": "same-origin",
          "X-Content-Type-Options": "nosniff",
        },
      },
    },
  },
  vite: {
    // The design-system ships .ts/.tsx source (workspace package); let vite transform it.
    ssr: { noExternal: ["@myelin/design-system"] },
  },
});
