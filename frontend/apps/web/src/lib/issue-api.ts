import { action, json, query, redirect } from "@solidjs/router";
import { edgeGet, edgePost, GatewayError, isUnauthorized } from "../server/gateway";
import {
  parseIssueId,
  parseIssueListInput,
  type IssueListInput,
  type IssueListState,
} from "./issue-read-input";
import { parseIssueMutation, type IssueMutation } from "./mutation-input";
import {
  parseIssue,
  parseIssueAuthorizationStatus,
  parseIssueCreateReceipt,
  parseIssuesPage,
} from "./mutation-response";

export type IssueStateCategory = "unstarted" | "started" | "completed" | "cancelled";

export interface IssueVM {
  id: string;
  ref: string;
  key: string;
  project_id: string;
  state: string;
  state_category: IssueStateCategory;
  title: string;
  version: number;
  created_at: string;
  updated_at: string;
}

export interface IssuesPage {
  items: IssueVM[];
  page: { next_cursor: string | null; limit: number };
}

export interface IssueCreateReceipt {
  issue: { id: string; ref: string; key: string; project_id: string };
  authorization: { status: "pending"; request_event_id: string };
}

export type IssueAuthorizationStatus =
  | { status: "pending"; issue: IssueCreateReceipt["issue"]; retry_after_ms: number }
  | { status: "active"; issue: IssueVM };

export type IssueErrorKind = "bad-input" | "not-found" | "unavailable" | "error";
export const ISSUE_ERR_PREFIX = "ISSUE_ERR:";

export class IssueRouteError extends Error {
  readonly kind: IssueErrorKind;
  constructor(kind: IssueErrorKind) {
    super(`${ISSUE_ERR_PREFIX}${kind}`);
    this.name = "IssueRouteError";
    this.kind = kind;
  }
}

/** Issues keeps 404 leak-free while distinguishing the retryable projection-unavailable state. */
async function issueAuthed<T>(fetcher: () => Promise<T>): Promise<T> {
  try {
    return await fetcher();
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    if (error instanceof GatewayError) {
      if (error.status === 400) throw new IssueRouteError("bad-input");
      if (error.status === 404 || error.status === 403) throw new IssueRouteError("not-found");
      if (error.status === 503) throw new IssueRouteError("unavailable");
    }
    if (error instanceof IssueRouteError) throw error;
    throw new IssueRouteError("error");
  }
}

function seg(value: string): string {
  return encodeURIComponent(value);
}

/** Authoritative Issues list. `key` is an ASCII issue-key prefix, never title/free-text search. */
export const getIssues = query(async (input: IssueListInput): Promise<IssuesPage> => {
  "use server";
  const parsed = parseIssueListInput(input);
  if (!parsed) throw new IssueRouteError("bad-input");
  const query = new URLSearchParams({ state: parsed.state });
  if (parsed.key) query.set("key", parsed.key);
  if (parsed.cursor) query.set("cursor", parsed.cursor);
  if (parsed.limit) query.set("limit", String(parsed.limit));
  return issueAuthed(async () => {
    const page = parseIssuesPage(await edgeGet(`/v1/issues?${query.toString()}`));
    if (!page) throw new IssueRouteError("error");
    return page;
  });
}, "issues-list");

export const getIssue = query(async (id: string): Promise<IssueVM> => {
  "use server";
  const parsed = parseIssueId(id);
  if (!parsed) throw new IssueRouteError("bad-input");
  return issueAuthed(async () => {
    const issue = parseIssue(await edgeGet(`/v1/issues/${seg(parsed)}`));
    if (!issue) throw new IssueRouteError("error");
    return issue;
  });
}, "issue-detail");

export type IssueMutationResult =
  | { ok: true; op: "create"; receipt: IssueCreateReceipt }
  | { ok: true; op: "close"; issue: IssueVM }
  | { ok: true; op: "activation"; status: IssueAuthorizationStatus }
  | { ok: false; error: IssueErrorKind };

export const ISSUE_ACTIVATION_STATUS_TIMEOUT_MS = 10_000;

/** One Issues mutation action. Project choices are untrusted browser input; Edge authorizes the
 * project and resolves its current prefix/default type before accepting the issue. */
export const issuesMutate = action(async (mutation: IssueMutation) => {
  "use server";
  const result = (value: IssueMutationResult) => json(value, { revalidate: [] });
  try {
    const parsed = parseIssueMutation(mutation);
    if (!parsed) return result({ ok: false, error: "bad-input" });
    if (parsed.op === "create") {
      const receipt = await issueAuthed(async () => {
        const decoded = parseIssueCreateReceipt(await edgePost(
          "/v1/issues",
          { project_id: parsed.projectId, title: parsed.title },
          { idempotencyKey: crypto.randomUUID() },
        ));
        if (!decoded) throw new IssueRouteError("error");
        return decoded;
      });
      return result({ ok: true, op: "create", receipt });
    }
    if (parsed.op === "activation") {
      const status = await issueAuthed(async () => {
        const decoded = parseIssueAuthorizationStatus(await edgeGet(
          `/v1/issues/authorization-requests/${seg(parsed.requestEventId)}`,
          { timeoutMs: ISSUE_ACTIVATION_STATUS_TIMEOUT_MS },
        ));
        if (!decoded) throw new IssueRouteError("error");
        return decoded;
      });
      return result({ ok: true, op: "activation", status });
    }
    const issue = await issueAuthed(async () => {
      const decoded = parseIssue(await edgePost(`/v1/issues/${seg(parsed.issueId)}/close`, {}));
      if (!decoded) throw new IssueRouteError("error");
      return decoded;
    });
    return result({ ok: true, op: "close", issue });
  } catch (error) {
    if (error instanceof IssueRouteError) return result({ ok: false, error: error.kind });
    throw error;
  }
}, "issues-mutate");

export type { IssueListInput, IssueListState, IssueMutation };
