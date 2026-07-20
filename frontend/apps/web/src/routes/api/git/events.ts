// The FIREHOSE BROWSER PROXY (R3.5, OQ-4) — a same-origin SSE route the browser's `EventSource` hits
// for live tenant events. It runs server-side, reads the httpOnly session token, and pipes the edge's
// UNIFIED firehose (`GET /v1/t/{tenant}/events`) back to the browser as `text/event-stream`. The
// tenant is ALWAYS the SESSION's (never a client selector) — the same IDOR floor the edge enforces.
// This is the single channel the first-run repos screen listens on for the typed
// `repo.created`/`repo.pushed` frames; it is NOT a second channel and it never mints inbox items.
//
// FLOOR: the dev-edge holds this stream open but emits no frames, so against the harness the live
// flip is inert (the manual Refresh is the fallback); against the real edge the typed frames arrive.
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
