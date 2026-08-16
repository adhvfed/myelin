import { describe, expect, it } from "vitest";

import { repoListHref, repoListInputFromSearch } from "./repo-list-state";

describe("repository list URL state", () => {
  it("round-trips a page coordinate without appending prior rows", () => {
    const cursor = "rl2_AgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAGgAGMDFKMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDJteWVsaW4";
    expect(repoListInputFromSearch("25", cursor)).toEqual({ limit: 25, cursor });
    expect(repoListHref({ limit: 25, cursor })).toBe(`/git/repos?limit=25&cursor=${cursor}`);
    expect(repoListHref({})).toBe("/git/repos");
  });

  it.each([
    ["01", undefined],
    ["0", undefined],
    ["101", undefined],
    [["1", "2"], undefined],
    ["1", ["rl2_YQ", "rl2_Yg"]],
    ["1", "opaque"],
  ])("rejects malformed or duplicate URL coordinates %#", (limit, cursor) => {
    expect(repoListInputFromSearch(limit, cursor)).toBeNull();
  });
});
