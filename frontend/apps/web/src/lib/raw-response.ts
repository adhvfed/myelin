export interface RawResponseHeaderOptions {
  attachment: boolean;
  contentType: string;
  path: string;
}

function mediaType(contentType: string): string {
  return contentType.split(";", 1)[0]!.trim().toLowerCase();
}

function safeBinaryInlineType(type: string): boolean {
  return (
    (type.startsWith("image/") && type !== "image/svg+xml") ||
    type.startsWith("audio/") ||
    type.startsWith("video/") ||
    type === "application/json" ||
    type.endsWith("+json")
  );
}

function encodedFilename(path: string): string {
  const filename = path.split("/").filter(Boolean).at(-1) || "download";
  return encodeURIComponent(filename).replace(/['()*]/g, (character) =>
    `%${character.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

/** Build browser-safe headers for a repository blob served from the authenticated app origin. */
export function rawResponseHeaders(options: RawResponseHeaderOptions): Headers {
  const headers = new Headers({ "x-content-type-options": "nosniff" });

  if (options.attachment) {
    headers.set("content-type", "application/octet-stream");
    headers.set(
      "content-disposition",
      `attachment; filename*=UTF-8''${encodedFilename(options.path)}`,
    );
    return headers;
  }

  const type = mediaType(options.contentType);
  if (type.startsWith("text/") || type === "application/xml" || type.endsWith("+xml")) {
    // Text is useful inline, but always label it plain so HTML, CSS, JS, SVG, and XML remain inert.
    headers.set("content-type", "text/plain; charset=utf-8");
    headers.set("content-disposition", "inline");
  } else if (safeBinaryInlineType(type)) {
    headers.set("content-type", options.contentType);
    headers.set("content-disposition", "inline");
  } else {
    // Unknown/plugin-capable formats are downloads. Raw previews are limited to an explicit inert
    // allowlist until they can move to a cookie-less asset origin.
    headers.set("content-type", "application/octet-stream");
    headers.set(
      "content-disposition",
      `attachment; filename*=UTF-8''${encodedFilename(options.path)}`,
    );
  }
  return headers;
}
