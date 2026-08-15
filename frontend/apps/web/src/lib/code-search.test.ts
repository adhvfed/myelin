import { describe, expect, it } from "vitest";
import { codeSearchHitHref, parseCodeSearchInput, parseCodeSearchPage } from "./code-search";

describe("code search boundary", () => {
  it("accepts the bounded response and builds a line link", () => {
    const page = parseCodeSearchPage({
      items: [{
        repo: "core",
        ref: "refs/heads/main",
        snapshot_oid: "a".repeat(40),
        path: "src/lib.ts",
        line: 7,
        excerpt: "export const ready = true;",
      }],
      page: { next_cursor: null, limit: 100 },
      complete: true,
    });
    expect(page).not.toBeNull();
    expect(codeSearchHitHref(page!.items[0]!)).toBe(
      "/git/repos/core/blob/refs%2Fheads%2Fmain/src/lib.ts#L7",
    );
  });

  it("rejects invalid inputs and untrusted response coordinates", () => {
    expect(parseCodeSearchInput({ q: "  " })).toBeNull();
    expect(parseCodeSearchInput({ q: "needle", repo: "../private" })).toBeNull();
    expect(parseCodeSearchInput({ q: "needle", repo: "platform.git/private" })).toBeNull();
    expect(parseCodeSearchPage({
      items: [{
        repo: "core",
        ref: "refs/heads/main",
        snapshot_oid: "a".repeat(40),
        path: "../secret",
        line: 1,
        excerpt: "hidden",
      }],
      page: { next_cursor: null, limit: 100 },
      complete: true,
    })).toBeNull();
  });
});
