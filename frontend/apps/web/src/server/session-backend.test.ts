import { describe, expect, it } from "vitest";

import { sessionBackend } from "./session-backend";

const sessionKey = Buffer.alloc(32, 7).toString("base64url");

describe("sessionBackend", () => {
  it("requires durable storage in production", () => {
    expect(() => sessionBackend(true, undefined, sessionKey)).toThrow(/REDIS_URL/);
    expect(() => sessionBackend(true, "  ", sessionKey)).toThrow(/REDIS_URL/);
  });

  it("uses Valkey whenever REDIS_URL is configured", () => {
    expect(sessionBackend(true, "rediss://cache.example/0", sessionKey)).toEqual({
      kind: "valkey",
      url: "rediss://cache.example/0",
      encryptionKey: sessionKey,
    });
    expect(sessionBackend(false, "redis://localhost:6380", sessionKey)).toEqual({
      kind: "valkey",
      url: "redis://localhost:6380",
      encryptionKey: sessionKey,
    });
  });

  it("requires a valid shared encryption key for every Valkey backend", () => {
    expect(() => sessionBackend(false, "redis://localhost:6380", undefined)).toThrow(
      /MYELIN_WEB_SESSION_KEY/,
    );
    expect(() => sessionBackend(false, "redis://localhost:6380", "not-a-key")).toThrow(
      /exactly 32 random bytes/,
    );
  });

  it("requires TLS for production session credentials", () => {
    expect(() => sessionBackend(true, "redis://cache.example:6379/0", sessionKey)).toThrow(
      "REDIS_URL must use rediss:// TLS in production",
    );
  });

  it.each([
    "not a URL",
    "https://cache.example",
    "redis://",
  ])("rejects an invalid Valkey URL: %s", (url) => {
    expect(() => sessionBackend(false, url, sessionKey)).toThrow(
      "REDIS_URL must be an absolute redis:// or rediss:// URL",
    );
  });

  it("keeps the hermetic memory backend for local development", () => {
    expect(sessionBackend(false, undefined, undefined)).toEqual({ kind: "memory" });
  });
});
