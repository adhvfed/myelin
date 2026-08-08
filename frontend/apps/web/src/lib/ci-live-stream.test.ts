import { describe, expect, it, vi } from "vitest";
import {
  consumeCiLiveStream,
  isCiLogCursor,
  parseCiLiveEvent,
} from "./ci-live-stream";

const run = "91000000-0000-4000-8000-000000000002";
const job = "92000000-0000-4000-8000-000000000002";

function eventStream(frames: string[]): Response {
  const bytes = new TextEncoder().encode(frames.join(""));
  return new Response(new ReadableStream({
    start(controller) {
      for (let offset = 0; offset < bytes.length; offset += 3) {
        controller.enqueue(bytes.slice(offset, offset + 3));
      }
      controller.close();
    },
  }), { headers: { "content-type": "text/event-stream; charset=utf-8" } });
}

describe("CI live SSE contract", () => {
  it("accepts canonical u64 cursors without losing precision", () => {
    expect(isCiLogCursor("0")).toBe(true);
    expect(isCiLogCursor("18446744073709551615")).toBe(true);
    for (const value of ["", "00", "01", "-1", "+1", "18446744073709551616"]) {
      expect(isCiLogCursor(value), value).toBe(false);
    }
  });

  it("parses only exact scope-bound pointer frames", () => {
    expect(parseCiLiveEvent(
      "ci.log.appended",
      "2",
      JSON.stringify({ run_id: run, job_id: job, byte_start: 5, byte_end: 11 }),
      { run, job },
    )).toEqual({
      kind: "appended",
      cursor: "2",
      run_id: run,
      job_id: job,
      byte_start: 5,
      byte_end: 11,
    });
    expect(parseCiLiveEvent(
      "ci.log.appended",
      "2",
      JSON.stringify({ run_id: run, job_id: job, byte_start: 5, byte_end: 11, data: "no" }),
      { run, job },
    )).toBeNull();
    expect(parseCiLiveEvent(
      "ci.log.appended",
      "2",
      JSON.stringify({ run_id: run, job_id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", byte_start: 5, byte_end: 11 }),
      { run, job },
    )).toBeNull();
  });

  it("waits for each split CRLF frame acknowledgement before accepting the next", async () => {
    const seen: string[] = [];
    let acknowledgeFirst: (() => void) | undefined;
    const firstAcknowledged = new Promise<void>((resolve) => {
      acknowledgeFirst = resolve;
    });
    const consuming = consumeCiLiveStream(eventStream([
      ": connected\r\n",
      "event: ci.log.ready\r\nid: 1\r\n",
      `data: ${JSON.stringify({ run_id: run, job_id: job, byte_end: 5 })}\r\n\r\n`,
      "event: ci.log.appended\nid: 2\n",
      `data: ${JSON.stringify({ run_id: run, job_id: job, byte_start: 5, byte_end: 11 })}\n\n`,
    ]), { run, job }, async (event) => {
      seen.push(event.cursor ?? "none");
      if (event.cursor === "1") await firstAcknowledged;
    });
    await vi.waitFor(() => expect(seen).toEqual(["1"]));
    acknowledgeFirst!();
    await consuming;
    expect(seen).toEqual(["1", "2"]);
  });

  it("refuses a truncated frame without acknowledging it", async () => {
    const callback = vi.fn(async () => undefined);
    await expect(consumeCiLiveStream(eventStream([
      "event: ci.log.ready\nid: 1\n",
      `data: ${JSON.stringify({ run_id: run, job_id: job, byte_end: 5 })}`,
    ]), { run, job }, callback)).rejects.toThrow("CI_LIVE_TRUNCATED_FRAME");
    expect(callback).not.toHaveBeenCalled();
  });
});
