import { describe, expect, it } from "vitest";

import {
  artifactRefHref,
  artifactRefLabel,
  parseArtifactRef,
  relatedArtifactRefError,
} from "./artifact-ref";

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
    "myelin://acme/issue/issue/MYL-7#step-18446744073709551616",
    "myelin://acme/issue/issue/MYL-7#L10-L9",
    "myelin://acme/issue/issue/MYL-7#L18446744073709551615-L18446744073709551616",
    "myelin://acme/issue/issue/MYL-7#unknown-anchor",
    "myelin://acme/knowledge/page/platform/api",
    "myelin://acme/git/repo/platform//api",
    "myelin://acme/issue/issue/MYL-7 extra",
    `myelin://acme/issue/issue/${"x".repeat(4 * 1024)}`,
  ])("refuses a non-canonical reference: %s", (value) => {
    expect(parseArtifactRef(value)).toBeNull();
  });

  it.each([
    "myelin://acme/issue/issue/MYL-7#step-18446744073709551615",
    "myelin://acme/git/blob/platform#L9-L10",
    "myelin://acme/git/blob/platform#L18446744073709551615-L18446744073709551615",
  ])("accepts a bounded numeric sub-reference: %s", (value) => {
    expect(parseArtifactRef(value)).not.toBeNull();
  });

  it("gives known work concise labels and local destinations", () => {
    expect(artifactRefLabel("myelin://acme/issue/issue/MYL-7")).toBe("MYL-7");
    expect(artifactRefHref("myelin://acme/issue/issue/MYL-7")).toBe("/issues?state=all&key=MYL-7");
    expect(artifactRefLabel("myelin://acme/git/pr/platform:42")).toBe("platform #42");
    expect(artifactRefHref("myelin://acme/git/pr/platform:42")).toBe("/git/repos/platform/prs/42");
    expect(artifactRefLabel("myelin://acme/git/pr/platform/api:42")).toBe("platform/api #42");
    expect(artifactRefHref("myelin://acme/git/pr/platform/api:42"))
      .toBe("/git/repos/platform%2Fapi/prs/42");
    expect(artifactRefHref("myelin://acme/git/repo/platform/api"))
      .toBe("/git/repos/platform%2Fapi");
  });

  it("validates a related-work edge against source, tenant, and existing edges", () => {
    const source = "myelin://acme/chat/channel/01J00000000000000000000000";
    const issue = "myelin://acme/issue/issue/MYL-7";
    expect(relatedArtifactRefError(source, issue, [])).toBeNull();
    expect(relatedArtifactRefError(source, "https://example.invalid/7", [])).toBe("invalid");
    expect(relatedArtifactRefError(source, "myelin://other/issue/issue/MYL-7", []))
      .toBe("cross-tenant");
    expect(relatedArtifactRefError(source, `${source}#message-7`, [])).toBe("self");
    expect(relatedArtifactRefError(source, issue, [issue])).toBe("duplicate");
  });
});
