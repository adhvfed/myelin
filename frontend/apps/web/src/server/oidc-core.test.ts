import { describe, expect, it } from "vitest";

import { oidcAuthorizationUrl, oidcClientAuthorization } from "./oidc-core";

describe("oidcAuthorizationUrl", () => {
  it("builds an exact code + nonce + S256 request", () => {
    const url = new URL(oidcAuthorizationUrl({
      authorizationEndpoint: "https://id.example/authorize",
      tokenEndpoint: "https://id.example/token",
      clientId: "myelin-web",
      clientSecret: "not-projected",
      scopes: "openid profile",
      redirectUri: "https://myelin.example/auth/oidc/callback",
    }, "state", "nonce", "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"));

    expect(Object.fromEntries(url.searchParams)).toEqual({
      response_type: "code",
      client_id: "myelin-web",
      redirect_uri: "https://myelin.example/auth/oidc/callback",
      scope: "openid profile",
      state: "state",
      nonce: "nonce",
      code_challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
      code_challenge_method: "S256",
    });
    expect(url.toString()).not.toContain("not-projected");
  });
});

describe("oidcClientAuthorization", () => {
  it("form-encodes credential components before Basic encoding", () => {
    const header = oidcClientAuthorization("client id", "s:ecret+");
    expect(Buffer.from(header.slice("Basic ".length), "base64").toString()).toBe(
      "client+id:s%3Aecret%2B",
    );
  });
});
