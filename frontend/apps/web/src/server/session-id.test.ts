import { describe, expect, it } from "vitest";

import { validSessionId } from "./session-id";

describe("validSessionId", () => {
  it("accepts only the fixed-width CSPRNG cookie format", () => {
    expect(validSessionId(`sess_${"a".repeat(32)}`)).toBe(true);
    expect(validSessionId(`sess_${"A0_-".repeat(8)}`)).toBe(true);
  });

  it("rejects attacker-controlled Redis key material", () => {
    expect(validSessionId("session-one")).toBe(false);
    expect(validSessionId(`sess_${"a".repeat(31)}`)).toBe(false);
    expect(validSessionId(`sess_${"a".repeat(33)}`)).toBe(false);
    expect(validSessionId(`sess_${"../".repeat(11)}`)).toBe(false);
    expect(validSessionId(`sess_${"a".repeat(10_000)}`)).toBe(false);
  });
});
