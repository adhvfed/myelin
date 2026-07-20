import { randomBytes } from "node:crypto";

import { createMiddleware } from "@solidjs/start/middleware";

import { securityHeaders } from "~/lib/security-headers";

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
  },
  onBeforeResponse: (event) => {
    const nonce = event.locals.cspNonce;
    if (nonce) applySecurityHeaders(event.response.headers, nonce);
  },
});
