import { randomBytes } from "node:crypto";

import { createMiddleware } from "@solidjs/start/middleware";

import { requestMethodPolicy, sameOriginVerdict } from "~/lib/csrf";
import { securityHeaders } from "~/lib/security-headers";
import { canonicalPublicOrigin } from "~/server/public-origin";

const production = process.env.NODE_ENV === "production";
const hsts = process.env.MYELIN_HSTS === "1";

function applySecurityHeaders(headers: Headers, nonce: string): void {
  for (const [name, value] of Object.entries(securityHeaders({ hsts, nonce, production }))) {
    // Route handlers may deliberately choose a more specific cache policy.
    if (name === "Cache-Control" && headers.has(name)) continue;
    headers.set(name, value);
  }
}

export default createMiddleware({
  onRequest: (event) => {
    const nonce = randomBytes(16).toString("base64");
    event.locals.cspNonce = nonce;
    applySecurityHeaders(event.response.headers, nonce);

    const methodPolicy = requestMethodPolicy(event.request.method);
    if (methodPolicy === "reject") {
      event.response.headers.set("Content-Type", "text/plain; charset=utf-8");
      event.response.headers.set("Allow", "GET, HEAD, OPTIONS");
      return new Response("method not allowed", {
        status: 405,
        headers: event.response.headers,
      });
    }
    if (methodPolicy === "verify-origin") {
      const expectedOrigin = canonicalPublicOrigin({
        production,
        configured: process.env.MYELIN_PUBLIC_ORIGIN,
        requestUrl: event.request.url,
      });
      const verdict = sameOriginVerdict({
        origin: event.request.headers.get("origin"),
        referer: event.request.headers.get("referer"),
        expectedOrigin,
      });
      if (verdict !== "ok") {
        event.response.headers.set("Content-Type", "text/plain; charset=utf-8");
        return new Response("forbidden", {
          status: 403,
          headers: event.response.headers,
        });
      }
    }
  },
  onBeforeResponse: (event) => {
    const nonce = event.locals.cspNonce;
    if (nonce) applySecurityHeaders(event.response.headers, nonce);
  },
});
