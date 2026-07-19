import { describe, expect, it } from "vitest";
import type { IssueVM, IssuesPage } from "./api";
import {
  isClosedCategory,
  issueErrorKind,
  issueKeyError,
  issueListHref,
  issueListState,
  issueTitleError,
  mergeIssuePages,
  normalizeIssueKey,
  pollIssueActivation,
} from "./issue-view";

function issue(id: string): IssueVM {
  return {
    id,
    key: `MYL-${id}`,
    project_id: "20aee030-c7fa-4757-8243-700faf528690",
    state: "Todo",
    state_category: "unstarted",
    title: `Issue ${id}`,
    version: 1,
    created_at: "2026-07-19T12:00:00Z",
    updated_at: "2026-07-19T12:00:00Z",
  };
}

function page(items: IssueVM[], next: string | null = null): IssuesPage {
  return { items, page: { next_cursor: next, limit: 2 } };
}

describe("issue list URL and filter mapping", () => {
  it("defaults unknown state to open and emits canonical URLs", () => {
    expect(issueListState(undefined)).toBe("open");
    expect(issueListState("cancelled")).toBe("open");
    expect(issueListState("closed")).toBe("closed");
    expect(issueListHref({ state: "open" })).toBe("/issues");
    expect(issueListHref({ state: "closed", key: "myl-1" })).toBe(
      "/issues?state=closed&key=MYL-1",
    );
    expect(issueListHref({ state: "all", create: true })).toBe(
      "/issues?state=all&new=1",
    );
  });

  it("normalizes only the edge's key-prefix grammar", () => {
    expect(normalizeIssueKey(" myl-12 ")).toBe("MYL-12");
    expect(normalizeIssueKey("title search")).toBeUndefined();
    expect(issueKeyError("MYL_12")).toContain("letters");
    expect(issueKeyError("")).toBeNull();
  });

  it("maps both terminal categories to the closed product filter", () => {
    expect(isClosedCategory("completed")).toBe(true);
    expect(isClosedCategory("cancelled")).toBe(true);
    expect(isClosedCategory("started")).toBe(false);
  });
});

describe("issue create and page state", () => {
  it("validates the backend's UTF-8 byte limit, not JavaScript character count", () => {
    expect(issueTitleError("x".repeat(512))).toBeNull();
    expect(issueTitleError("ø".repeat(256))).toBeNull();
    expect(issueTitleError(`ø${"x".repeat(511)}`)).toContain("512 UTF-8 bytes");
    expect(issueTitleError("   ")).toBe("Enter an issue title.");
  });

  it("appends opaque-cursor pages without duplicating a boundary row", () => {
    expect(
      mergeIssuePages(page([issue("1"), issue("2")]), [page([issue("2"), issue("3")])])
        .map((row) => row.id),
    ).toEqual(["1", "2", "3"]);
  });

  it("recovers typed route errors after server serialization", () => {
    expect(issueErrorKind(new Error("ISSUE_ERR:unavailable"))).toBe("unavailable");
    expect(issueErrorKind(new Error("database secret"))).toBe("error");
  });

  it("polls a fresh status until the actual pending-to-active transition", async () => {
    const statuses = [
      {
        status: "pending" as const,
        issue: { id: "1", key: "MYL-1", project_id: "project" },
        retry_after_ms: 1_000,
      },
      { status: "active" as const, issue: issue("1") },
    ];
    let clock = 0;
    const outcome = await pollIssueActivation(
      "01REQUEST",
      async () => statuses.shift()!,
      {
        budgetMs: 5_000,
        now: () => clock,
        sleep: async (ms) => { clock += ms; },
      },
    );
    expect(outcome).toEqual({ phase: "active", issue: issue("1") });
    expect(statuses).toHaveLength(0);
  });

  it("stops bounded polling as unconfirmed, never as a fabricated failure", async () => {
    let clock = 0;
    const outcome = await pollIssueActivation(
      "01REQUEST",
      async () => ({
        status: "pending",
        issue: { id: "1", key: "MYL-1", project_id: "project" },
        retry_after_ms: 1_000,
      }),
      {
        budgetMs: 1_500,
        now: () => clock,
        sleep: async (ms) => { clock += ms; },
      },
    );
    expect(outcome).toEqual({ phase: "unconfirmed" });
  });

  it("floors a malformed retry hint instead of busy-looping", async () => {
    let clock = 0;
    let polls = 0;
    const outcome = await pollIssueActivation(
      "01REQUEST",
      async () => ++polls === 1
        ? {
            status: "pending",
            issue: { id: "1", key: "MYL-1", project_id: "project" },
            retry_after_ms: Number.NaN,
          }
        : { status: "active", issue: issue("1") },
      {
        budgetMs: 1_000,
        now: () => clock,
        sleep: async (ms) => { clock += ms; },
      },
    );
    expect(clock).toBe(250);
    expect(outcome.phase).toBe("active");
  });
});
