import { describe, expect, it } from "vitest";

import {
  InitialTreeReader,
  repoHomeContinuationHref,
  treeCursorValue,
  treeHref,
  treeLimitValue,
  treeReloadHref,
  treeSearchValue,
} from "./tree-browse-state";

const OID = "0123456789abcdef0123456789abcdef01234567";

describe("tree browse URL state", () => {
  it("fetches the unfiltered README page once across search and page transitions", async () => {
    const requests: Array<{ repo: string; ref: string; path: string; limit?: number }> = [];
    const reader = new InitialTreeReader(async (input) => {
      requests.push(input);
      return {
        ref: input.ref,
        path: input.path,
        entries: [],
        readme: "# README",
        snapshot_oid: OID,
        page: { next_cursor: null, limit: 100 },
      };
    });
    const coordinates = { repo: "core", ref: "refs/heads/main", path: "src" };

    await reader.read(coordinates);
    treeHref({ ...coordinates, q: "readme" });
    await reader.read(coordinates);
    treeHref({ ...coordinates, q: "readme", cursor: "gt1_next" });
    await reader.read(coordinates);

    expect(requests).toEqual([{ ...coordinates, limit: 100 }]);
    await reader.read({ ...coordinates, path: "src/lib" });
    expect(requests).toHaveLength(2);
  });

  it("reflects a server search and cursor in a qualified tree URL", () => {
    expect(treeHref({
      repo: "core",
      ref: "refs/heads/main",
      path: "src/lib",
      cursor: "gt1_a-b_c",
      q: "readme & docs",
    })).toBe(
      "/git/repos/core/tree/refs%2Fheads%2Fmain/src/lib?cursor=gt1_a-b_c&q=readme+%26+docs",
    );
  });

  it("restores only canonical URL pagination values", () => {
    expect(treeSearchValue("readme")).toBe("readme");
    expect(treeSearchValue(["a", "b"])).toBe("");
    expect(treeCursorValue("gt1_a-b_c")).toBe("gt1_a-b_c");
    expect(treeCursorValue("opaque")).toBeUndefined();
    expect(treeCursorValue(`gt1_${"a".repeat(8 * 1024 - 4)}`)).toHaveLength(8 * 1024);
    expect(treeCursorValue(`gt1_${"a".repeat(8 * 1024 - 3)}`)).toBeUndefined();
    expect(treeSearchValue("ø".repeat(128))).toBe("ø".repeat(128));
    expect(treeSearchValue("ø".repeat(129))).toBe("");
    expect(treeSearchValue("bad\nquery")).toBe("");
    expect(treeLimitValue("1")).toBe(1);
    expect(treeLimitValue("100")).toBe(100);
    expect(treeLimitValue("01")).toBe(100);
  });

  it("reloads a stale filtered page without its cursor", () => {
    expect(treeReloadHref({
      repo: "core",
      ref: "refs/heads/main",
      path: "src",
      limit: 25,
      q: "readme & docs",
      cursor: "gt1_stale",
    })).toBe(
      "/git/repos/core/tree/refs%2Fheads%2Fmain/src?limit=25&q=readme+%26+docs",
    );
  });

  it("uses repo-home's exact qualified ref and cursor for continuation", () => {
    expect(repoHomeContinuationHref("core", {
      ref: "refs/heads/main",
      next_cursor: "gt1_next",
      limit: 25,
      snapshot_oid: OID,
    })).toBe(
      "/git/repos/core/tree/refs%2Fheads%2Fmain?limit=25&cursor=gt1_next",
    );
  });
});
