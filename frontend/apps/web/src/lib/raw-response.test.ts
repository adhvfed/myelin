import { describe, expect, it } from "vitest";

import { rawResponseHeaders } from "./raw-response";

describe("rawResponseHeaders", () => {
  it.each([
    "text/html",
    "TEXT/HTML; charset=utf-8",
    "text/javascript",
    "image/svg+xml",
    "application/xhtml+xml",
  ])(
    "serves active inline type %s as inert text",
    (contentType) => {
      const headers = rawResponseHeaders({ attachment: false, contentType, path: "site/index.html" });

      expect(headers.get("content-type")).toBe("text/plain; charset=utf-8");
      expect(headers.get("content-disposition")).toBe("inline");
      expect(headers.get("x-content-type-options")).toBe("nosniff");
    },
  );

  it("preserves an inert inline media type", () => {
    const headers = rawResponseHeaders({
      attachment: false,
      contentType: "image/png",
      path: "images/logo.png",
    });

    expect(headers.get("content-type")).toBe("image/png");
    expect(headers.get("content-disposition")).toBe("inline");
  });

  it("forces an unknown inline format to download", () => {
    const headers = rawResponseHeaders({
      attachment: false,
      contentType: "application/pdf",
      path: "docs/report.pdf",
    });

    expect(headers.get("content-type")).toBe("application/octet-stream");
    expect(headers.get("content-disposition")).toBe(
      "attachment; filename*=UTF-8''report.pdf",
    );
  });

  it("forces downloads to an inert type and generates its own safe filename", () => {
    const headers = rawResponseHeaders({
      attachment: true,
      contentType: "text/html",
      path: "docs/report (final).html",
    });

    expect(headers.get("content-type")).toBe("application/octet-stream");
    expect(headers.get("content-disposition")).toBe(
      "attachment; filename*=UTF-8''report%20%28final%29.html",
    );
  });
});
