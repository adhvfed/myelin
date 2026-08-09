import { describe, expect, test } from "vitest";

import { SystemEventStream } from "./event-stream.js";

describe("SystemEventStream", () => {
  test("parses fragmented, multiline SSE frames and skips comments", async () => {
    const encoder = new TextEncoder();
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(encoder.encode(": connected\r\n\r\nevent: repo."));
        controller.enqueue(
          encoder.encode("created\nid: 42\ndata: first line\ndata: second line\n\n"),
        );
        controller.close();
      },
    });
    const stream = new SystemEventStream(body, new AbortController());

    await expect(
      stream.waitFor((event) => event.event === "repo.created", {
        description: "a fragmented repository event",
      }),
    ).resolves.toEqual({
      event: "repo.created",
      id: "42",
      data: "first line\nsecond line",
    });
  });
});
