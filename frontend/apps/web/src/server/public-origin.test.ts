import { describe, expect, it } from "vitest";
import { canonicalPublicOrigin } from "./public-origin";

describe("canonicalPublicOrigin", () => {
  it("normalizes an explicitly configured origin", () => {
    expect(
      canonicalPublicOrigin({
        production: true,
        configured: "  https://app.myelin.dev:8443/  ",
      }),
    ).toBe("https://app.myelin.dev:8443");
  });

  it("derives the origin from the request URL only outside production", () => {
    expect(
      canonicalPublicOrigin({
        production: false,
        requestUrl: "http://localhost:3000/login?next=%2Fgit",
      }),
    ).toBe("http://localhost:3000");
    expect(() =>
      canonicalPublicOrigin({
        production: true,
        requestUrl: "https://app.myelin.dev/login",
      }),
    ).toThrow("MYELIN_PUBLIC_ORIGIN is required in production");
  });

  it("requires HTTPS for the production origin", () => {
    expect(() =>
      canonicalPublicOrigin({ production: true, configured: "http://app.myelin.dev" }),
    ).toThrow("MYELIN_PUBLIC_ORIGIN must use HTTPS in production");
  });

  it.each([
    "https://user@app.myelin.dev",
    "https://app.myelin.dev/login",
    "https://app.myelin.dev?tenant=acme",
    "https://app.myelin.dev#login",
    "ftp://app.myelin.dev",
    "not a URL",
  ])("rejects a value that is not an HTTP(S) origin: %s", (configured) => {
    expect(() => canonicalPublicOrigin({ production: false, configured })).toThrow();
  });
});
