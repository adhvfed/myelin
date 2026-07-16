import { describe, expect, it, vi } from "vitest";
import {
  DEFAULT_TOKEN_SCHEME,
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
  ...over,
});

describe("runTokenLogin (R4.0 — the operator-token login decision)", () => {
  it("on a valid token (whoami 200): issues a session with the whoami facts + redirects into the app", async () => {
    const issue = vi.fn<(rec: TokenSessionInput) => void>();
    const verify = vi.fn(async () => whoami());
    const out = await runTokenLogin("  cap.tok.abc  ", undefined, { verify, issue });

    expect(out.redirectTo).toBe(TOKEN_LOGIN_SUCCESS);
    // The token was trimmed before verify + issue; the default scheme was applied.
    expect(verify).toHaveBeenCalledWith("cap.tok.abc", DEFAULT_TOKEN_SCHEME);
    expect(issue).toHaveBeenCalledTimes(1);
    expect(issue).toHaveBeenCalledWith({
      token: "cap.tok.abc",
      // A pasted operator token has NO refresh credential — empty is deliberate (re-paste on expiry).
      refreshToken: "",
      scheme: DEFAULT_TOKEN_SCHEME,
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
    await runTokenLogin("tok", "pat", { verify, issue });
    expect(verify).toHaveBeenCalledWith("tok", "pat");
    expect(issue).toHaveBeenCalledWith(expect.objectContaining({ scheme: "pat" }));
  });

  it("on an invalid/expired token (verify REJECTS, e.g. edge 401): redirects to the honest error + issues NO session", async () => {
    const issue = vi.fn();
    const verify = vi.fn(async () => {
      throw new Error("token verification failed (HTTP 401)");
    });
    const out = await runTokenLogin("bad.token", undefined, { verify, issue });
    expect(out.redirectTo).toBe(TOKEN_LOGIN_ERROR);
    expect(issue).not.toHaveBeenCalled();
  });

  it("on an empty/whitespace token: never even calls verify — straight to the honest error, no session", async () => {
    const issue = vi.fn();
    const verify = vi.fn(async () => whoami());
    const out = await runTokenLogin("   ", undefined, { verify, issue });
    expect(out.redirectTo).toBe(TOKEN_LOGIN_ERROR);
    expect(verify).not.toHaveBeenCalled();
    expect(issue).not.toHaveBeenCalled();
  });

  it("on a whoami missing a principal id (unexpected shape): honest error, no session", async () => {
    const issue = vi.fn();
    const verify = vi.fn(async () => whoami({ principal_id: "" }));
    const out = await runTokenLogin("tok", undefined, { verify, issue });
    expect(out.redirectTo).toBe(TOKEN_LOGIN_ERROR);
    expect(issue).not.toHaveBeenCalled();
  });
});
