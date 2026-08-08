import { describe, expect, it } from "vitest";
import {
  hasAppliedAction,
  parseAppliedComment,
  parseAppliedMerge,
  parseAppliedReview,
  parseAppliedThread,
  parseIssue,
  parseIssueAuthorizationStatus,
  parseIssueCreateReceipt,
  parseIssuesPage,
  parsePrChecks,
  parsePrThreads,
} from "./mutation-response";

const SHA1 = "a".repeat(40);
const UUID = "123e4567-e89b-12d3-a456-426614174000";
const PROJECT = "223e4567-e89b-12d3-a456-426614174000";
const ULID = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const principal = {
  kind: "human",
  display: "reviewer@example.invalid",
  on_behalf_of: null,
  trigger: null,
};
const comment = {
  id: "c-2",
  author: principal,
  body_md: "Looks good",
  created_at: 1_719_450_001,
  edited_at: null,
  state: "visible",
  review_id: null,
  pending: false,
};

describe("issue mutation response decoders", () => {
  const summary = { id: UUID, key: "MY-42", project_id: PROJECT };
  const issue = {
    ...summary,
    state: "Done",
    state_category: "completed",
    title: "Ship the boundary",
    version: 2,
    created_at: "2026-07-22T08:00:00.000Z",
    updated_at: "2026-07-22T08:01:00.000Z",
  };

  it("projects valid create, pending, active, and close responses", () => {
    expect(parseIssueCreateReceipt({
      issue: { ...summary, ignored: "drop me" },
      authorization: { status: "pending", request_event_id: ULID },
    })).toEqual({
      issue: summary,
      authorization: { status: "pending", request_event_id: ULID },
    });
    expect(parseIssueAuthorizationStatus({ status: "pending", issue: summary, retry_after_ms: 1_000 }))
      .toEqual({ status: "pending", issue: summary, retry_after_ms: 1_000 });
    expect(parseIssueAuthorizationStatus({ status: "active", issue })).toEqual({ status: "active", issue });
    expect(parseIssue(issue)).toEqual(issue);
  });

  it.each([
    { issue: summary, authorization: { status: "active", request_event_id: ULID } },
    { issue: { ...summary, id: "not-a-uuid" }, authorization: { status: "pending", request_event_id: ULID } },
    { issue: summary, authorization: { status: "pending", request_event_id: ULID.toLowerCase() } },
  ])("rejects malformed create receipts %#", (value) => {
    expect(parseIssueCreateReceipt(value)).toBeNull();
  });

  it("rejects unbounded polling and invalid issue state", () => {
    expect(parseIssueAuthorizationStatus({ status: "pending", issue: summary, retry_after_ms: 60_001 }))
      .toBeNull();
    expect(parseIssue({ ...issue, state_category: "deleted" })).toBeNull();
    expect(parseIssue({ ...issue, updated_at: "not-a-date" })).toBeNull();
  });

  it("bounds and projects an Issues page", () => {
    expect(parseIssuesPage({
      items: [{ ...issue, internal_relation: "must not cross" }],
      page: { next_cursor: null, limit: 50, total: 1 },
      internal: "drop",
    })).toEqual({ items: [issue], page: { next_cursor: null, limit: 50 } });
    expect(parseIssuesPage({ items: Array(101).fill(issue), page: { next_cursor: null, limit: 100 } }))
      .toBeNull();
    expect(parseIssuesPage({ items: [], page: { next_cursor: "x".repeat(193), limit: 50 } }))
      .toBeNull();
  });
});

describe("PR mutation response decoders", () => {
  it("projects a revision-bound thread and drops response-only extras", () => {
    const response = {
      applied: {
        action: "git.pr.thread.create",
        thread: {
          id: "t-1",
          anchor: {
            path: "src/lib.rs",
            line: 7,
            side: "new",
            base_oid: SHA1,
            head_oid: "b".repeat(40),
            anchor_state: "live",
          },
          resolved: false,
          comments: [{ ...comment, secret: "discarded" }],
          internal: "discarded",
        },
      },
      durable: true,
    };
    expect(parseAppliedThread(response)).toEqual({
      id: "t-1",
      anchor: {
        path: "src/lib.rs",
        line: 7,
        side: "new",
        base_oid: SHA1,
        head_oid: "b".repeat(40),
        anchor_state: "live",
      },
      resolved: false,
      comments: [comment],
    });
  });

  it("validates action identity, durability, and nested records", () => {
    expect(parseAppliedThread({ applied: { action: "git.pr.thread.create", thread: {} }, durable: true }))
      .toBeNull();
    expect(parseAppliedComment({ applied: { action: "git.pr.comment.create", comment }, durable: false }, "git.pr.comment.create"))
      .toBeNull();
    expect(parseAppliedComment({ applied: { action: "git.pr.review.comment", comment }, durable: true }, "git.pr.comment.create"))
      .toBeNull();
    expect(parseAppliedReview({
      applied: {
        action: "git.pr.review.start",
        review: { id: "r-1", reviewer: principal, verdict: "in_progress", advisory: false, submitted_at: null, summary_md: null },
      },
      durable: true,
    })?.id).toBe("r-1");
    expect(hasAppliedAction({ applied: { action: "git.pr.review.submit" }, durable: true }, "git.pr.review.submit"))
      .toBe(true);
    expect(hasAppliedAction({ applied: { action: "git.pr.review.submit" }, durable: false }, "git.pr.review.submit"))
      .toBe(false);
  });

  it("accepts only an admitted merge receipt with a canonical oid and branch ref", () => {
    expect(parseAppliedMerge({
      applied: { action: "git.pr.merge", merged: true, base_ref: "refs/heads/main", new_oid: SHA1, ignored: true },
      durable: true,
    })).toEqual({ base_ref: "refs/heads/main", new_oid: SHA1 });
    for (const value of [
      { applied: { action: "git.pr.merge", merged: false, base_ref: "refs/heads/main", new_oid: SHA1 }, durable: true },
      { applied: { action: "git.pr.merge", merged: true, base_ref: "main", new_oid: SHA1 }, durable: true },
      { applied: { action: "git.pr.merge", merged: true, base_ref: "refs/heads/main", new_oid: "bad" }, durable: true },
      { applied: { action: "git.pr.open", merged: true, base_ref: "refs/heads/main", new_oid: SHA1 }, durable: true },
    ]) expect(parseAppliedMerge(value)).toBeNull();
  });

  it("validates the authoritative checks carried by a blocked merge", () => {
    const checks = {
      required_contexts: ["build"],
      required_approvals: 1,
      green_contexts: [],
      endorsed_contexts: [],
      fork_unendorsed_contexts: [],
      gate_admitted: false,
      changes_requested: true,
      current_approvals: 0,
      durable: true,
    };
    expect(parsePrChecks(checks)).toEqual(checks);
    expect(parsePrChecks({ ...checks, gate_admitted: "false" })).toBeNull();
    expect(parsePrChecks({ ...checks, required_contexts: Array(4_097).fill("build") })).toBeNull();
  });

  it("projects viewer-scoped conversation arrays and enforces their classification", () => {
    const discussion = { id: "t-1", anchor: null, resolved: false, comments: [comment] };
    const anchored = {
      id: "t-2",
      anchor: {
        path: "src/lib.rs",
        line: 7,
        side: "new",
        base_oid: SHA1,
        head_oid: "b".repeat(40),
        anchor_state: "live",
      },
      resolved: false,
      comments: [{ ...comment, id: "c-3" }],
    };
    const review = {
      id: "r-1",
      reviewer: principal,
      verdict: "in_progress",
      advisory: false,
      submitted_at: null,
      summary_md: null,
    };
    expect(parsePrThreads({
      discussion: [{ ...discussion, private_storage_key: "drop" }],
      anchored: [anchored],
      threads: [discussion, anchored],
      reviews: [{ ...review, internal: "drop" }],
      durable: true,
      internal: "drop",
    })).toEqual({
      discussion: [discussion],
      anchored: [anchored],
      threads: [discussion, anchored],
      reviews: [review],
      durable: true,
    });
    expect(parsePrThreads({
      discussion: [],
      anchored: [discussion],
      threads: [discussion],
      reviews: [],
      durable: true,
    })).toBeNull();
    expect(parsePrThreads({
      discussion: [],
      anchored: [],
      threads: [],
      reviews: [],
      durable: false,
    })).toBeNull();
  });
});
