import { describe, expect, it } from "vitest";
import { sameOriginVerdict } from "./csrf";

const HOST = "app.myelin.dev";

describe("sameOriginVerdict (peer-review #21c — login-CSRF origin check)", () => {
  it("accepts a same-origin POST by Origin", () => {
    expect(
      sameOriginVerdict({ origin: `https://${HOST}`, referer: null, host: HOST }),
    ).toBe("ok");
    // The port is part of the host and must match exactly.
    expect(
      sameOriginVerdict({
        origin: "https://app.myelin.dev:8443",
        referer: null,
        host: "app.myelin.dev:8443",
      }),
    ).toBe("ok");
  });

  it("REJECTS a cross-site POST — the login-CSRF vector SameSite=Lax leaves open", () => {
    expect(
      sameOriginVerdict({ origin: "https://evil.example", referer: null, host: HOST }),
    ).toBe("reject");
    // Same registrable domain but different host still rejects (host equality, not eTLD+1).
    expect(
      sameOriginVerdict({ origin: `https://evil.${HOST}`, referer: null, host: HOST }),
    ).toBe("reject");
    // A differing port is a different origin.
    expect(
      sameOriginVerdict({ origin: `https://${HOST}:9999`, referer: null, host: HOST }),
    ).toBe("reject");
  });

  it("treats an opaque `Origin: null` as cross-site (sandboxed iframe / cross-site redirect POST)", () => {
    expect(
      sameOriginVerdict({ origin: "null", referer: null, host: HOST }),
    ).toBe("reject");
  });

  it("falls back to Referer only when Origin is absent", () => {
    expect(
      sameOriginVerdict({ origin: null, referer: `https://${HOST}/login`, host: HOST }),
    ).toBe("ok");
    expect(
      sameOriginVerdict({ origin: null, referer: "https://evil.example/x", host: HOST }),
    ).toBe("reject");
    // Origin present-and-bad wins over a same-origin Referer (never fall through to the weaker signal).
    expect(
      sameOriginVerdict({
        origin: "https://evil.example",
        referer: `https://${HOST}/login`,
        host: HOST,
      }),
    ).toBe("reject");
  });

  it("fails CLOSED when it cannot prove same-origin", () => {
    // No Host to compare against.
    expect(
      sameOriginVerdict({ origin: `https://${HOST}`, referer: null, host: null }),
    ).toBe("reject");
    // Neither Origin nor Referer — a browser form POST always sends at least one; absence is suspect.
    expect(sameOriginVerdict({ origin: null, referer: null, host: HOST })).toBe("reject");
    // Unparseable Origin.
    expect(
      sameOriginVerdict({ origin: "not a url", referer: null, host: HOST }),
    ).toBe("reject");
  });
});
