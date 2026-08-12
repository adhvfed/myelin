import { describe, expect, it } from "vitest";

import { artifactRefHref, artifactRefLabel, parseArtifactRef } from "./artifact-ref";

describe("artifact references", () => {
  it("keeps the canonical root while understanding a structured sub-reference", () => {
    expect(parseArtifactRef("myelin://acme/knowledge/page/01J00000000000000000000000#b42"))
      .toEqual({
        tenant: "acme",
        subsystem: "knowledge",
        type: "page",
        id: "01J00000000000000000000000",
        sub: "b42",
        root: "myelin://acme/knowledge/page/01J00000000000000000000000",
      });
  });

  it.each([
    "https://example.invalid/issue/7",
    "myelin://acme/issue/issue",
    "myelin://acme/invented/issue/MYL-7",
    "myelin://acme/issue/invented/MYL-7",
    "myelin://acme/issue/issue/MYL-7#step-01",
    "myelin://acme/issue/issue/MYL-7#unknown-anchor",
    "myelin://acme/issue/issue/MYL-7 extra",
    `myelin://acme/issue/issue/${"x".repeat(4 * 1024)}`,
  ])("refuses a non-canonical reference: %s", (value) => {
    expect(parseArtifactRef(value)).toBeNull();
  });

  it("gives known work concise labels and local destinations", () => {
    expect(artifactRefLabel("myelin://acme/issue/issue/MYL-7")).toBe("MYL-7");
    expect(artifactRefHref("myelin://acme/issue/issue/MYL-7")).toBe("/issues?state=all&key=MYL-7");
    expect(artifactRefLabel("myelin://acme/git/pr/platform:42")).toBe("platform #42");
    expect(artifactRefHref("myelin://acme/git/pr/platform:42")).toBe("/git/repos/platform/prs/42");
  });
});
