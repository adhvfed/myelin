import { describe, expect, it } from "vitest";

import { sessionBackend } from "./session-backend";

describe("sessionBackend", () => {
  it("requires durable storage in production", () => {
    expect(() => sessionBackend(true, undefined)).toThrow(/REDIS_URL/);
    expect(() => sessionBackend(true, "  ")).toThrow(/REDIS_URL/);
  });

  it("uses Valkey whenever REDIS_URL is configured", () => {
    expect(sessionBackend(true, "rediss://cache.example/0")).toEqual({
      kind: "valkey",
      url: "rediss://cache.example/0",
    });
    expect(sessionBackend(false, "redis://localhost:6380")).toEqual({
      kind: "valkey",
      url: "redis://localhost:6380",
    });
  });

  it("requires TLS for production session credentials", () => {
    expect(() => sessionBackend(true, "redis://cache.example:6379/0")).toThrow(
      "REDIS_URL must use rediss:// TLS in production",
    );
  });

  it.each([
    "not a URL",
    "https://cache.example",
    "redis://",
  ])("rejects an invalid Valkey URL: %s", (url) => {
    expect(() => sessionBackend(false, url)).toThrow(
      "REDIS_URL must be an absolute redis:// or rediss:// URL",
    );
  });

  it("keeps the hermetic memory backend for local development", () => {
    expect(sessionBackend(false, undefined)).toEqual({ kind: "memory" });
  });
});
