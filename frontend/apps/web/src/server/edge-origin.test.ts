import { describe, expect, it } from "vitest";
import { canonicalEdgeOrigin } from "./edge-origin";

describe("canonicalEdgeOrigin", () => {
  it("keeps the loopback development seam only outside production", () => {
    expect(canonicalEdgeOrigin({ production: false, configured: undefined })).toBe(
      "http://127.0.0.1:8787",
    );
  });

  it("requires an explicit production upstream", () => {
    expect(() => canonicalEdgeOrigin({ production: true, configured: undefined })).toThrow(
      "MYELIN_EDGE_URL is required in production",
    );
  });

  it.each(["https://edge.internal", "https://edge.internal/"])(
    "accepts and normalizes a production HTTPS origin: %s",
    (configured) => {
      expect(canonicalEdgeOrigin({ production: true, configured })).toBe(configured.replace(/\/$/, ""));
    },
  );

  it("allows explicit HTTP only for the local non-production harness", () => {
    expect(canonicalEdgeOrigin({ production: false, configured: "http://edge:8080" })).toBe(
      "http://edge:8080",
    );
    expect(() => canonicalEdgeOrigin({ production: true, configured: "http://edge:8080" })).toThrow(
      "MYELIN_EDGE_URL must use https:// in production",
    );
  });

  it.each([
    "edge.internal",
    "ftp://edge.internal",
    "https://user:secret@edge.internal",
    "https://edge.internal/v1",
    "https://edge.internal/?region=eu",
    "https://edge.internal/#health",
  ])("rejects a non-origin upstream: %s", (configured) => {
    expect(() => canonicalEdgeOrigin({ production: true, configured })).toThrow();
  });
});
