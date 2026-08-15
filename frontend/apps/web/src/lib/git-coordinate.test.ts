import { describe, expect, it } from "vitest";

import {
  isGitPullRequestNumber,
  isGitRepositorySlug,
  MAX_GIT_REPOSITORY_SLUG_BYTES,
  normalizeGitRepositorySlug,
  parseGitPullRequestNumberText,
} from "./git-coordinate";

describe("Git coordinates", () => {
  it("shares the storage-safe repository grammar at every browser boundary", () => {
    for (const slug of ["core", "platform/api", "release.git"]) {
      expect(isGitRepositorySlug(slug), slug).toBe(true);
    }
    for (const slug of [
      "",
      "../core",
      "platform//api",
      "platform.git/api",
      "PLATFORM.GIT/api",
      "x".repeat(MAX_GIT_REPOSITORY_SLUG_BYTES + 1),
    ]) expect(isGitRepositorySlug(slug), slug).toBe(false);
    expect(normalizeGitRepositorySlug("  platform/api  ")).toBe("platform/api");
  });

  it("keeps JSON and path pull-request numbers in the same safe identity space", () => {
    expect(isGitPullRequestNumber(Number.MAX_SAFE_INTEGER)).toBe(true);
    expect(parseGitPullRequestNumberText(String(Number.MAX_SAFE_INTEGER)))
      .toBe(Number.MAX_SAFE_INTEGER);
    for (const value of ["", "0", "01", "1e2", "9007199254740992"]) {
      expect(parseGitPullRequestNumberText(value), value).toBeNull();
    }
    expect(isGitPullRequestNumber(Number.MAX_SAFE_INTEGER + 1)).toBe(false);
  });
});
