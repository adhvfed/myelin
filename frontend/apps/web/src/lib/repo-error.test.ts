import { describe, expect, it } from "vitest";

import { mapPrDiffStatusToKind, mapStatusToKind, RepoRouteError } from "./repo-error";

describe("repository route error mapping", () => {
  it("treats only a PR-diff 413 as an intentional interactive-capacity state", () => {
    expect(mapPrDiffStatusToKind(413)).toBe("diff-too-large");
    expect(mapStatusToKind(413)).toBe("error");
    expect(mapPrDiffStatusToKind(403)).toBe("no-access");
    expect(mapPrDiffStatusToKind(404)).toBe("not-found");
    expect(mapPrDiffStatusToKind(503)).toBe("error");
  });

  it("serializes the safe category without carrying raw edge detail", () => {
    const error = new RepoRouteError("diff-too-large");
    expect(error.message).toBe("REPO_ERR:diff-too-large");
    expect(error.kind).toBe("diff-too-large");
  });
});
