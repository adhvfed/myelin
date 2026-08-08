import { describe, expect, it } from "vitest";

import { parseBlame, splitRepositoryLines } from "./blame-response";

const FIRST = "0123456789abcdef0123456789abcdef01234567";
const SECOND = "89abcdef0123456789abcdef0123456789abcdef";

function response() {
  return {
    path: "src/main.rs",
    ref: "main",
    snapshot_oid: SECOND,
    contents: "fn main() {\n    run();\n}\n",
    hunks: [
      {
        start_line: 1,
        line_count: 1,
        commit: { oid: FIRST, summary: "Start service", author: "Ada", committed_at: 1 },
      },
      {
        start_line: 2,
        line_count: 2,
        commit: { oid: SECOND, summary: "Run workload", author: "Lin", committed_at: 2 },
      },
    ],
    internal: "drop",
  };
}

describe("blame response projection", () => {
  it("projects a complete snapshot and drops surplus fields", () => {
    expect(parseBlame(response())).toEqual({
      path: "src/main.rs",
      ref: "main",
      snapshot_oid: SECOND,
      contents: "fn main() {\n    run();\n}\n",
      hunks: [
        {
          start_line: 1,
          line_count: 1,
          commit: { oid: FIRST, summary: "Start service", author: "Ada", committed_at: 1 },
        },
        {
          start_line: 2,
          line_count: 2,
          commit: { oid: SECOND, summary: "Run workload", author: "Lin", committed_at: 2 },
        },
      ],
    });
  });

  it("uses Git line semantics for terminal newlines", () => {
    expect(splitRepositoryLines("")).toEqual([]);
    expect(splitRepositoryLines("one")).toEqual(["one"]);
    expect(splitRepositoryLines("one\n")).toEqual(["one"]);
    expect(splitRepositoryLines("one\ntwo\n")).toEqual(["one", "two"]);
  });

  it.each([
    { ...response(), path: "../secret" },
    { ...response(), snapshot_oid: "short" },
    { ...response(), hunks: [{ ...response().hunks[0], start_line: 2 }] },
    { ...response(), hunks: [response().hunks[0]] },
    { ...response(), hunks: [{ ...response().hunks[0], line_count: 0 }] },
    {
      ...response(),
      hunks: [{ ...response().hunks[0], commit: { ...response().hunks[0]!.commit, oid: "bad" } }],
    },
  ])("rejects unsafe or incomplete attribution %#", (value) => {
    expect(parseBlame(value)).toBeNull();
  });
});
