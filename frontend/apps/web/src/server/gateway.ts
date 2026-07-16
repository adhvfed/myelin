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

import { runGateway, Unauthorized } from "./gateway-core";
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
