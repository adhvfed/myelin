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

/** What a request shows about whether it may reach a server function. */
export interface ServerFunctionRead {
  method: string;
  /** The request target as sent, before any URL normalization (path and query). */
  rawTarget: string;
  /** The `X-Server-Instance` header SolidStart's own client sends on every call. */
  instance: string | null;
  /** The browser's `Sec-Fetch-Site`, when it sends one. */
  fetchSite: string | null;
}

const SERVER_FUNCTION_MOUNT = "/_server";

function decodeOnce(text: string): string {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
}

/** The path the way h3 routes it: percent-decoded once, except that an encoded slash stays
 * encoded. h3 mounts the server-function handler on this path by plain prefix, with no segment
 * boundary, so `/%5Fserver/`, `/_serverx`, and `/_server/x` all reach it. */
export function routedPath(rawTarget: string): string {
  const query = rawTarget.indexOf("?");
  const path = query === -1 ? rawTarget : rawTarget.slice(0, query);
  return decodeOnce(path.replace(/%25/g, "%2525").replace(/%2F/gi, "%252F"));
}

/**
 * SolidStart runs any server function, actions included, for a GET as well as a POST, and the
 * origin check above covers only unsafe methods. The app's own reads (router queries) are GETs,
 * but always fetched with SolidStart's custom headers, which a cross-site link or form cannot
 * add. So anything but a POST that would reach the handler must carry `X-Server-Instance` and,
 * when the browser says, be same-origin. POSTs keep the origin check.
 */
export function refusesServerFunctionRead(read: ServerFunctionRead): boolean {
  if (read.method.toUpperCase() === "POST") return false;
  if (!routedPath(read.rawTarget).startsWith(SERVER_FUNCTION_MOUNT)) return false;
  return !read.instance || (read.fetchSite !== null && read.fetchSite !== "same-origin");
}
