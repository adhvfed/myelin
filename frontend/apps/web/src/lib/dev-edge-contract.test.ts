import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import {
  parseRepoListQuery,
  parsePrCommitsQuery,
  prChecksJson,
  prDiffCapacityEnvelope,
  prCommitCursorExpiredEnvelope,
  prCommitsEnvelope,
  prJson,
  prThreadsJson,
  parseTreeQuery,
  fileLinesJson,
  refsJson,
  repoHomeJson,
  repoListEnvelope,
  SEED_REFS,
  treeJson,
  validPrOperationId,
} from "../../dev-edge/dev-contract.mjs";

const LIST_FILTER_BLOB = "c3d4e5f60718293a4b5c6d7e8f90011223344556";
const HEAD_COMMIT = "b2c3d4e5f60718293a4b5c6d7e8f900112233445";

// Both this consumer and the Rust provider integration load this exact committed artifact.
const GIT_READ_GOLDEN_PATH = "contracts/git-read-dev-edge.golden.json";
const gitReadGolden = JSON.parse(readFileSync(
  new URL(`../../../../../${GIT_READ_GOLDEN_PATH}`, import.meta.url),
  "utf8",
)) as {
  contract_id: string;
  vectors: Array<{
    id: string;
    endpoint: "refs" | "tree";
    after?: string;
    mutation?: "add-ref";
    request: { limit: number; current?: string; ref?: string; path?: string; q?: string };
    expected: Record<string, unknown>;
  }>;
  capacity_vectors: Array<{
    id: string;
    endpoint: "pr-diff";
    request: { repo: string; number: number };
    expected: { status: number; body: Record<string, unknown> };
  }>;
};

describe("the dev Edge PR commit pagination contract", () => {
  it("keeps namespaced PR coordinates openable across every overview read", () => {
    const repo = "platform/myelin";
    expect(prJson(repo, 1)).toMatchObject({
      number: 1,
      ref: "myelin://acme/git/pr/platform/myelin:1",
    });
    expect(prChecksJson(repo, 1)).toMatchObject({ gate_admitted: false });
    expect(prThreadsJson(repo, 1)).toMatchObject({ durable: true });
    expect(prCommitsEnvelope(repo, 1, { limit: 20, position: 0 }))
      .toMatchObject({ items: expect.any(Array), page: { limit: 20 } });
    expect(prJson("platform.git/myelin", 1)).toBeNull();
  });

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

describe("the dev Edge expand-context object contract", () => {
  it("serves only the projected blob object, never an arbitrary or head commit oid", () => {
    expect(fileLinesJson("myelin", LIST_FILTER_BLOB, 5, 6)).toEqual({
      lines: [
        { origin: " ", content: "    // context line 5", old_no: null, new_no: 5 },
        { origin: " ", content: "    // context line 6", old_no: null, new_no: 6 },
      ],
    });
    expect(fileLinesJson("myelin", HEAD_COMMIT, 5, 6)).toBeNull();
    expect(fileLinesJson("other", LIST_FILTER_BLOB, 5, 6)).toBeNull();
  });
});

describe("the dev Edge repository list contract", () => {
  it("strictly parses coordinates and keyset-pages shape-separated fixtures", () => {
    expect(parseRepoListQuery("limit=1"))
      .toEqual({ limit: 1 });
    const first = repoListEnvelope({ limit: 1 });
    if (!first) throw new Error("expected the first repository list page");
    expect(first).toEqual({
      items: [{
        state: "populated",
        slug: "acme/myelin",
        clone_url: "/acme/eu-west/myelin.git",
      }],
      page: {
        next_cursor:
          "rl2_AgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAGgAGMDFKMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDJteWVsaW4",
        limit: 1,
      },
    });
    expect(first.items[0]).not.toHaveProperty("default_branch");
    expect(first.items[0]).not.toHaveProperty("entries");

    const second = repoListEnvelope({ limit: 1, cursor: first.page.next_cursor });
    expect(second).toEqual({
      items: [{ state: "empty", slug: "acme/sandbox" }],
      page: { next_cursor: null, limit: 1 },
    });
    expect(repoHomeJson("myelin")).toMatchObject({
      ref: "myelin://acme/git/repo/myelin",
      default_branch: "main",
      entries: expect.any(Array),
    });
  });

  it("accepts defaults and rejects unknown, duplicate, noncanonical, and out-of-bounds queries", () => {
    expect(parseRepoListQuery("")).toEqual({ limit: 50 });
    for (const query of [
      "view=summary", "other=1", "limit=1&limit=1",
      "limit=01", "limit=0", "limit=101",
      "cursor=", "cursor=opaque",
      "cursor=rl2_YR", `cursor=rl2_${"a".repeat(512)}`,
      "x".repeat(16 * 1024 + 1),
    ]) expect(parseRepoListQuery(query), query).toBeNull();
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
      branches: [{ name: "A", is_default: false }],
      tags: [],
      default_branch: "main",
      pinned: [
        { kind: "branch", full_name: "refs/heads/feature", name: "feature", is_default: false },
        { kind: "branch", full_name: "refs/heads/main", name: "main", is_default: true },
      ],
      page: { next_cursor: expect.stringMatching(/^gr1_[A-Za-z0-9_-]+$/), limit: 1 },
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

describe("the shared Git read golden contract", () => {
  it("matches the same request/response vectors as the Rust Edge integration", () => {
    expect(gitReadGolden.contract_id).toBe("git-read-dev-edge-parity");
    const cursors = new Map<string, string>();
    let refsNamespace = [...SEED_REFS];

    for (const vector of gitReadGolden.vectors) {
      const cursor = vector.after ? cursors.get(vector.after) : undefined;
      if (vector.after && !cursor) throw new Error(`missing cursor from ${vector.after}`);
      if (vector.mutation === "add-ref") {
        refsNamespace = [...refsNamespace, {
          kind: "tag",
          full_name: "refs/tags/stale-add",
          name: "stale-add",
          oid: SEED_REFS[0]?.oid ?? "b2c3d4e5f60718293a4b5c6d7e8f900112233445",
          is_default: false,
        }];
      }

      const response = vector.endpoint === "refs"
        ? refsJson("myelin", { ...vector.request, cursor }, refsNamespace)
        : treeJson(
            "myelin",
            vector.request.ref ?? "refs/heads/main",
            vector.request.path ?? "",
            { limit: vector.request.limit, q: vector.request.q, cursor },
          );
      if (!response) throw new Error(`no response for ${vector.id}`);
      const status = "__status" in response ? response.__status : 200;
      let normalized: Record<string, unknown> = { status };
      if (status === 200 && vector.endpoint === "refs" && "branches" in response) {
        const refs = response as {
          branches: Array<{ name: string }>;
          tags: Array<{ name: string }>;
          default_branch: string;
          pinned: Array<{ full_name: string }>;
          page: { next_cursor: string | null; limit: number };
        };
        const next = refs.page.next_cursor;
        if (next) {
          expect(next).toMatch(/^gr1_[A-Za-z0-9_-]+$/);
          expect(next).not.toMatch(/^gr1_\d+$/);
          cursors.set(vector.id, next);
        }
        normalized = {
          status,
          branch_names: refs.branches.map((row) => row.name),
          tag_names: refs.tags.map((row) => row.name),
          default_branch: refs.default_branch,
          pinned_full_names: refs.pinned.map((row) => row.full_name),
          limit: refs.page.limit,
          next_cursor: next ? "gr1_<opaque>" : null,
        };
      } else if (status === 200 && vector.endpoint === "tree" && "entries" in response) {
        const tree = response as {
          entries: Array<{ name: string }>;
          path: string;
          page: { next_cursor: string | null; limit: number };
        };
        const next = tree.page.next_cursor;
        if (next) cursors.set(vector.id, next);
        normalized = {
          status,
          entry_names: tree.entries.map((row) => row.name),
          path: tree.path,
          limit: tree.page.limit,
          next_cursor: next ? "gt1_<opaque>" : null,
        };
      }
      expect(normalized, vector.id).toEqual(vector.expected);
    }
  });

  it("pins the bounded PR-diff response consumed by the browser mock", () => {
    expect(gitReadGolden.capacity_vectors).toHaveLength(1);
    const vector = gitReadGolden.capacity_vectors[0];
    expect(vector).toMatchObject({
      id: "pr-diff-too-large",
      endpoint: "pr-diff",
      request: { repo: "myelin", number: 5 },
      expected: { status: 413 },
    });
    expect(prDiffCapacityEnvelope()).toEqual(vector?.expected.body);
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
