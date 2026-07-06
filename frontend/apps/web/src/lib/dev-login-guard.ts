// R0.6 — the pure decision behind the dev-login guard (review fe-web #1: the dev-login seam mints a
// full app session with no environment guard, an app-shell auth bypass if the bundle is ever
// deployed). Kept dependency-free (no "use server", no session/cookie imports) so it is unit-testable
// in isolation; `auth.ts` composes it with the loud refusal + /login redirect.

/** The environment facts the dev-login decision reads. */
export interface DevLoginEnv {
  NODE_ENV?: string;
  MYELIN_DEV_LOGIN?: string;
}

/**
 * The dev-login seam may mint a session ONLY when BOTH gates hold — two INDEPENDENT conditions so no
 * single misconfiguration re-opens the bypass:
 *   1. the build is NOT production (`NODE_ENV !== "production"`), AND
 *   2. dev-login is EXPLICITLY opted in (`MYELIN_DEV_LOGIN === "1"`).
 * A production build is refused regardless of the flag; a non-production build without the flag is
 * also refused. Fail-closed: anything unexpected (unset vars, other values) returns false.
 */
export function devLoginAllowed(env: DevLoginEnv): boolean {
  const isProduction = env.NODE_ENV === "production";
  const explicitlyOptedIn = env.MYELIN_DEV_LOGIN === "1";
  return !isProduction && explicitlyOptedIn;
}
