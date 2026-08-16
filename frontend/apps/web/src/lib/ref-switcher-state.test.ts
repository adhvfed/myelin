import { describe, expect, it } from "vitest";

import type { GitRefsInput } from "./git-read-input";
import type { RefsVM } from "./api";
import {
  REF_SWITCHER_ROW_CAP,
  RefSwitcherController,
  friendlyRefName,
  refOptionHref,
  visibleRefGroups,
  type RefSwitcherSnapshot,
  type SwitchRefRow,
} from "./ref-switcher-state";

const OID = "0123456789abcdef0123456789abcdef01234567";

function page(
  names: string[],
  next: string | null,
  pinned: RefsVM["pinned"] = [],
): RefsVM {
  return {
    branches: names.map((name) => ({ name, oid: OID, is_default: name === "main" })),
    tags: [],
    default_branch: "main",
    pinned,
    page: { next_cursor: next, limit: 100 },
  };
}

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  return { promise: new Promise<T>((done) => { resolve = done; }), resolve };
}

describe("RefSwitcher pagination state", () => {
  it("routes same-named branches and tags through distinct qualified refs", () => {
    const branch: SwitchRefRow = {
      kind: "branch", fullName: "refs/heads/release", name: "release", oid: OID,
      isDefault: false,
    };
    const tag: SwitchRefRow = {
      kind: "tag", fullName: "refs/tags/release", name: "release", oid: OID,
      isDefault: false,
    };
    const hrefFor = (ref: string) => `/tree/${encodeURIComponent(ref)}`;
    expect(refOptionHref(branch, hrefFor)).toBe("/tree/refs%2Fheads%2Frelease");
    expect(refOptionHref(tag, hrefFor)).toBe("/tree/refs%2Ftags%2Frelease");
    expect(refOptionHref(branch, hrefFor)).not.toBe(refOptionHref(tag, hrefFor));
    expect(friendlyRefName(branch.fullName)).toBe("release");
    expect(friendlyRefName(tag.fullName)).toBe("release");
  });
  it("resets pages on search and ignores a stale older response", async () => {
    const requests: GitRefsInput[] = [];
    const first = deferred<RefsVM>();
    const second = deferred<RefsVM>();
    const snapshots: RefSwitcherSnapshot[] = [];
    const controller = new RefSwitcherController((input) => {
      requests.push(input);
      return requests.length === 1 ? first.promise : second.promise;
    }, (snapshot) => snapshots.push(snapshot));

    const oldSearch = controller.search({ repo: "core", query: "old" });
    const newSearch = controller.search({
      repo: "core", query: "new", current: "refs/heads/main",
    });
    expect(controller.snapshot()).toMatchObject({ query: "new", rows: [], nextCursor: null });
    second.resolve(page(["new-result"], null));
    await newSearch;
    first.resolve(page(["stale-result"], null));
    await oldSearch;

    expect(controller.snapshot().rows.map((row) => row.name)).toEqual(["new-result"]);
    expect(requests).toEqual([
      { repo: "core", limit: 100, q: "old" },
      { repo: "core", limit: 100, q: "new", current: "refs/heads/main" },
    ]);
    expect(snapshots.some((snapshot) => snapshot.query === "new" && snapshot.rows.length === 0))
      .toBe(true);
  });

  it("clears loaded rows as soon as a debounced query is prepared", async () => {
    const controller = new RefSwitcherController(async () => page(["old"], null), () => {});
    await controller.search({ repo: "core", query: "old" });
    controller.prepare({ repo: "core", query: "new" });
    expect(controller.snapshot()).toMatchObject({
      query: "new", rows: [], pins: [], nextCursor: null, loading: true,
    });
  });

  it("loads more, deduplicates pinned refs from rows, and preserves server search semantics", async () => {
    const requests: GitRefsInput[] = [];
    const pin = {
      kind: "branch" as const, full_name: "refs/heads/main", name: "main", oid: OID,
      is_default: true,
    };
    const responses = [page(["main", "one"], "gr1_next", [pin]), page(["two"], null, [pin])];
    const controller = new RefSwitcherController(async (input) => {
      requests.push(input);
      return responses.shift()!;
    }, () => {});

    await controller.search({ repo: "core", query: "server query" });
    await controller.loadMore();
    const groups = visibleRefGroups(controller.snapshot());
    expect(groups.pins.map((row) => row.name)).toEqual(["main"]);
    expect(groups.branches.map((row) => row.name)).toEqual(["one", "two"]);
    expect(requests[1]).toEqual({
      repo: "core", limit: 100, q: "server query", cursor: "gr1_next",
    });
  });

  it("caps retained rows at 300 and stops offering further pages", async () => {
    let pageNumber = 0;
    const controller = new RefSwitcherController(async () => {
      const start = pageNumber++ * 100;
      return page(
        Array.from({ length: 100 }, (_, offset) => `branch-${start + offset}`),
        `gr1_${pageNumber}`,
      );
    }, () => {});

    await controller.search({ repo: "core", query: "" });
    await controller.loadMore();
    await controller.loadMore();
    expect(controller.snapshot()).toMatchObject({
      nextCursor: null, capped: true,
    });
    expect(controller.snapshot().rows).toHaveLength(REF_SWITCHER_ROW_CAP);
    await controller.loadMore();
    expect(pageNumber).toBe(3);
  });

  it("keeps current/default pins visible when a server page has no matching rows", async () => {
    const pins: RefsVM["pinned"] = [
      { kind: "branch", full_name: "refs/heads/current", name: "current", oid: OID, is_default: false },
      { kind: "branch", full_name: "refs/heads/main", name: "main", oid: OID, is_default: true },
    ];
    const controller = new RefSwitcherController(async () => page([], null, pins), () => {});
    await controller.search({ repo: "core", query: "no-match", current: "refs/heads/current" });
    const groups = visibleRefGroups(controller.snapshot());
    expect(groups.pins.map((row) => row.fullName)).toEqual([
      "refs/heads/current", "refs/heads/main",
    ]);
    expect(groups.branches).toEqual([]);
    expect(groups.tags).toEqual([]);
  });
});
