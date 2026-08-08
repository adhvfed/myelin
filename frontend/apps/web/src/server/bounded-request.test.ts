import { describe, expect, it } from "vitest";

import { boundMutationRequest } from "./bounded-request";

function streamingRequest(chunks: string[], headers?: HeadersInit): Request {
  const encoder = new TextEncoder();
  return new Request("https://myelin.example/action", {
    method: "POST",
    headers,
    body: new ReadableStream({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
        controller.close();
      },
    }),
    duplex: "half",
  } as RequestInit & { duplex: "half" });
}

describe("boundMutationRequest", () => {
  it("refuses an oversized declared body without reading it", async () => {
    const request = new Request("https://myelin.example/action", {
      method: "POST",
      headers: { "Content-Length": "9" },
      body: new ReadableStream({
        pull() {},
      }),
      duplex: "half",
    } as RequestInit & { duplex: "half" });

    const result = await boundMutationRequest(request, 8);

    expect(result.ok).toBe(false);
    if (result.ok) throw new Error("expected the declared body to be refused");
    expect(result.response.status).toBe(413);
    expect(result.response.headers.get("connection")).toBe("close");
    expect(request.body?.locked).toBe(false);
  });

  it("refuses chunked overflow at the observed boundary", async () => {
    const result = await boundMutationRequest(streamingRequest(["1234", "5678", "9"]), 8);

    expect(result.ok).toBe(false);
    if (result.ok) throw new Error("expected the streaming body to be refused");
    expect(result.response.status).toBe(413);
    expect(result.response.headers.get("connection")).toBe("close");
  });

  it("returns immediately after overflow even when the sender stalls the remainder", async () => {
    let controller: ReadableStreamDefaultController<Uint8Array> | undefined;
    const request = new Request("https://myelin.example/action", {
      method: "POST",
      body: new ReadableStream<Uint8Array>({
        start(value) {
          controller = value;
          value.enqueue(new TextEncoder().encode("123456789"));
        },
      }),
      duplex: "half",
    } as RequestInit & { duplex: "half" });

    const started = performance.now();
    const result = await boundMutationRequest(request, 8, 1_000);

    expect(performance.now() - started).toBeLessThan(500);
    expect(result.ok).toBe(false);
    if (result.ok) throw new Error("expected the streaming body to be refused");
    expect(result.response.status).toBe(413);
    controller?.close();
  });

  it("returns a replayable request with the original metadata", async () => {
    const result = await boundMutationRequest(
      streamingRequest(["token=", "secret"], { "Content-Type": "application/x-www-form-urlencoded" }),
      64,
    );

    expect(result.ok).toBe(true);
    if (!result.ok) throw new Error("expected the bounded body to be admitted");
    expect(result.request.method).toBe("POST");
    expect(result.request.headers.get("content-type")).toBe("application/x-www-form-urlencoded");
    expect(await result.request.text()).toBe("token=secret");
  });

  it("rejects an ambiguous Content-Length", async () => {
    const result = await boundMutationRequest(
      streamingRequest(["x"], { "Content-Length": "1, 1" }),
      8,
    );

    expect(result.ok).toBe(false);
    if (result.ok) throw new Error("expected the malformed length to be refused");
    expect(result.response.status).toBe(400);
  });

  it("does not consume safe-method requests", async () => {
    const request = new Request("https://myelin.example/resource", { method: "GET" });
    const result = await boundMutationRequest(request, 0);

    expect(result).toEqual({ ok: true, request });
  });

  it("bounds a stalled chunked body by one total read deadline", async () => {
    let controller: ReadableStreamDefaultController<Uint8Array> | undefined;
    const request = new Request("https://myelin.example/action", {
      method: "POST",
      body: new ReadableStream<Uint8Array>({
        start(value) {
          controller = value;
          value.enqueue(new TextEncoder().encode("partial"));
        },
      }),
      duplex: "half",
    } as RequestInit & { duplex: "half" });

    const started = performance.now();
    const result = await boundMutationRequest(request, 64, 20);

    expect(performance.now() - started).toBeLessThan(500);
    expect(result.ok).toBe(false);
    if (result.ok) throw new Error("expected the stalled body to time out");
    expect(result.response.status).toBe(408);
    expect(result.response.headers.get("connection")).toBe("close");
    controller?.close();
  });

  it.each([0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1])(
    "rejects an invalid body timeout %#",
    async (timeout) => {
      await expect(boundMutationRequest(streamingRequest(["x"]), 8, timeout))
        .rejects.toThrow(RangeError);
    },
  );
});
