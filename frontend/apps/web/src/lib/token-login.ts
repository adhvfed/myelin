// R4.0 — the pure decision behind the OPERATOR-TOKEN login (the browser on-ramp for dogfood: the
// founder pastes the capability token `edge bootstrap` printed; the frontend VERIFIES it against the
// real edge and mints a session). Kept dependency-free (no "use server", no session/cookie/fetch
// imports) so it is unit-testable in node in isolation — exactly like `gateway-core.ts` and
// `dev-login-guard.ts`. `auth.ts` wires the real whoami-verify + session-issue deps onto it.
//
// THE TOKEN IS A SECRET. It is only ever the caller-supplied value flowing through these deps
// server-side; it is never logged, never returned to the client, never put in a query string.

/** The edge's `GET /v1/whoami` view-model for a valid token (crates/myelin-edge gateway.rs). */
export interface TokenWhoami {
  principal_id: string;
  tenant: string;
  region: string;
  kind?: string;
}

/** The session record the login mints (mirrors {@link ../server/session#SessionRecord}). */
export interface TokenSessionInput {
  token: string;
  refreshToken: string;
  scheme: string;
  principalId: string;
  displayName: string;
  region: string;
  tenant: string;
}

/** The injectable dependencies (the real ones live in `auth.ts`; tests fake them). */
export interface TokenLoginDeps {
  /** Re-read the edge's authoritative auth posture immediately before verification. Only an
   *  explicit `true` admits token login; false/missing config and transport errors fail closed. */
  isEnabled: () => Promise<boolean>;
  /** Verify a CALLER-SUPPLIED token against the real edge whoami. Resolves with the viewer facts on a
   *  200; REJECTS on any non-200 / network error (invalid or expired token). Never leaks the raw error. */
  verify: (token: string, scheme: string) => Promise<TokenWhoami>;
  /** Issue the session server-side (store record + set the httpOnly cookie). */
  issue: (rec: TokenSessionInput) => void | Promise<void>;
}

/** Where the login sends the browser next. Success → the repos home; failure → the honest error state. */
export interface TokenLoginResult {
  redirectTo: string;
}

/** On any failure the founder is bounced back with a DISTINCT, honest error param (blames the token /
 *  the bootstrap step, never the user) — never the raw edge error, never the token. */
export const TOKEN_LOGIN_ERROR = "/login?error=token_invalid";
/** A disabled or unavailable auth mode returns to the login chooser without exposing edge detail. */
export const TOKEN_LOGIN_DISABLED = "/login";
/** On success, land in the app exactly where the dev seam lands. */
export const TOKEN_LOGIN_SUCCESS = "/git/repos";
/** The edge's default capability-token scheme; the paste form defaults here but may override. */
export const DEFAULT_TOKEN_SCHEME = "agent";

/** Derive an honest identity-menu label from the principal id — whoami carries no human display name
 *  for a capability token, so the PII-free principal id IS the honest label (never a fabricated name). */
export function deriveDisplayName(principalId: string): string {
  return principalId;
}

/**
 * Run the operator-token login decision.
 *
 * 1. Empty token → the honest error (nothing to verify).
 * 2. Re-read the authoritative edge config. Anything except explicit enabled (including a config
 *    fetch error) → the login chooser; `verify` and `issue` are NEVER called.
 * 3. `verify` REJECTS (invalid/expired token or edge unreachable) → the honest error; NO session. The
 *    raw edge error is swallowed here so it can never leak to the client.
 * 4. `verify` resolves but the whoami shape is missing a principal id → the honest error; NO session.
 * 5. Otherwise mint the session (with an EMPTY refresh token — a pasted bootstrap token has none; it
 *    simply expires and the founder re-pastes, see `auth.ts`) and land in the app.
 */
export async function runTokenLogin(
  rawToken: string,
  rawScheme: string | undefined,
  deps: TokenLoginDeps,
): Promise<TokenLoginResult> {
  const token = (rawToken ?? "").trim();
  const scheme = (rawScheme ?? "").trim() || DEFAULT_TOKEN_SCHEME;

  if (!token) return { redirectTo: TOKEN_LOGIN_ERROR };

  try {
    if ((await deps.isEnabled()) !== true) {
      return { redirectTo: TOKEN_LOGIN_DISABLED };
    }
  } catch {
    // The public edge config is the auth-mode authority. An unavailable or malformed response must
    // never degrade into accepting a token through this public web bridge.
    return { redirectTo: TOKEN_LOGIN_DISABLED };
  }

  let who: TokenWhoami;
  try {
    who = await deps.verify(token, scheme);
  } catch {
    // Swallow the raw edge error — the founder sees only the honest, system-blaming error copy.
    return { redirectTo: TOKEN_LOGIN_ERROR };
  }

  if (!who || typeof who.principal_id !== "string" || !who.principal_id) {
    return { redirectTo: TOKEN_LOGIN_ERROR };
  }

  await deps.issue({
    token,
    // A pasted operator/bootstrap token has NO refresh credential. Empty is correct + tolerated: on a
    // future 401 the refresh round-trip Bearers this empty string, the edge answers 401, and the
    // gateway clears the session → /login. The founder simply re-pastes a fresh bootstrap token.
    refreshToken: "",
    scheme,
    principalId: who.principal_id,
    displayName: deriveDisplayName(who.principal_id),
    region: who.region,
    tenant: who.tenant,
  });

  return { redirectTo: TOKEN_LOGIN_SUCCESS };
}
