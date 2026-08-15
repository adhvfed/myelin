// The raw/download BROWSER PROXY (R3.4) — a same-origin server route the browser can hit for a file's
// bytes. It runs server-side, reads the httpOnly session token, and streams the edge's gateway-proxied
// raw/download endpoint back with a proxy-owned disposition. This keeps bytes IN-REGION behind the
// auth gate — no public signed URL, no CDN of private bytes (the sovereignty rail, BINDING) — while
// ensuring repo-controlled HTML/SVG cannot become active content on the authenticated app origin.
// `?d=attachment` selects a forced download; anything else is the inert inline `raw` variant.
import type { APIEvent } from "@solidjs/start/server";
import { rawResponseHeaders } from "~/lib/raw-response";
import { parseGitRepositoryRouteParam } from "~/lib/git-route";
import { edgeGetRaw, isUnauthorized } from "~/server/gateway";

export async function GET(event: APIEvent) {
  const { ref } = event.params;
  const repo = parseGitRepositoryRouteParam(event.params.repo);
  const path = (event.params as Record<string, string>).path ?? "";
  const url = new URL(event.request.url);
  const attachment = url.searchParams.get("d") === "attachment";

  if (!repo || !ref || !path) {
    return new Response("bad request", { status: 400 });
  }
  const encPath = path.split("/").map(encodeURIComponent).join("/");
  const kind = attachment ? "download" : "raw";
  const edgePath = `/v1/git/repos/${encodeURIComponent(repo)}/${kind}/${encodeURIComponent(ref)}/${encPath}`;

  try {
    const res = await edgeGetRaw(edgePath);
    if (res.status < 200 || res.status >= 300) {
      await res.body.cancel().catch(() => undefined);
      const status = res.status >= 400 && res.status <= 599 ? res.status : 502;
      return new Response(status === 404 ? "not found" : "could not load file", { status });
    }
    const headers = rawResponseHeaders({ attachment, contentType: res.contentType, path });
    return new Response(res.body, { status: res.status, headers });
  } catch (e) {
    if (isUnauthorized(e)) {
      // The browser is not authenticated for this byte stream — send it to login.
      return new Response(null, { status: 302, headers: { location: "/login" } });
    }
    return new Response("could not load file", { status: 502 });
  }
}
