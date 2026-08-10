// Dependency-free token login decision. auth.ts supplies verification and session persistence.
// Tokens remain server-side and must not be logged or returned to the client.

/** The edge's `GET /v1/whoami` view-model for a valid token (crates/myelin-edge gateway.rs). */
export interface TokenWhoami {
  principal_id: string;
  tenant: string;
  region: string;
  kind?: string;
  expires_at: number;
}

/** The session record the login mints (mirrors {@link ../server/session#SessionRecord}). */
export interface TokenSessionInput {
  token: string;
  refreshToken: string;
  scheme: string;
  credentialExpiresAtMs: number;
  principalId: string;
  displayName: string;
  region: string;
  tenant: string;
}

/** Dependencies supplied by `auth.ts` and replaced in tests. */
export interface TokenLoginDeps {
  /** Re-read the edge's authoritative auth posture immediately before verification. Only an
   *  explicit `true` admits token login; false/missing config and transport errors fail closed. */
  isEnabled: () => Promise<boolean>;
  /** Verify the supplied token with the edge. Rejects on a non-200 response or network error. */
  verify: (token: string, scheme: string) => Promise<TokenWhoami>;
  /** Issue the session server-side (store record + set the httpOnly cookie). */
  issue: (rec: TokenSessionInput) => void | Promise<void>;
}

/** Where the login sends the browser next. */
export interface TokenLoginResult {
  redirectTo: string;
}

/** Authentication failures return a stable error state without exposing the token or edge detail. */
export const TOKEN_LOGIN_ERROR = "/login?error=token_invalid";
/** A disabled or unavailable auth mode returns to the login chooser without exposing edge detail. */
export const TOKEN_LOGIN_DISABLED = "/login";
/** Successful login destination. */
export const TOKEN_LOGIN_SUCCESS = "/git/repos";
/** The edge's default capability-token scheme; the paste form defaults here but may override. */
export const DEFAULT_TOKEN_SCHEME = "agent";
export const MAX_TOKEN_LOGIN_BYTES = 32 * 1024;

function boundedIdentityField(value: unknown, maxBytes: number): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= maxBytes &&
    new TextEncoder().encode(value).byteLength <= maxBytes &&
    !hasControlCharacter(value);
}

function hasControlCharacter(value: string): boolean {
  for (const character of value) {
    const codePoint = character.codePointAt(0)!;
    if (codePoint <= 0x1f || codePoint === 0x7f) return true;
  }
  return false;
}

/** Capability-token responses have no display name, so use the principal ID. */
export function deriveDisplayName(principalId: string): string {
  return principalId;
}

/**
 * Run the operator-token login decision.
 *
 * 1. Empty or malformed token: return an authentication error.
 * 2. Re-read the authoritative edge config. Anything except explicit enabled (including a config
 *    fetch error): return to the login chooser without verification.
 * 3. Verification failure or an invalid identity response: return an authentication error.
 * 4. Otherwise mint the session. Pasted tokens have no refresh credential and expire normally.
 */
export async function runTokenLogin(
  rawToken: string,
  rawScheme: string | undefined,
  deps: TokenLoginDeps,
): Promise<TokenLoginResult> {
  const token = (rawToken ?? "").trim();
  const scheme = (rawScheme ?? "").trim() || DEFAULT_TOKEN_SCHEME;

  if (
    !token ||
    token.length > MAX_TOKEN_LOGIN_BYTES ||
    !/^[\x21-\x7e]+$/.test(token) ||
    !/^[a-z][a-z0-9_]{0,31}$/.test(scheme)
  ) {
    return { redirectTo: TOKEN_LOGIN_ERROR };
  }

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
    // Raw edge errors must not reach the browser.
    return { redirectTo: TOKEN_LOGIN_ERROR };
  }

  if (
    !who ||
    !boundedIdentityField(who.principal_id, 512) ||
    !boundedIdentityField(who.tenant, 128) ||
    !boundedIdentityField(who.region, 128) ||
    !Number.isSafeInteger(who.expires_at) ||
    who.expires_at > Math.floor(Number.MAX_SAFE_INTEGER / 1_000) ||
    who.expires_at <= Math.floor(Date.now() / 1_000)
  ) {
    return { redirectTo: TOKEN_LOGIN_ERROR };
  }

  await deps.issue({
    token,
    // Pasted tokens have no refresh credential. A later 401 clears the session and returns to login.
    refreshToken: "",
    scheme,
    credentialExpiresAtMs: who.expires_at * 1_000,
    principalId: who.principal_id,
    displayName: deriveDisplayName(who.principal_id),
    region: who.region,
    tenant: who.tenant,
  });

  return { redirectTo: TOKEN_LOGIN_SUCCESS };
}
