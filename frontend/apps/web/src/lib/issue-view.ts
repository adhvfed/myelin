import type {
  IssueAuthorizationStatus,
  IssueErrorKind,
  IssueListState,
  IssueStateCategory,
  IssueVM,
  IssuesPage,
} from "./issue-api";

const ISSUE_ERR_PREFIX = "ISSUE_ERR:";

export const MAX_ISSUE_TITLE_BYTES = 512;
export const MAX_ISSUE_KEY_PREFIX_BYTES = 32;
export const ISSUE_ACTIVATION_POLL_BUDGET_MS = 30_000;

export interface PendingIssue {
  id: string;
  key: string;
  requestEventId: string;
  phase: "pending" | "unconfirmed";
}

export type ActivationOutcome =
  | { phase: "active"; issue: IssueVM }
  | { phase: "unconfirmed" };

interface ActivationPollOptions {
  budgetMs?: number;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
  signal?: AbortSignal;
}

function abortError(): Error {
  const error = new Error("Operation aborted");
  error.name = "AbortError";
  return error;
}

/** Stop awaiting a promise promptly on abort while still consuming any late settlement. */
export function awaitWithAbort<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) return Promise.reject(abortError());
  return new Promise<T>((resolve, reject) => {
    let settled = false;
    const finish = (callback: () => void) => {
      if (settled) return;
      settled = true;
      signal.removeEventListener("abort", onAbort);
      callback();
    };
    const onAbort = () => finish(() => reject(abortError()));
    signal.addEventListener("abort", onAbort, { once: true });
    promise.then(
      (value) => finish(() => resolve(value)),
      (error) => finish(() => reject(error)),
    );
  });
}

/** Settle one status read inside the remaining polling budget, even if transport never settles. */
function pollWithinDeadline(
  poll: (signal: AbortSignal) => Promise<IssueAuthorizationStatus>,
  remainingMs: number,
  outerSignal?: AbortSignal,
): Promise<IssueAuthorizationStatus | null> {
  const controller = new AbortController();
  return new Promise((resolve, reject) => {
    let settled = false;

    const cleanup = () => {
      clearTimeout(timer);
      outerSignal?.removeEventListener("abort", abort);
    };
    const finish = (value: IssueAuthorizationStatus | null) => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(value);
    };
    const fail = (error: unknown) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error);
    };
    const abort = () => {
      controller.abort();
      finish(null);
    };

    if (outerSignal?.aborted || remainingMs <= 0) {
      controller.abort();
      resolve(null);
      return;
    }
    outerSignal?.addEventListener("abort", abort, { once: true });
    const timer = setTimeout(abort, remainingMs);
    Promise.resolve()
      .then(() => poll(controller.signal))
      .then(finish, (error) => controller.signal.aborted ? finish(null) : fail(error));
  });
}

/** Poll the uncached status action within a finite budget. Pending never becomes a false failure. */
export async function pollIssueActivation(
  requestEventId: string,
  poll: (requestEventId: string, signal: AbortSignal) => Promise<IssueAuthorizationStatus>,
  options: ActivationPollOptions = {},
): Promise<ActivationOutcome> {
  const budget = options.budgetMs ?? ISSUE_ACTIVATION_POLL_BUDGET_MS;
  const now = options.now ?? Date.now;
  const sleep = options.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
  const started = now();
  while (!options.signal?.aborted) {
    const remaining = budget - (now() - started);
    const status = await pollWithinDeadline(
      (signal) => poll(requestEventId, signal),
      remaining,
      options.signal,
    );
    if (!status) return { phase: "unconfirmed" };
    if (status.status === "active") return { phase: "active", issue: status.issue };
    const retryAfter = Number.isFinite(status.retry_after_ms) ? status.retry_after_ms : 250;
    const delay = Math.min(Math.max(retryAfter, 250), 5_000);
    if (now() - started + delay > budget) return { phase: "unconfirmed" };
    try {
      const pause = sleep(delay);
      if (options.signal) await awaitWithAbort(pause, options.signal);
      else await pause;
    } catch (error) {
      if (options.signal?.aborted) return { phase: "unconfirmed" };
      throw error;
    }
  }
  return { phase: "unconfirmed" };
}

export function issueListState(value: unknown): IssueListState {
  return value === "closed" || value === "all" ? value : "open";
}

/** Match the edge's bounded ASCII key-prefix grammar. Empty means no search filter. */
export function normalizeIssueKey(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const key = value.trim().toUpperCase();
  if (!key) return undefined;
  if (
    key.length > MAX_ISSUE_KEY_PREFIX_BYTES ||
    !/^[A-Z0-9-]+$/.test(key)
  ) {
    return undefined;
  }
  return key;
}

export function issueKeyError(value: string): string | null {
  const key = value.trim();
  if (!key) return null;
  if (key.length > MAX_ISSUE_KEY_PREFIX_BYTES) {
    return `Issue keys are at most ${MAX_ISSUE_KEY_PREFIX_BYTES} characters.`;
  }
  if (!/^[A-Za-z0-9-]+$/.test(key)) {
    return "Use letters, numbers, and hyphens only.";
  }
  return null;
}

export function issueListHref(input: {
  state: IssueListState;
  key?: string;
  create?: boolean;
}): string {
  const p = new URLSearchParams();
  if (input.state !== "open") p.set("state", input.state);
  const key = normalizeIssueKey(input.key);
  if (key) p.set("key", key);
  if (input.create) p.set("new", "1");
  const query = p.toString();
  return `/issues${query ? `?${query}` : ""}`;
}

export function issueTitleError(value: string): string | null {
  const title = value.trim();
  if (!title) return "Enter an issue title.";
  const bytes = new TextEncoder().encode(title).byteLength;
  if (bytes > MAX_ISSUE_TITLE_BYTES) {
    return `Keep the title to ${MAX_ISSUE_TITLE_BYTES} UTF-8 bytes or fewer.`;
  }
  return null;
}

export function isClosedCategory(category: IssueStateCategory): boolean {
  return category === "completed" || category === "cancelled";
}

export function issueTimestamp(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  return new Intl.DateTimeFormat("en-GB", {
    year: "numeric",
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    timeZone: "UTC",
    timeZoneName: "short",
  }).format(date);
}

export function mergeIssuePages(first: IssuesPage | undefined, extra: IssuesPage[]): IssueVM[] {
  const seen = new Set<string>();
  const rows: IssueVM[] = [];
  for (const page of first ? [first, ...extra] : extra) {
    for (const issue of page.items) {
      if (seen.has(issue.id)) continue;
      seen.add(issue.id);
      rows.push(issue);
    }
  }
  return rows;
}

export function issueErrorKind(error: unknown): IssueErrorKind {
  const message = error instanceof Error ? error.message : String(error ?? "");
  const encoded = message.startsWith(ISSUE_ERR_PREFIX)
    ? message.slice(ISSUE_ERR_PREFIX.length)
    : "";
  return encoded === "bad-input" ||
    encoded === "not-found" ||
    encoded === "unavailable"
    ? encoded
    : "error";
}
