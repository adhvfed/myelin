import { sessionBackend } from "./session-backend";
import { canonicalPublicOrigin } from "./public-origin";
import { edgeOrigin } from "./edge-origin";
import { oidcClientConfig } from "./oidc-config";

/** Nitro startup plugin: reject a production process before it accepts traffic when durable session
 * storage or a trusted deployment origin/upstream is absent. Failing during a streamed auth loader
 * would otherwise strand a partial 200. */
export default function validateRuntimeConfig(): void {
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
