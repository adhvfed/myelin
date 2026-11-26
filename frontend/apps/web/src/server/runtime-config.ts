import { sessionBackend } from "./session-backend";
import { canonicalPublicOrigin } from "./public-origin";
import { edgeOrigin } from "./edge-origin";
import { oidcClientConfig } from "./oidc-config";
import { checkServerFunctionMount } from "./server-function-mount";

/** Nitro startup plugin: reject a production process before it accepts traffic when durable session
 * storage or a trusted deployment origin/upstream is absent, or the server-function handler is not
 * where the guard expects it. Failing during a streamed auth loader would otherwise strand a
 * partial 200. */
export default function validateRuntimeConfig(nitroApp?: {
  h3App?: { stack?: readonly { route: string }[] };
}): void {
  const app = (globalThis as { app?: { config?: { routers?: { name?: string; base?: string }[] } } })
    .app;
  checkServerFunctionMount(
    app?.config?.routers,
    nitroApp?.h3App?.stack?.map((layer) => layer.route),
  );
  const production = process.env.NODE_ENV === "production";
  sessionBackend(production, process.env.REDIS_URL, process.env.MYELIN_WEB_SESSION_KEY);
  edgeOrigin();
  oidcClientConfig(process.env, production);
  if (production || process.env.MYELIN_PUBLIC_ORIGIN?.trim()) {
    canonicalPublicOrigin({
      production,
      configured: process.env.MYELIN_PUBLIC_ORIGIN,
    });
  }
}
