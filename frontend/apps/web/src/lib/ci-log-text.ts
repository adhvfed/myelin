import type { CiLogRangeVM } from "./ci-read-response";

const UTF8_CONTEXT_BYTES = 3;
const MAX_CI_LOG_RANGE_BYTES = 256 * 1024;

export interface CiLogTextPage {
  run_id: string;
  job_id: string;
  byte_start: number;
  byte_end: number;
  total_end: number;
  next_offset: number | null;
  text: string;
}

export function ciLogTextRequest(start: number, limit: number): {
  start: number;
  limit: number;
} | null {
  if (!Number.isSafeInteger(start) || start < 0 || !Number.isSafeInteger(limit) || limit < 1) {
    return null;
  }
  const transportStart = Math.max(0, start - UTF8_CONTEXT_BYTES);
  const transportLimit = limit + (start - transportStart) + UTF8_CONTEXT_BYTES;
  return transportLimit <= MAX_CI_LOG_RANGE_BYTES
    ? { start: transportStart, limit: transportLimit }
    : null;
}

export function ciLogTextPage(
  range: CiLogRangeVM,
  requestedStart: number,
  requestedLimit: number,
): CiLogTextPage | null {
  if (!Number.isSafeInteger(requestedStart) || requestedStart < 0 ||
      !Number.isSafeInteger(requestedLimit) || requestedLimit < 1) return null;
  if (requestedStart >= range.total_end) {
    return {
      run_id: range.run_id,
      job_id: range.job_id,
      byte_start: requestedStart,
      byte_end: requestedStart,
      total_end: range.total_end,
      next_offset: null,
      text: "",
    };
  }
  const desiredEnd = Math.min(requestedStart + requestedLimit, range.total_end);
  if (!Number.isSafeInteger(desiredEnd) || range.byte_start > requestedStart ||
      range.byte_end < desiredEnd) return null;
  const bytes = decodeBase64(range.data);
  const relativeStart = requestedStart - range.byte_start;
  const relativeEnd = desiredEnd - range.byte_start;
  if (relativeStart < 0 || relativeEnd > bytes.byteLength) return null;
  const startSequence = sequenceContaining(bytes, relativeStart);
  const endSequence = sequenceContaining(bytes, relativeEnd);
  const start = startSequence?.start ?? relativeStart;
  const end = endSequence?.end ?? relativeEnd;
  const byteStart = range.byte_start + start;
  const byteEnd = range.byte_start + end;
  return {
    run_id: range.run_id,
    job_id: range.job_id,
    byte_start: byteStart,
    byte_end: byteEnd,
    total_end: range.total_end,
    next_offset: byteEnd < range.total_end ? byteEnd : null,
    text: new TextDecoder("utf-8", { fatal: false }).decode(bytes.slice(start, end)),
  };
}

export function boundedCiLogWindow(
  bytes: Uint8Array<ArrayBufferLike>,
  maximum: number,
  startsAfterZero: boolean,
): Uint8Array<ArrayBufferLike> {
  let start = Math.max(0, bytes.byteLength - maximum);
  const split = sequenceContaining(bytes, start);
  if (split) start = split.end;
  if (start === 0 && startsAfterZero) {
    while (start < bytes.byteLength && start < UTF8_CONTEXT_BYTES && isContinuation(bytes[start]!)) {
      start += 1;
    }
  }
  return bytes.slice(start);
}

export function decodeCiLogWindow(
  bytes: Uint8Array<ArrayBufferLike>,
  terminal: boolean,
): string {
  return new TextDecoder("utf-8", { fatal: false }).decode(bytes, { stream: !terminal });
}

function decodeBase64(value: string): Uint8Array {
  const decoded = atob(value);
  return Uint8Array.from(decoded, (character) => character.charCodeAt(0));
}

function sequenceContaining(
  bytes: Uint8Array<ArrayBufferLike>,
  boundary: number,
): { start: number; end: number } | null {
  if (boundary <= 0 || boundary >= bytes.byteLength || !isContinuation(bytes[boundary]!)) {
    return null;
  }
  const first = Math.max(0, boundary - UTF8_CONTEXT_BYTES);
  for (let start = first; start < boundary; start += 1) {
    const end = validSequenceEnd(bytes, start);
    if (end !== null && start < boundary && boundary < end) return { start, end };
  }
  return null;
}

function validSequenceEnd(bytes: Uint8Array<ArrayBufferLike>, start: number): number | null {
  const first = bytes[start]!;
  const length = first >= 0xc2 && first <= 0xdf ? 2
    : first >= 0xe0 && first <= 0xef ? 3
    : first >= 0xf0 && first <= 0xf4 ? 4
    : null;
  if (length === null || start + length > bytes.byteLength) return null;
  const second = bytes[start + 1]!;
  if (!isContinuation(second) || (first === 0xe0 && second < 0xa0) ||
      (first === 0xed && second > 0x9f) || (first === 0xf0 && second < 0x90) ||
      (first === 0xf4 && second > 0x8f)) return null;
  for (let index = start + 2; index < start + length; index += 1) {
    if (!isContinuation(bytes[index]!)) return null;
  }
  return start + length;
}

function isContinuation(byte: number): boolean {
  return byte >= 0x80 && byte <= 0xbf;
}
