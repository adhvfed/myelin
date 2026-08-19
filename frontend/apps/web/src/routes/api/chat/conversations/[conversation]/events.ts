// Same-origin SSE proxy for one conversation's live-delivery stream. The tenant and
// credentials come from the server-side session; the edge authorizes the subscription
// with the same visibility gate as message reads. Frames carry references only
// (conversation id, message id) - clients revalidate through the read API.
import type { APIEvent } from "@solidjs/start/server";
import { isChatUlid } from "~/lib/chat-response";
import { edgeGetEventStream, isUnauthorized } from "~/server/gateway";

export async function GET(event: APIEvent): Promise<Response> {
  const { conversation } = event.params;
  const requestUrl = new URL(event.request.url);
  if (!isChatUlid(conversation) || requestUrl.search !== "") {
    return new Response("bad request", { status: 400 });
  }
  let upstream: Response;
  try {
    upstream = await edgeGetEventStream(
      `/v1/chat/conversations/${encodeURIComponent(conversation)}/events`,
      { signal: event.request.signal },
    );
  } catch (error) {
    if (isUnauthorized(error)) return new Response("unauthorized", { status: 401 });
    return new Response("chat events unavailable", { status: 502 });
  }
  if (!upstream.ok || !upstream.body) {
    await upstream.body?.cancel().catch(() => undefined);
    const status = upstream.status >= 400 && upstream.status <= 599 ? upstream.status : 502;
    return new Response("chat events unavailable", { status });
  }
  const contentType = upstream.headers.get("content-type")?.split(";", 1)[0]?.trim().toLowerCase();
  if (contentType !== "text/event-stream") {
    await upstream.body.cancel().catch(() => undefined);
    return new Response("chat events unavailable", { status: 502 });
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
