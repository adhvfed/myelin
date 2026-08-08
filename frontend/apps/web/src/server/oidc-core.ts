import { createHash } from "node:crypto";

import type { OidcClientConfig } from "./oidc-config";

/** Build the authorization request from server-generated transaction secrets. */
export function oidcAuthorizationUrl(
  oidc: OidcClientConfig,
  state: string,
  nonce: string,
  codeVerifier: string,
): string {
  const authorization = new URL(oidc.authorizationEndpoint);
  authorization.search = new URLSearchParams({
    response_type: "code",
    client_id: oidc.clientId,
    redirect_uri: oidc.redirectUri,
    scope: oidc.scopes,
    state,
    nonce,
    code_challenge: createHash("sha256").update(codeVerifier).digest("base64url"),
    code_challenge_method: "S256",
  }).toString();
  return authorization.toString();
}

function oauthEncode(value: string): string {
  return new URLSearchParams({ value }).toString().slice("value=".length);
}

/** RFC 6749 client_secret_basic encodes each credential component before joining with a colon. */
export function oidcClientAuthorization(clientId: string, clientSecret: string): string {
  const credentials = `${oauthEncode(clientId)}:${oauthEncode(clientSecret)}`;
  return `Basic ${Buffer.from(credentials).toString("base64")}`;
}
