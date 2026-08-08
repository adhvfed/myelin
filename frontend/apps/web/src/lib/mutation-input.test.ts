import { describe, expect, it } from "vitest";
import {
  MAX_ISSUE_TITLE_BYTES,
  MAX_PR_MARKDOWN_BYTES,
  parseIssueMutation,
  parsePrMutation,
} from "./mutation-input";

describe("parseIssueMutation", () => {
  it("normalizes a valid create and admits canonical close/activation ids", () => {
    expect(parseIssueMutation({ op: "create", title: "  Ship it  " })).toEqual({
      op: "create",
      title: "Ship it",
    });
    expect(parseIssueMutation({
      op: "close",
      issueId: "123e4567-e89b-12d3-a456-426614174000",
    })).not.toBeNull();
    expect(parseIssueMutation({
      op: "activation",
      requestEventId: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
    })).not.toBeNull();
  });

  it.each([
    null,
    [],
    { op: "reopen", issueId: "123e4567-e89b-12d3-a456-426614174000" },
    { op: "close", issueId: "not-a-uuid" },
    { op: "activation", requestEventId: "01arz3ndektsv4rrffq69g5fav" },
    { op: "create", title: "ok", project_id: "caller-scope" },
    { op: "create", title: "line\nbreak" },
    { op: "create", title: "x".repeat(MAX_ISSUE_TITLE_BYTES + 1) },
  ])("rejects malformed or non-exact input %#", (value) => {
    expect(parseIssueMutation(value)).toBeNull();
  });
});

describe("parsePrMutation", () => {
  it("normalizes valid markdown and preserves an exact anchor", () => {
    expect(parsePrMutation({
      op: "thread",
      repo: "team/core",
      n: 42,
      body_md: "  review this  ",
      anchor: { path: "src/lib.rs", line: 7, side: "new" },
    })).toEqual({
      op: "thread",
      repo: "team/core",
      n: 42,
      body_md: "review this",
      anchor: { path: "src/lib.rs", line: 7, side: "new" },
    });
  });

  it.each([
    null,
    [],
    { op: "unknown", repo: "core", n: 1 },
    { op: "merge", repo: "../core", n: 1 },
    { op: "merge", repo: "core", n: 0 },
    { op: "merge", repo: "core", n: Number.MAX_SAFE_INTEGER + 1 },
    { op: "merge", repo: "core", n: 1, surprise: true },
    { op: "comment", repo: "core", n: 1, threadId: "r-1", body_md: "x" },
    { op: "review-discard", repo: "core", n: 1, reviewId: "t-1" },
    { op: "review-submit", repo: "core", n: 1, reviewId: "r-1", verdict: "dismissed" },
    { op: "thread", repo: "core", n: 1, body_md: "x", anchor: { path: "src/x", line: 1 } },
    { op: "thread", repo: "core", n: 1, body_md: " ", anchor: { path: "src/x", line: 1 } },
    { op: "thread", repo: "core", n: 1, body_md: "x", anchor: { path: "../secret", line: 1 } },
    { op: "thread", repo: "core", n: 1, body_md: "x", anchor: { path: "src/x", line: -1 } },
    { op: "thread", repo: "core", n: 1, body_md: "x".repeat(MAX_PR_MARKDOWN_BYTES + 1) },
  ])("rejects malformed, unsafe, oversized, or non-exact input %#", (value) => {
    expect(parsePrMutation(value)).toBeNull();
  });
});
