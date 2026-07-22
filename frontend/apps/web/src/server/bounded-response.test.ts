import { describe, expect, it } from "vitest";

import { readLimitedBytes, readLimitedText } from "./bounded-response";

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
});
