// CSRF origin verification for every unsafe browser request.
//
// SameSite=Lax already withholds cookies from cross-site unsafe requests in current browsers. Origin
// verification remains necessary defense in depth against same-site sibling origins, regressions,
// and login CSRF. Middleware applies this pure decision to POST/PUT/PATCH/DELETE globally.

/** The request signals the verdict is computed from (all optional — a browser POST sends at least one). */
export interface OriginSignals {
  /** The `Origin` request header (the reliable signal on a POST). */
  origin: string | null;
  /** The `Referer` request header (the fallback when `Origin` is absent). */
  referer: string | null;
  /** The deployment's canonical origin (scheme + host + port). */
  expectedOrigin: string | null;
}

/** `"ok"` = same-origin (or provably not a cross-site forgery); `"reject"` = cross-site / indeterminate. */
export type OriginVerdict = "ok" | "reject";
export type RequestMethodPolicy = "safe" | "verify-origin" | "reject";

/** Safe methods bypass CSRF verification; every other method is unsafe by default. */
export function requestMethodPolicy(method: string): RequestMethodPolicy {
  const normalized = method.toUpperCase();
  if (normalized === "GET" || normalized === "HEAD" || normalized === "OPTIONS") return "safe";
  if (normalized === "TRACE") return "reject";
  return "verify-origin";
}

/** Parse the serialized origin out of an absolute URL; `null` for opaque or invalid origins. */
function originOf(url: string): string | null {
  try {
    const parsed = new URL(url);
    return parsed.origin === "null" ? null : parsed.origin;
  } catch {
    return null;
  }
}

/**
 * The CSRF origin decision for a state-changing (session-minting) POST.
 *
 * - No canonical origin → we cannot compare → **reject**.
 * - `Origin` present → its full origin MUST match, else **reject** (the primary check; browsers always
 *   send `Origin` on a cross-site form POST, so a mismatch is a forgery).
 * - `Origin` absent, `Referer` present → the `Referer` origin MUST match, else **reject** (some
 *   clients omit `Origin`; `Referer` is the fallback same-site witness).
 * - Both absent → **reject** (a browser form POST sends at least one; absence is suspicious — a
 *   non-browser/forged request. Fail-closed: a legitimate login is always browser-initiated).
 */
export function sameOriginVerdict(sig: OriginSignals): OriginVerdict {
  const expectedOrigin = sig.expectedOrigin ? originOf(sig.expectedOrigin) : null;
  if (!expectedOrigin) return "reject";
  if (sig.origin) {
    return originOf(sig.origin) === expectedOrigin ? "ok" : "reject";
  }
  if (sig.referer) {
    return originOf(sig.referer) === expectedOrigin ? "ok" : "reject";
  }
  return "reject";
}
