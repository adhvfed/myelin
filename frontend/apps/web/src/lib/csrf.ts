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
 * Verify a state-changing request against the configured origin. Prefer `Origin`, fall back to
 * `Referer`, and reject when the expected origin or both request headers are absent.
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
