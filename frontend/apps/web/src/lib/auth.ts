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
  edgeMintDevSession,
  edgeWhoami,
  edgeWhoamiWithToken,
  isUnauthorized,
} from "../server/gateway";
import type { EdgeWhoami } from "../server/gateway";
import { beginOidcLogin, interactiveOidcConfigured } from "../server/oidc";
import {
  DEV_ACCESS_TOKEN,
  DEV_PRINCIPAL,
  DEV_REFRESH_TOKEN,
  DEV_SCHEME,
} from "../../dev-edge/dev-contract.mjs";
import { devLoginAllowed, type DevLoginEnv } from "./dev-login-guard";
import {
  AccountError,
  edgeAcceptInvitation,
  edgeCreateInvitation,
  edgeForgotPassword,
  edgeGetInvitation,
  edgePasswordLogin,
  validInvitationCode,
} from "../server/account-gateway";
import type {
  AccountFailure,
  EdgeCreatedInvitation,
  EdgeInvitation,
  EdgeSessionGrant,
} from "../server/account-gateway";
import { authFailureDestination, DEFAULT_AUTH_RETURN_TO, safeAuthReturnTo } from "./auth-return";
import { SESSION_ABSOLUTE_TTL_MS } from "../server/session-store";
import {
  toAuthConfig,
} from "./auth-config";
import type { AuthConfig, AuthProvider, EdgeAuthConfig } from "./auth-config";

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

/** Verify the authenticated app shell's viewer with the edge. `null` means verification is
 * temporarily unavailable and is safe to retry; missing, rejected, or mismatched sessions redirect. */
export const requireViewer = query(async (): Promise<Viewer | null> => {
  "use server";
  const rec = await getSessionRecord();
  if (!rec) throw redirect("/login");

  let who: EdgeWhoami;
  try {
    who = await edgeWhoami();
  } catch (error) {
    if (isUnauthorized(error)) {
      await clearCurrentSession();
      throw redirect("/login");
    }
    return null;
  }

  if (
    who.principal_id !== rec.principalId ||
    who.tenant !== rec.tenant ||
    who.region !== rec.region
  ) {
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
  // The real assembled stack configures an operator bootstrap capability for local development.
  // Exchange it immediately for a HumanSession so browser actions use the same least-privilege
  // purpose as OIDC. The in-process contract edge has no device broker, so its fixed fixture keeps
  // the legacy refresh pair solely inside that test harness.
  const delegated = configuredToken ? await edgeMintDevSession(token, scheme) : null;
  await issueSession({
    token: delegated?.accessToken ?? token,
    refreshToken: delegated ? "" : DEV_REFRESH_TOKEN,
    scheme: delegated?.scheme ?? scheme,
    credentialExpiresAtMs: Math.min(
      (delegated?.expiresAt ?? whoami.expires_at) * 1_000,
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
    // Keep both SSO and development login disabled when config is unavailable.
    edge = { sso_configured: false, providers: [], dev_login_enabled: false };
  }
  // The mapping (dev-seam composition + the edge flags) is the pure {@link toAuthConfig}.
  return toAuthConfig(
    edge,
    process.env as DevLoginEnv,
    import.meta.env.PROD,
    interactiveOidcConfigured(),
  );
}, "auth-config");

/** The region this deployment keeps its data in, for the logged-out pages. Null when unknown. */
export const getDeploymentRegion = query(async (): Promise<string | null> => {
  "use server";
  const region = process.env.MYELIN_REGION?.trim() ?? "";
  return /^[a-z0-9][a-z0-9-]{0,62}$/.test(region) ? region : null;
}, "deployment-region");

/** Turn a fresh human session grant into the browser's server-side session. */
async function signInWithGrant(grant: EdgeSessionGrant): Promise<void> {
  const who = await edgeWhoamiWithToken(grant.accessToken, grant.scheme);
  await issueSession({
    token: grant.accessToken,
    refreshToken: "",
    scheme: grant.scheme,
    credentialExpiresAtMs: Math.min(
      Math.min(grant.expiresAt, who.expires_at) * 1_000,
      Date.now() + SESSION_ABSOLUTE_TTL_MS,
    ),
    principalId: who.principal_id,
    displayName: who.display_name?.trim() || who.principal_id,
    region: who.region,
    tenant: who.tenant,
  });
}

function accountFailure(error: unknown): AccountFailure {
  return error instanceof AccountError ? error.reason : "unavailable";
}

/** Sign in with an email and password. The password is never logged or returned to the client. */
export const loginWithPassword = action(async (formData: FormData) => {
  "use server";
  const email = String(formData.get("email") ?? "").trim();
  const password = String(formData.get("password") ?? "");
  const returnTo = formData.get("return_to");
  let failure: AccountFailure | null = null;
  if (!email || !password || email.length > 320 || password.length > 4_096) {
    failure = "credentials";
  } else {
    try {
      await signInWithGrant(await edgePasswordLogin(email, password));
    } catch (error) {
      failure = accountFailure(error);
    }
  }
  if (failure) throw redirect(authFailureDestination(failure, returnTo));
  throw redirect(safeAuthReturnTo(returnTo));
}, "login-with-password");

/** What the invitation page needs to render: the invitation, or why it cannot be shown. */
export type InvitationView =
  | { state: "ready"; invitation: EdgeInvitation }
  | { state: "invalid" }
  | { state: "unavailable" };

export const getInvitation = query(async (code: string): Promise<InvitationView> => {
  "use server";
  if (!validInvitationCode(code)) return { state: "invalid" };
  try {
    const invitation = await edgeGetInvitation(code);
    return invitation ? { state: "ready", invitation } : { state: "invalid" };
  } catch {
    return { state: "unavailable" };
  }
}, "invitation");

/** Accept an invitation link: create the account or set the new password, then sign in. */
export const acceptInvitation = action(async (formData: FormData) => {
  "use server";
  const code = String(formData.get("code") ?? "");
  if (!validInvitationCode(code)) throw redirect("/login");
  const password = String(formData.get("password") ?? "");
  const rawName = formData.get("display_name");
  const displayName = rawName == null ? undefined : String(rawName).trim();
  let failure: AccountFailure | null = null;
  if (displayName !== undefined && (displayName.length === 0 || displayName.length > 120)) {
    failure = "invalid_name";
  } else {
    try {
      await signInWithGrant(await edgeAcceptInvitation(code, { displayName, password }));
    } catch (error) {
      failure = accountFailure(error);
    }
  }
  if (failure) throw redirect(`/join/${code}?${new URLSearchParams({ error: failure })}`);
  throw redirect(DEFAULT_AUTH_RETURN_TO);
}, "accept-invitation");

export type InvitationResult =
  | ({ ok: true } & EdgeCreatedInvitation)
  | { ok: false; reason: AccountFailure; retryAfterSecs?: number };

/** Invite a teammate. With mail on the edge emails the link; otherwise it is shown once, to be
 * passed on by hand. */
export const createInvitation = action(async (formData: FormData): Promise<InvitationResult> => {
  "use server";
  const email = String(formData.get("email") ?? "").trim();
  if (!email || email.length > 320 || !email.includes("@")) {
    return { ok: false, reason: "invalid_email" };
  }
  try {
    return { ok: true, ...(await edgeCreateInvitation(email)) };
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    const retryAfterSecs = error instanceof AccountError ? error.retryAfterSecs : undefined;
    return { ok: false, reason: accountFailure(error), ...(retryAfterSecs ? { retryAfterSecs } : {}) };
  }
}, "create-invitation");

export type PasswordResetRequest =
  | { ok: true; email: string }
  | { ok: false; reason: AccountFailure };

/** Ask for a reset link by email. The answer is the same whether or not the address has an
 * account, so the page can only say that a link is on its way if one exists. */
export const requestPasswordReset = action(
  async (formData: FormData): Promise<PasswordResetRequest> => {
    "use server";
    const email = String(formData.get("email") ?? "").trim();
    if (!email || email.length > 320 || !email.includes("@")) {
      return { ok: false, reason: "invalid_email" };
    }
    try {
      await edgeForgotPassword(email);
      return { ok: true, email };
    } catch (error) {
      return { ok: false, reason: accountFailure(error) };
    }
  },
  "request-password-reset",
);

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
