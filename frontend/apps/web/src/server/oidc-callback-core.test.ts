import { describe, expect, it, vi } from "vitest";

import {
  matchesOidcStateCookie,
  oidcStateCookieName,
  runOidcCallback,
} from "./oidc-callback-core";

const state = "A".repeat(43);
const redirectUri = "https://myelin.example/auth/oidc/callback";

function dependencies() {
  return {
    consume: vi.fn().mockResolvedValue({
      codeVerifier: "verifier",
      nonce: "nonce",
      redirectUri,
      returnTo: "/cli/auth?code=ABCD-EFGH",
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
    }, deps)).toEqual({
      authenticated: true,
      returnTo: "/cli/auth?code=ABCD-EFGH",
    });
    expect(deps.consume).toHaveBeenCalledWith(state);
    expect(deps.exchange).toHaveBeenCalledWith("authorization-code", "verifier");
    expect(deps.establish).toHaveBeenCalledWith("id-token", "nonce");
    expect(deps.consume.mock.invocationCallOrder[0]).toBeLessThan(
      deps.exchange.mock.invocationCallOrder[0]!,
    );
  });

  it.each([
    { states: [state, state], codes: ["code"], cookieState: state, providerError: false },
    { states: [state], codes: ["code"], cookieState: "B".repeat(43), providerError: false },
  ])("rejects ambiguous or mismatched callbacks before consumption", async (input) => {
    const deps = dependencies();
    expect(await runOidcCallback({ ...input, redirectUri }, deps)).toBeNull();
    expect(deps.consume).not.toHaveBeenCalled();
    expect(deps.exchange).not.toHaveBeenCalled();
  });

  it.each([
    { codes: ["code", "code"], providerError: false },
    { codes: ["code"], providerError: true },
  ])("consumes a matched transaction before rejecting a provider or code error", async (over) => {
    const deps = dependencies();
    expect(await runOidcCallback({
      states: [state],
      cookieState: state,
      redirectUri,
      ...over,
    }, deps)).toEqual({
      authenticated: false,
      returnTo: "/cli/auth?code=ABCD-EFGH",
    });
    expect(deps.consume).toHaveBeenCalledWith(state);
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
    }, deps)).toBeNull();
    expect(deps.exchange).not.toHaveBeenCalled();
  });

  it("retains the local destination when the provider exchange fails", async () => {
    const deps = dependencies();
    deps.exchange.mockRejectedValue(new Error("provider unavailable"));

    expect(await runOidcCallback({
      states: [state],
      codes: ["code"],
      cookieState: state,
      providerError: false,
      redirectUri,
    }, deps)).toEqual({
      authenticated: false,
      returnTo: "/cli/auth?code=ABCD-EFGH",
    });
    expect(deps.establish).not.toHaveBeenCalled();
  });

  it("uses an independent host cookie for every concurrent transaction", () => {
    const otherState = "B".repeat(43);
    expect(oidcStateCookieName("__Host-myelin_oidc_state_", state)).toBe(
      `__Host-myelin_oidc_state_${state}`,
    );
    expect(oidcStateCookieName("__Host-myelin_oidc_state_", otherState)).not.toBe(
      oidcStateCookieName("__Host-myelin_oidc_state_", state),
    );
  });

  it("rejects malformed cookie names and mismatched state values", () => {
    expect(oidcStateCookieName("prefix_", "short")).toBeNull();
    expect(matchesOidcStateCookie(state, state)).toBe(true);
    expect(matchesOidcStateCookie(state, "B".repeat(43))).toBe(false);
  });
});
