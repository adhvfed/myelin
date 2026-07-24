import type { APIEvent } from "@solidjs/start/server";
import { isCiLogCursor } from "~/lib/ci-live-stream";
import { isCiUuid } from "~/lib/ci-read-input";
import { edgeGetEventStream, isUnauthorized } from "~/server/gateway";

export async function GET(event: APIEvent): Promise<Response> {
  const { run, job } = event.params;
  const requestUrl = new URL(event.request.url);
  if (!isCiUuid(run) || !isCiUuid(job) || requestUrl.search !== "") {
    return new Response("bad request", { status: 400 });
  }
  const cursor = event.request.headers.get("last-event-id");
  if (cursor !== null && !isCiLogCursor(cursor)) {
    return new Response("bad request", { status: 400 });
  }
  let upstream: Response;
  try {
    upstream = await edgeGetEventStream(
      `/v1/ci/runs/${encodeURIComponent(run)}/jobs/${encodeURIComponent(job)}/log/live`,
      {
        signal: event.request.signal,
        ...(cursor === null ? {} : { lastEventId: cursor }),
      },
    );
  } catch (error) {
    if (isUnauthorized(error)) return new Response("unauthorized", { status: 401 });
    return new Response("CI live log unavailable", { status: 502 });
  }
  if (!upstream.ok || !upstream.body) {
    await upstream.body?.cancel().catch(() => undefined);
    const status = upstream.status >= 400 && upstream.status <= 599 ? upstream.status : 502;
    return new Response(
      status === 409 ? "CI live log cursor is stale; reload archived log" : "CI live log unavailable",
      { status },
    );
  }
  const contentType = upstream.headers.get("content-type")?.split(";", 1)[0]?.trim().toLowerCase();
  if (contentType !== "text/event-stream") {
    await upstream.body.cancel().catch(() => undefined);
    return new Response("CI live log unavailable", { status: 502 });
  }
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
