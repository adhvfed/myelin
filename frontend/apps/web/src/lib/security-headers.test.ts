import { describe, expect, it } from "vitest";

import { contentSecurityPolicy, securityHeaders } from "./security-headers";

describe("contentSecurityPolicy", () => {
  it("allows only nonce-bearing scripts in production", () => {
    const policy = contentSecurityPolicy({ nonce: "fixed-nonce", production: true });

    expect(policy).toContain("script-src 'self' 'nonce-fixed-nonce' 'strict-dynamic'");
    expect(policy).toContain("script-src-attr 'none'");
    expect(policy).toContain("frame-ancestors 'none'");
    expect(policy).not.toContain("'unsafe-eval'");
    expect(policy).not.toContain("ws:");
  });

  it("permits only the extra connections and evaluation required by the dev client", () => {
    const policy = contentSecurityPolicy({ nonce: "dev-nonce", production: false });

    expect(policy).toContain("script-src 'self' 'nonce-dev-nonce' 'strict-dynamic' 'unsafe-eval'");
    expect(policy).toContain("connect-src 'self' ws: wss:");
  });
});

describe("securityHeaders", () => {
  it("returns the production browser-security baseline", () => {
    const headers = securityHeaders({ hsts: true, nonce: "fixed-nonce", production: true });

    expect(headers).toMatchObject({
      "Cache-Control": "no-store",
      "Cross-Origin-Resource-Policy": "same-origin",
      "Referrer-Policy": "same-origin",
      "Strict-Transport-Security": "max-age=31536000",
      "X-Content-Type-Options": "nosniff",
      "X-Frame-Options": "DENY",
      "X-Permitted-Cross-Domain-Policies": "none",
      "X-XSS-Protection": "0",
    });
  });

  it("does not advertise HSTS from a development server", () => {
    const headers = securityHeaders({ hsts: true, nonce: "dev-nonce", production: false });

    expect(headers["Strict-Transport-Security"]).toBeUndefined();
  });

  it("requires an explicit HSTS deployment opt-in", () => {
    const headers = securityHeaders({ nonce: "fixed-nonce", production: true });

    expect(headers["Strict-Transport-Security"]).toBeUndefined();
  });
});
