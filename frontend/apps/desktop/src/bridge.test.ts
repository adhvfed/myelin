import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock Tauri IPC to verify command names, arguments, and result forwarding. Rust tests cover the
// content round trip itself.
const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import { coreInfo, MAX_RENDER_MARKDOWN_BYTES, renderMarkdown } from "./bridge";

describe("the Tauri shared-core bridge", () => {
  beforeEach(() => vi.clearAllMocks());

  it("invokes render_markdown with the md arg and returns the Rust result", async () => {
    invoke.mockResolvedValueOnce({
      input: "**x**",
      output: "**x**",
      roundTrips: true,
    });
    const r = await renderMarkdown("**x**");
    expect(invoke).toHaveBeenCalledWith("render_markdown", { md: "**x**" });
    expect(r.roundTrips).toBe(true);
    expect(r.output).toBe("**x**");
  });

  it("rejects oversized UTF-8 before invoking the native command", async () => {
    await expect(renderMarkdown("ø".repeat(MAX_RENDER_MARKDOWN_BYTES / 2 + 1)))
      .rejects.toThrow(RangeError);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("rejects a malformed or inconsistent native result", async () => {
    invoke.mockResolvedValueOnce({ input: "**x**", output: "changed", roundTrips: true });
    await expect(renderMarkdown("**x**")).rejects.toThrow("invalid result");
  });

  it("invokes core_info and surfaces the shared-crate facts", async () => {
    invoke.mockResolvedValueOnce({
      contentCorpusPassed: 18,
      contentCorpusTotal: 18,
      clientTimeoutMs: 2000,
    });
    const i = await coreInfo();
    expect(invoke).toHaveBeenCalledWith("core_info");
    expect(i.contentCorpusTotal).toBe(18);
    expect(i.clientTimeoutMs).toBe(2000);
  });
});
