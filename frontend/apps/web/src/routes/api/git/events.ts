// Same-origin SSE proxy for tenant events. The tenant and credentials come from the server-side
// session rather than request parameters. The local edge keeps this stream open without emitting.
import { getSessionRecord } from "~/server/session";
import { edgeOrigin } from "~/server/edge-origin";

export async function GET() {
  const rec = await getSessionRecord();
  if (!rec) {
    return new Response("unauthorized", { status: 401 });
  }
  let upstream: Response;
  try {
    upstream = await fetch(`${edgeOrigin()}/v1/t/${encodeURIComponent(rec.tenant)}/events`, {
      headers: {
        authorization: `Bearer ${rec.token}`,
        "x-myelin-token-scheme": rec.scheme,
        accept: "text/event-stream",
      },
      redirect: "error",
    });
  } catch {
    return new Response("firehose unavailable", { status: 502 });
  }
  if (!upstream.ok || !upstream.body) {
    return new Response("firehose unavailable", { status: upstream.status || 502 });
  }
  // Pipe the edge stream straight through — no buffering, no re-shaping (references-not-payloads).
  return new Response(upstream.body, {
    status: 200,
    headers: {
      "content-type": "text/event-stream",
      "cache-control": "no-cache, no-transform",
      connection: "keep-alive",
      "x-accel-buffering": "no",
    },
  });
}
