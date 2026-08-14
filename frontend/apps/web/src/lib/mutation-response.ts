// Runtime decoders for Edge mutation responses. Generic TypeScript parameters on edgeGet/edgePost
// do not validate JSON, so server actions project only bounded, contract-valid values into browser
// state. Extra response fields are deliberately discarded.

import type {
  PrChecksVM,
  PrCommentVM,
  PrReviewVM,
  PrThreadVM,
  PrThreadsVM,
} from "./api";
import type {
  IssueAuthorizationStatus,
  IssueCreateReceipt,
  IssueVM,
  IssuesPage,
} from "./issue-api";
import { parseArtifactRef } from "./artifact-ref";
import { isBranchRef } from "./git-ref";

type WireRecord = Record<string, unknown>;

const MAX_MARKDOWN_BYTES = 64 * 1024;
const MAX_DISPLAY_BYTES = 4 * 1024;
const MAX_PATH_BYTES = 4 * 1024;
const MAX_CHECK_CONTEXTS = 4_096;
const utf8 = new TextEncoder();

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function boundedString(value: unknown, maxBytes: number, allowEmpty = false): value is string {
  return typeof value === "string" && (allowEmpty || value.length > 0) &&
    utf8.encode(value).byteLength <= maxBytes;
}

function nullableBoundedString(value: unknown, maxBytes: number): value is string | null {
  return value === null || boundedString(value, maxBytes, true);
}

function safeNonNegativeInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function canonicalUuid(value: unknown): value is string {
  return typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
}

function canonicalUlid(value: unknown): value is string {
  return typeof value === "string" && /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/.test(value);
}

function oid(value: unknown): value is string {
  return typeof value === "string" && /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(value);
}

function issueSummary(value: unknown): IssueCreateReceipt["issue"] | null {
  const input = record(value);
  const reference = parseArtifactRef(input?.ref);
  if (!input || !canonicalUuid(input.id) || !canonicalUuid(input.project_id) ||
      typeof input.key !== "string" || !/^[A-Z0-9]{2,10}-[1-9][0-9]{0,18}$/.test(input.key) ||
      !reference || reference.sub !== null || reference.subsystem !== "issue" ||
      reference.type !== "issue" || reference.id !== input.key) {
    return null;
  }
  return { id: input.id, ref: input.ref as string, key: input.key, project_id: input.project_id };
}

export function parseIssueCreateReceipt(value: unknown): IssueCreateReceipt | null {
  const input = record(value);
  const issue = issueSummary(input?.issue);
  const authorization = record(input?.authorization);
  if (!issue || !authorization || authorization.status !== "pending" ||
      !canonicalUlid(authorization.request_event_id)) return null;
  return {
    issue,
    authorization: { status: "pending", request_event_id: authorization.request_event_id },
  };
}

export function parseIssue(value: unknown): IssueVM | null {
  const input = record(value);
  const summary = issueSummary(input);
  if (!input || !summary || !boundedString(input.state, 256) ||
      !["unstarted", "started", "completed", "cancelled"].includes(input.state_category as string) ||
      !boundedString(input.title, 512) || !Number.isSafeInteger(input.version) ||
      (input.version as number) <= 0 || !boundedString(input.created_at, 64) ||
      !boundedString(input.updated_at, 64) || !Number.isFinite(Date.parse(input.created_at)) ||
      !Number.isFinite(Date.parse(input.updated_at))) return null;
  return {
    ...summary,
    state: input.state,
    state_category: input.state_category as IssueVM["state_category"],
    title: input.title,
    version: input.version as number,
    created_at: input.created_at,
    updated_at: input.updated_at,
  };
}

export function parseIssueAuthorizationStatus(value: unknown): IssueAuthorizationStatus | null {
  const input = record(value);
  if (!input) return null;
  if (input.status === "active") {
    const issue = parseIssue(input.issue);
    return issue ? { status: "active", issue } : null;
  }
  const issue = issueSummary(input.issue);
  if (input.status !== "pending" || !issue || !Number.isSafeInteger(input.retry_after_ms) ||
      (input.retry_after_ms as number) < 0 || (input.retry_after_ms as number) > 60_000) return null;
  return { status: "pending", issue, retry_after_ms: input.retry_after_ms as number };
}

export function parseIssuesPage(value: unknown): IssuesPage | null {
  const input = record(value);
  const page = record(input?.page);
  if (!input || !Array.isArray(input.items) || input.items.length > 100 || !page ||
      !(page.next_cursor === null || boundedString(page.next_cursor, 192)) ||
      !Number.isSafeInteger(page.limit) || (page.limit as number) < 1 || (page.limit as number) > 100) {
    return null;
  }
  const items = input.items.map(parseIssue);
  if (items.some((item) => item === null)) return null;
  return {
    items: items as IssueVM[],
    page: { next_cursor: page.next_cursor, limit: page.limit as number },
  };
}

function principal(value: unknown): PrCommentVM["author"] | null {
  const input = record(value);
  if (!input || !["human", "agent", "service"].includes(input.kind as string) ||
      !boundedString(input.display, MAX_DISPLAY_BYTES) ||
      !nullableBoundedString(input.on_behalf_of, MAX_DISPLAY_BYTES) ||
      !nullableBoundedString(input.trigger, MAX_DISPLAY_BYTES)) return null;
  return {
    kind: input.kind as PrCommentVM["author"]["kind"],
    display: input.display,
    on_behalf_of: input.on_behalf_of,
    trigger: input.trigger,
  };
}

function comment(value: unknown): PrCommentVM | null {
  const input = record(value);
  const author = principal(input?.author);
  if (!input || !author || typeof input.id !== "string" || !/^c-[1-9][0-9]{0,19}$/.test(input.id) ||
      !nullableBoundedString(input.body_md, MAX_MARKDOWN_BYTES) || !safeNonNegativeInteger(input.created_at) ||
      !(input.edited_at === null || safeNonNegativeInteger(input.edited_at)) ||
      !["visible", "removed"].includes(input.state as string) ||
      !(input.review_id === null || (typeof input.review_id === "string" && /^r-[1-9][0-9]{0,19}$/.test(input.review_id))) ||
      typeof input.pending !== "boolean" || (input.state === "removed" && input.body_md !== null)) return null;
  return {
    id: input.id,
    author,
    body_md: input.body_md,
    created_at: input.created_at,
    edited_at: input.edited_at,
    state: input.state as PrCommentVM["state"],
    review_id: input.review_id,
    pending: input.pending,
  };
}

function anchor(value: unknown): PrThreadVM["anchor"] | null | undefined {
  if (value === null) return null;
  const input = record(value);
  if (!input || !boundedString(input.path, MAX_PATH_BYTES) ||
      !(input.line === null || (Number.isSafeInteger(input.line) && (input.line as number) > 0)) ||
      !(input.side === null || input.side === "old" || input.side === "new") ||
      !(input.base_oid === null || oid(input.base_oid)) || !(input.head_oid === null || oid(input.head_oid)) ||
      !["live", "moved", "outdated"].includes(input.anchor_state as string)) return undefined;
  return {
    path: input.path,
    line: input.line as number | null,
    side: input.side,
    base_oid: input.base_oid,
    head_oid: input.head_oid,
    anchor_state: input.anchor_state as NonNullable<PrThreadVM["anchor"]>["anchor_state"],
  };
}

function thread(value: unknown): PrThreadVM | null {
  const input = record(value);
  const parsedAnchor = anchor(input?.anchor);
  if (!input || parsedAnchor === undefined || typeof input.id !== "string" ||
      !/^t-[1-9][0-9]{0,19}$/.test(input.id) || typeof input.resolved !== "boolean" ||
      !Array.isArray(input.comments) || input.comments.length > 8_192) return null;
  const comments = input.comments.map(comment);
  if (comments.some((entry) => entry === null)) return null;
  return { id: input.id, anchor: parsedAnchor, resolved: input.resolved, comments: comments as PrCommentVM[] };
}

function review(value: unknown): PrReviewVM | null {
  const input = record(value);
  const reviewer = principal(input?.reviewer);
  if (!input || !reviewer || typeof input.id !== "string" || !/^r-[1-9][0-9]{0,19}$/.test(input.id) ||
      !["in_progress", "approved", "changes_requested", "commented"].includes(input.verdict as string) ||
      typeof input.advisory !== "boolean" ||
      !(input.submitted_at === null || safeNonNegativeInteger(input.submitted_at)) ||
      !nullableBoundedString(input.summary_md, MAX_MARKDOWN_BYTES)) return null;
  return {
    id: input.id,
    reviewer,
    verdict: input.verdict as PrReviewVM["verdict"],
    advisory: input.advisory,
    submitted_at: input.submitted_at,
    summary_md: input.summary_md,
  };
}

function applied(value: unknown, action: string): WireRecord | null {
  const input = record(value);
  const payload = record(input?.applied);
  return input?.durable === true && payload?.action === action ? payload : null;
}

export function parseAppliedThread(value: unknown): PrThreadVM | null {
  const payload = applied(value, "git.pr.thread.create");
  return payload ? thread(payload.thread) : null;
}

export function parseAppliedComment(value: unknown, action: "git.pr.comment.create" | "git.pr.review.comment"): PrCommentVM | null {
  const payload = applied(value, action);
  return payload ? comment(payload.comment) : null;
}

export function parseAppliedReview(value: unknown): PrReviewVM | null {
  const payload = applied(value, "git.pr.review.start");
  return payload ? review(payload.review) : null;
}

function threadArray(value: unknown, expected: "discussion" | "anchored" | "all"): PrThreadVM[] | null {
  if (!Array.isArray(value) || value.length > 4_096) return null;
  const parsed = value.map(thread);
  if (parsed.some((entry) => entry === null)) return null;
  const threads = parsed as PrThreadVM[];
  if (expected === "discussion" && threads.some((entry) => entry.anchor !== null)) return null;
  if (expected === "anchored" && threads.some((entry) => entry.anchor === null)) return null;
  return threads;
}

export function parsePrThreads(value: unknown): PrThreadsVM | null {
  const input = record(value);
  if (!input || input.durable !== true || !Array.isArray(input.reviews) || input.reviews.length > 1_024) {
    return null;
  }
  const discussion = threadArray(input.discussion, "discussion");
  const anchored = threadArray(input.anchored, "anchored");
  const threads = threadArray(input.threads, "all");
  const reviews = input.reviews.map(review);
  if (!discussion || !anchored || !threads || reviews.some((entry) => entry === null)) return null;
  return {
    discussion,
    anchored,
    threads,
    reviews: reviews as PrReviewVM[],
    durable: true,
  };
}

export function hasAppliedAction(value: unknown, action: "git.pr.review.submit" | "git.pr.review.discard"): boolean {
  return applied(value, action) !== null;
}

export function parseAppliedMerge(value: unknown): { base_ref: string; new_oid: string } | null {
  const payload = applied(value, "git.pr.merge");
  if (!payload || payload.merged !== true || !isBranchRef(payload.base_ref) || !oid(payload.new_oid)) return null;
  return { base_ref: payload.base_ref, new_oid: payload.new_oid };
}

function contextArray(value: unknown): string[] | null {
  if (!Array.isArray(value) || value.length > MAX_CHECK_CONTEXTS ||
      value.some((entry) => !boundedString(entry, 512))) return null;
  return [...value] as string[];
}

export function parsePrChecks(value: unknown): PrChecksVM | null {
  const input = record(value);
  if (!input) return null;
  const required_contexts = contextArray(input.required_contexts);
  const green_contexts = contextArray(input.green_contexts);
  const endorsed_contexts = contextArray(input.endorsed_contexts);
  const fork_unendorsed_contexts = contextArray(input.fork_unendorsed_contexts);
  if (!required_contexts || !green_contexts || !endorsed_contexts || !fork_unendorsed_contexts ||
      !safeNonNegativeInteger(input.required_approvals) || typeof input.gate_admitted !== "boolean" ||
      input.durable !== true ||
      !(input.changes_requested === undefined || typeof input.changes_requested === "boolean") ||
      !(input.current_approvals === undefined || safeNonNegativeInteger(input.current_approvals))) return null;
  return {
    required_contexts,
    required_approvals: input.required_approvals,
    green_contexts,
    endorsed_contexts,
    fork_unendorsed_contexts,
    gate_admitted: input.gate_admitted,
    ...(input.changes_requested === undefined ? {} : { changes_requested: input.changes_requested }),
    ...(input.current_approvals === undefined ? {} : { current_approvals: input.current_approvals }),
    durable: input.durable,
  };
}
