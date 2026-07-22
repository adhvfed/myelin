import { describe, expect, it, vi } from "vitest";

import { runOidcCallback } from "./oidc-callback-core";

const state = "A".repeat(43);
const redirectUri = "https://myelin.example/auth/oidc/callback";

function dependencies() {
  return {
    consume: vi.fn().mockResolvedValue({
      codeVerifier: "verifier",
      nonce: "nonce",
      redirectUri,
    }),
    exchange: vi.fn().mockResolvedValue("id-token"),
    establish: vi.fn().mockResolvedValue(undefined),
  };
}

describe("runOidcCallback", () => {
  it("consumes state before exchanging and establishing the session", async () => {
    const deps = dependencies();
    expect(await runOidcCallback({
      states: [state],
      codes: ["authorization-code"],
      cookieState: state,
      providerError: false,
      redirectUri,
    }, deps)).toBe(true);
    expect(deps.consume).toHaveBeenCalledWith(state);
    expect(deps.exchange).toHaveBeenCalledWith("authorization-code", "verifier");
    expect(deps.establish).toHaveBeenCalledWith("id-token", "nonce");
    expect(deps.consume.mock.invocationCallOrder[0]).toBeLessThan(
      deps.exchange.mock.invocationCallOrder[0]!,
    );
  });

  it.each([
    { states: [state, state], codes: ["code"], cookieState: state, providerError: false },
    { states: [state], codes: ["code", "code"], cookieState: state, providerError: false },
    { states: [state], codes: ["code"], cookieState: "B".repeat(43), providerError: false },
    { states: [state], codes: ["code"], cookieState: state, providerError: true },
  ])("rejects ambiguous, mismatched, or provider-error callbacks before consumption", async (input) => {
    const deps = dependencies();
    expect(await runOidcCallback({ ...input, redirectUri }, deps)).toBe(false);
    expect(deps.consume).not.toHaveBeenCalled();
    expect(deps.exchange).not.toHaveBeenCalled();
  });

  it("refuses a replay after the transaction has already been consumed", async () => {
    const deps = dependencies();
    deps.consume.mockResolvedValue(null);
    expect(await runOidcCallback({
      states: [state],
      codes: ["code"],
      cookieState: state,
      providerError: false,
      redirectUri,
    }, deps)).toBe(false);
    expect(deps.exchange).not.toHaveBeenCalled();
  });
});
