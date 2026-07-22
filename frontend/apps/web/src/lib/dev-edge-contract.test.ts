import { describe, expect, it } from "vitest";

import { refsJson, validPrOperationId } from "../../dev-edge/dev-contract.mjs";

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
