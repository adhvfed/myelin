import { describe, expect, it } from "vitest";

import {
  gitBlobPath,
  gitRepositoryPath,
  parseGitPullRequestRouteParam,
  parseGitRepositoryRouteParam,
} from "./git-route";

describe("Git repository routes", () => {
  it("round-trips a hierarchical slug through one route segment", () => {
    const path = gitRepositoryPath("platform/myelin");
    expect(path).toBe("/git/repos/platform%2Fmyelin");
    expect(parseGitRepositoryRouteParam(path.slice("/git/repos/".length)))
      .toBe("platform/myelin");
  });

  it("keeps a nested file path below one encoded repository and ref", () => {
    expect(gitBlobPath("platform/myelin", "refs/heads/main", ".myelin/ci.toml"))
      .toBe("/git/repos/platform%2Fmyelin/blob/refs%2Fheads%2Fmain/.myelin/ci.toml");
  });

  it.each([
    "",
    "%",
    "platform%252Fmyelin",
    "platform%2F..%2Fmyelin",
    "platform.git%2Fmyelin",
  ])("rejects an invalid or multiply encoded route segment: %s", (value) => {
    expect(parseGitRepositoryRouteParam(value)).toBeNull();
  });

  it("accepts only canonical browser-safe pull request numbers", () => {
    expect(parseGitPullRequestRouteParam("42")).toBe(42);
    for (const value of ["", "0", "01", "1.5", "1e2", "9007199254740992"]) {
      expect(parseGitPullRequestRouteParam(value), value).toBeNull();
    }
  });
});
