function responseLimitError(): Error {
  return new Error("upstream response exceeded the byte limit");
}

function validateResponseLimit(response: Response, maxBytes: number): void {
  if (!Number.isSafeInteger(maxBytes) || maxBytes < 0) {
    throw new RangeError("response byte limit must be a non-negative safe integer");
  }

  const declared = response.headers.get("content-length");
  if (declared !== null) {
    const value = declared.trim();
    if (!/^\d+$/.test(value) || BigInt(value) > BigInt(maxBytes)) {
      throw responseLimitError();
    }
  }
}

/** Read an upstream response without allowing an unbounded body to accumulate in process memory. */
export async function readLimitedBytes(
  response: Response,
  maxBytes: number,
): Promise<Uint8Array<ArrayBuffer>> {
  try {
    validateResponseLimit(response, maxBytes);
  } catch (error) {
    await response.body?.cancel(error).catch(() => undefined);
    throw error;
  }
  if (!response.body) return new Uint8Array();

  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    if (value.byteLength > maxBytes - size) {
      await reader.cancel().catch(() => undefined);
      throw responseLimitError();
    }
    size += value.byteLength;
    chunks.push(value);
  }

  const body = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
}

/** Read a bounded, strictly valid UTF-8 response body. */
export async function readLimitedText(response: Response, maxBytes: number): Promise<string> {
  return new TextDecoder("utf-8", { fatal: true }).decode(
    await readLimitedBytes(response, maxBytes),
  );
}

/**
 * Stream an upstream response with a hard observed-byte cap and no whole-body copy. The returned
 * stream propagates browser cancellation upstream so abandoned downloads stop consuming an edge
 * connection. A declared oversized body is rejected before any bytes are exposed to the caller.
 */
export function streamLimitedBytes(
  response: Response,
  maxBytes: number,
): ReadableStream<Uint8Array> {
  try {
    validateResponseLimit(response, maxBytes);
  } catch (error) {
    void response.body?.cancel(error).catch(() => undefined);
    throw error;
  }

  if (!response.body) {
    return new ReadableStream({
      start(controller) {
        controller.close();
      },
    });
  }

  const reader = response.body.getReader();
  let size = 0;
  let finished = false;

  function release(): void {
    if (finished) return;
    finished = true;
    reader.releaseLock();
  }

  async function cancel(reason?: unknown): Promise<void> {
    if (finished) return;
    try {
      await reader.cancel(reason);
    } catch {
      // The consumer is already gone; cancellation is best-effort cleanup.
    } finally {
      release();
    }
  }

  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      if (finished) return;
      try {
        const { done, value } = await reader.read();
        if (done) {
          release();
          controller.close();
          return;
        }
        if (value.byteLength > maxBytes - size) {
          const error = responseLimitError();
          await cancel(error);
          controller.error(error);
          return;
        }
        size += value.byteLength;
        controller.enqueue(value);
      } catch (error) {
        await cancel(error);
        controller.error(error);
      }
    },
    cancel,
  });
}
