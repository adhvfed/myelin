// The gateway-client CORE — pure, dependency-injected, unit-testable in node (no vinxi/fetch import).
// It encodes the doc 10 §5 contract EXACTLY: read the session token, call the edge, and on a 401 do a
// SINGLE refresh round-trip + ONE retry, else throw `Unauthorized` (the loader turns that into a
// /login redirect). Typed errors extract the edge's `{error:{message,code}}` envelope. This file is
// the security-load-bearing logic; `gateway.ts` wires the real cookie/fetch deps onto it.

/** The edge's `{error:{message, code?}}` envelope (crates/myelin-edge/src/error.rs). */
export interface EdgeErrorBody {
  error?: { message?: string; code?: string };
}

/** A typed edge error — extracts the envelope's `message` (so toasts read like the API author wrote
 *  them) while preserving the HTTP `status`, the machine `code`, and the raw body. */
export class GatewayError extends Error {
  readonly status: number;
  readonly code: string | undefined;
  readonly body: unknown;
  constructor(message: string, status: number, code: string | undefined, body: unknown) {
    super(message);
    this.name = "GatewayError";
    this.status = status;
    this.code = code;
    this.body = body;
  }
}

/** Authentication failed after the single-refresh-then-retry — the loader maps this to `/login`. */
export class Unauthorized extends Error {
  constructor(message = "Unauthorized") {
    super(message);
    this.name = "Unauthorized";
  }
}

/** A transport-agnostic edge response (status + raw body text) the core parses. */
export interface GwResponse {
  status: number;
  bodyText: string;
}

/** The injectable dependencies the core needs (the real ones live in `gateway.ts`; tests fake them). */
export interface GatewayDeps {
  /** The server-side access token for this request (from the httpOnly-cookie session), or null. */
  getToken: () => string | null;
  /** Perform the edge call with the given Bearer token. */
  doFetch: (token: string) => Promise<GwResponse>;
  /** The single refresh round-trip: return a fresh access token, or null if the session is dead. */
  refresh: () => Promise<string | null>;
  /** Drop the now-invalid session (so the next request starts clean). */
  clearSession: () => void;
}

/**
 * Run the edge call through the auth lifecycle.
 *
 * 1. No token → `Unauthorized` (the request was never authenticated).
 * 2. 401 → ONE refresh; if it yields a token, retry ONCE; a second 401 → clear + `Unauthorized`.
 * 3. A non-2xx (other than the handled 401) → `GatewayError` carrying the envelope message/code.
 * 4. 2xx → the parsed JSON body.
 */
export async function runGateway<T = unknown>(deps: GatewayDeps): Promise<T> {
  const token = deps.getToken();
  if (!token) throw new Unauthorized("no session token (not authenticated)");

  let res = await deps.doFetch(token);

  if (res.status === 401) {
    const fresh = await deps.refresh();
    if (!fresh) {
      deps.clearSession();
      throw new Unauthorized("session refresh failed");
    }
    res = await deps.doFetch(fresh);
    if (res.status === 401) {
      deps.clearSession();
      throw new Unauthorized("still unauthorized after one refresh");
    }
  }

  const json = res.bodyText ? safeJson(res.bodyText) : null;

  if (res.status < 200 || res.status >= 300) {
    const env = json as EdgeErrorBody | null;
    const message = env?.error?.message ?? `edge request failed (HTTP ${res.status})`;
    throw new GatewayError(message, res.status, env?.error?.code, json);
  }

  return json as T;
}

function safeJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}
