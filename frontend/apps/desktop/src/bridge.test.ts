import { describe, expect, it, vi } from "vitest";

// Mock the Tauri IPC boundary: in a test runner there is no native host, so we assert the bridge
// shapes the `invoke` call correctly (command name + args) and passes the Rust result through. The
// REAL myelin-content round-trip is proven on the Rust side (src-tauri unit test + the desktop
// binary build); this test pins the JS↔Rust contract the shell relies on.
const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import { coreInfo, renderMarkdown } from "./bridge";

describe("the Tauri shared-core bridge", () => {
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
