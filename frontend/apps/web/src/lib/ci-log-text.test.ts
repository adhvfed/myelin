import { describe, expect, it } from "vitest";

import {
  boundedCiLogWindow,
  ciLogTextPage,
  ciLogTextRequest,
  decodeCiLogWindow,
} from "./ci-log-text";
import { parseCiLogRange } from "./ci-read-response";

const RUN = "91000000-0000-4000-8000-000000000001";
const JOB = "92000000-0000-4000-8000-000000000001";
const encoder = new TextEncoder();

function range(log: Uint8Array, start: number, limit: number) {
  const end = start < log.byteLength ? Math.min(start + limit, log.byteLength) : start;
  const data = start < log.byteLength
    ? btoa(String.fromCharCode(...log.slice(start, end)))
    : "";
  return parseCiLogRange({
    run_id: RUN,
    job_id: JOB,
    byte_start: start,
    byte_end: end,
    total_end: log.byteLength,
    next_offset: end < log.byteLength ? end : null,
    encoding: "base64",
    data,
  })!;
}

describe("CI log text windows", () => {
  it("moves a page boundary around one whole UTF-8 character", () => {
    const log = encoder.encode("a😀b");
    const firstRequest = ciLogTextRequest(0, 3)!;
    const first = ciLogTextPage(
      range(log, firstRequest.start, firstRequest.limit),
      0,
      3,
    )!;
    expect(first).toMatchObject({ byte_start: 0, byte_end: 5, next_offset: 5, text: "a😀" });

    const secondRequest = ciLogTextRequest(first.next_offset!, 3)!;
    const second = ciLogTextPage(
      range(log, secondRequest.start, secondRequest.limit),
      first.next_offset!,
      3,
    )!;
    expect(second).toMatchObject({ byte_start: 5, byte_end: 6, next_offset: null, text: "b" });
    expect(first.text + second.text).toBe("a😀b");
  });

  it("repairs a deep link that begins within a UTF-8 character", () => {
    const log = encoder.encode("a😀b");
    const request = ciLogTextRequest(3, 1)!;
    expect(ciLogTextPage(range(log, request.start, request.limit), 3, 1)).toMatchObject({
      byte_start: 1,
      byte_end: 5,
      next_offset: 5,
      text: "😀",
    });
  });

  it("keeps malformed bytes visible while withholding only incomplete live tails", () => {
    const invalid = Uint8Array.from([0x61, 0x80, 0x62]);
    expect(decodeCiLogWindow(invalid, true)).toBe("a�b");

    const partial = encoder.encode("a😀").slice(0, 3);
    expect(decodeCiLogWindow(partial, false)).toBe("a");
    expect(decodeCiLogWindow(partial, true)).toBe("a�");
  });

  it("drops a split leading character from a bounded live window", () => {
    const bytes = encoder.encode("a😀bc");
    expect(new TextDecoder().decode(boundedCiLogWindow(bytes, 4, false))).toBe("bc");
    expect(new TextDecoder().decode(boundedCiLogWindow(bytes.slice(3), 8, true))).toBe("bc");
  });

  it("normalizes an empty beyond-end page to the requested coordinate", () => {
    const log = encoder.encode("done");
    expect(ciLogTextPage(range(log, 97, 7), 100, 1)).toMatchObject({
      byte_start: 100,
      byte_end: 100,
      total_end: 4,
      next_offset: null,
      text: "",
    });
  });
});
