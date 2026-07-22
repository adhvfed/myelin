import { describe, expect, it } from "vitest";

import {
  parseTreeQuery,
  refsJson,
  repoHomeJson,
  treeJson,
  validPrOperationId,
} from "../../dev-edge/dev-contract.mjs";

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
