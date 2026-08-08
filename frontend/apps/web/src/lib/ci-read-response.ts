import { isCiRunCursor, isCiUuid } from "./ci-read-input";

const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

export type CiRunState =
  | "queued" | "running" | "succeeded" | "failed"
  | "cancelled" | "timed_out" | "reaped";
export type CiJobState =
  | "queued" | "leased" | "running" | "succeeded"
  | "failed" | "cancelled" | "reaped";
export type CiStepState = "running" | "passed" | "failed" | "skipped";

export interface CiRunVM {
  run_id: string;
  pipeline_id: string;
  repo_ref: string;
  commit_oid: string | null;
  trigger_kind: "push" | "pull_request" | "issue_transition" | "manual" | "agent" | "schedule";
  trust_tier: "trusted" | "untrusted_fork" | "self_hosted";
  state: CiRunState;
  cost_settled: boolean;
  created_at: string;
  finished_at: string | null;
}

export interface CiRunsPage {
  items: CiRunVM[];
  page: { next_cursor: string | null; limit: number };
}

export interface CiJobVM {
  job_id: string;
  stage: string;
  name: string;
  needs: string[];
  matrix_key: unknown;
  state: CiJobState;
  attempt: number;
  result_summary: unknown;
}

export interface CiStepVM {
  job_id: string;
  step_id: string;
  byte_start: number;
  byte_end: number | null;
  status: CiStepState;
  details_ref: string;
}

export interface CiRunDetailVM {
  run: CiRunVM;
  jobs: CiJobVM[];
  steps: CiStepVM[];
}

export interface CiLogRangeVM {
  run_id: string;
  job_id: string;
  byte_start: number;
  byte_end: number;
  total_end: number;
  next_offset: number | null;
  encoding: "base64";
  data: string;
  text: string;
}

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  const actual = Object.keys(value);
  return actual.length === keys.length && actual.every((key) => allowed.has(key));
}

function bounded(value: unknown, maximum: number, allowEmpty = false): value is string {
  return typeof value === "string" && (allowEmpty || value.length > 0) &&
    utf8.encode(value).byteLength <= maximum &&
    ![...value].some((character) => {
      const point = character.codePointAt(0)!;
      return point <= 0x1f || point === 0x7f;
    });
}

function canonicalTime(value: unknown): value is string {
  return typeof value === "string" &&
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}Z$/.test(value) &&
    !Number.isNaN(Date.parse(value));
}

function safeNonNegative(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function run(value: unknown): CiRunVM | null {
  const row = record(value);
  if (!row || !exact(row, [
    "run_id", "pipeline_id", "repo_ref", "commit_oid", "trigger_kind", "trust_tier",
    "state", "cost_settled", "created_at", "finished_at",
  ]) || !isCiUuid(row.run_id) || !isCiUuid(row.pipeline_id) ||
      !bounded(row.repo_ref, 1_024) ||
      (row.commit_oid !== null && !bounded(row.commit_oid, 256)) ||
      !["push", "pull_request", "issue_transition", "manual", "agent", "schedule"]
        .includes(row.trigger_kind as string) ||
      !["trusted", "untrusted_fork", "self_hosted"].includes(row.trust_tier as string) ||
      !["queued", "running", "succeeded", "failed", "cancelled", "timed_out", "reaped"]
        .includes(row.state as string) ||
      typeof row.cost_settled !== "boolean" || !canonicalTime(row.created_at) ||
      (row.finished_at !== null && !canonicalTime(row.finished_at))) return null;
  return row as unknown as CiRunVM;
}

function job(value: unknown): CiJobVM | null {
  const row = record(value);
  if (!row || !exact(row, [
    "job_id", "stage", "name", "needs", "matrix_key", "state", "attempt", "result_summary",
  ]) || !isCiUuid(row.job_id) || !bounded(row.stage, 256) || !bounded(row.name, 512) ||
      !Array.isArray(row.needs) || row.needs.length > 1_000 ||
      !row.needs.every(isCiUuid) ||
      !["queued", "leased", "running", "succeeded", "failed", "cancelled", "reaped"]
        .includes(row.state as string) ||
      !Number.isSafeInteger(row.attempt) || (row.attempt as number) < 1) return null;
  return row as unknown as CiJobVM;
}

function step(value: unknown): CiStepVM | null {
  const row = record(value);
  if (!row || !exact(row, [
    "job_id", "step_id", "byte_start", "byte_end", "status", "details_ref",
  ]) || !isCiUuid(row.job_id) || !bounded(row.step_id, 512) ||
      !safeNonNegative(row.byte_start) ||
      (row.byte_end !== null && (!safeNonNegative(row.byte_end) ||
        (row.byte_end as number) < (row.byte_start as number))) ||
      !["running", "passed", "failed", "skipped"].includes(row.status as string) ||
      row.details_ref !== `#step-${row.step_id}`) return null;
  return row as unknown as CiStepVM;
}

function canonicalBase64(value: unknown): Uint8Array | null {
  if (typeof value !== "string" || value.length % 4 !== 0 ||
      !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)) {
    return null;
  }
  try {
    const decoded = atob(value);
    if (btoa(decoded) !== value) return null;
    return Uint8Array.from(decoded, (character) => character.charCodeAt(0));
  } catch {
    return null;
  }
}

export function parseCiRunsPage(value: unknown): CiRunsPage | null {
  const envelope = record(value);
  const page = record(envelope?.page);
  if (!envelope || !exact(envelope, ["items", "page"]) || !Array.isArray(envelope.items) ||
      !page || !exact(page, ["next_cursor", "limit"]) ||
      !Number.isSafeInteger(page.limit) || (page.limit as number) < 1 ||
      (page.limit as number) > 100 || envelope.items.length > (page.limit as number) ||
      (page.next_cursor !== null && !isCiRunCursor(page.next_cursor))) return null;
  const items = envelope.items.map(run);
  return items.every((item): item is CiRunVM => item !== null)
    ? {
        items,
        page: {
          next_cursor: page.next_cursor as string | null,
          limit: page.limit as number,
        },
      }
    : null;
}

export function parseCiRunDetail(value: unknown, expectedRunId?: string): CiRunDetailVM | null {
  const envelope = record(value);
  if (!envelope || !exact(envelope, ["run", "jobs", "steps"]) ||
      !Array.isArray(envelope.jobs) || envelope.jobs.length > 10_000 ||
      !Array.isArray(envelope.steps) || envelope.steps.length > 100_000) return null;
  const runValue = run(envelope.run);
  const jobs = envelope.jobs.map(job);
  const steps = envelope.steps.map(step);
  if (!runValue || !jobs.every((item): item is CiJobVM => item !== null) ||
      !steps.every((item): item is CiStepVM => item !== null)) return null;
  if (expectedRunId !== undefined && runValue.run_id !== expectedRunId) return null;
  const jobIds = new Set(jobs.map((item) => item.job_id));
  if (jobIds.size !== jobs.length ||
      jobs.some((item) => new Set(item.needs).size !== item.needs.length ||
        item.needs.some((need) => !jobIds.has(need) || need === item.job_id))) return null;
  const byId = new Map(jobs.map((item) => [item.job_id, item]));
  const visiting = new Set<string>();
  const visited = new Set<string>();
  const acyclic = (jobId: string): boolean => {
    if (visiting.has(jobId)) return false;
    if (visited.has(jobId)) return true;
    visiting.add(jobId);
    const valid = byId.get(jobId)!.needs.every(acyclic);
    visiting.delete(jobId);
    if (valid) visited.add(jobId);
    return valid;
  };
  if (!jobs.every((item) => acyclic(item.job_id))) return null;
  const stepIds = new Set(steps.map((item) => `${item.job_id}\0${item.step_id}`));
  if (stepIds.size !== steps.length || steps.some((item) => !jobIds.has(item.job_id))) return null;
  return { run: runValue, jobs, steps };
}

export function parseCiLogRange(
  value: unknown,
  expected?: { run: string; job: string },
): CiLogRangeVM | null {
  const range = record(value);
  if (!range || !exact(range, [
    "run_id", "job_id", "byte_start", "byte_end", "total_end", "next_offset", "encoding", "data",
  ]) || !isCiUuid(range.run_id) || !isCiUuid(range.job_id) ||
      !safeNonNegative(range.byte_start) || !safeNonNegative(range.byte_end) ||
      !safeNonNegative(range.total_end) || (range.byte_end as number) < (range.byte_start as number) ||
      (range.next_offset !== null && (!safeNonNegative(range.next_offset) ||
        range.next_offset !== range.byte_end ||
        (range.next_offset as number) >= (range.total_end as number))) ||
      range.encoding !== "base64") return null;
  if (expected && (range.run_id !== expected.run || range.job_id !== expected.job)) return null;
  const start = range.byte_start as number;
  const end = range.byte_end as number;
  const total = range.total_end as number;
  const beyondEnd = start >= total;
  if ((beyondEnd && (end !== start || range.next_offset !== null)) ||
      (!beyondEnd && (end > total || (end < total) !== (range.next_offset !== null)))) return null;
  const bytes = canonicalBase64(range.data);
  if (!bytes || bytes.byteLength !== end - start) {
    return null;
  }
  return {
    run_id: range.run_id,
    job_id: range.job_id,
    byte_start: start,
    byte_end: end,
    total_end: total,
    next_offset: range.next_offset as number | null,
    encoding: "base64",
    data: range.data as string,
    text: new TextDecoder("utf-8", { fatal: false }).decode(bytes),
  };
}

export function ciRepoLabel(repoRef: string): string {
  const marker = "/git/repo/";
  const index = repoRef.indexOf(marker);
  return index >= 0 ? repoRef.slice(index + marker.length) : "repository";
}
