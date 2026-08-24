import { describe, expect, it } from "vitest";

import {
  artifactRefHref,
  artifactRefLabel,
  parseArtifactRef,
  parseGitCommitRef,
  parseGitPullRequestRef,
  parseGitReferenceRef,
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
    "myelin://acme/issue/issue/MYL-7#L0-L1",
    "myelin://acme/issue/issue/MYL-7#L01-L2",
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
    expect(artifactRefHref("myelin://acme/git/repo/platform.git/api")).toBeUndefined();
    expect(artifactRefHref("myelin://acme/git/pr/platform:9007199254740992")).toBeUndefined();
    expect(artifactRefLabel(
      "myelin://acme/git/commit/platform/api:0123456789abcdef0123456789abcdef01234567",
    )).toBe("platform/api · 0123456789ab");
    expect(artifactRefHref(
      "myelin://acme/git/commit/platform/api:0123456789abcdef0123456789abcdef01234567",
    )).toBe("/git/repos/platform%2Fapi/commit/0123456789abcdef0123456789abcdef01234567");
    expect(artifactRefHref("myelin://acme/git/commit/platform:deadbeef")).toBeUndefined();
    expect(artifactRefLabel(
      "myelin://acme/git/ref/platform%2Fapi:refs%2Fheads%2Frelease%2Fone",
    )).toBe("platform/api · release/one");
    expect(artifactRefHref(
      "myelin://acme/git/ref/platform%2Fapi:refs%2Fheads%2Frelease%2Fone",
    )).toBe("/git/repos/platform%2Fapi/tree/refs%2Fheads%2Frelease%2Fone");
    expect(artifactRefLabel("myelin://acme/ci/run/91000000-0000-4000-8000-000000000001"))
      .toBe("CI run · 00000001");
    expect(artifactRefHref("myelin://acme/ci/run/91000000-0000-4000-8000-000000000001"))
      .toBe("/ci/runs/91000000-0000-4000-8000-000000000001");
    expect(artifactRefHref("myelin://acme/ci/run/NOT-A-UUID")).toBeUndefined();
    expect(artifactRefHref("myelin://acme/chat/channel/01J00000000000000000000000"))
      .toBe("/chat?conversation=01J00000000000000000000000");
    expect(artifactRefHref("myelin://acme/chat/channel/not-a-ulid")).toBeUndefined();
    expect(artifactRefHref(
      "myelin://acme/agent/thread/92000000-0000-4000-8000-000000000001",
    )).toBe("/agents?thread=92000000-0000-4000-8000-000000000001");
    expect(artifactRefHref("myelin://acme/agent/thread/NOT-A-UUID")).toBeUndefined();
  });

  it("parses a nested pull-request coordinate once without losing numeric precision", () => {
    expect(parseGitPullRequestRef(
      "myelin://acme/git/pr/platform/api:18446744073709551615#comment-7",
    )).toEqual({
      tenant: "acme",
      repo: "platform/api",
      number: "18446744073709551615",
      sub: "comment-7",
      root: "myelin://acme/git/pr/platform/api:18446744073709551615",
    });
    for (const value of [
      "myelin://acme/git/pr/platform/api:0",
      "myelin://acme/git/pr/platform/api:01",
      "myelin://acme/git/pr/platform/api:18446744073709551616",
      "myelin://acme/git/pr/platform.git/api:1",
    ]) expect(parseGitPullRequestRef(value), value).toBeNull();
  });

  it("parses a nested repository commit without accepting an abbreviated or uppercase object id", () => {
    const oid = "0123456789abcdef0123456789abcdef01234567";
    expect(parseGitCommitRef(`myelin://acme/git/commit/platform/api:${oid}`)).toEqual({
      tenant: "acme",
      repo: "platform/api",
      oid,
      sub: null,
      root: `myelin://acme/git/commit/platform/api:${oid}`,
    });
    expect(parseGitCommitRef("myelin://acme/git/commit/platform:deadbeef")).toBeNull();
    expect(parseGitCommitRef(`myelin://acme/git/commit/platform:${oid.toUpperCase()}`)).toBeNull();
  });

  it("parses only canonically encoded branch and tag event coordinates", () => {
    const branch = "myelin://acme/git/ref/platform%2Fapi:refs%2Fheads%2Frelease%2Fone";
    expect(parseGitReferenceRef(branch)).toEqual({
      tenant: "acme",
      repo: "platform/api",
      ref: "refs/heads/release/one",
      sub: null,
      root: branch,
    });
    expect(parseGitReferenceRef("myelin://acme/git/ref/platform:refs%2Ftags%2Fv1")).toEqual({
      tenant: "acme",
      repo: "platform",
      ref: "refs/tags/v1",
      sub: null,
      root: "myelin://acme/git/ref/platform:refs%2Ftags%2Fv1",
    });
    for (const value of [
      "myelin://acme/git/ref/platform%2fapi:refs%2Fheads%2Fmain",
      "myelin://acme/git/ref/platform:refs%2Fnotes%2Fbuild",
      "myelin://acme/git/ref/platform:refs%252Fheads%252Fmain",
      "myelin://acme/git/ref/%70latform:refs%2Fheads%2Fmain",
      `myelin://acme/git/ref/${"a".repeat(1025)}:refs%2Fheads%2Fmain`,
    ]) expect(parseGitReferenceRef(value), value).toBeNull();
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
