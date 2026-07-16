// The PURE mapping behind `getAuthConfig` (the logged-out login page's render source). Kept
// dependency-free (no "use server", no @solidjs/router — which is a client-only API that throws when
// imported in plain node) so it is unit-testable in isolation, exactly like `dev-login-guard.ts` and
// `gateway-core.ts`. `auth.ts`'s `getAuthConfig` query does only the edge fetch + floor-tolerant
// fallback, then delegates the mapping here.

import { devSeamAllowed, type DevLoginEnv } from "./dev-login-guard";

/** One SSO provider the login page names on its primary button (edge `GET /v1/auth/config`). */
export interface AuthProvider {
  id: string;
  label: string;
}

/** The logged-out login page's honest render source — the edge's SSO posture + the two login-seam
 *  gates (dev seam, operator-token). */
export interface AuthConfig {
  sso_configured: boolean;
  providers: AuthProvider[];
  dev_login_enabled: boolean;
  /** R4.0 — whether the edge accepts the OPERATOR-TOKEN login (env `MYELIN_TOKEN_LOGIN=1`). When true
   *  the login page paints the paste-your-bootstrap-token card. Unlike `dev_login_enabled` this is a
   *  REAL working path (it verifies against the live edge), so the edge flag alone is authoritative —
   *  no frontend build/env gate. Fail-closed: false if the edge is unreachable. */
  token_login_enabled: boolean;
}

/** The edge's raw `/v1/auth/config` shape (its `dev_login_enabled` reflects the EDGE env). */
export interface EdgeAuthConfig {
  sso_configured?: boolean;
  providers?: AuthProvider[];
  dev_login_enabled?: boolean;
  token_login_enabled?: boolean;
}

/**
 * Map the edge's raw config to the login page's render source.
 *  - `dev_login_enabled` is the FRONTEND-authoritative composition (build + frontend env + edge flag)
 *    via {@link devSeamAllowed} — the frontend owns the dev seam.
 *  - `token_login_enabled` is a REAL working path, so the edge flag alone gates it (no build/env kill
 *    switch). Both default fail-closed to false (unset field, or the caller's edge-unreachable stub).
 */
export function toAuthConfig(
  edge: EdgeAuthConfig,
  env: DevLoginEnv,
  isProdBuild: boolean,
): AuthConfig {
  return {
    sso_configured: edge.sso_configured ?? false,
    providers: edge.providers ?? [],
    dev_login_enabled: devSeamAllowed(edge.dev_login_enabled ?? false, env, isProdBuild),
    token_login_enabled: edge.token_login_enabled ?? false,
  };
}
