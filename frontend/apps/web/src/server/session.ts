// The server-side httpOnly-cookie SESSION machinery (doc 10 §5 / mirrors crates/myelin-edge/src/
// session.rs). This is REAL: the cookie carries ONLY an opaque session id; the Bearer token + the
// principal facts live in a SERVER-SIDE store keyed by that id. So **tokens never reach client JS** —
// `document.cookie` holds only the opaque, httpOnly id, and the token is never serialised to the page.
//
// FLOORS NAMED (honest): (1) the store is an in-memory Map — the model, exactly like the Rust
// SessionStore; a durable backing (Valkey/PG) is the named follow-on, the cookie/lookup SEMANTICS are
// complete now. (2) the dev session is MINTED by the dev-login seam (`createDevSession`) — the
// clearly-marked stand-in the deferred real OIDC login replaces; it is NOT production auth.

import { getCookie, setCookie, deleteCookie } from "vinxi/http";
import { randomBytes } from "node:crypto";
import {
  MemorySessionStore,
  type SessionRecord,
} from "./session-store";
import { sessionCookieSettings } from "./session-cookie";

export type { SessionRecord } from "./session-store";

/** Production uses the cookie-prefix contract: Secure, host-only, and Path=/. Local HTTP uses the
 * unprefixed name because browsers correctly reject a `__Host-` cookie without Secure. */
const cookie = sessionCookieSettings(import.meta.env.PROD);
export const SESSION_COOKIE = cookie.name;

/** The in-memory session store (the model; durable backing is the named floor).
 *
 * Backed on `globalThis` so the SSR bundle and the server-functions bundle — which vinxi/Nitro loads
 * as SEPARATE module graphs in the SAME process — share ONE store. Without this, a session written by a
 * server action (server-fns bundle) is invisible to the SSR `requireViewer` on a full-reload/deep-link
 * navigation, so every app route would bounce to `/login` on refresh. (A durable backing — Valkey/PG —
 * is the named follow-on; this keeps the in-memory model honest while making deep-links/refresh work.) */
const globalStore = globalThis as unknown as {
  __myelinSessionStore?: MemorySessionStore;
};
const store = (globalStore.__myelinSessionStore ??= new MemorySessionStore());

/** Generate an opaque, unguessable session id (CSPRNG — the production-grade id the Rust model flagged). */
function freshId(): string {
  return `sess_${randomBytes(24).toString("base64url")}`;
}

/** Read the current request's session record (via the httpOnly cookie), or null. Server-only. */
export function getSessionRecord(): SessionRecord | null {
  const id = getCookie(SESSION_COOKIE);
  if (!id) return null;
  return store.get(id);
}

/** Persist a (possibly rotated) token onto the current session, if one exists. Server-only. */
export function updateSessionToken(token: string): void {
  const id = getCookie(SESSION_COOKIE);
  if (!id) return;
  store.updateToken(id, token);
}

/** Issue a session: store the record server-side, set the httpOnly cookie carrying ONLY the opaque id. */
export function issueSession(rec: SessionRecord): string {
  // Re-authentication rotates the browser session and revokes the prior id immediately. A copied old
  // cookie cannot remain live until its TTL merely because the same browser signed in again.
  const priorId = getCookie(SESSION_COOKIE);
  if (priorId) store.delete(priorId);

  const id = freshId();
  store.issue(id, rec);
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
export function clearCurrentSession(): void {
  const id = getCookie(SESSION_COOKIE);
  if (id) store.delete(id);
  deleteCookie(SESSION_COOKIE, {
    httpOnly: true,
    sameSite: "lax",
    path: "/",
    secure: cookie.secure,
  });
}
