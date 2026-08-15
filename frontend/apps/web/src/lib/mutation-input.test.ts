import { describe, expect, it } from "vitest";
import {
  MAX_ISSUE_TITLE_BYTES,
  MAX_PR_MARKDOWN_BYTES,
  parseIssueMutation,
  parsePrMutation,
} from "./mutation-input";

describe("parseIssueMutation", () => {
  it("normalizes a valid create and admits canonical close/activation ids", () => {
    expect(parseIssueMutation({
      op: "create",
      projectId: "123e4567-e89b-12d3-a456-426614174000",
      title: "  Ship it  ",
      clientNonce: "issue-create_1",
    })).toEqual({
      op: "create",
      projectId: "123e4567-e89b-12d3-a456-426614174000",
      title: "Ship it",
      clientNonce: "issue-create_1",
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
    { op: "create", projectId: "not-a-uuid", title: "ok", clientNonce: "create_1" },
    { op: "create", projectId: "123e4567-e89b-12d3-a456-426614174000", title: "ok" },
    { op: "create", projectId: "123e4567-e89b-12d3-a456-426614174000", title: "ok", clientNonce: "has spaces" },
    { op: "create", projectId: "123e4567-e89b-12d3-a456-426614174000", title: "line\nbreak", clientNonce: "create_1" },
    { op: "create", projectId: "123e4567-e89b-12d3-a456-426614174000", title: "x".repeat(MAX_ISSUE_TITLE_BYTES + 1), clientNonce: "create_1" },
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
      clientNonce: "thread_1",
      anchor: { path: "src/lib.rs", line: 7, side: "new" },
    })).toEqual({
      op: "thread",
      repo: "team/core",
      n: 42,
      body_md: "review this",
      clientNonce: "thread_1",
      anchor: { path: "src/lib.rs", line: 7, side: "new" },
    });
    expect(parsePrMutation({
      op: "resolve",
      repo: "team/core",
      n: 42,
      threadId: "t-7",
      resolved: true,
    })).toEqual({
      op: "resolve",
      repo: "team/core",
      n: 42,
      threadId: "t-7",
      resolved: true,
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
    { op: "comment", repo: "core", n: 1, threadId: "r-1", body_md: "x", clientNonce: "comment_1" },
    { op: "resolve", repo: "core", n: 1, threadId: "t-1", resolved: "yes" },
    { op: "review-discard", repo: "core", n: 1, reviewId: "t-1" },
    { op: "review-submit", repo: "core", n: 1, reviewId: "r-1", verdict: "dismissed" },
    { op: "thread", repo: "core", n: 1, body_md: "x", clientNonce: "thread_1", anchor: { path: "src/x", line: 1 } },
    { op: "thread", repo: "core", n: 1, body_md: " ", clientNonce: "thread_1", anchor: { path: "src/x", line: 1 } },
    { op: "thread", repo: "core", n: 1, body_md: "x", clientNonce: "thread_1", anchor: { path: "../secret", line: 1 } },
    { op: "thread", repo: "core", n: 1, body_md: "x", clientNonce: "thread_1", anchor: { path: "src/x", line: -1 } },
    { op: "thread", repo: "core", n: 1, body_md: "x", clientNonce: "has spaces" },
    { op: "thread", repo: "core", n: 1, body_md: "x".repeat(MAX_PR_MARKDOWN_BYTES + 1), clientNonce: "thread_1" },
  ])("rejects malformed, unsafe, oversized, or non-exact input %#", (value) => {
    expect(parsePrMutation(value)).toBeNull();
  });
});
