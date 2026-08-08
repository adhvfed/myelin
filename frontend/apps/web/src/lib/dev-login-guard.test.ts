import { describe, expect, it } from "vitest";
import { devLoginAllowed, devSeamAllowed } from "./dev-login-guard";

describe("dev-login guard (R0.6 — fe-web #1 auth-bypass fix)", () => {
  it("permits dev-login ONLY when non-production AND explicitly opted in", () => {
    expect(devLoginAllowed({ NODE_ENV: "development", MYELIN_DEV_LOGIN: "1" })).toBe(true);
    expect(devLoginAllowed({ NODE_ENV: "test", MYELIN_DEV_LOGIN: "1" })).toBe(true);
    // `undefined` NODE_ENV is still non-production; the flag is what gates it.
    expect(devLoginAllowed({ MYELIN_DEV_LOGIN: "1" })).toBe(true);
  });

  it("REFUSES in production even with the opt-in flag set (production gate is independent)", () => {
    expect(devLoginAllowed({ NODE_ENV: "production", MYELIN_DEV_LOGIN: "1" })).toBe(false);
  });

  it("REFUSES in non-production when the opt-in flag is missing or not exactly '1'", () => {
    expect(devLoginAllowed({ NODE_ENV: "development" })).toBe(false);
    expect(devLoginAllowed({ NODE_ENV: "development", MYELIN_DEV_LOGIN: "0" })).toBe(false);
    expect(devLoginAllowed({ NODE_ENV: "development", MYELIN_DEV_LOGIN: "true" })).toBe(false);
    expect(devLoginAllowed({ NODE_ENV: "development", MYELIN_DEV_LOGIN: "" })).toBe(false);
  });

  it("fail-closed on an empty / unknown environment", () => {
    expect(devLoginAllowed({})).toBe(false);
    expect(devLoginAllowed({ NODE_ENV: "staging" })).toBe(false);
  });
});

describe("dev-seam RENDER gate (R3.5 / OQ-6 — the login page's dev-seam visibility)", () => {
  const devEnv = { NODE_ENV: "development", MYELIN_DEV_LOGIN: "1" };

  it("renders the dev seam ONLY when non-prod build AND frontend opt-in AND edge flag all hold", () => {
    expect(devSeamAllowed(true, devEnv, false)).toBe(true);
  });

  it("a production BUILD hides the seam even if the edge flag + frontend opt-in are set (kill switch)", () => {
    expect(devSeamAllowed(true, devEnv, true)).toBe(false);
  });

  it("the edge flag OFF hides the seam (server truth is one required gate)", () => {
    expect(devSeamAllowed(false, devEnv, false)).toBe(false);
  });

  it("the frontend opt-in missing hides the seam regardless of the edge flag", () => {
    expect(devSeamAllowed(true, { NODE_ENV: "development" }, false)).toBe(false);
    expect(devSeamAllowed(true, {}, false)).toBe(false);
  });
});
