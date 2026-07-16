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

/**
 * The dev-login SEAM RENDER gate (R3.5 / OQ-6). The login page paints the dev seam ONLY when THREE
 * independent gates all hold — the most conservative posture so no single misconfiguration shows a
 * dev seam on a real deployment:
 *   1. the frontend build is NOT production (`isProdBuild === false`, the build-time PROD kill switch),
 *   2. the frontend server opted the seam in (`devLoginAllowed(env)`), AND
 *   3. the edge's public config agrees (`edgeDevLoginEnabled` from `GET /v1/auth/config`).
 * Server truth (the frontend that owns the seam) wins; the edge flag is one more required gate. Pure +
 * injectable, so it is unit-tested without touching any server module.
 */
export function devSeamAllowed(
  edgeDevLoginEnabled: boolean,
  env: DevLoginEnv,
  isProdBuild: boolean,
): boolean {
  return !isProdBuild && devLoginAllowed(env) && edgeDevLoginEnabled;
}
