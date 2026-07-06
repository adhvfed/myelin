import { describe, expect, it } from "vitest";
import { devLoginAllowed } from "./dev-login-guard";

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
