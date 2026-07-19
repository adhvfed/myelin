import { describe, expect, it } from "vitest";
import { gatewayRequestSignal } from "./gateway";

describe("gateway request deadlines", () => {
  it("aborts a request signal at its bounded timeout", async () => {
    const signal = gatewayRequestSignal({ timeoutMs: 5 });

    expect(signal?.aborted).toBe(false);
    await new Promise<void>((resolve) => signal?.addEventListener("abort", () => resolve(), { once: true }));
    expect(signal?.aborted).toBe(true);
  });

  it("composes a caller abort with the timeout", () => {
    const controller = new AbortController();
    const signal = gatewayRequestSignal({ signal: controller.signal, timeoutMs: 10_000 });

    controller.abort();
    expect(signal?.aborted).toBe(true);
  });
});
