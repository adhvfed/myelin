import { describe, expect, it, vi } from "vitest";
import {
  GatewayError,
  Unauthorized,
  runGateway,
  type GwResponse,
} from "./gateway-core";

const ok = (body: unknown): GwResponse => ({ status: 200, bodyText: JSON.stringify(body) });
const unauthorized = (): GwResponse => ({
  status: 401,
  bodyText: JSON.stringify({ error: { message: "authentication required", code: "unauthorized" } }),
});

describe("gateway-core (the cookie-auth lifecycle, doc 10 §5)", () => {
  it("throws Unauthorized when there is no session token (never even calls the edge)", async () => {
    const doFetch = vi.fn();
    await expect(
      runGateway({ getToken: () => null, doFetch, refresh: async () => null, clearSession: () => {} }),
    ).rejects.toBeInstanceOf(Unauthorized);
    expect(doFetch).not.toHaveBeenCalled();
  });

  it("returns the parsed JSON body on a 2xx", async () => {
    const out = await runGateway<{ ok: boolean }>({
      getToken: () => "t",
      doFetch: async () => ok({ ok: true }),
      refresh: async () => null,
      clearSession: () => {},
    });
    expect(out).toEqual({ ok: true });
  });

  it("on 401 does a SINGLE refresh + ONE retry, then succeeds", async () => {
    const doFetch = vi
      .fn<(t: string) => Promise<GwResponse>>()
      .mockResolvedValueOnce(unauthorized()) // first call: stale token → 401
      .mockResolvedValueOnce(ok({ items: [] })); // retry with fresh token → 200
    const refresh = vi.fn(async () => "fresh-token");
    const out = await runGateway({ getToken: () => "stale", doFetch, refresh, clearSession: () => {} });
    expect(out).toEqual({ items: [] });
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(doFetch).toHaveBeenCalledTimes(2);
    expect(doFetch).toHaveBeenNthCalledWith(2, "fresh-token");
  });

  it("on 401 with a failed refresh: clears the session + throws Unauthorized (→ /login)", async () => {
    const clearSession = vi.fn();
    await expect(
      runGateway({
        getToken: () => "stale",
        doFetch: async () => unauthorized(),
        refresh: async () => null,
        clearSession,
      }),
    ).rejects.toBeInstanceOf(Unauthorized);
    expect(clearSession).toHaveBeenCalledTimes(1);
  });

  it("on a second 401 after a successful refresh: clears + throws Unauthorized (no infinite retry)", async () => {
    const clearSession = vi.fn();
    const doFetch = vi.fn(async () => unauthorized());
    await expect(
      runGateway({ getToken: () => "x", doFetch, refresh: async () => "fresh", clearSession }),
    ).rejects.toBeInstanceOf(Unauthorized);
    expect(doFetch).toHaveBeenCalledTimes(2); // one retry only
    expect(clearSession).toHaveBeenCalledTimes(1);
  });

  it("maps a non-2xx to a GatewayError carrying the envelope message + code + status", async () => {
    const err = await runGateway({
      getToken: () => "t",
      doFetch: async () => ({
        status: 409,
        bodyText: JSON.stringify({ error: { message: "the file changed since you opened it", code: "conflict" } }),
      }),
      refresh: async () => null,
      clearSession: () => {},
    }).catch((e) => e);
    expect(err).toBeInstanceOf(GatewayError);
    expect((err as GatewayError).status).toBe(409);
    expect((err as GatewayError).code).toBe("conflict");
    expect((err as GatewayError).message).toBe("the file changed since you opened it");
  });
});
