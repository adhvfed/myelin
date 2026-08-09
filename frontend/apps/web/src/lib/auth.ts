// The session lifecycle as SolidStart server functions (`"use server"` → server-only RPC). The
// dev-login seam mints a real local session; production uses the OIDC authorization-code flow below.
// The gateway client + session machinery underneath are shared by both paths. The
// global middleware verifies the full Origin of every unsafe browser request before any action runs.
import { action, query, redirect } from "@solidjs/router";
import {
  clearCurrentSession,
  getSessionRecord,
  issueSession,
} from "../server/session";
import {
  edgeGetPublic,
  edgeWhoami,
  edgeWhoamiWithToken,
  isUnauthorized,
} from "../server/gateway";
import { beginOidcLogin, interactiveOidcConfigured } from "../server/oidc";
import {
  DEV_ACCESS_TOKEN,
  DEV_PRINCIPAL,
  DEV_REFRESH_TOKEN,
  DEV_SCHEME,
} from "../../dev-edge/dev-contract.mjs";
import { devLoginAllowed, type DevLoginEnv } from "./dev-login-guard";
import {
  runTokenLogin,
  TOKEN_LOGIN_DISABLED,
  TOKEN_LOGIN_ERROR,
  TOKEN_LOGIN_SUCCESS,
} from "./token-login";
import {
  authFailureDestination,
  authenticationDestination,
  safeAuthReturnTo,
} from "./auth-return";
import { SESSION_ABSOLUTE_TTL_MS } from "../server/session-store";
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
  const rec = await getSessionRecord();
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
  const rec = await getSessionRecord();
  if (!rec) throw redirect("/login");
  try {
    const who = await edgeWhoami();
    if (
      who.principal_id !== rec.principalId ||
      who.tenant !== rec.tenant ||
      who.region !== rec.region
    ) {
      await clearCurrentSession();
      throw redirect("/login");
    }
  } catch (error) {
    if (!isUnauthorized(error)) throw error;
    await clearCurrentSession();
    throw redirect("/login");
  }
  return {
    principalId: rec.principalId,
    displayName: rec.displayName,
    tenant: rec.tenant,
    region: rec.region,
  };
}, "require-viewer");

/** Development login is available only in a non-production build with an explicit opt-in. */
function assertDevLoginAllowed(): void {
  if (!devLoginAllowed(process.env)) {
    console.warn(
      `Development login is disabled (NODE_ENV=${process.env.NODE_ENV ?? "<unset>"}, ` +
        `MYELIN_DEV_LOGIN=${process.env.MYELIN_DEV_LOGIN ?? "<unset>"}).`,
    );
    throw redirect("/login");
  }
}

/** Verify the configured development capability with the edge, then create a server-side session. */
export const loginDev = action(async (formData: FormData) => {
  "use server";
  if (import.meta.env.PROD) {
    throw redirect("/login");
  }
  assertDevLoginAllowed();
  const configuredToken = process.env.MYELIN_DEV_ACCESS_TOKEN?.trim();
  const token = configuredToken || DEV_ACCESS_TOKEN;
  const scheme = process.env.MYELIN_DEV_TOKEN_SCHEME?.trim() || DEV_SCHEME;
  const whoami = await edgeWhoamiWithToken(token, scheme);
  await issueSession({
    token,
    refreshToken: configuredToken ? "" : DEV_REFRESH_TOKEN,
    scheme,
    credentialExpiresAtMs: Math.min(
      whoami.expires_at * 1_000,
      Date.now() + SESSION_ABSOLUTE_TTL_MS,
    ),
    principalId: whoami.principal_id,
    displayName: process.env.MYELIN_DEV_DISPLAY_NAME?.trim() || DEV_PRINCIPAL.displayName,
    region: whoami.region,
    tenant: whoami.tenant,
  });
  throw redirect(safeAuthReturnTo(formData.get("return_to")));
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
  let edge: unknown = {};
  try {
    edge = await edgeGetPublic<EdgeAuthConfig>("/v1/auth/config");
  } catch {
    // Fail-closed render: no SSO, no dev seam. The login page still renders (honest "unavailable").
    edge = { sso_configured: false, providers: [], dev_login_enabled: false };
  }
  // The mapping (dev-seam composition + the token-login edge flag) is the pure {@link toAuthConfig}.
  return toAuthConfig(
    edge,
    process.env as DevLoginEnv,
    import.meta.env.PROD,
    interactiveOidcConfigured(),
  );
}, "auth-config");

/**
 * Verify a pasted capability token against the edge, then issue a server-side session carrying the
 * returned principal, tenant, and region. The token is never logged, returned to the client, or put
 * in a URL. The auth mode is checked again here because actions can be invoked without rendering the
 * login form first.
 */
export const loginWithToken = action(async (formData: FormData) => {
  "use server";
  const token = String(formData.get("token") ?? "");
  const schemeRaw = formData.get("scheme");
  const result = await runTokenLogin(
    token,
    schemeRaw != null ? String(schemeRaw) : undefined,
    {
      isEnabled: async () => {
        const config = await edgeGetPublic<EdgeAuthConfig>("/v1/auth/config");
        return config.token_login_enabled === true;
      },
      verify: (t, s) => edgeWhoamiWithToken(t, s),
      issue: async (rec) => { await issueSession(rec); },
    },
  );
  const returnTo = formData.get("return_to");
  if (result.redirectTo === TOKEN_LOGIN_SUCCESS) {
    throw redirect(safeAuthReturnTo(returnTo));
  }
  if (result.redirectTo === TOKEN_LOGIN_ERROR) {
    throw redirect(authFailureDestination("token_invalid", returnTo));
  }
  if (result.redirectTo === TOKEN_LOGIN_DISABLED) {
    throw redirect(authenticationDestination(returnTo));
  }
  throw redirect("/login");
}, "login-with-token");

/**
 * Begin a one-time OIDC Authorization Code + S256 PKCE transaction. The mode is re-read from both
 * the edge and local server configuration so invoking the action directly cannot bypass the render
 * gate. State, nonce, and verifier remain server-side; only the opaque state cookie reaches the
 * browser.
 */
export const startSso = action(async (formData: FormData) => {
  "use server";
  const returnTo = formData.get("return_to");
  let destination: string;
  try {
    const edge = await edgeGetPublic<EdgeAuthConfig>("/v1/auth/config");
    if (edge.sso_configured !== true || !interactiveOidcConfigured()) {
      throw new Error("SSO is unavailable");
    }
    destination = await beginOidcLogin(returnTo);
  } catch {
    destination = authFailureDestination("sso_start_unavailable", returnTo);
  }
  throw redirect(destination);
}, "start-sso");

/** Log out: clear the server-side session + the cookie, return to /login. */
export const logout = action(async () => {
  "use server";
  await clearCurrentSession();
  throw redirect("/login");
}, "logout");
