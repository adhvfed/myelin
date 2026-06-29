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
 * THE DEV-LOGIN SEAM (clearly marked — NOT production auth). Mints a session carrying the well-known
 * dev token the dev edge accepts, then redirects into the app. The real OIDC/human login (MR-012
 * deferred — the edge's `POST /v1/auth/login` REFUSES, refuse-not-mock) replaces THIS function; the
 * session store, the httpOnly cookie, and the gateway client it feeds are all real and unchanged.
 */
export const loginDev = action(async () => {
  "use server";
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
