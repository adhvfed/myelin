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

/** The session cookie name (httpOnly; carries ONLY the opaque id, never the token). */
export const SESSION_COOKIE = "myelin_session";

/** The server-side session record (NEVER exposed to client JS). */
export interface SessionRecord {
  /** The server-side access token (Bearer material) — server-only. */
  token: string;
  /** The refresh credential used for the single-refresh round-trip — server-only. */
  refreshToken: string;
  /** The credential scheme the token authenticates under (sent as `x-myelin-token-scheme`). */
  scheme: string;
  /** The verified principal id (PII-free id, for the identity menu). */
  principalId: string;
  /** A display label for the identity menu. */
  displayName: string;
  /** The data-residency region (drives the residency cue in the chrome). */
  region: string;
  /** The operating tenant. */
  tenant: string;
}

/** The in-memory session store (the model; durable backing is the named floor). */
const store = new Map<string, SessionRecord>();

/** Generate an opaque, unguessable session id (CSPRNG — the production-grade id the Rust model flagged). */
function freshId(): string {
  return `sess_${randomBytes(24).toString("base64url")}`;
}

/** Read the current request's session record (via the httpOnly cookie), or null. Server-only. */
export function getSessionRecord(): SessionRecord | null {
  const id = getCookie(SESSION_COOKIE);
  if (!id) return null;
  return store.get(id) ?? null;
}

/** Persist a (possibly rotated) token onto the current session, if one exists. Server-only. */
export function updateSessionToken(token: string): void {
  const id = getCookie(SESSION_COOKIE);
  if (!id) return;
  const rec = store.get(id);
  if (rec) store.set(id, { ...rec, token });
}

/** Issue a session: store the record server-side, set the httpOnly cookie carrying ONLY the opaque id. */
export function issueSession(rec: SessionRecord): string {
  const id = freshId();
  store.set(id, rec);
  setCookie(SESSION_COOKIE, id, {
    httpOnly: true,
    sameSite: "strict",
    path: "/",
    // Secure in production; relaxed for the http dev/test harness so the cookie is actually set.
    secure: process.env.NODE_ENV === "production",
    maxAge: 60 * 60 * 8,
  });
  return id;
}

/** Clear the current session (logout / dead session): drop the record AND the cookie. Idempotent. */
export function clearCurrentSession(): void {
  const id = getCookie(SESSION_COOKIE);
  if (id) store.delete(id);
  deleteCookie(SESSION_COOKIE, { path: "/" });
}
