import { describe, expect, it } from "vitest";

import {
  deviceCookieSettings,
  deviceTokenFor,
  MAX_DEVICE_TOKENS,
  withDeviceToken,
} from "./device-cookie";

const token = (n: number) => `device-token-${String(n).padStart(8, "0")}`;

describe("device cookie", () => {
  it("is host-only and Secure in production and lasts a year", () => {
    expect(deviceCookieSettings(true)).toEqual({
      name: "__Host-myelin_device",
      secure: true,
      maxAgeSeconds: 31_536_000,
    });
    expect(deviceCookieSettings(false).name).toBe("myelin_device");
  });

  it("keeps one token per email, matched however the email is typed", () => {
    let cookie = withDeviceToken(undefined, "ada@example.com", token(1));
    cookie = withDeviceToken(cookie, "grace@example.com", token(2));
    cookie = withDeviceToken(cookie, " ADA@example.com ", token(3));
    expect(deviceTokenFor(cookie, "ada@example.com")).toBe(token(3));
    expect(deviceTokenFor(cookie, "grace@example.com")).toBe(token(2));
    expect(deviceTokenFor(cookie, "nobody@example.com")).toBeUndefined();
    expect(cookie).not.toContain("ada");
    expect(cookie.split(".")).toHaveLength(2);
  });

  it("drops the oldest past the limit", () => {
    let cookie: string | undefined;
    for (let n = 0; n <= MAX_DEVICE_TOKENS; n += 1) {
      cookie = withDeviceToken(cookie, `person${n}@example.com`, token(n));
    }
    expect(deviceTokenFor(cookie, "person0@example.com")).toBeUndefined();
    expect(deviceTokenFor(cookie, `person${MAX_DEVICE_TOKENS}@example.com`)).toBe(token(MAX_DEVICE_TOKENS));
    expect(cookie!.split(".")).toHaveLength(MAX_DEVICE_TOKENS);
  });

  it("ignores entries it did not write", () => {
    const cookie = `junk.${"a".repeat(22)}:short.${"b".repeat(22)}:${token(1)}:extra`;
    expect(deviceTokenFor(cookie, "ada@example.com")).toBeUndefined();
    expect(withDeviceToken(cookie, "ada@example.com", token(2)).split(".")).toHaveLength(1);
  });
});
