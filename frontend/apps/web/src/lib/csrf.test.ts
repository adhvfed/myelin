import { describe, expect, it } from "vitest";
import { sameOriginVerdict } from "./csrf";

const HOST = "app.myelin.dev";
const ORIGIN = `https://${HOST}`;

describe("sameOriginVerdict (peer-review #21c — login-CSRF origin check)", () => {
  it("accepts a same-origin POST by Origin", () => {
    expect(
      sameOriginVerdict({ origin: ORIGIN, referer: null, expectedOrigin: ORIGIN }),
    ).toBe("ok");
    // The port is part of the host and must match exactly.
    expect(
      sameOriginVerdict({
        origin: "https://app.myelin.dev:8443",
        referer: null,
        expectedOrigin: "https://app.myelin.dev:8443",
      }),
    ).toBe("ok");
  });

  it("REJECTS a cross-origin POST", () => {
    expect(
      sameOriginVerdict({ origin: "https://evil.example", referer: null, expectedOrigin: ORIGIN }),
    ).toBe("reject");
    // Same registrable domain but different host still rejects (host equality, not eTLD+1).
    expect(
      sameOriginVerdict({ origin: `https://evil.${HOST}`, referer: null, expectedOrigin: ORIGIN }),
    ).toBe("reject");
    // A differing port is a different origin.
    expect(
      sameOriginVerdict({ origin: `https://${HOST}:9999`, referer: null, expectedOrigin: ORIGIN }),
    ).toBe("reject");
    // A scheme downgrade on the same host is also a different origin.
    expect(
      sameOriginVerdict({ origin: `http://${HOST}`, referer: null, expectedOrigin: ORIGIN }),
    ).toBe("reject");
  });

  it("treats an opaque `Origin: null` as cross-site (sandboxed iframe / cross-site redirect POST)", () => {
    expect(
      sameOriginVerdict({ origin: "null", referer: null, expectedOrigin: ORIGIN }),
    ).toBe("reject");
  });

  it("falls back to Referer only when Origin is absent", () => {
    expect(
      sameOriginVerdict({ origin: null, referer: `${ORIGIN}/login`, expectedOrigin: ORIGIN }),
    ).toBe("ok");
    expect(
      sameOriginVerdict({ origin: null, referer: "https://evil.example/x", expectedOrigin: ORIGIN }),
    ).toBe("reject");
    // Origin present-and-bad wins over a same-origin Referer (never fall through to the weaker signal).
    expect(
      sameOriginVerdict({
        origin: "https://evil.example",
        referer: `${ORIGIN}/login`,
        expectedOrigin: ORIGIN,
      }),
    ).toBe("reject");
  });

  it("fails CLOSED when it cannot prove same-origin", () => {
    // No canonical deployment origin to compare against.
    expect(
      sameOriginVerdict({ origin: ORIGIN, referer: null, expectedOrigin: null }),
    ).toBe("reject");
    // Neither Origin nor Referer — a browser form POST always sends at least one; absence is suspect.
    expect(sameOriginVerdict({ origin: null, referer: null, expectedOrigin: ORIGIN })).toBe("reject");
    // Unparseable Origin.
    expect(
      sameOriginVerdict({ origin: "not a url", referer: null, expectedOrigin: ORIGIN }),
    ).toBe("reject");
  });
});
