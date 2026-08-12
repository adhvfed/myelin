// Dependency-injected gateway flow used by the server adapter and Node tests. A 401 permits one
// refresh and one retry before `Unauthorized` is returned.

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
const UNAUTHORIZED_BRAND_KEY = "myelin.gateway.Unauthorized.v1";

export class Unauthorized extends Error {
  constructor(message = "Unauthorized") {
    super(message);
    this.name = "Unauthorized";
    Object.defineProperty(this, Symbol.for(UNAUTHORIZED_BRAND_KEY), { value: true });
  }
}

/** Vinxi loads the SSR and server-function graphs separately, so the same gateway module can have
 * two constructor identities in one process. Keep auth classification stable across that boundary
 * without accepting arbitrary messages or response bodies as authorization failures. */
export function isUnauthorized(error: unknown): error is Unauthorized {
  if (error instanceof Unauthorized) return true;
  if (typeof error !== "object" || error === null) return false;
  return (error as Record<symbol, unknown>)[Symbol.for(UNAUTHORIZED_BRAND_KEY)] === true;
}

/** A transport-agnostic edge response (status + raw body text) the core parses. */
export interface GwResponse {
  status: number;
  bodyText: string;
}

/** The injectable dependencies the core needs (the real ones live in `gateway.ts`; tests fake them). */
export interface GatewayDeps {
  /** The server-side access token for this request (from the httpOnly-cookie session), or null. */
  getToken: () => string | null | Promise<string | null>;
  /** Perform the edge call with the given Bearer token. */
  doFetch: (token: string) => Promise<GwResponse>;
  /** The single refresh round-trip: return a fresh access token, or null if the session is dead. */
  refresh: () => Promise<string | null>;
  /** Drop the now-invalid session (so the next request starts clean). */
  clearSession: () => void | Promise<void>;
}

/**
 * Run the edge call through the auth lifecycle.
 *
 * 1. Missing token: `Unauthorized`.
 * 2. First 401: refresh and retry; second 401: clear the session and return `Unauthorized`.
 * 3. Other non-2xx response: `GatewayError`.
 * 4. Success: parsed JSON.
 */
export async function runGateway<T = unknown>(deps: GatewayDeps): Promise<T> {
  const token = await deps.getToken();
  if (!token) throw new Unauthorized("no session token (not authenticated)");

  let res = await deps.doFetch(token);

  if (res.status === 401) {
    const fresh = await deps.refresh();
    if (!fresh) {
      await deps.clearSession();
      throw new Unauthorized("session refresh failed");
    }
    res = await deps.doFetch(fresh);
    if (res.status === 401) {
      await deps.clearSession();
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
