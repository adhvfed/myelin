import { describe, expect, it } from "vitest";

import { checkServerFunctionMount } from "./server-function-mount";

describe("checkServerFunctionMount", () => {
  it("accepts the mount the guard was written for", () => {
    expect(() =>
      checkServerFunctionMount([
        { name: "public", base: "/" },
        { name: "ssr", base: "/" },
        { name: "client", base: "/_build" },
        { name: "server-fns", base: "/_server" },
      ]),
    ).not.toThrow();
  });

  it("refuses a moved app or handler, and a missing configuration", () => {
    expect(() =>
      checkServerFunctionMount([
        { name: "ssr", base: "/app" },
        { name: "server-fns", base: "/app/_server" },
      ]),
    ).toThrow(/served from \//);
    expect(() =>
      checkServerFunctionMount([
        { name: "ssr", base: "/" },
        { name: "server-fns", base: "/_fns" },
      ]),
    ).toThrow(/_server/);
    expect(() => checkServerFunctionMount(undefined)).toThrow(/unavailable/);
  });
});
