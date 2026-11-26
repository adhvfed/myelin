import { describe, expect, it } from "vitest";

import { checkServerFunctionMount } from "./server-function-mount";

/** The server app's handler routes when it is served from `/`, as Nitro mounts them. */
const MOUNTS = ["", "", "/_server", "", ""];
const ROUTERS = [
  { name: "ssr", base: "/" },
  { name: "server-fns", base: "/_server" },
];

describe("checkServerFunctionMount", () => {
  it("accepts the mount the guard was written for", () => {
    expect(() =>
      checkServerFunctionMount([
        { name: "public", base: "/" },
        { name: "ssr", base: "/" },
        { name: "client", base: "/_build" },
        { name: "server-fns", base: "/_server" },
      ], MOUNTS),
    ).not.toThrow();
  });

  it("refuses a handler Nitro's base URL moved, which the guard would not see", () => {
    for (const mounts of [["", "/app/_server", "/app"], ["", "/_server", "/app/_server"], [""]]) {
      expect(() => checkServerFunctionMount(ROUTERS, mounts)).toThrow(/NITRO_APP_BASE_URL/);
    }
  });

  it("refuses a moved app or handler, and a missing configuration", () => {
    expect(() =>
      checkServerFunctionMount([
        { name: "ssr", base: "/app" },
        { name: "server-fns", base: "/app/_server" },
      ], MOUNTS),
    ).toThrow(/served from \//);
    expect(() =>
      checkServerFunctionMount([
        { name: "ssr", base: "/" },
        { name: "server-fns", base: "/_fns" },
      ], MOUNTS),
    ).toThrow(/_server/);
    expect(() => checkServerFunctionMount(undefined, MOUNTS)).toThrow(/unavailable/);
    expect(() => checkServerFunctionMount(ROUTERS, undefined)).toThrow(/unavailable/);
  });
});
