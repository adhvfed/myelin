const BODY_METHODS = new Set(["POST", "PUT", "PATCH", "DELETE"]);

export const MAX_MUTATION_BODY_BYTES = 1024 * 1024;

export type BoundedRequestResult =
  | { ok: true; request: Request }
  | { ok: false; response: Response };

function rejection(status: number, message: string): BoundedRequestResult {
  return {
    ok: false,
    response: new Response(message, {
      status,
      headers: {
        "Cache-Control": "no-store",
        "Content-Type": "text/plain; charset=utf-8",
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
): Promise<BoundedRequestResult> {
  if (!BODY_METHODS.has(request.method.toUpperCase()) || request.body == null) {
    return { ok: true, request };
  }
  if (!Number.isSafeInteger(limit) || limit < 0) {
    throw new RangeError("request body limit must be a non-negative safe integer");
  }

  const length = declaredLength(request);
  if (length === "invalid") return rejection(400, "invalid Content-Length");
  if (length != null && length > limit) return rejection(413, "request body too large");

  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let observed = 0;
  let overflow = false;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (overflow) continue;
      observed += value.byteLength;
      if (observed > limit) {
        // H3 1.x does not tolerate cancellation of its IncomingMessage-backed Web stream: its
        // eventual `end` handler closes the already-cancelled controller and crashes Node. Drain
        // the remainder without retaining it so chunked overflow stays constant-memory.
        overflow = true;
        chunks.length = 0;
        continue;
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  if (overflow) return rejection(413, "request body too large");

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
