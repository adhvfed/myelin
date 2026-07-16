// The raw/download BROWSER PROXY (R3.4) — a same-origin server route the browser can hit for a file's
// bytes. It runs server-side, reads the httpOnly session token, and streams the edge's gateway-proxied
// raw/download endpoint back to the browser WITH the edge's `Content-Disposition` (attachment for a
// download). This keeps bytes IN-REGION behind the auth gate — no public signed URL, no CDN of private
// bytes (the sovereignty rail, BINDING). `?d=attachment` selects the download (attachment) variant;
// anything else is the inline `raw` variant.
import type { APIEvent } from "@solidjs/start/server";
import { edgeGetRaw } from "~/server/gateway";
import { Unauthorized } from "~/server/gateway";

export async function GET(event: APIEvent) {
  const { repo, ref } = event.params;
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
    const headers = new Headers({
      "content-type": res.contentType,
      // Never let the browser sniff a repo blob into an executable type.
      "x-content-type-options": "nosniff",
    });
    if (res.contentDisposition) headers.set("content-disposition", res.contentDisposition);
    return new Response(res.body, { status: res.status, headers });
  } catch (e) {
    if (e instanceof Unauthorized) {
      // The browser is not authenticated for this byte stream — send it to login.
      return new Response(null, { status: 302, headers: { location: "/login" } });
    }
    return new Response("could not load file", { status: 502 });
  }
}
