import { describe, expect, it } from "vitest";

import { sessionCookieSettings } from "./session-cookie";

describe("sessionCookieSettings", () => {
  it("uses a host-only Secure prefix in production", () => {
    expect(sessionCookieSettings(true)).toEqual({
      name: "__Host-myelin_session",
      secure: true,
      maxAgeSeconds: 28_800,
    });
  });

  it("uses an HTTP-compatible name only for local development", () => {
    expect(sessionCookieSettings(false)).toEqual({
      name: "myelin_session",
      secure: false,
      maxAgeSeconds: 28_800,
    });
  });
});
