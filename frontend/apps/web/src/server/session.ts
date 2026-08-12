// Server-side session storage. The HTTP-only cookie contains an opaque ID; credentials and identity
// fields remain in the server-side store. Deployed instances use Valkey, while local development can
// use the in-memory backend.

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

/** Keep one store reference across Vinxi's separate SSR and server-function module graphs. */
const globalStore = globalThis as unknown as {
  __myelinSessionStore?: SessionStore;
};
const store = (globalStore.__myelinSessionStore ??= createSessionStore(
  sessionBackend(
    import.meta.env.PROD,
    process.env.REDIS_URL,
    process.env.MYELIN_WEB_SESSION_KEY,
  ),
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
export async function updateSessionToken(token: string): Promise<boolean> {
  const id = getCookie(SESSION_COOKIE);
  if (!id || !validSessionId(id)) return false;
  return await store.updateToken(id, token);
}

/** Issue a session: store the record server-side, set the httpOnly cookie carrying ONLY the opaque id. */
export async function issueSession(rec: SessionRecord): Promise<string> {
  const priorId = getCookie(SESSION_COOKIE);
  const id = freshId();
  // Re-authentication issues the replacement and revokes the prior id atomically. An outage cannot
  // delete the working session first and then fail before creating its replacement.
  if (priorId && validSessionId(priorId)) await store.rotate(priorId, id, rec);
  else await store.issue(id, rec);
  setCookie(SESSION_COOKIE, id, {
    httpOnly: true,
    // `lax` permits top-level deep links while withholding the cookie from cross-site subrequests.
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
  try {
    if (id && validSessionId(id)) await store.delete(id);
  } finally {
    // Local cookie removal must not depend on Valkey availability. The deletion error still
    // propagates so operators can observe failed server-side revocation.
    deleteCookie(SESSION_COOKIE, {
      httpOnly: true,
      sameSite: "lax",
      path: "/",
      secure: cookie.secure,
    });
  }
}
