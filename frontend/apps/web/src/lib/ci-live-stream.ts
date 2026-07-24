import { isCiUuid } from "./ci-read-input";

const MAX_SSE_FRAME_BYTES = 16 * 1024;
const MAX_U64 = BigInt("18446744073709551615");
const utf8 = new TextEncoder();

type WireRecord = Record<string, unknown>;

export type CiLiveEvent =
  | {
      kind: "ready";
      cursor: string;
      run_id: string;
      job_id: string;
      byte_end: number;
    }
  | {
      kind: "appended";
      cursor: string;
      run_id: string;
      job_id: string;
      byte_start: number;
      byte_end: number;
    }
  | {
      kind: "complete";
      cursor?: string;
      run_id: string;
      job_id: string;
      byte_end: number;
    };

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  return Object.keys(value).length === keys.length &&
    Object.keys(value).every((key) => allowed.has(key));
}

function byteOffset(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

export function isCiLogCursor(value: unknown): value is string {
  if (typeof value !== "string" || !/^(?:0|[1-9][0-9]*)$/.test(value)) return false;
  try {
    return BigInt(value) <= MAX_U64;
  } catch {
    return false;
  }
}

export function parseCiLiveEvent(
  event: string,
  id: string | undefined,
  data: string,
  expected: { run: string; job: string },
): CiLiveEvent | null {
  if (!isCiUuid(expected.run) || !isCiUuid(expected.job) ||
      (id !== undefined && !isCiLogCursor(id))) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(data);
  } catch {
    return null;
  }
  const body = record(parsed);
  if (!body || body.run_id !== expected.run || body.job_id !== expected.job) return null;

  if (event === "ci.log.ready") {
    if (!id || !exact(body, ["run_id", "job_id", "byte_end"]) ||
        !byteOffset(body.byte_end)) return null;
    return {
      kind: "ready",
      cursor: id,
      run_id: expected.run,
      job_id: expected.job,
      byte_end: body.byte_end,
    };
  }
  if (event === "ci.log.appended") {
    if (!id || !exact(body, ["run_id", "job_id", "byte_start", "byte_end"]) ||
        !byteOffset(body.byte_start) || !byteOffset(body.byte_end) ||
        body.byte_end <= body.byte_start) return null;
    return {
      kind: "appended",
      cursor: id,
      run_id: expected.run,
      job_id: expected.job,
      byte_start: body.byte_start,
      byte_end: body.byte_end,
    };
  }
  if (event === "ci.log.complete") {
    if (!exact(body, ["run_id", "job_id", "byte_end"]) || !byteOffset(body.byte_end)) return null;
    return {
      kind: "complete",
      ...(id === undefined ? {} : { cursor: id }),
      run_id: expected.run,
      job_id: expected.job,
      byte_end: body.byte_end,
    };
  }
  return null;
}

interface PendingFrame {
  event?: string;
  id?: string;
  data?: string;
}

/**
 * Consume one bounded Edge SSE response. The callback resolves before the next frame is accepted,
 * so a durable pointer is never acknowledged locally before its archive bytes have been read.
 */
export async function consumeCiLiveStream(
  response: Response,
  expected: { run: string; job: string },
  onEvent: (event: CiLiveEvent) => Promise<void>,
): Promise<void> {
  const contentType = response.headers.get("content-type")?.split(";", 1)[0]?.trim().toLowerCase();
  if (!response.body || contentType !== "text/event-stream") {
    throw new Error("CI_LIVE_NOT_EVENT_STREAM");
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let frame: PendingFrame = {};
  let frameBytes = 0;

  const dispatch = async () => {
    if (frame.event === undefined && frame.id === undefined && frame.data === undefined) return;
    if (frame.event === undefined || frame.data === undefined) {
      throw new Error("CI_LIVE_INCOMPLETE_FRAME");
    }
    const parsed = parseCiLiveEvent(frame.event, frame.id, frame.data, expected);
    if (!parsed) throw new Error("CI_LIVE_INVALID_FRAME");
    await onEvent(parsed);
  };

  const line = async (value: string) => {
    frameBytes += utf8.encode(value).byteLength + 1;
    if (frameBytes > MAX_SSE_FRAME_BYTES) throw new Error("CI_LIVE_FRAME_TOO_LARGE");
    if (value === "") {
      await dispatch();
      frame = {};
      frameBytes = 0;
      return;
    }
    if (value.startsWith(":")) return;
    const colon = value.indexOf(":");
    const field = colon < 0 ? value : value.slice(0, colon);
    const raw = colon < 0 ? "" : value.slice(colon + 1);
    const fieldValue = raw.startsWith(" ") ? raw.slice(1) : raw;
    if (field === "event") {
      if (frame.event !== undefined) throw new Error("CI_LIVE_DUPLICATE_FIELD");
      frame.event = fieldValue;
    } else if (field === "id") {
      if (frame.id !== undefined || fieldValue.includes("\0")) {
        throw new Error("CI_LIVE_INVALID_ID");
      }
      frame.id = fieldValue;
    } else if (field === "data") {
      if (frame.data !== undefined) throw new Error("CI_LIVE_DUPLICATE_FIELD");
      frame.data = fieldValue;
    } else if (field !== "retry") {
      throw new Error("CI_LIVE_UNKNOWN_FIELD");
    }
  };

  try {
    while (true) {
      const next = await reader.read();
      buffer += decoder.decode(next.value, { stream: !next.done });
      while (true) {
        const lf = buffer.indexOf("\n");
        const cr = buffer.indexOf("\r");
        const end = lf < 0 ? cr : cr < 0 ? lf : Math.min(lf, cr);
        if (end < 0) break;
        if (buffer[end] === "\r" && end + 1 === buffer.length && !next.done) break;
        const consumed = buffer[end] === "\r" && buffer[end + 1] === "\n" ? 2 : 1;
        const value = buffer.slice(0, end);
        buffer = buffer.slice(end + consumed);
        await line(value);
      }
      if (next.done) break;
      if (utf8.encode(buffer).byteLength + frameBytes > MAX_SSE_FRAME_BYTES) {
        throw new Error("CI_LIVE_FRAME_TOO_LARGE");
      }
    }
    // Edge terminates every frame with a blank line. An unterminated tail is not acknowledged.
    if (buffer.length > 0 || frame.event !== undefined || frame.id !== undefined ||
        frame.data !== undefined) throw new Error("CI_LIVE_TRUNCATED_FRAME");
  } catch (error) {
    await reader.cancel().catch(() => undefined);
    throw error;
  } finally {
    reader.releaseLock();
  }
}
