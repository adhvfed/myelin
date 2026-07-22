import { describe, expect, it } from "vitest";

import {
  parseRepoSummaryQuery,
  parsePrCommitsQuery,
  prCommitCursorExpiredEnvelope,
  prCommitsEnvelope,
  parseTreeQuery,
  refsJson,
  repoHomeJson,
  repoSummaryEnvelope,
  treeJson,
  validPrOperationId,
} from "../../dev-edge/dev-contract.mjs";

describe("the dev Edge PR commit pagination contract", () => {
  it("mints canonical fixed-frame cursors and serves a distinct terminal page", () => {
    const firstInput = parsePrCommitsQuery("myelin", 1, "limit=20");
    expect(firstInput).toEqual({ limit: 20, position: 0 });
    if (!firstInput) throw new Error("expected first-page input");
    const first = prCommitsEnvelope("myelin", 1, firstInput);
    if (!first || "expired" in first || !first.page.next_cursor) {
      throw new Error("expected continuation cursor");
    }
    expect(first.items).toHaveLength(20);
    expect(first.page.next_cursor).toMatch(/^pc1_[A-Za-z0-9_-]+$/);

    const secondInput = parsePrCommitsQuery(
      "myelin", 1, `cursor=${first.page.next_cursor}&limit=20`,
    );
    expect(secondInput).toEqual({
      limit: 20,
      position: 20,
      snapshot: {
        base_oid: "a1b2c3d4e5f60718293a4b5c6d7e8f9001122334",
        head_oid: "b2c3d4e5f60718293a4b5c6d7e8f900112233445",
        position: 20,
      },
    });
    if (!secondInput) throw new Error("expected continuation input");
    const second = prCommitsEnvelope("myelin", 1, secondInput);
    if (!second || "expired" in second) throw new Error("expected terminal continuation page");
    expect(second.items).toHaveLength(3);
    expect(second.page).toEqual({ next_cursor: null, limit: 20 });
    expect(new Set([...first.items, ...second.items].map((row) => row.oid)).size)
      .toBe(23);
  });

  it("rejects malformed, duplicate, unknown, and cross-scope PR commit coordinates", () => {
    const first = prCommitsEnvelope("myelin", 1, { limit: 20, position: 0 });
    const cursor = first && !("expired" in first) ? first.page.next_cursor : null;
    if (!cursor) throw new Error("expected fixture cursor");
    for (const query of [
      "limit=01", "limit=0", "limit=101", "limit=20&limit=20", "unknown=1",
      "cursor=", "cursor=opaque", `cursor=${cursor}=`, `cursor=${cursor}&cursor=${cursor}`,
      "x".repeat(16 * 1024 + 1),
    ]) expect(parsePrCommitsQuery("myelin", 1, query), query).toBeNull();
    expect(parsePrCommitsQuery("myelin", 2, `cursor=${cursor}&limit=20`)).toBeNull();
  });

  it("separates a canonical in-scope cursor with expired snapshot coordinates from malformed input", () => {
    const first = prCommitsEnvelope("myelin", 1, { limit: 20, position: 0 });
    const cursor = first && !("expired" in first) ? first.page.next_cursor : null;
    if (!cursor) throw new Error("expected fixture cursor");
    const frame = Buffer.from(cursor.slice(4), "base64url");
    frame[54] = (frame[54] ?? 0) ^ 0xff;
    const expiredCursor = `pc1_${frame.toString("base64url")}`;

    const input = parsePrCommitsQuery("myelin", 1, `cursor=${expiredCursor}&limit=20`);
    expect(input).toMatchObject({
      limit: 20,
      position: 20,
      snapshot: { head_oid: expect.not.stringMatching(/^b2c3d4/) },
    });
    if (!input) throw new Error("expected structurally valid expired cursor input");
    expect(prCommitsEnvelope("myelin", 1, input)).toEqual({ expired: true });
    expect(prCommitCursorExpiredEnvelope()).toEqual({
      error: { message: "pull request commit cursor expired", code: "conflict" },
    });
  });
});

describe("the dev Edge repository summary contract", () => {
  it("strictly parses summary coordinates and keyset-pages shape-separated fixtures", () => {
    expect(parseRepoSummaryQuery("view=summary&limit=1"))
      .toEqual({ limit: 1 });
    const first = repoSummaryEnvelope({ limit: 1 });
    if (!first) throw new Error("expected the first repository summary page");
    expect(first).toEqual({
      items: [{
        state: "populated",
        slug: "acme/myelin",
        clone_url: "/acme/eu-west/myelin.git",
      }],
      page: { next_cursor: "rl1_YWNtZS9teWVsaW4", limit: 1 },
    });
    expect(first.items[0]).not.toHaveProperty("default_branch");
    expect(first.items[0]).not.toHaveProperty("entries");

    const second = repoSummaryEnvelope({ limit: 1, cursor: first.page.next_cursor });
    expect(second).toEqual({
      items: [{ state: "empty", slug: "acme/sandbox" }],
      page: { next_cursor: null, limit: 1 },
    });
    expect(repoHomeJson("myelin")).toMatchObject({ default_branch: "main", entries: expect.any(Array) });
  });

  it("rejects unknown, duplicate, noncanonical, and out-of-bounds summary queries", () => {
    for (const query of [
      "", "view=home", "view=summary&view=summary", "view=summary&other=1",
      "view=summary&limit=01", "view=summary&limit=0", "view=summary&limit=101",
      "view=summary&cursor=", "view=summary&cursor=opaque",
      "view=summary&cursor=rl1_YR", `view=summary&cursor=rl1_${"a".repeat(512)}`,
      "x".repeat(16 * 1024 + 1),
    ]) expect(parseRepoSummaryQuery(query), query).toBeNull();
  });
});

describe("the dev Edge production-write contract", () => {
  it.each([
    "merge-operation-1",
    "550e8400-e29b-41d4-a716-446655440000",
    ` ${"x".repeat(128)} `,
  ])("accepts a production-valid PR operation id: %s", (value) => {
    expect(validPrOperationId(value)).toBe(true);
  });

  it.each([
    undefined,
    ["duplicate", "headers"],
    "",
    "   ",
    "contains space",
    "contains\nnewline",
    "ø",
    "x".repeat(129),
  ])("rejects a missing, ambiguous, or malformed PR operation id %#", (value) => {
    expect(validPrOperationId(value)).toBe(false);
  });
});

describe("the dev Edge refs contract", () => {
  it("mirrors pagination and keeps current/default pins outside search results", () => {
    const first = refsJson("myelin", {
      limit: 1,
      current: "refs/heads/feature",
    });

    expect(first).toMatchObject({
      branches: [{ name: "main", is_default: true }],
      tags: [],
      default_branch: "main",
      pinned: [
        { kind: "branch", full_name: "refs/heads/feature", name: "feature", is_default: false },
        { kind: "branch", full_name: "refs/heads/main", name: "main", is_default: true },
      ],
      page: { next_cursor: "gr1_1", limit: 1 },
    });

    expect(
      refsJson("myelin", {
        limit: 100,
        q: "v0",
        current: "refs/heads/feature",
      }),
    ).toMatchObject({
      branches: [],
      tags: [{ name: "v0.1" }],
      pinned: [
        { full_name: "refs/heads/feature" },
        { full_name: "refs/heads/main" },
      ],
      page: { next_cursor: null, limit: 100 },
    });
  });
});

describe("the dev Edge tree pagination contract", () => {
  it("strictly accepts only the Edge tree query grammar", () => {
    expect(parseTreeQuery("limit=%31&q=Readme+File&cursor=gt1_abc"))
      .toEqual({ limit: 1, q: "Readme File", cursor: "gt1_abc" });
    expect(parseTreeQuery("")).toEqual({ limit: 100 });
    for (const query of [
      "limit", "=1", "other=1", "q=a&q=b", "q=a&%71=b", "limit=01", "limit=0",
      "limit=101", "cursor=", "q=%", "q=%FF", "q=%00", "q=a&&limit=1",
      `q=${"x".repeat(257)}`, `cursor=${"x".repeat(8 * 1024 + 1)}`,
      "x".repeat(16 * 1024 + 1),
    ]) expect(parseTreeQuery(query), query).toBeNull();
  });

  it("pages and searches immediate basenames while omitting README after the first page", () => {
    const first = treeJson("myelin", "refs/heads/main", "", { limit: 1 });
    if (!first || !("page" in first) || !first.page) throw new Error("expected a modern first page");
    expect(first).toMatchObject({
      ref: "refs/heads/main",
      path: "",
      snapshot_oid: "b2c3d4e5f60718293a4b5c6d7e8f900112233445",
      entries: [{ name: "crates" }],
      readme: expect.stringContaining("# acme/myelin"),
      page: { limit: 1 },
    });
    const second = treeJson("myelin", "refs/heads/main", "", {
      limit: 1,
      cursor: first.page.next_cursor,
    });
    if (!second || !("entries" in second) || !second.entries) {
      throw new Error("expected a modern continuation page");
    }
    expect(second.entries).toHaveLength(1);
    expect(second).not.toHaveProperty("readme");

    const searched = treeJson("myelin", "refs/heads/main", "", { q: "  ReAdMe  " });
    if (!searched || !("entries" in searched) || !searched.entries) {
      throw new Error("expected a modern search page");
    }
    expect(searched.entries.map((entry) => entry.name)).toEqual(["README.md"]);
    expect(searched).not.toHaveProperty("readme");
  });

  it("distinguishes cursor scope errors from a moved snapshot", () => {
    const first = treeJson("myelin", "refs/heads/main", "", { limit: 1 });
    if (!first || !("page" in first) || !first.page || !first.page.next_cursor) {
      throw new Error("expected a modern first page cursor");
    }
    expect(treeJson("myelin", "refs/heads/main", "", {
      limit: 1, q: "other", cursor: first.page.next_cursor,
    })).toEqual({ __status: 400 });

    const encoded = first.page.next_cursor.slice("gt1_".length);
    const frame = JSON.parse(Buffer.from(encoded, "base64url").toString("utf8"));
    frame[4] = "1".repeat(40);
    const stale = `gt1_${Buffer.from(JSON.stringify(frame)).toString("base64url")}`;
    expect(treeJson("myelin", "refs/heads/main", "", { limit: 1, cursor: stale }))
      .toEqual({ __status: 409 });

    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    const last = encoded.at(-1);
    if (!last) throw new Error("expected a cursor frame");
    const index = alphabet.indexOf(last);
    const unusedBits = encoded.length % 4 === 2 ? 4 : encoded.length % 4 === 3 ? 2 : 0;
    if (!unusedBits) throw new Error("fixture cursor needs trailing base64url pad bits");
    const noncanonical = `${encoded.slice(0, -1)}${alphabet[index + 1]}`;
    expect(Buffer.from(noncanonical, "base64url").equals(Buffer.from(encoded, "base64url")))
      .toBe(true);
    expect(treeJson("myelin", "refs/heads/main", "", {
      limit: 1, cursor: `gt1_${noncanonical}`,
    })).toEqual({ __status: 400 });
  });

  it("serves repo-home continuation coordinates from the same snapshot", () => {
    expect(repoHomeJson("myelin")).toMatchObject({
      snapshot_oid: "b2c3d4e5f60718293a4b5c6d7e8f900112233445",
      entries_page: {
        ref: "refs/heads/main",
        next_cursor: null,
        limit: 100,
        snapshot_oid: "b2c3d4e5f60718293a4b5c6d7e8f900112233445",
      },
    });
  });
});
