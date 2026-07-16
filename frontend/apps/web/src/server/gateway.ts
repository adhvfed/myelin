// The REAL server-side cookie-auth gateway client (doc 10 §5). Runs ONLY server-side (it imports
// vinxi/http + node fetch). It wires the real cookie-session + edge-fetch deps onto the pure
// `runGateway` core: reads the session token from the httpOnly-cookie store, adds the Bearer token to
// the edge call, and on 401 does the single refresh + one retry. **Tokens never reach client JS** —
// this module is never bundled to the client (it is reachable only through `"use server"` functions).
//
// The edge it calls is the MR-014/015 contract (`/v1/...`, Bearer/cookie auth, `{error:{message}}`,
// pagination). `MYELIN_EDGE_URL` points at it: in the harness that is the clearly-marked DEV EDGE
// (`dev-edge/server.mjs`, which serves the real contract over the real Git ViewModel JSON because the
// real `myelin-edge` binary can't yet issue a human a capability token — MR-012 deferred). Pointing
// this at the real `edge` binary is a one-line env change, not new plumbing.

import { runGateway, GatewayError, Unauthorized } from "./gateway-core";
import {
  clearCurrentSession,
  getSessionRecord,
  updateSessionToken,
} from "./session";

export { GatewayError, Unauthorized } from "./gateway-core";

function edgeUrl(): string {
  return process.env.MYELIN_EDGE_URL ?? "http://127.0.0.1:8787";
}

/** GET a JSON view-model from the edge through the full auth lifecycle. */
export async function edgeGet<T = unknown>(path: string): Promise<T> {
  return edgeRequest<T>("GET", path);
}

/** POST to the edge (write verbs) through the full auth lifecycle. */
export async function edgePost<T = unknown>(path: string, body?: unknown): Promise<T> {
  return edgeRequest<T>("POST", path, body);
}

/**
 * GET an UNAUTHENTICATED edge endpoint (no Bearer, no session) — for the logged-out `GET
 * /v1/auth/config` the login page reads before any session exists (R3.5). No auth lifecycle: a
 * non-2xx is a `GatewayError` the caller may floor-tolerate (the login page falls back to the
 * fail-closed "SSO unavailable" render if the edge is unreachable). Still server-only — the URL/env
 * never reach client JS.
 */
export async function edgeGetPublic<T = unknown>(path: string): Promise<T> {
  const res = await fetch(`${edgeUrl()}${path}`, {
    method: "GET",
    headers: { accept: "application/json" },
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
}

/**
 * VERIFY a CALLER-SUPPLIED capability token (R4.0 operator-token login). Calls the edge's
 * authenticated `GET /v1/whoami` with the PASTED token (NOT the session's) + the token scheme header,
 * and returns the viewer facts on a 200. This is how the frontend proves a bootstrap token actually
 * authenticates before minting a session. Server-only — the token never reaches client JS, and a
 * non-200 throws a token-FREE `Unauthorized` (the raw edge body is NEVER attached, so it can't leak).
 */
export async function edgeWhoamiWithToken(token: string, scheme = "agent"): Promise<EdgeWhoami> {
  const res = await fetch(`${edgeUrl()}/v1/whoami`, {
    method: "GET",
    headers: {
      accept: "application/json",
      authorization: `Bearer ${token}`,
      "x-myelin-token-scheme": scheme,
    },
  });
  if (res.status !== 200) {
    // Do NOT attach the response body — it could echo the token or an internal error. Honest, opaque.
    throw new Unauthorized(`token verification failed (HTTP ${res.status})`);
  }
  const who = (await res.json().catch(() => null)) as EdgeWhoami | null;
  if (!who || typeof who.principal_id !== "string" || !who.principal_id) {
    throw new Unauthorized("token verification returned an unexpected shape");
  }
  return who;
}

async function edgeRequest<T>(method: string, path: string, body?: unknown): Promise<T> {
  const scheme = getSessionRecord()?.scheme ?? "pat";
  return runGateway<T>({
    getToken: () => getSessionRecord()?.token ?? null,
    doFetch: async (token) => {
      const res = await fetch(`${edgeUrl()}${path}`, {
        method,
        headers: {
          authorization: `Bearer ${token}`,
          "x-myelin-token-scheme": scheme,
          ...(body !== undefined ? { "content-type": "application/json" } : {}),
        },
        body: body !== undefined ? JSON.stringify(body) : undefined,
      });
      return { status: res.status, bodyText: await res.text() };
    },
    refresh: async () => {
      const rec = getSessionRecord();
      if (!rec) return null;
      const res = await fetch(`${edgeUrl()}/v1/auth/refresh`, {
        method: "POST",
        headers: {
          authorization: `Bearer ${rec.refreshToken}`,
          "x-myelin-token-scheme": "refresh",
        },
      });
      if (res.status !== 200) return null;
      // The refresh response may rotate the access token; persist it server-side and use it for the retry.
      const json = (await res.json().catch(() => null)) as { access_token?: string } | null;
      const fresh = json?.access_token ?? rec.token;
      updateSessionToken(fresh);
      return fresh;
    },
    clearSession: () => clearCurrentSession(),
  });
}

/** The RAW byte-fetch (R3.4 raw/download proxy). Streams an edge blob through the SAME server-side
 *  auth (Bearer from the session cookie; never a public signed URL — the sovereignty rail), with ONE
 *  refresh retry on 401. Returns the status, the edge's content-type + content-disposition (so the
 *  proxy route forwards `attachment`), and the raw bytes. Binary-safe (never text-decodes the body). */
export interface RawEdgeResponse {
  status: number;
  contentType: string;
  contentDisposition: string | null;
  body: ArrayBuffer;
}

export async function edgeGetRaw(path: string): Promise<RawEdgeResponse> {
  const scheme = getSessionRecord()?.scheme ?? "pat";
  const doFetch = (token: string) =>
    fetch(`${edgeUrl()}${path}`, {
      method: "GET",
      headers: { authorization: `Bearer ${token}`, "x-myelin-token-scheme": scheme },
    });

  const token = getSessionRecord()?.token;
  if (!token) throw new Unauthorized("no session token (not authenticated)");
  let res = await doFetch(token);
  if (res.status === 401) {
    // ONE refresh round-trip + retry (mirrors runGateway), then give up.
    const rec = getSessionRecord();
    let fresh: string | null = null;
    if (rec) {
      const rr = await fetch(`${edgeUrl()}/v1/auth/refresh`, {
        method: "POST",
        headers: { authorization: `Bearer ${rec.refreshToken}`, "x-myelin-token-scheme": "refresh" },
      });
      if (rr.status === 200) {
        const json = (await rr.json().catch(() => null)) as { access_token?: string } | null;
        fresh = json?.access_token ?? rec.token;
        updateSessionToken(fresh);
      }
    }
    if (!fresh) {
      clearCurrentSession();
      throw new Unauthorized("still unauthorized after one refresh");
    }
    res = await doFetch(fresh);
  }
  return {
    status: res.status,
    contentType: res.headers.get("content-type") ?? "application/octet-stream",
    contentDisposition: res.headers.get("content-disposition"),
    body: await res.arrayBuffer(),
  };
}
