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

/** What a request to the server-function endpoint shows about where it came from. */
export interface ServerFunctionRead {
  method: string;
  pathname: string;
  /** The `X-Server-Instance` header SolidStart's own client sends on every call. */
  instance: string | null;
  /** The browser's `Sec-Fetch-Site`, when it sends one. */
  fetchSite: string | null;
}

/**
 * SolidStart runs any server function, actions included, for `GET /_server?id=…&name=…`, and the
 * origin check above covers only unsafe methods. The app's own reads (router queries) are GETs
 * too, but always fetched with SolidStart's custom headers, which a cross-site link or form cannot
 * add. So a read there must carry `X-Server-Instance` and, when the browser says, be same-origin.
 */
export function refusesServerFunctionRead(read: ServerFunctionRead): boolean {
  const method = read.method.toUpperCase();
  if ((method !== "GET" && method !== "HEAD") || read.pathname.replace(/\/+$/, "") !== "/_server") {
    return false;
  }
  return !read.instance || (read.fetchSite !== null && read.fetchSite !== "same-origin");
}
