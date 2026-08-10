// Dependency-free mapping for `getAuthConfig`, kept separate for Node tests.

import { devSeamAllowed, type DevLoginEnv } from "./dev-login-guard";

/** One SSO provider the login page names on its primary button (edge `GET /v1/auth/config`). */
export interface AuthProvider {
  id: string;
  label: string;
}

/** Authentication methods available on the logged-out page. */
export interface AuthConfig {
  sso_configured: boolean;
  providers: AuthProvider[];
  dev_login_enabled: boolean;
  /** Whether the edge accepts operator-token login. Defaults to false when config is unavailable. */
  token_login_enabled: boolean;
}

/** The edge's raw `/v1/auth/config` response. */
export interface EdgeAuthConfig {
  sso_configured?: boolean;
  providers?: AuthProvider[];
  dev_login_enabled?: boolean;
  token_login_enabled?: boolean;
}

/**
 * Map the edge's raw config to the login page's render source.
 * Development login requires the edge flag, frontend environment, and a non-production build.
 * Operator-token login depends only on the edge flag. Missing values default to false.
 */
export function toAuthConfig(
  edge: unknown,
  env: DevLoginEnv,
  isProdBuild: boolean,
  interactiveSsoConfigured = false,
): AuthConfig {
  const raw = edge && typeof edge === "object" && !Array.isArray(edge)
    ? edge as Record<string, unknown>
    : {};
  const ssoConfigured = raw.sso_configured === true && interactiveSsoConfigured;
  return {
    sso_configured: ssoConfigured,
    providers: ssoConfigured ? validProviders(raw.providers) : [],
    dev_login_enabled: devSeamAllowed(raw.dev_login_enabled === true, env, isProdBuild),
    token_login_enabled: raw.token_login_enabled === true,
  };
}

function validProviders(value: unknown): AuthProvider[] {
  if (!Array.isArray(value)) return [];
  const providers: AuthProvider[] = [];
  for (const candidate of value.slice(0, 16)) {
    if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) continue;
    const provider = candidate as Record<string, unknown>;
    if (
      typeof provider.id !== "string" ||
      !/^[a-z][a-z0-9_-]{0,63}$/.test(provider.id) ||
      !boundedLabel(provider.label)
    ) continue;
    providers.push({ id: provider.id, label: provider.label });
  }
  return providers;
}

function boundedLabel(value: unknown): value is string {
  if (typeof value !== "string" || value.length === 0 || value.length > 128) return false;
  if (new TextEncoder().encode(value).byteLength > 128) return false;
  for (const character of value) {
    const codePoint = character.codePointAt(0)!;
    if (codePoint <= 0x1f || codePoint === 0x7f) return false;
  }
  return true;
}
