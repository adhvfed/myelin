import { describe, expect, it } from "vitest";
import { transportVerdict } from "./transport-security";

describe("transportVerdict", () => {
  it("does not impose a proxy contract on local development", () => {
    expect(transportVerdict(false, "POST", null)).toBe("allow");
  });

  it("accepts an exact HTTPS assertion from the production proxy", () => {
    expect(transportVerdict(true, "GET", "https")).toBe("allow");
    expect(transportVerdict(true, "POST", " HTTPS ")).toBe("allow");
  });

  it.each([null, "http", "https, http", "ftp"])(
    "redirects insecure production navigations: %s",
    (forwardedProto) => {
      expect(transportVerdict(true, "GET", forwardedProto)).toBe("redirect");
      expect(transportVerdict(true, "HEAD", forwardedProto)).toBe("redirect");
    },
  );

  it.each(["POST", "PUT", "PATCH", "DELETE", "OPTIONS", "PURGE"])(
    "rejects an insecure %s instead of replaying its request",
    (method) => {
      expect(transportVerdict(true, method, "http")).toBe("reject");
    },
  );
});
