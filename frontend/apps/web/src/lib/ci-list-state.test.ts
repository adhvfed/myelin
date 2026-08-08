import { describe, expect, it } from "vitest";

import { ciRunsHref, ciRunsInputFromSearch, CI_WEB_PAGE_LIMIT } from "./ci-list-state";

const CURSOR = (() => {
  const frame = Buffer.alloc(60);
  frame[0] = 1;
  frame.write("2026-07-24T12:00:00.000000Z", 1, "ascii");
  return `cr1_${frame.toString("base64url")}`;
})();

describe("CI run-list URL state", () => {
  it("round-trips filter and keyset coordinates without retaining an old cursor on filter change", () => {
    expect(ciRunsInputFromSearch("failed", "1", CURSOR)).toEqual({
      state: "failed",
      limit: 1,
      cursor: CURSOR,
    });
    expect(ciRunsHref({ state: "failed", limit: 1, cursor: CURSOR }))
      .toBe(`/ci?state=failed&limit=1&cursor=${CURSOR}`);
    expect(ciRunsHref({ state: "running" })).toBe("/ci?state=running");
    expect(ciRunsHref({ state: "all" })).toBe("/ci");
    expect(ciRunsInputFromSearch(undefined, undefined, undefined))
      .toEqual({ limit: CI_WEB_PAGE_LIMIT });
  });

  it.each([
    ["passed", undefined, undefined],
    [["failed", "running"], undefined, undefined],
    ["all", "01", undefined],
    ["all", ["1", "2"], undefined],
    ["all", "25", ["a", "b"]],
    ["all", "25", "opaque"],
  ])("rejects malformed or duplicate URL coordinates %#", (state, limit, cursor) => {
    expect(ciRunsInputFromSearch(state, limit, cursor)).toBeNull();
  });
});
