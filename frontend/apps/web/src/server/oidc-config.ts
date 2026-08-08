import { canonicalPublicOrigin } from "./public-origin";

export interface OidcEnvironment {
  MYELIN_OIDC_AUTHORIZATION_ENDPOINT?: string;
  MYELIN_OIDC_TOKEN_ENDPOINT?: string;
  MYELIN_OIDC_CLIENT_ID?: string;
  MYELIN_OIDC_CLIENT_SECRET?: string;
  MYELIN_OIDC_SCOPES?: string;
  MYELIN_PUBLIC_ORIGIN?: string;
}

export interface OidcClientConfig {
  authorizationEndpoint: string;
  tokenEndpoint: string;
  clientId: string;
  clientSecret: string;
  scopes: string;
  redirectUri: string;
}

const CONFIG_FIELDS = [
  "MYELIN_OIDC_AUTHORIZATION_ENDPOINT",
  "MYELIN_OIDC_TOKEN_ENDPOINT",
  "MYELIN_OIDC_CLIENT_ID",
  "MYELIN_OIDC_CLIENT_SECRET",
] as const;

function endpoint(value: string, name: string, production: boolean): string {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error(`${name} must be an absolute HTTP(S) URL`);
  }
  if (
    (url.protocol !== "http:" && url.protocol !== "https:") ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  ) {
    throw new Error(`${name} must be a credential-free HTTP(S) URL without query or fragment`);
  }
  if (production && url.protocol !== "https:") {
    throw new Error(`${name} must use HTTPS in production`);
  }
  return url.toString();
}

/** Parse the all-or-nothing interactive OIDC client configuration. No fields means SSO is off. */
export function oidcClientConfig(
  env: OidcEnvironment,
  production: boolean,
): OidcClientConfig | null {
  const values = CONFIG_FIELDS.map((name) => env[name]?.trim() ?? "");
  if (values.every((value) => !value)) return null;
  const missing = CONFIG_FIELDS.filter((_, index) => !values[index]);
  if (missing.length) {
    throw new Error(`interactive OIDC configuration is incomplete: missing ${missing.join(", ")}`);
  }

  const [authorizationEndpoint, tokenEndpoint, clientId, clientSecret] = values as [
    string,
    string,
    string,
    string,
  ];
  if (clientId.length > 512 || clientSecret.length > 4096) {
    throw new Error("interactive OIDC client credentials exceed the supported length");
  }
  const scopeTokens = (env.MYELIN_OIDC_SCOPES?.trim() || "openid profile email").split(/\s+/);
  if (
    !scopeTokens.includes("openid") ||
    scopeTokens.some(
      (scope) => scope.length > 128 || !/^[\x21\x23-\x5b\x5d-\x7e]+$/.test(scope),
    )
  ) {
    throw new Error("MYELIN_OIDC_SCOPES must be printable space-separated scopes including openid");
  }
  const publicOrigin = canonicalPublicOrigin({
    production,
    configured: env.MYELIN_PUBLIC_ORIGIN,
  });
  return {
    authorizationEndpoint: endpoint(
      authorizationEndpoint,
      "MYELIN_OIDC_AUTHORIZATION_ENDPOINT",
      production,
    ),
    tokenEndpoint: endpoint(tokenEndpoint, "MYELIN_OIDC_TOKEN_ENDPOINT", production),
    clientId,
    clientSecret,
    scopes: [...new Set(scopeTokens)].join(" "),
    redirectUri: `${publicOrigin}/auth/oidc/callback`,
  };
}
