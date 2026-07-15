// The session lifecycle as SolidStart server functions (`"use server"` → server-only RPC). The
// dev-login seam mints a real session; the gateway client + session machinery underneath are REAL —
// only the token ISSUANCE is the clearly-marked dev stand-in the deferred OIDC login replaces.
import { action, query, redirect } from "@solidjs/router";
import {
  clearCurrentSession,
  getSessionRecord,
  issueSession,
} from "../server/session";
import {
  DEV_ACCESS_TOKEN,
  DEV_PRINCIPAL,
  DEV_REFRESH_TOKEN,
  DEV_SCHEME,
} from "../../dev-edge/dev-contract.mjs";
import { devLoginAllowed } from "./dev-login-guard";

/** The PII-free viewer facts the chrome renders (identity menu + residency cue). Null = no session. */
export interface Viewer {
  principalId: string;
  displayName: string;
  tenant: string;
  region: string;
}

/** Read the current viewer from the httpOnly-cookie session (server-only). */
export const getViewer = query(async (): Promise<Viewer | null> => {
  "use server";
  const rec = getSessionRecord();
  if (!rec) return null;
  return {
    principalId: rec.principalId,
    displayName: rec.displayName,
    tenant: rec.tenant,
    region: rec.region,
  };
}, "viewer");

/** Like `getViewer`, but for the authenticated app layout: a missing session throws a `/login`
 *  redirect (the auth guard the whole app shell sits behind). */
export const requireViewer = query(async (): Promise<Viewer> => {
  "use server";
  const rec = getSessionRecord();
  if (!rec) throw redirect("/login");
  return {
    principalId: rec.principalId,
    displayName: rec.displayName,
    tenant: rec.tenant,
    region: rec.region,
  };
}, "require-viewer");

/**
 * R0.6 fail-closed guard on the dev-login seam (review fe-web #1 — "dev-login mints a full session
 * with no environment guard", a full app-shell auth bypass if this bundle is ever deployed). The
 * seam may mint a session ONLY when BOTH conditions hold — two INDEPENDENT gates so no single
 * misconfiguration re-opens it:
 *   1. the build is NOT production (`NODE_ENV !== "production"`), AND
 *   2. dev-login is EXPLICITLY opted in (`MYELIN_DEV_LOGIN === "1"`).
 * A production build refuses outright; a non-production build that forgot the flag also refuses.
 * Refusal is LOUD (server-side warn) and fail-closed (redirect to /login, no session minted). When
 * the real OIDC login lands (R2.5) this whole seam is deleted; until then this guard is what keeps a
 * forgotten dev seam from authenticating anyone at exposure time.
 */
function assertDevLoginAllowed(): void {
  if (!devLoginAllowed(process.env)) {
    // Loud, server-side audit line — a refusal here means the seam was reachable in a posture it
    // must never mint sessions in (production build, or non-prod without the explicit opt-in).
    console.warn(
      `[R0.6] dev-login REFUSED: NODE_ENV=${process.env.NODE_ENV ?? "<unset>"} ` +
        `MYELIN_DEV_LOGIN=${process.env.MYELIN_DEV_LOGIN ?? "<unset>"} — no session minted.`,
    );
    throw redirect("/login");
  }
}

/**
 * THE DEV-LOGIN SEAM (clearly marked — NOT production auth). Mints a session carrying the well-known
 * dev token the dev edge accepts, then redirects into the app. The real OIDC/human login (MR-012
 * deferred — the edge's `POST /v1/auth/login` REFUSES, refuse-not-mock) replaces THIS function; the
 * session store, the httpOnly cookie, and the gateway client it feeds are all real and unchanged.
 * Guarded by {@link assertDevLoginAllowed} (R0.6) so it cannot mint a session once deployed.
 */
export const loginDev = action(async () => {
  "use server";
  // R2.5 — BUILD-TIME kill switch (defense-in-depth ON TOP of the R0.6 runtime guard below). In a
  // production BUILD Vite replaces `import.meta.env.PROD` with the literal `true`, so everything
  // after this `throw` is statically UNREACHABLE — the bundler dead-code-eliminates the dev-token
  // issuance and tree-shakes the now-unused `dev-contract` import (the well-known `DEV_ACCESS_TOKEN`)
  // OUT of the production bundle. The seam is thus structurally absent in prod, not merely refused at
  // runtime. In a dev/test build `import.meta.env.PROD` is `false`, so the seam is retained and the
  // runtime R0.6 guard (`assertDevLoginAllowed`) still gates it on `MYELIN_DEV_LOGIN`.
  if (import.meta.env.PROD) {
    throw redirect("/login");
  }
  assertDevLoginAllowed();
  issueSession({
    token: DEV_ACCESS_TOKEN,
    refreshToken: DEV_REFRESH_TOKEN,
    scheme: DEV_SCHEME,
    principalId: DEV_PRINCIPAL.principalId,
    displayName: DEV_PRINCIPAL.displayName,
    region: DEV_PRINCIPAL.region,
    tenant: DEV_PRINCIPAL.tenant,
  });
  throw redirect("/git/repos");
}, "login-dev");

/** Log out: clear the server-side session + the cookie, return to /login. */
export const logout = action(async () => {
  "use server";
  clearCurrentSession();
  throw redirect("/login");
}, "logout");
