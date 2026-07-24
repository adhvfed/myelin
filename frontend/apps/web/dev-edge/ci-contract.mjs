// CT-005 browser-facing CI read contract. This is a local second implementation of the production
// Edge surface, so it deliberately owns no fixture convention of its own: the shared golden vectors
// in `contracts/ci-read-dev-edge.golden.json` are executed against this module and the Rust handler.

import { createHash, createHmac, timingSafeEqual } from "node:crypto";

const CURSOR_KEY = Buffer.from(
  "myelin dev edge CI cursor authority — test only",
  "utf8",
);
const RUN_STATES = new Set([
  "all", "queued", "running", "succeeded", "failed", "cancelled", "timed_out", "reaped",
]);
const JOB_STATES = new Set([
  "queued", "leased", "running", "succeeded", "failed", "cancelled", "reaped",
]);
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const CURSOR_PREFIX = "cr1_";
const TIMESTAMP_BYTES = 27;
const UUID_BYTES = 16;
const TAG_BYTES = 16;
const FRAME_BYTES = 1 + TIMESTAMP_BYTES + UUID_BYTES + TAG_BYTES;
const DEFAULT_LIMIT = 50;
const MAX_LIMIT = 100;
const DEFAULT_LOG_LIMIT = 64 * 1024;
const MAX_LOG_LIMIT = 256 * 1024;

const PIPELINE = "93000000-0000-4000-8000-000000000001";
export const CI_NEWEST_RUN = "91000000-0000-4000-8000-000000000001";
export const CI_OLDER_RUN = "91000000-0000-4000-8000-000000000002";
export const CI_FAILED_JOB = "92000000-0000-4000-8000-000000000001";
const ARCHIVED_LOG = Buffer.from("prep\ncafé\nfailed\n", "utf8");
export const CI_VISIBLE_REPO_REFS = Object.freeze([
  "myelin://acme/git/repo/😀",
  "myelin://acme/git/repo/alpha",
  "myelin://acme/git/repo/é",
  "myelin://acme/git/repo/é",
  "myelin://acme/git/repo/alpha",
]);

const NEWEST_RUN = Object.freeze({
    run_id: CI_NEWEST_RUN,
    pipeline_id: PIPELINE,
    repo_ref: "myelin://acme/git/repo/alpha",
    commit_oid: "0123456789abcdef",
    trigger_kind: "push",
    trust_tier: "trusted",
    state: "failed",
    cost_settled: true,
    created_at: "2026-07-24T12:00:00.000000Z",
    finished_at: "2026-07-24T12:05:00.000000Z",
  });
const OLDER_RUN = Object.freeze({
    run_id: CI_OLDER_RUN,
    pipeline_id: PIPELINE,
    repo_ref: "myelin://acme/git/repo/alpha",
    commit_oid: "fedcba9876543210",
    trigger_kind: "pull_request",
    trust_tier: "trusted",
    state: "running",
    cost_settled: false,
    created_at: "2026-07-24T11:00:00.000000Z",
    finished_at: null,
  });

// Deliberately not in output order. `ciRunsEnvelope` must execute the production keyset order rather
// than inheriting fixture order by convention.
export const CI_RUN_FIXTURES = Object.freeze([
  OLDER_RUN,
  NEWEST_RUN,
]);

export function isCiUuid(value) {
  return typeof value === "string" && UUID.test(value);
}

const FAILED_DETAIL = Object.freeze({
  run: NEWEST_RUN,
  jobs: Object.freeze([
    Object.freeze({
      job_id: CI_FAILED_JOB,
      stage: "test",
      name: "contract",
      needs: Object.freeze([]),
      matrix_key: null,
      state: "failed",
      attempt: 1,
      result_summary: Object.freeze({ message: "contract failed" }),
    }),
  ]),
  steps: Object.freeze([
    Object.freeze({
      job_id: CI_FAILED_JOB,
      step_id: "contract",
      byte_start: 0,
      byte_end: ARCHIVED_LOG.byteLength,
      status: "failed",
      details_ref: "#step-contract",
    }),
  ]),
});

function parseCanonicalInteger(value, maximum, allowZero) {
  if (typeof value !== "string" || !/^(?:0|[1-9][0-9]*)$/.test(value)) return null;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed > maximum || (!allowZero && parsed === 0)) return null;
  return parsed;
}

function rawQuery(query, allowed) {
  const values = new Map();
  if (!query) return values;
  for (const pair of query.split("&")) {
    if (!pair || !pair.includes("=")) return null;
    const [name, ...tail] = pair.split("=");
    const value = tail.join("=");
    if (!name || !allowed.has(name) || values.has(name)) return null;
    values.set(name, value);
  }
  return values;
}

function uuidBytes(value) {
  return Buffer.from(value.replaceAll("-", ""), "hex");
}

function uuidFromBytes(bytes) {
  const hex = bytes.toString("hex");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function utf8Compare(left, right) {
  return Buffer.compare(Buffer.from(left, "utf8"), Buffer.from(right, "utf8"));
}

export function canonicalVisibleRepoRefs(values) {
  if (!Array.isArray(values) || values.length > 10_000 ||
      values.some((value) => typeof value !== "string" ||
        Buffer.byteLength(value, "utf8") === 0 || Buffer.byteLength(value, "utf8") > 1_024)) {
    throw new Error("invalid dev-edge CI visibility set");
  }
  const sorted = [...values].sort(utf8Compare);
  return sorted.filter((value, index) => index === 0 || value !== sorted[index - 1]);
}

function scopeDigest(state, visibleRepoRefs, tenant = "acme", region = "eu-west") {
  const hash = createHash("sha256");
  hash.update("myelin:dev-edge:ci-run-cursor-scope:v1\0");
  for (const value of [tenant, region, state, ...canonicalVisibleRepoRefs(visibleRepoRefs)]) {
    const bytes = Buffer.from(value, "utf8");
    const length = Buffer.alloc(8);
    length.writeBigUInt64BE(BigInt(bytes.byteLength));
    hash.update(length);
    hash.update(bytes);
  }
  return hash.digest().subarray(0, TAG_BYTES);
}

function cursorTag(scope, coordinate) {
  return createHmac("sha256", CURSOR_KEY)
    .update("myelin:dev-edge:ci-run-cursor-coordinate:v1\0")
    .update(scope)
    .update(coordinate)
    .digest()
    .subarray(0, TAG_BYTES);
}

function encodeCursor(run, state, visibleRepoRefs) {
  const coordinate = Buffer.concat([
    Buffer.from(run.created_at, "ascii"),
    uuidBytes(run.run_id),
  ]);
  const frame = Buffer.concat([
    Buffer.from([1]),
    coordinate,
    cursorTag(scopeDigest(state, visibleRepoRefs), coordinate),
  ]);
  return `${CURSOR_PREFIX}${frame.toString("base64url")}`;
}

function decodeCursor(value, state, visibleRepoRefs) {
  if (typeof value !== "string" || value.length > 256 ||
      !value.startsWith(CURSOR_PREFIX) || !/^cr1_[A-Za-z0-9_-]+$/.test(value)) {
    return { kind: "bad" };
  }
  const encoded = value.slice(CURSOR_PREFIX.length);
  let frame;
  try {
    frame = Buffer.from(encoded, "base64url");
  } catch {
    return { kind: "bad" };
  }
  if (frame.byteLength !== FRAME_BYTES || frame[0] !== 1 ||
      frame.toString("base64url") !== encoded) return { kind: "bad" };
  const timestamp = frame.subarray(1, 1 + TIMESTAMP_BYTES).toString("ascii");
  const runId = uuidFromBytes(frame.subarray(1 + TIMESTAMP_BYTES, 1 + TIMESTAMP_BYTES + UUID_BYTES));
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}Z$/.test(timestamp) ||
      Number.isNaN(Date.parse(timestamp)) || !UUID.test(runId)) return { kind: "bad" };
  const coordinate = frame.subarray(1, 1 + TIMESTAMP_BYTES + UUID_BYTES);
  const actualTag = frame.subarray(1 + TIMESTAMP_BYTES + UUID_BYTES);
  const expectedTag = cursorTag(scopeDigest(state, visibleRepoRefs), coordinate);
  if (!timingSafeEqual(actualTag, expectedTag)) return { kind: "stale" };
  return { kind: "ok", createdAt: timestamp, runId };
}

export function parseCiRunsQuery(query) {
  const values = rawQuery(query, new Set(["state", "limit", "cursor"]));
  if (!values) return null;
  const state = values.get("state") ?? "all";
  if (!RUN_STATES.has(state)) return null;
  const limitValue = values.get("limit");
  const limit = limitValue === undefined
    ? DEFAULT_LIMIT
    : parseCanonicalInteger(limitValue, MAX_LIMIT, false);
  if (limit === null) return null;
  const cursor = values.get("cursor");
  if (cursor === "") return null;
  return { state, limit, ...(cursor === undefined ? {} : { cursor }) };
}

export function ciRunsEnvelope(input, options = {}) {
  const visibleRepoRefs = options.visibleRepoRefs ?? CI_VISIBLE_REPO_REFS;
  const visible = new Set(canonicalVisibleRepoRefs(visibleRepoRefs));
  const allRows = options.empty ? [] : CI_RUN_FIXTURES;
  let coordinate;
  if (input.cursor !== undefined) {
    const decoded = decodeCursor(input.cursor, input.state, visibleRepoRefs);
    if (decoded.kind !== "ok") return { [decoded.kind]: true };
    coordinate = decoded;
  }
  const filtered = allRows.filter((run) =>
    visible.has(run.repo_ref) &&
    (input.state === "all" || run.state === input.state) &&
    (!coordinate || run.created_at < coordinate.createdAt ||
      (run.created_at === coordinate.createdAt && run.run_id < coordinate.runId))
  ).sort((left, right) =>
    utf8Compare(right.created_at, left.created_at) ||
    utf8Compare(right.run_id, left.run_id)
  );
  const items = filtered.slice(0, input.limit);
  const next = filtered.length > input.limit && items.length > 0
    ? encodeCursor(items.at(-1), input.state, visibleRepoRefs)
    : null;
  return {
    items,
    page: { next_cursor: next, limit: input.limit },
  };
}

export function ciRunJson(runId, options = {}) {
  if (options.empty || runId !== CI_NEWEST_RUN) return null;
  return FAILED_DETAIL;
}

export function parseCiLogQuery(query) {
  const values = rawQuery(query, new Set(["start", "limit"]));
  if (!values) return null;
  const startValue = values.get("start");
  const limitValue = values.get("limit");
  const start = startValue === undefined
    ? 0
    : parseCanonicalInteger(startValue, Number.MAX_SAFE_INTEGER, true);
  const limit = limitValue === undefined
    ? DEFAULT_LOG_LIMIT
    : parseCanonicalInteger(limitValue, MAX_LOG_LIMIT, false);
  return start === null || limit === null ? null : { start, limit };
}

export function ciLogJson(runId, jobId, input, options = {}) {
  if (options.empty || runId !== CI_NEWEST_RUN || jobId !== CI_FAILED_JOB) return null;
  if (!JOB_STATES.has(FAILED_DETAIL.jobs[0].state)) throw new Error("invalid CI fixture state");
  const byteEnd = input.start < ARCHIVED_LOG.byteLength
    ? Math.min(input.start + input.limit, ARCHIVED_LOG.byteLength)
    : input.start;
  return {
    run_id: runId,
    job_id: jobId,
    byte_start: input.start,
    byte_end: byteEnd,
    total_end: ARCHIVED_LOG.byteLength,
    next_offset: byteEnd < ARCHIVED_LOG.byteLength ? byteEnd : null,
    encoding: "base64",
    data: ARCHIVED_LOG.subarray(input.start, byteEnd).toString("base64"),
  };
}
