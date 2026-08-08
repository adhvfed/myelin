const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

export const CI_RUN_STATES = [
  "all",
  "queued",
  "running",
  "succeeded",
  "failed",
  "cancelled",
  "timed_out",
  "reaped",
] as const;

export type CiRunStateFilter = (typeof CI_RUN_STATES)[number];

export interface CiRunsInput {
  state?: CiRunStateFilter;
  limit?: number;
  cursor?: string;
}

export interface CiLogInput {
  run: string;
  job: string;
  start?: number;
  limit?: number;
}

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  return Object.keys(value).every((key) => allowed.has(key));
}

function canonicalBase64UrlFrame(value: string): boolean {
  const encoded = value.slice("cr1_".length);
  if (encoded.length % 4 === 1) return false;
  try {
    const padded = encoded.replace(/-/g, "+").replace(/_/g, "/") +
      "=".repeat((4 - encoded.length % 4) % 4);
    const bytes = atob(padded);
    const canonical = btoa(bytes)
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/, "");
    return canonical === encoded && bytes.length === 60 && bytes.charCodeAt(0) === 1;
  } catch {
    return false;
  }
}

export function isCiRunCursor(value: unknown): value is string {
  return typeof value === "string" && utf8.encode(value).byteLength <= 256 &&
    /^cr1_[A-Za-z0-9_-]+$/.test(value) && canonicalBase64UrlFrame(value);
}

export function isCiUuid(value: unknown): value is string {
  return typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
}

export function parseCiRunsInput(value: unknown): CiRunsInput | null {
  const input = record(value);
  if (!input || !exact(input, ["state", "limit", "cursor"]) ||
      (input.state !== undefined && !CI_RUN_STATES.includes(input.state as CiRunStateFilter)) ||
      (input.limit !== undefined && (!Number.isSafeInteger(input.limit) ||
        (input.limit as number) < 1 || (input.limit as number) > 100)) ||
      (input.cursor !== undefined && !isCiRunCursor(input.cursor))) return null;
  return {
    ...(input.state === undefined ? {} : { state: input.state as CiRunStateFilter }),
    ...(input.limit === undefined ? {} : { limit: input.limit as number }),
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
  };
}

export function ciRunsSearchParams(input: CiRunsInput): URLSearchParams {
  const params = new URLSearchParams();
  if (input.state !== undefined) params.set("state", input.state);
  if (input.limit !== undefined) params.set("limit", String(input.limit));
  if (input.cursor !== undefined) params.set("cursor", input.cursor);
  return params;
}

export function parseCiRunId(value: unknown): string | null {
  return isCiUuid(value) ? value : null;
}

export function parseCiLogInput(value: unknown): CiLogInput | null {
  const input = record(value);
  if (!input || !exact(input, ["run", "job", "start", "limit"]) ||
      !isCiUuid(input.run) || !isCiUuid(input.job) ||
      (input.start !== undefined && (!Number.isSafeInteger(input.start) ||
        (input.start as number) < 0)) ||
      (input.limit !== undefined && (!Number.isSafeInteger(input.limit) ||
        (input.limit as number) < 1 || (input.limit as number) > 256 * 1024))) return null;
  return {
    run: input.run,
    job: input.job,
    ...(input.start === undefined ? {} : { start: input.start as number }),
    ...(input.limit === undefined ? {} : { limit: input.limit as number }),
  };
}

export function ciLogSearchParams(input: CiLogInput): URLSearchParams {
  const params = new URLSearchParams();
  if (input.start !== undefined) params.set("start", String(input.start));
  if (input.limit !== undefined) params.set("limit", String(input.limit));
  return params;
}
