import { describe, expect, it } from "vitest";
import {
  refusesServerFunctionRead,
  requestMethodPolicy,
  routedPath,
  sameOriginVerdict,
} from "./csrf";

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

describe("requestMethodPolicy", () => {
  it.each(["GET", "head", "Options"])("allows the safe method %s", (method) => {
    expect(requestMethodPolicy(method)).toBe("safe");
  });

  it.each(["POST", "PUT", "PATCH", "DELETE", "PURGE", "PROPFIND", "CONNECT"])(
    "origin-checks the unsafe method %s",
    (method) => {
      expect(requestMethodPolicy(method)).toBe("verify-origin");
    },
  );

  it("rejects TRACE regardless of origin", () => {
    expect(requestMethodPolicy("TRACE")).toBe("reject");
  });
});

describe("refusesServerFunctionRead", () => {
  const read = (overrides: Partial<Parameters<typeof refusesServerFunctionRead>[0]>) =>
    refusesServerFunctionRead({
      method: "GET",
      rawTarget: "/_server/?id=a&name=b",
      instance: "server-fn:3",
      fetchSite: "same-origin",
      ...overrides,
    });

  // Every spelling h3 routes to the server-function handler.
  const routedSpellings = [
    "/_server?id=a&name=b",
    "/_server/?id=a&name=b",
    "/%5Fserver/?id=a&name=b",
    "/%5fserver/?id=a&name=b",
    "/_%73erver?id=a&name=b",
    "/_serverx?id=a&name=b",
    "/_server.json?id=a&name=b",
    "/_server/x/?id=a&name=b",
    "/_server/../?id=a&name=b",
    "/_server%2F?id=a&name=b",
    "/_server;x?id=a",
  ];

  it("decodes a path the way h3 does before routing it", () => {
    expect(routedPath("/%5Fserver/?id=a")).toBe("/_server/");
    expect(routedPath("/_%73erver")).toBe("/_server");
    expect(routedPath("/_server%2Fx")).toBe("/_server%2Fx");
    expect(routedPath("/bad%E0%A4%A")).toBe("/bad%E0%A4%A");
  });

  it("lets the app's own query reads through", () => {
    expect(read({})).toBe(false);
    expect(read({ fetchSite: null })).toBe(false);
  });

  it("refuses every routed spelling without SolidStart's header, as a cross-site link sends", () => {
    for (const rawTarget of routedSpellings) {
      for (const method of ["GET", "head", "OPTIONS", "PUT"]) {
        expect(read({ rawTarget, method, instance: null, fetchSite: "cross-site" }), rawTarget).toBe(
          true,
        );
        expect(read({ rawTarget, method, instance: null, fetchSite: null }), rawTarget).toBe(true);
      }
    }
  });

  it("refuses a read the browser says came from another site", () => {
    for (const rawTarget of routedSpellings) {
      expect(read({ rawTarget, fetchSite: "cross-site" }), rawTarget).toBe(true);
      expect(read({ rawTarget, fetchSite: "same-site" }), rawTarget).toBe(true);
    }
  });

  it("leaves POSTs (origin-checked) and paths h3 routes elsewhere alone", () => {
    expect(read({ method: "POST", instance: null })).toBe(false);
    for (const rawTarget of ["/profile", "/_Server/?id=a", "//_server/?id=a", "/api/_server"]) {
      expect(read({ rawTarget, instance: null }), rawTarget).toBe(false);
    }
  });
});
