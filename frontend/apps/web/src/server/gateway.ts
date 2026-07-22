// The REAL server-side cookie-auth gateway client (doc 10 §5). Runs ONLY server-side (it imports
// vinxi/http + node fetch). It wires the real cookie-session + edge-fetch deps onto the pure
// `runGateway` core: reads the session token from the httpOnly-cookie store, adds the Bearer token to
// the edge call, and on 401 does the single refresh + one retry. **Tokens never reach client JS** —
// this module is never bundled to the client (it is reachable only through `"use server"` functions).
//
// The edge it calls is the MR-014/015 contract (`/v1/...`, Bearer/cookie auth, `{error:{message}}`,
// pagination). `MYELIN_EDGE_URL` points at it: in the harness that is the clearly-marked DEV EDGE
// (`dev-edge/server.mjs`, which serves the real contract over the real Git ViewModel JSON). Pointing
// this at the real `edge` binary is an environment change, not a second data path.

import { runGateway, GatewayError, Unauthorized } from "./gateway-core";
import {
  clearCurrentSession,
  getSessionRecord,
  updateSessionToken,
} from "./session";
import { edgeOrigin } from "./edge-origin";

export { GatewayError, Unauthorized } from "./gateway-core";

export const DEFAULT_EDGE_REQUEST_TIMEOUT_MS = 15_000;

export interface GatewayRequestOptions {
  /** One deadline spans the edge attempt, token refresh, and single retry. Defaults to 15 seconds. */
  signal?: AbortSignal;
  timeoutMs?: number;
}

export function gatewayRequestSignal(options: GatewayRequestOptions = {}): AbortSignal {
  const timeoutMs = options.timeoutMs ?? DEFAULT_EDGE_REQUEST_TIMEOUT_MS;
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    throw new RangeError("edge request timeout must be a positive finite number");
  }
  const deadline = AbortSignal.timeout(timeoutMs);
  return options.signal ? AbortSignal.any([deadline, options.signal]) : deadline;
}

/** GET a JSON view-model from the edge through the full auth lifecycle. */
export async function edgeGet<T = unknown>(path: string, options?: GatewayRequestOptions): Promise<T> {
  return edgeRequest<T>("GET", path, undefined, options);
}

/** POST to the edge (write verbs) through the full auth lifecycle. */
export async function edgePost<T = unknown>(
  path: string,
  body?: unknown,
  options?: GatewayRequestOptions,
): Promise<T> {
  return edgeRequest<T>("POST", path, body, options);
}

/**
 * GET an UNAUTHENTICATED edge endpoint (no Bearer, no session) — for the logged-out `GET
 * /v1/auth/config` the login page reads before any session exists (R3.5). No auth lifecycle: a
 * non-2xx is a `GatewayError` the caller may floor-tolerate (the login page falls back to the
 * fail-closed "SSO unavailable" render if the edge is unreachable). Still server-only — the URL/env
 * never reach client JS.
 */
export async function edgeGetPublic<T = unknown>(path: string): Promise<T> {
  const res = await fetch(`${edgeOrigin()}${path}`, {
    method: "GET",
    headers: { accept: "application/json" },
    redirect: "error",
    signal: gatewayRequestSignal(),
  });
  const bodyText = await res.text();
  if (res.status < 200 || res.status >= 300) {
    throw new GatewayError(`auth/config GET failed (${res.status})`, res.status, undefined, bodyText);
  }
  return JSON.parse(bodyText) as T;
}

/** The edge's `GET /v1/whoami` view-model (crates/myelin-edge gateway.rs). */
export interface EdgeWhoami {
  principal_id: string;
  tenant: string;
  region: string;
  kind: string;
  expires_at: number;
}

export interface EdgeOidcLogin {
  accessToken: string;
  scheme: "session";
  expiresAt: number;
}

function validCredentialExpirySeconds(value: unknown): value is number {
  return (
    Number.isSafeInteger(value) &&
    (value as number) <= Math.floor(Number.MAX_SAFE_INTEGER / 1_000) &&
    (value as number) > Math.floor(Date.now() / 1_000)
  );
}

/** Exchange a verified OIDC ID token + browser nonce for the edge's bounded human capability. */
export async function edgeLoginWithOidc(
  idToken: string,
  nonce: string,
): Promise<EdgeOidcLogin> {
  const res = await fetch(`${edgeOrigin()}/v1/auth/login`, {
    method: "POST",
    headers: { accept: "application/json", "content-type": "application/json" },
    body: JSON.stringify({ scheme: "oidc", material: idToken, nonce }),
    redirect: "error",
    signal: gatewayRequestSignal(),
  });
  const text = await res.text();
  if (res.status !== 200 || text.length > 64 * 1024) {
    throw new Unauthorized(`OIDC login failed (HTTP ${res.status})`);
  }
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    throw new Unauthorized("OIDC login returned an unexpected shape");
  }
  const login = body as Record<string, unknown>;
  if (
    typeof login.access_token !== "string" ||
    !login.access_token ||
    login.scheme !== "session" ||
    login.token_type !== "Bearer" ||
    typeof login.expires_at !== "number" ||
    !validCredentialExpirySeconds(login.expires_at)
  ) {
    throw new Unauthorized("OIDC login returned an unexpected shape");
  }
  return {
    accessToken: login.access_token,
    scheme: "session",
    expiresAt: login.expires_at,
  };
}

/**
 * VERIFY a CALLER-SUPPLIED capability token (R4.0 operator-token login). Calls the edge's
 * authenticated `GET /v1/whoami` with the PASTED token (NOT the session's) + the token scheme header,
 * and returns the viewer facts on a 200. This is how the frontend proves a bootstrap token actually
 * authenticates before minting a session. Server-only — the token never reaches client JS, and a
 * non-200 throws a token-FREE `Unauthorized` (the raw edge body is NEVER attached, so it can't leak).
 */
export async function edgeWhoamiWithToken(token: string, scheme = "agent"): Promise<EdgeWhoami> {
  const res = await fetch(`${edgeOrigin()}/v1/whoami`, {
    method: "GET",
    headers: {
      accept: "application/json",
      authorization: `Bearer ${token}`,
      "x-myelin-token-scheme": scheme,
    },
    redirect: "error",
    signal: gatewayRequestSignal(),
  });
  if (res.status !== 200) {
    // Do NOT attach the response body — it could echo the token or an internal error. Honest, opaque.
    throw new Unauthorized(`token verification failed (HTTP ${res.status})`);
  }
  const who = (await res.json().catch(() => null)) as EdgeWhoami | null;
  if (
    !who ||
    typeof who.principal_id !== "string" ||
    !who.principal_id ||
    typeof who.tenant !== "string" ||
    !who.tenant ||
    typeof who.region !== "string" ||
    !who.region ||
    !validCredentialExpirySeconds(who.expires_at)
  ) {
    throw new Unauthorized("token verification returned an unexpected shape");
  }
  return who;
}

async function edgeRequest<T>(
  method: string,
  path: string,
  body?: unknown,
  options?: GatewayRequestOptions,
): Promise<T> {
  const initialSession = await getSessionRecord();
  const scheme = initialSession?.scheme ?? "pat";
  const signal = gatewayRequestSignal(options);
  return runGateway<T>({
    getToken: () => initialSession?.token ?? null,
    doFetch: async (token) => {
      const res = await fetch(`${edgeOrigin()}${path}`, {
        method,
        headers: {
          authorization: `Bearer ${token}`,
          "x-myelin-token-scheme": scheme,
          ...(body !== undefined ? { "content-type": "application/json" } : {}),
        },
        body: body !== undefined ? JSON.stringify(body) : undefined,
        redirect: "error",
        signal,
      });
      return { status: res.status, bodyText: await res.text() };
    },
    refresh: async () => {
      const rec = await getSessionRecord();
      if (!rec || !rec.refreshToken) return null;
      const res = await fetch(`${edgeOrigin()}/v1/auth/refresh`, {
        method: "POST",
        headers: {
          authorization: `Bearer ${rec.refreshToken}`,
          "x-myelin-token-scheme": "refresh",
        },
        redirect: "error",
        signal,
      });
      if (res.status !== 200) return null;
      // The refresh response may rotate the access token; persist it server-side and use it for the retry.
      const json = (await res.json().catch(() => null)) as { access_token?: string } | null;
      const fresh = json?.access_token ?? rec.token;
      // Revocation/expiry may delete the session while refresh is in flight. Never authorize the
      // retry unless the fresh credential was persisted onto that still-live session.
      return (await updateSessionToken(fresh)) ? fresh : null;
    },
    clearSession: () => clearCurrentSession(),
  });
}

/** The RAW byte-fetch (R3.4 raw/download proxy). Streams an edge blob through the SAME server-side
 *  auth (Bearer from the session cookie; never a public signed URL — the sovereignty rail), with ONE
 *  refresh retry on 401. Returns the status, the edge's content-type, and raw bytes. The browser
 *  proxy owns a safe Content-Disposition rather than trusting blob metadata. Binary-safe. */
export interface RawEdgeResponse {
  status: number;
  contentType: string;
  body: ArrayBuffer;
}

export async function edgeGetRaw(
  path: string,
  options?: GatewayRequestOptions,
): Promise<RawEdgeResponse> {
  const initialSession = await getSessionRecord();
  const scheme = initialSession?.scheme ?? "pat";
  const signal = gatewayRequestSignal(options);
  const doFetch = (token: string) =>
    fetch(`${edgeOrigin()}${path}`, {
      method: "GET",
      headers: { authorization: `Bearer ${token}`, "x-myelin-token-scheme": scheme },
      redirect: "error",
      signal,
    });

  const token = initialSession?.token;
  if (!token) throw new Unauthorized("no session token (not authenticated)");
  let res = await doFetch(token);
  if (res.status === 401) {
    // ONE refresh round-trip + retry (mirrors runGateway), then give up.
    const rec = await getSessionRecord();
    let fresh: string | null = null;
    if (rec?.refreshToken) {
      const rr = await fetch(`${edgeOrigin()}/v1/auth/refresh`, {
        method: "POST",
        headers: { authorization: `Bearer ${rec.refreshToken}`, "x-myelin-token-scheme": "refresh" },
        redirect: "error",
        signal,
      });
      if (rr.status === 200) {
        const json = (await rr.json().catch(() => null)) as { access_token?: string } | null;
        const candidate = json?.access_token ?? rec.token;
        if (await updateSessionToken(candidate)) fresh = candidate;
      }
    }
    if (!fresh) {
      await clearCurrentSession();
      throw new Unauthorized("still unauthorized after one refresh");
    }
    res = await doFetch(fresh);
  }
  return {
    status: res.status,
    contentType: res.headers.get("content-type") ?? "application/octet-stream",
    body: await res.arrayBuffer(),
  };
}
