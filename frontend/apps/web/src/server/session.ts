// The server-side httpOnly-cookie SESSION machinery (doc 10 §5 / mirrors crates/myelin-edge/src/
// session.rs). This is REAL: the cookie carries ONLY an opaque session id; the Bearer token + the
// principal facts live in a SERVER-SIDE store keyed by that id. So **tokens never reach client JS** —
// `document.cookie` holds only the opaque, httpOnly id, and the token is never serialised to the page.
//
// Production records live in region-local Valkey so every web replica sees the same revocation and
// rotation state. The in-memory implementation is retained only for hermetic local development.
// The dev session is MINTED by the explicitly gated dev-login seam; it is NOT production auth.

import { getCookie, setCookie, deleteCookie } from "vinxi/http";
import { randomBytes } from "node:crypto";
import type { SessionRecord, SessionStore } from "./session-store";
import { sessionCookieSettings } from "./session-cookie";
import { createSessionStore, sessionBackend } from "./session-backend";
import { validSessionId } from "./session-id";

export type { SessionRecord } from "./session-store";

/** Production uses the cookie-prefix contract: Secure, host-only, and Path=/. Local HTTP uses the
 * unprefixed name because browsers correctly reject a `__Host-` cookie without Secure. */
const cookie = sessionCookieSettings(import.meta.env.PROD);
export const SESSION_COOKIE = cookie.name;

/** Backed on `globalThis` so the SSR bundle and the server-functions bundle — which vinxi/Nitro loads
 * as SEPARATE module graphs in the SAME process — share ONE store. Without this, a session written by a
 * server action (server-fns bundle) is invisible to the SSR `requireViewer` on a full-reload/deep-link
 * navigation. In production that process-local reference points to the shared Valkey transport. */
const globalStore = globalThis as unknown as {
  __myelinSessionStore?: SessionStore;
};
const store = (globalStore.__myelinSessionStore ??= createSessionStore(
  sessionBackend(import.meta.env.PROD, process.env.REDIS_URL),
));

/** Generate an opaque, unguessable session id (CSPRNG — the production-grade id the Rust model flagged). */
function freshId(): string {
  return `sess_${randomBytes(24).toString("base64url")}`;
}

export async function sessionStoreReady(): Promise<void> {
  await store.ready();
}

/** Read the current request's session record (via the httpOnly cookie), or null. Server-only. */
export async function getSessionRecord(): Promise<SessionRecord | null> {
  const id = getCookie(SESSION_COOKIE);
  if (!id || !validSessionId(id)) return null;
  return await store.get(id);
}

/** Persist a (possibly rotated) token onto the current session, if one exists. Server-only. */
export async function updateSessionToken(token: string): Promise<void> {
  const id = getCookie(SESSION_COOKIE);
  if (!id || !validSessionId(id)) return;
  await store.updateToken(id, token);
}

/** Issue a session: store the record server-side, set the httpOnly cookie carrying ONLY the opaque id. */
export async function issueSession(rec: SessionRecord): Promise<string> {
  // Re-authentication rotates the browser session and revokes the prior id immediately. A copied old
  // cookie cannot remain live until its TTL merely because the same browser signed in again.
  const priorId = getCookie(SESSION_COOKIE);
  if (priorId && validSessionId(priorId)) await store.delete(priorId);

  const id = freshId();
  await store.issue(id, rec);
  setCookie(SESSION_COOKIE, id, {
    httpOnly: true,
    // `lax` (not `strict`): the session cookie MUST ride a top-level deep-link/full-reload navigation
    // (e.g. opening `/git/repos/{repo}/prs/{n}` directly), which `strict` suppresses — while `lax`
    // still withholds it from cross-site sub-requests, so the CSRF posture holds. (httpOnly keeps the
    // opaque id out of client JS regardless.)
    sameSite: "lax",
    path: "/",
    // Secure in production; relaxed for the http dev/test harness so the cookie is actually set.
    secure: cookie.secure,
    maxAge: cookie.maxAgeSeconds,
  });
  return id;
}

/** Clear the current session (logout / dead session): drop the record AND the cookie. Idempotent. */
export async function clearCurrentSession(): Promise<void> {
  const id = getCookie(SESSION_COOKIE);
  if (id && validSessionId(id)) await store.delete(id);
  deleteCookie(SESSION_COOKIE, {
    httpOnly: true,
    sameSite: "lax",
    path: "/",
    secure: cookie.secure,
  });
}
