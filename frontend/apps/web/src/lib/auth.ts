// The session lifecycle as SolidStart server functions (`"use server"` → server-only RPC). The
// dev-login seam mints a real session; the gateway client + session machinery underneath are REAL —
// only the token ISSUANCE is the clearly-marked dev stand-in the deferred OIDC login replaces.
import { action, query, redirect } from "@solidjs/router";
import {
  clearCurrentSession,
  getSessionRecord,
  issueSession,
} from "../server/session";
import { edgeGetPublic, edgeWhoamiWithToken } from "../server/gateway";
import {
  DEV_ACCESS_TOKEN,
  DEV_PRINCIPAL,
  DEV_REFRESH_TOKEN,
  DEV_SCHEME,
} from "../../dev-edge/dev-contract.mjs";
import { devLoginAllowed, type DevLoginEnv } from "./dev-login-guard";
import { runTokenLogin } from "./token-login";
import {
  toAuthConfig,
  type AuthConfig,
  type AuthProvider,
  type EdgeAuthConfig,
} from "./auth-config";

// Re-exported from the pure mapping module so existing importers (`../lib/auth`) are unchanged.
export type { AuthConfig, AuthProvider };

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

/**
 * The logged-out login page's config (R3.5 / OQ-3). Reads the edge's UNAUTHENTICATED
 * `GET /v1/auth/config` for the SSO posture + provider labels, then computes the FRONTEND-
 * authoritative `dev_login_enabled` render gate ({@link devSeamAllowed}) — the frontend owns the dev
 * seam, so its build/env truth wins, with the edge flag as one more required gate. Floor-tolerant: if
 * the edge is unreachable the page renders fail-closed (SSO unavailable, no dev seam) rather than
 * throwing — a logged-out user is never shown a stack trace.
 */
export const getAuthConfig = query(async (): Promise<AuthConfig> => {
  "use server";
  let edge: EdgeAuthConfig = {};
  try {
    edge = await edgeGetPublic<EdgeAuthConfig>("/v1/auth/config");
  } catch {
    // Fail-closed render: no SSO, no dev seam. The login page still renders (honest "unavailable").
    edge = { sso_configured: false, providers: [], dev_login_enabled: false };
  }
  // The mapping (dev-seam composition + the token-login edge flag) is the pure {@link toAuthConfig}.
  return toAuthConfig(edge, process.env as DevLoginEnv, import.meta.env.PROD);
}, "auth-config");

/**
 * **R4.0 — THE OPERATOR-TOKEN LOGIN (the real browser on-ramp for dogfood).** The founder pastes the
 * capability token that `edge bootstrap` printed; this action VERIFIES it server-side against the real
 * edge (`GET /v1/whoami` with the pasted token) and, on a 200, mints a session carrying the token +
 * the whoami-returned principal/tenant/region — then lands in the app. Unlike the dev seam this is NOT
 * a stand-in: it authenticates against the live edge. The token is a SECRET — it flows only as the
 * submitted form value → this server function → the edge; it is never logged, never returned to the
 * client, never placed in a URL. On any failure (invalid/expired token, edge unreachable) the founder
 * is bounced to `/login?error=token_invalid` with NO session and NO leaked edge detail. The decision
 * lives in the pure {@link runTokenLogin} core; this wires the real whoami-verify + session-issue deps.
 */
export const loginWithToken = action(async (formData: FormData) => {
  "use server";
  const token = String(formData.get("token") ?? "");
  const schemeRaw = formData.get("scheme");
  const result = await runTokenLogin(
    token,
    schemeRaw != null ? String(schemeRaw) : undefined,
    {
      verify: (t, s) => edgeWhoamiWithToken(t, s),
      issue: (rec) => issueSession(rec),
    },
  );
  throw redirect(result.redirectTo);
}, "login-with-token");

/**
 * **Begin SSO login (R3.5).** The primary button posts here when SSO is configured. VERIFIED FLOOR:
 * R2.5 landed only the OIDC VERIFICATION half at the edge (`POST /v1/auth/login` validates an ID
 * token the caller already holds) — there is NO browser authorization-code INITIATION route
 * (`/v1/auth/oidc/start` → 302 to the IdP does not exist; the edge config carries issuer/audience/
 * JWKS for verification only, no authorization_endpoint/client_id/redirect_uri). So this action
 * surfaces the honest login-error state (system-blaming, never the user) rather than fabricating a
 * redirect that cannot complete. When the interactive initiation half lands, THIS is the seam it
 * wires into — the button + copy do not change. See `design-planning/09-r3-sketches/05-first-run`.
 */
export const startSso = action(async () => {
  "use server";
  // The interactive OIDC start is not wired at the edge yet (verify-only, R2.5). Blame the
  // deployment's missing initiation surface, not the user — same posture as the CI floor.
  throw redirect("/login?error=sso_start_unavailable");
}, "start-sso");

/** Log out: clear the server-side session + the cookie, return to /login. */
export const logout = action(async () => {
  "use server";
  clearCurrentSession();
  throw redirect("/login");
}, "logout");
