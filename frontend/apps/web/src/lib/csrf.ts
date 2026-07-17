// Peer-review finding 2026-07-16 #21c — CSRF origin verification for the session-MINTING login actions.
//
// SolidStart server actions are `POST`ed from a `<form action={...}>`. A cookie with `SameSite=Lax`
// is STILL SENT on a top-level cross-site form POST, so `SameSite=Lax` alone does NOT stop a malicious
// site from submitting our `login` form and logging the victim in as the ATTACKER (a login-CSRF /
// session-fixation vector). The classic stateless defense is to VERIFY THE ORIGIN: a browser sends the
// `Origin` header on every POST (and `Referer` as a fallback); a cross-site POST carries an origin that
// is NOT our own host, so we reject it. This is the pure, testable decision; `auth.ts` wires the real
// request headers onto it (via `getRequestEvent`).

/** The request signals the verdict is computed from (all optional — a browser POST sends at least one). */
export interface OriginSignals {
  /** The `Origin` request header (the reliable signal on a POST). */
  origin: string | null;
  /** The `Referer` request header (the fallback when `Origin` is absent). */
  referer: string | null;
  /** The `Host` request header — the site's own host the origin must match. */
  host: string | null;
}

/** `"ok"` = same-origin (or provably not a cross-site forgery); `"reject"` = cross-site / indeterminate. */
export type OriginVerdict = "ok" | "reject";

/** Parse the host (`host:port`) out of an absolute URL; `null` if it is not a parseable absolute URL. */
function hostOf(url: string): string | null {
  try {
    return new URL(url).host;
  } catch {
    return null;
  }
}

/**
 * The CSRF origin decision for a state-changing (session-minting) POST.
 *
 * - No `Host` → we cannot compare → **reject** (fail-closed; a real request always carries `Host`).
 * - `Origin` present → its host MUST equal `Host`, else **reject** (the primary check; browsers always
 *   send `Origin` on a cross-site form POST, so a mismatch is a forgery).
 * - `Origin` absent, `Referer` present → the `Referer` host MUST equal `Host`, else **reject** (some
 *   clients omit `Origin`; `Referer` is the fallback same-site witness).
 * - Both absent → **reject** (a browser form POST sends at least one; absence is suspicious — a
 *   non-browser/forged request. Fail-closed: a legitimate login is always browser-initiated).
 */
export function sameOriginVerdict(sig: OriginSignals): OriginVerdict {
  const host = sig.host;
  if (!host) return "reject";
  if (sig.origin) {
    // `Origin: null` (an opaque origin — e.g. a sandboxed iframe, a `data:`/cross-site redirect POST)
    // never matches our host, so it correctly rejects.
    return hostOf(sig.origin) === host ? "ok" : "reject";
  }
  if (sig.referer) {
    return hostOf(sig.referer) === host ? "ok" : "reject";
  }
  return "reject";
}
