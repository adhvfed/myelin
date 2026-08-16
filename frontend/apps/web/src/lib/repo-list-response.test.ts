import { describe, expect, it } from "vitest";

import { parseRepoHome } from "./repo-read-response";
import { parseRepoListPage } from "./repo-list-response";

const CURSOR = "rl2_AgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAGgAGMDFKMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDJteWVsaW4";

describe("repository catalogue response", () => {
  it("decodes summary rows without weakening the full RepoHome contract", () => {
    expect(parseRepoListPage({
      items: [
        { state: "populated", slug: "acme/core", clone_url: "/acme/eu/core.git" },
        { state: "empty", slug: "acme/sandbox" },
      ],
      page: { next_cursor: CURSOR, limit: 2 },
    })).toEqual({
      items: [
        { state: "populated", slug: "acme/core", clone_url: "/acme/eu/core.git" },
        { state: "empty", slug: "acme/sandbox" },
      ],
      page: { next_cursor: CURSOR, limit: 2 },
    });

    expect(parseRepoHome({ state: "populated", slug: "acme/core", clone_url: "/x" }))
      .toBeNull();
    expect(parseRepoHome({ state: "empty", slug: "acme/sandbox" })).toBeNull();
  });

  it.each([
    { items: [{ state: "populated", slug: "acme/core" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ state: "populated", slug: "acme/core", clone_url: "" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ state: "populated", slug: "acme/core", clone_url: "x\nsmuggled" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ state: "populated", slug: "acme/core", clone_url: "x\u0085smuggled" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ state: "populated", slug: "acme/core", clone_url: "x smuggled" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ state: "populated", slug: "acme/core", clone_url: "x\tsmuggled" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ state: "empty", slug: "../core" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ state: "empty", slug: "acme.git/core" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ state: "unknown", slug: "acme/core" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ state: "restricted" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ state: "empty", slug: "acme/core", default_branch: "main" }], page: { next_cursor: null, limit: 1 } },
    { items: [{ state: "populated", slug: "acme/core", clone_url: "/x", entries: [] }], page: { next_cursor: null, limit: 1 } },
    { items: [{ state: "empty", slug: "acme/a" }, { state: "empty", slug: "acme/b" }], page: { next_cursor: null, limit: 1 } },
    { items: [], page: { next_cursor: null, limit: 0 } },
    { items: [], page: { next_cursor: null, limit: 101 } },
    { items: [], page: { next_cursor: "opaque", limit: 1 } },
    { items: [], page: { next_cursor: "rl2_YR", limit: 1 } },
    { items: [], page: { next_cursor: null, limit: 1, total: 0 } },
    { items: [], page: { next_cursor: null, limit: 1 }, internal: true },
  ])("rejects malformed or out-of-bounds summary payload %#", (value) => {
    expect(parseRepoListPage(value)).toBeNull();
  });

  it("caps both row cardinality and clone URL bytes", () => {
    const rows = Array.from({ length: 100 }, (_, index) => ({
      state: "populated",
      slug: `acme/repo-${index}`,
      clone_url: `/${"x".repeat(4 * 1024 - 1)}`,
    }));
    expect(parseRepoListPage({ items: rows, page: { next_cursor: null, limit: 100 } })?.items)
      .toHaveLength(100);
    expect(parseRepoListPage({
      items: [{ state: "populated", slug: "acme/core", clone_url: "x".repeat(4 * 1024 + 1) }],
      page: { next_cursor: null, limit: 1 },
    })).toBeNull();
  });
});
