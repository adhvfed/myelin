import { describe, expect, it } from "vitest";

import { oidcClientConfig, type OidcEnvironment } from "./oidc-config";

const complete: OidcEnvironment = {
  MYELIN_OIDC_AUTHORIZATION_ENDPOINT: "https://id.example/authorize",
  MYELIN_OIDC_TOKEN_ENDPOINT: "https://id.example/token",
  MYELIN_OIDC_CLIENT_ID: "myelin-web",
  MYELIN_OIDC_CLIENT_SECRET: "secret",
  MYELIN_PUBLIC_ORIGIN: "https://myelin.example",
};

describe("oidcClientConfig", () => {
  it("is disabled only when every interactive field is absent", () => {
    expect(oidcClientConfig({}, true)).toBeNull();
    expect(() => oidcClientConfig({ MYELIN_OIDC_CLIENT_ID: "partial" }, true)).toThrow(
      /configuration is incomplete/,
    );
  });

  it("builds the exact callback and defaults to OpenID scopes", () => {
    expect(oidcClientConfig(complete, true)).toMatchObject({
      authorizationEndpoint: "https://id.example/authorize",
      tokenEndpoint: "https://id.example/token",
      scopes: "openid profile email",
      redirectUri: "https://myelin.example/auth/oidc/callback",
    });
  });

  it("requires HTTPS endpoints in production and rejects parameter-bearing endpoints", () => {
    expect(() => oidcClientConfig({
      ...complete,
      MYELIN_OIDC_TOKEN_ENDPOINT: "http://id.example/token",
    }, true)).toThrow(/HTTPS/);
    expect(() => oidcClientConfig({
      ...complete,
      MYELIN_OIDC_AUTHORIZATION_ENDPOINT: "https://id.example/authorize?prompt=login",
    }, true)).toThrow(/without query or fragment/);
  });

  it("requires the openid scope", () => {
    expect(() => oidcClientConfig({ ...complete, MYELIN_OIDC_SCOPES: "profile email" }, true))
      .toThrow(/including openid/);
  });

  it("rejects characters outside OAuth's scope-token grammar", () => {
    expect(() => oidcClientConfig({
      ...complete,
      MYELIN_OIDC_SCOPES: 'openid profile"admin',
    }, true)).toThrow(/printable space-separated scopes/);
  });
});
