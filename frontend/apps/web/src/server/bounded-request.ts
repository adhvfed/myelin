const BODY_METHODS = new Set(["POST", "PUT", "PATCH", "DELETE"]);

export const MAX_MUTATION_BODY_BYTES = 1024 * 1024;
export const MAX_MUTATION_BODY_READ_MS = 15_000;

const READ_DEADLINE = Symbol("request-body-read-deadline");

export type BoundedRequestResult =
  | { ok: true; request: Request }
  | { ok: false; response: Response };

function rejection(status: number, message: string, closeConnection = false): BoundedRequestResult {
  return {
    ok: false,
    response: new Response(message, {
      status,
      headers: {
        "Cache-Control": "no-store",
        "Content-Type": "text/plain; charset=utf-8",
        ...(closeConnection ? { Connection: "close" } : {}),
      },
    }),
  };
}

function declaredLength(request: Request): number | null | "invalid" {
  const raw = request.headers.get("content-length");
  if (raw == null) return null;
  if (!/^(0|[1-9]\d*)$/.test(raw)) return "invalid";
  const parsed = Number(raw);
  return Number.isSafeInteger(parsed) ? parsed : "invalid";
}

/**
 * Read a mutation body once, refusing both declared and streaming overflow before framework parsers
 * buffer attacker-controlled form or JSON input. The returned Request owns a fresh replayable body.
 */
export async function boundMutationRequest(
  request: Request,
  limit = MAX_MUTATION_BODY_BYTES,
  timeoutMs = MAX_MUTATION_BODY_READ_MS,
): Promise<BoundedRequestResult> {
  if (!BODY_METHODS.has(request.method.toUpperCase()) || request.body == null) {
    return { ok: true, request };
  }
  if (!Number.isSafeInteger(limit) || limit < 0) {
    throw new RangeError("request body limit must be a non-negative safe integer");
  }
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0) {
    throw new RangeError("request body timeout must be a positive safe integer");
  }

  const length = declaredLength(request);
  if (length === "invalid") return rejection(400, "invalid Content-Length", true);
  if (length != null && length > limit) return rejection(413, "request body too large", true);

  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let observed = 0;
  let handedOff = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<typeof READ_DEADLINE>((resolve) => {
    timer = setTimeout(() => resolve(READ_DEADLINE), timeoutMs);
  });
  try {
    for (;;) {
      const result = await Promise.race([reader.read(), deadline]);
      if (result === READ_DEADLINE) {
        chunks.length = 0;
        handedOff = true;
        void drainDetached(reader);
        return rejection(408, "request body timed out", true);
      }
      const { done, value } = result;
      if (done) break;
      observed += value.byteLength;
      if (observed > limit) {
        // H3 1.x does not tolerate cancellation of its IncomingMessage-backed Web stream: its
        // eventual `end` handler closes the already-cancelled controller and crashes Node. Drain
        // the remainder without retaining it in a detached task. Closing this HTTP connection lets
        // the middleware return 413 immediately and prevents a slow remainder from occupying it.
        chunks.length = 0;
        handedOff = true;
        void drainDetached(reader);
        return rejection(413, "request body too large", true);
      }
      chunks.push(value);
    }
  } finally {
    if (timer !== undefined) clearTimeout(timer);
    if (!handedOff) reader.releaseLock();
  }

  const body = new Uint8Array(observed);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }

  const init: RequestInit & { duplex: "half" } = {
    body,
    duplex: "half",
  };
  return { ok: true, request: new Request(request, init) };
}

/** Keep consuming an H3-backed request after the response is decided. No chunk is retained, and all
 * stream failures are contained because the client may disappear as the close response is sent. */
async function drainDetached(reader: ReadableStreamDefaultReader<Uint8Array>): Promise<void> {
  try {
    while (!(await reader.read()).done) {
      // Deliberately empty: consuming applies backpressure without retaining attacker bytes.
    }
  } catch {
    // A closed connection is the normal terminal state for an early rejection.
  } finally {
    reader.releaseLock();
  }
}
