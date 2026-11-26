import { createHash, randomBytes } from "node:crypto";
import { describe, expect, it } from "vitest";

import {
  deviceCookieSettings,
  deviceTokenFor,
  MAX_DEVICE_TOKENS,
  withDeviceToken,
  withoutDeviceToken,
} from "./device-cookie";

const token = (n: number) => `device-token-${String(n).padStart(8, "0")}`;
const secret = randomBytes(32);

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
    let cookie = withDeviceToken(undefined, secret, "ada@example.com", token(1));
    cookie = withDeviceToken(cookie, secret, "grace@example.com", token(2));
    cookie = withDeviceToken(cookie, secret, " ADA@example.com ", token(3));
    expect(deviceTokenFor(cookie, secret, "ada@example.com")).toBe(token(3));
    expect(deviceTokenFor(cookie, secret, "grace@example.com")).toBe(token(2));
    expect(deviceTokenFor(cookie, secret, "nobody@example.com")).toBeUndefined();
    expect(cookie.split(".")).toHaveLength(2);
  });

  it("keys entries under the server's secret, so the cookie cannot be checked against addresses", () => {
    const cookie = withDeviceToken(undefined, secret, "ada@example.com", token(1));
    const bareHash = createHash("sha256").update("ada@example.com").digest("base64url").slice(0, 22);
    expect(cookie).not.toContain(bareHash);
    expect(cookie).not.toContain("ada");
    expect(deviceTokenFor(cookie, randomBytes(32), "ada@example.com")).toBeUndefined();
  });

  it("drops the oldest past the limit", () => {
    let cookie: string | undefined;
    for (let n = 0; n <= MAX_DEVICE_TOKENS; n += 1) {
      cookie = withDeviceToken(cookie, secret, `person${n}@example.com`, token(n));
    }
    expect(deviceTokenFor(cookie, secret, "person0@example.com")).toBeUndefined();
    expect(deviceTokenFor(cookie, secret, `person${MAX_DEVICE_TOKENS}@example.com`)).toBe(
      token(MAX_DEVICE_TOKENS),
    );
    expect(cookie!.split(".")).toHaveLength(MAX_DEVICE_TOKENS);
  });

  it("forgets one email, and leaves nothing once the last is gone", () => {
    let cookie = withDeviceToken(undefined, secret, "ada@example.com", token(1));
    cookie = withDeviceToken(cookie, secret, "grace@example.com", token(2));
    const rest = withoutDeviceToken(cookie, secret, "ADA@example.com");
    expect(deviceTokenFor(rest, secret, "ada@example.com")).toBeUndefined();
    expect(deviceTokenFor(rest, secret, "grace@example.com")).toBe(token(2));
    expect(withoutDeviceToken(rest, secret, "grace@example.com")).toBeUndefined();
  });

  it("ignores entries it did not write", () => {
    const cookie = `junk.${"a".repeat(22)}:short.${"b".repeat(22)}:${token(1)}:extra`;
    expect(deviceTokenFor(cookie, secret, "ada@example.com")).toBeUndefined();
    expect(withDeviceToken(cookie, secret, "ada@example.com", token(2)).split(".")).toHaveLength(1);
  });
});
