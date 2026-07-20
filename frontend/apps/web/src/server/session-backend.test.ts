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

  it("keeps the hermetic memory backend for local development", () => {
    expect(sessionBackend(false, undefined)).toEqual({ kind: "memory" });
  });
});
