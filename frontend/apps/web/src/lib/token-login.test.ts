import { describe, expect, it, vi } from "vitest";
import {
  DEFAULT_TOKEN_SCHEME,
  MAX_TOKEN_LOGIN_BYTES,
  TOKEN_LOGIN_DISABLED,
  TOKEN_LOGIN_ERROR,
  TOKEN_LOGIN_SUCCESS,
  runTokenLogin,
  type TokenSessionInput,
  type TokenWhoami,
} from "./token-login";

const whoami = (over: Partial<TokenWhoami> = {}): TokenWhoami => ({
  principal_id: "u_founder",
  tenant: "acme",
  region: "eu-west",
  kind: "human",
  expires_at: 4_102_444_800,
  ...over,
});

describe("runTokenLogin (R4.0 — the operator-token login decision)", () => {
  it("on a valid token (whoami 200): issues a session with the whoami facts + redirects into the app", async () => {
    const issue = vi.fn<(rec: TokenSessionInput) => void>();
    const verify = vi.fn(async () => whoami());
    const isEnabled = vi.fn(async () => true);
    const out = await runTokenLogin("  cap.tok.abc  ", undefined, { isEnabled, verify, issue });

    expect(out.redirectTo).toBe(TOKEN_LOGIN_SUCCESS);
    expect(isEnabled).toHaveBeenCalledTimes(1);
    // The token was trimmed before verify + issue; the default scheme was applied.
    expect(verify).toHaveBeenCalledWith("cap.tok.abc", DEFAULT_TOKEN_SCHEME);
    expect(issue).toHaveBeenCalledTimes(1);
    expect(issue).toHaveBeenCalledWith({
      token: "cap.tok.abc",
      // A pasted operator token has NO refresh credential — empty is deliberate (re-paste on expiry).
      refreshToken: "",
      scheme: DEFAULT_TOKEN_SCHEME,
      credentialExpiresAtMs: 4_102_444_800_000,
      principalId: "u_founder",
      // No human name in whoami → the PII-free principal id IS the honest display label.
      displayName: "u_founder",
      region: "eu-west",
      tenant: "acme",
    });
  });

  it("honours an explicit non-default scheme", async () => {
    const issue = vi.fn();
    const verify = vi.fn(async () => whoami());
    await runTokenLogin("tok", "pat", { isEnabled: async () => true, verify, issue });
    expect(verify).toHaveBeenCalledWith("tok", "pat");
    expect(issue).toHaveBeenCalledWith(expect.objectContaining({ scheme: "pat" }));
  });

  it("on an invalid/expired token (verify REJECTS, e.g. edge 401): redirects to the honest error + issues NO session", async () => {
    const issue = vi.fn();
    const verify = vi.fn(async () => {
      throw new Error("token verification failed (HTTP 401)");
    });
    const out = await runTokenLogin("bad.token", undefined, {
      isEnabled: async () => true,
      verify,
      issue,
    });
    expect(out.redirectTo).toBe(TOKEN_LOGIN_ERROR);
    expect(issue).not.toHaveBeenCalled();
  });

  it("on an empty/whitespace token: never even calls verify — straight to the honest error, no session", async () => {
    const issue = vi.fn();
    const verify = vi.fn(async () => whoami());
    const isEnabled = vi.fn(async () => true);
    const out = await runTokenLogin("   ", undefined, { isEnabled, verify, issue });
    expect(out.redirectTo).toBe(TOKEN_LOGIN_ERROR);
    expect(isEnabled).not.toHaveBeenCalled();
    expect(verify).not.toHaveBeenCalled();
    expect(issue).not.toHaveBeenCalled();
  });

  it("refuses oversized tokens and malformed schemes before contacting the edge", async () => {
    const malformed: Array<[string, string]> = [
      ["x".repeat(MAX_TOKEN_LOGIN_BYTES + 1), "agent"],
      ["token", "agent\r\nx-forged"],
      ["token", "A".repeat(33)],
    ];
    for (const [token, scheme] of malformed) {
      const verify = vi.fn(async () => whoami());
      const isEnabled = vi.fn(async () => true);
      const issue = vi.fn();
      const out = await runTokenLogin(token, scheme, { isEnabled, verify, issue });
      expect(out.redirectTo).toBe(TOKEN_LOGIN_ERROR);
      expect(isEnabled).not.toHaveBeenCalled();
      expect(verify).not.toHaveBeenCalled();
      expect(issue).not.toHaveBeenCalled();
    }
  });

  it("on a whoami missing a principal id (unexpected shape): honest error, no session", async () => {
    const issue = vi.fn();
    const verify = vi.fn(async () => whoami({ principal_id: "" }));
    const out = await runTokenLogin("tok", undefined, {
      isEnabled: async () => true,
      verify,
      issue,
    });
    expect(out.redirectTo).toBe(TOKEN_LOGIN_ERROR);
    expect(issue).not.toHaveBeenCalled();
  });

  it("refuses malformed or unbounded trust-rooted identity fields", async () => {
    for (const malformed of [
      { principal_id: `u_${"x".repeat(512)}` },
      { tenant: "" },
      { tenant: "x".repeat(129) },
      { region: "eu\nwest" },
    ]) {
      const issue = vi.fn();
      const out = await runTokenLogin("tok", undefined, {
        isEnabled: async () => true,
        verify: async () => whoami(malformed),
        issue,
      });
      expect(out.redirectTo).toBe(TOKEN_LOGIN_ERROR);
      expect(issue).not.toHaveBeenCalled();
    }
  });

  it("refuses an elapsed or unrepresentable capability expiry", async () => {
    for (const expires_at of [1, Number.MAX_SAFE_INTEGER]) {
      const issue = vi.fn();
      const out = await runTokenLogin("tok", undefined, {
        isEnabled: async () => true,
        verify: async () => whoami({ expires_at }),
        issue,
      });
      expect(out.redirectTo).toBe(TOKEN_LOGIN_ERROR);
      expect(issue).not.toHaveBeenCalled();
    }
  });

  it("when the edge disables token login: refuses before token verification or session issuance", async () => {
    const issue = vi.fn();
    const verify = vi.fn(async () => whoami());
    const isEnabled = vi.fn(async () => false);

    const out = await runTokenLogin("live-but-disabled-token", undefined, {
      isEnabled,
      verify,
      issue,
    });

    expect(out.redirectTo).toBe(TOKEN_LOGIN_DISABLED);
    expect(isEnabled).toHaveBeenCalledTimes(1);
    expect(verify).not.toHaveBeenCalled();
    expect(issue).not.toHaveBeenCalled();
  });

  it("when the auth config cannot be read: fails closed before verification or session issuance", async () => {
    const issue = vi.fn();
    const verify = vi.fn(async () => whoami());
    const isEnabled = vi.fn(async () => {
      throw new Error("edge auth config unavailable");
    });

    const out = await runTokenLogin("live-but-unverifiable-mode", undefined, {
      isEnabled,
      verify,
      issue,
    });

    expect(out.redirectTo).toBe(TOKEN_LOGIN_DISABLED);
    expect(verify).not.toHaveBeenCalled();
    expect(issue).not.toHaveBeenCalled();
  });
});
