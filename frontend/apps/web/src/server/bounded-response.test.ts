import { describe, expect, it } from "vitest";

import { readLimitedBytes, readLimitedText, streamLimitedBytes } from "./bounded-response";

describe("bounded upstream responses", () => {
  it("returns bodies at the byte limit", async () => {
    await expect(readLimitedText(new Response("four"), 4)).resolves.toBe("four");
  });

  it("refuses an oversized declared body before reading it", async () => {
    const response = new Response("small", { headers: { "content-length": "100" } });
    await expect(readLimitedBytes(response, 99)).rejects.toThrow(/byte limit/);
  });

  it("refuses a chunked body once its observed bytes exceed the limit", async () => {
    const response = new Response(new ReadableStream({
      start(controller) {
        controller.enqueue(new Uint8Array([1, 2]));
        controller.enqueue(new Uint8Array([3, 4]));
        controller.close();
      },
    }));
    await expect(readLimitedBytes(response, 3)).rejects.toThrow(/byte limit/);
  });

  it("rejects invalid limits and invalid UTF-8", async () => {
    await expect(readLimitedBytes(new Response(""), -1)).rejects.toThrow(RangeError);
    await expect(readLimitedText(new Response(new Uint8Array([0xff])), 1)).rejects.toThrow();
  });

  it("streams a chunk before the upstream body finishes", async () => {
    let upstream: ReadableStreamDefaultController<Uint8Array> | undefined;
    const response = new Response(new ReadableStream<Uint8Array>({
      start(controller) {
        upstream = controller;
      },
    }));
    const reader = streamLimitedBytes(response, 4).getReader();

    upstream!.enqueue(new Uint8Array([1, 2]));
    await expect(reader.read()).resolves.toEqual({ done: false, value: new Uint8Array([1, 2]) });

    upstream!.enqueue(new Uint8Array([3, 4]));
    upstream!.close();
    await expect(reader.read()).resolves.toEqual({ done: false, value: new Uint8Array([3, 4]) });
    await expect(reader.read()).resolves.toEqual({ done: true, value: undefined });
  });

  it("errors and cancels the upstream stream when observed bytes exceed the cap", async () => {
    let cancelled = false;
    const response = new Response(new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new Uint8Array([1, 2]));
        controller.enqueue(new Uint8Array([3, 4]));
      },
      cancel() {
        cancelled = true;
      },
    }));
    const reader = streamLimitedBytes(response, 3).getReader();

    await expect(reader.read()).resolves.toEqual({ done: false, value: new Uint8Array([1, 2]) });
    await expect(reader.read()).rejects.toThrow(/byte limit/);
    expect(cancelled).toBe(true);
  });

  it("propagates consumer cancellation to the upstream stream", async () => {
    let reason: unknown;
    const response = new Response(new ReadableStream<Uint8Array>({
      cancel(value) {
        reason = value;
      },
    }));
    const stream = streamLimitedBytes(response, 10);

    await stream.cancel("browser disconnected");

    expect(reason).toBe("browser disconnected");
  });

  it("rejects malformed or oversized declarations before exposing a stream", async () => {
    for (const declared of ["invalid", "11"]) {
      let cancelled = false;
      const response = new Response(new ReadableStream<Uint8Array>({
        cancel() {
          cancelled = true;
        },
      }), { headers: { "content-length": declared } });

      expect(() => streamLimitedBytes(response, 10)).toThrow(/byte limit/);
      await Promise.resolve();
      expect(cancelled).toBe(true);
    }
  });
});
