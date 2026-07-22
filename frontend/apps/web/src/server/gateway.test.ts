import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const session = vi.hoisted(() => ({
  clearCurrentSession: vi.fn(),
  getSessionRecord: vi.fn(),
  updateSessionToken: vi.fn(),
}));

vi.mock("./session", () => session);

import {
  DEFAULT_EDGE_REQUEST_TIMEOUT_MS,
  MAX_EDGE_JSON_RESPONSE_BYTES,
  MAX_EDGE_PUBLIC_RESPONSE_BYTES,
  MAX_EDGE_RAW_RESPONSE_BYTES,
  edgeGet,
  edgeGetPublic,
  edgeGetRaw,
  edgeLoginWithOidc,
  edgePost,
  gatewayRequestSignal,
} from "./gateway";

const unauthorized = () => new Response(
  JSON.stringify({ error: { message: "authentication required", code: "unauthorized" } }),
  { status: 401, headers: { "content-type": "application/json" } },
);

function stallUntilAborted(init?: RequestInit): Promise<Response> {
  const signal = init?.signal;
  return new Promise((_resolve, reject) => {
    const abort = () => reject(signal?.reason ?? Object.assign(new Error("aborted"), { name: "AbortError" }));
    if (signal?.aborted) abort();
    else signal?.addEventListener("abort", abort, { once: true });
  });
}

beforeEach(() => {
  session.getSessionRecord.mockReturnValue({
    token: "stale-token",
    refreshToken: "refresh-token",
    scheme: "pat",
  });
  session.updateSessionToken.mockResolvedValue(true);
  session.clearCurrentSession.mockResolvedValue(undefined);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("gateway request deadlines", () => {
  it("applies the bounded default when a caller does not supply a timeout", () => {
    const timeout = vi.spyOn(AbortSignal, "timeout");

    const signal = gatewayRequestSignal();

    expect(timeout).toHaveBeenCalledWith(DEFAULT_EDGE_REQUEST_TIMEOUT_MS);
    expect(signal).toBeInstanceOf(AbortSignal);
    timeout.mockRestore();
  });

  it.each([0, -1, Number.POSITIVE_INFINITY, Number.NaN])(
    "rejects an invalid timeout: %s",
    (timeoutMs) => {
      expect(() => gatewayRequestSignal({ timeoutMs })).toThrow(
        "edge request timeout must be a positive finite number",
      );
    },
  );

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

  it("aborts a stalled initial fetch and never begins refresh or retry after the deadline", async () => {
    const fetchMock = vi.fn((_input: RequestInfo | URL, init?: RequestInit) => stallUntilAborted(init));
    vi.stubGlobal("fetch", fetchMock);

    await expect(edgeGet("/v1/issues", { timeoutMs: 10 })).rejects.toMatchObject({ name: "TimeoutError" });
    await new Promise((resolve) => setTimeout(resolve, 15));

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]?.[1]?.signal?.aborted).toBe(true);
    expect(session.updateSessionToken).not.toHaveBeenCalled();
  });

  it("uses the same deadline to abort a stalled refresh and never starts the retry", async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(unauthorized())
      .mockImplementationOnce((_input: RequestInfo | URL, init?: RequestInit) => stallUntilAborted(init));
    vi.stubGlobal("fetch", fetchMock);

    await expect(edgeGet("/v1/issues", { timeoutMs: 10 })).rejects.toMatchObject({ name: "TimeoutError" });

    expect(fetchMock).toHaveBeenCalledTimes(2);
    const initialSignal = fetchMock.mock.calls[0]?.[1]?.signal;
    const refreshSignal = fetchMock.mock.calls[1]?.[1]?.signal;
    expect(refreshSignal).toBe(initialSignal);
    expect(initialSignal?.aborted).toBe(true);
    expect(session.updateSessionToken).not.toHaveBeenCalled();
  });

  it("uses the same deadline to abort the single retry", async () => {
    const refresh = new Response(JSON.stringify({ access_token: "fresh-token" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(unauthorized())
      .mockResolvedValueOnce(refresh)
      .mockImplementationOnce((_input: RequestInfo | URL, init?: RequestInit) => stallUntilAborted(init));
    vi.stubGlobal("fetch", fetchMock);

    await expect(edgeGet("/v1/issues", { timeoutMs: 10 })).rejects.toMatchObject({ name: "TimeoutError" });

    expect(fetchMock).toHaveBeenCalledTimes(3);
    const initialSignal = fetchMock.mock.calls[0]?.[1]?.signal;
    const refreshSignal = fetchMock.mock.calls[1]?.[1]?.signal;
    const retrySignal = fetchMock.mock.calls[2]?.[1]?.signal;
    expect(refreshSignal).toBe(initialSignal);
    expect(retrySignal).toBe(initialSignal);
    expect(initialSignal?.aborted).toBe(true);
    expect(session.updateSessionToken).toHaveBeenCalledWith("fresh-token");
  });
});

describe("gateway response limits", () => {
  it("rejects an oversized JSON response before buffering it", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(new Response("{}", {
      status: 200,
      headers: { "content-length": String(MAX_EDGE_JSON_RESPONSE_BYTES + 1) },
    })));

    await expect(edgeGet("/v1/issues")).rejects.toThrow(/byte limit/);
  });

  it("rejects an oversized raw response before buffering it", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(new Response("raw", {
      status: 200,
      headers: { "content-length": String(MAX_EDGE_RAW_RESPONSE_BYTES + 1) },
    })));

    await expect(edgeGetRaw("/v1/git/raw")).rejects.toThrow(/byte limit/);
  });

  it("uses a small dedicated cap for the unauthenticated auth config", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(new Response("{}", {
      status: 200,
      headers: { "content-length": String(MAX_EDGE_PUBLIC_RESPONSE_BYTES + 1) },
    })));

    await expect(edgeGetPublic("/v1/auth/config")).rejects.toThrow(/byte limit/);
  });
});

describe("gateway refresh revocation races", () => {
  it("preserves a mutation idempotency key across an authentication retry", async () => {
    const refresh = new Response(JSON.stringify({ access_token: "fresh-token" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(unauthorized())
      .mockResolvedValueOnce(refresh)
      .mockResolvedValueOnce(new Response("{}", { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    await edgePost("/v1/git/repos/core/prs/1/merge", {}, {
      idempotencyKey: "merge-operation-1",
    });

    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(new Headers(fetchMock.mock.calls[0]?.[1]?.headers).get("idempotency-key"))
      .toBe("merge-operation-1");
    expect(new Headers(fetchMock.mock.calls[2]?.[1]?.headers).get("idempotency-key"))
      .toBe("merge-operation-1");
  });

  it("does not send a refresh request for a non-refreshable SSO session", async () => {
    session.getSessionRecord.mockReturnValue({
      token: "human-session",
      refreshToken: "",
      scheme: "session",
    });
    const fetchMock = vi.fn().mockResolvedValueOnce(unauthorized());
    vi.stubGlobal("fetch", fetchMock);

    await expect(edgeGet("/v1/issues")).rejects.toMatchObject({ name: "Unauthorized" });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(session.clearCurrentSession).toHaveBeenCalledTimes(1);
  });

  it("does not retry a JSON request when the session vanished during refresh", async () => {
    const refresh = new Response(JSON.stringify({ access_token: "fresh-token" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(unauthorized())
      .mockResolvedValueOnce(refresh);
    session.updateSessionToken.mockResolvedValueOnce(false);
    vi.stubGlobal("fetch", fetchMock);

    await expect(edgeGet("/v1/issues")).rejects.toMatchObject({ name: "Unauthorized" });

    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(session.updateSessionToken).toHaveBeenCalledWith("fresh-token");
    expect(session.clearCurrentSession).toHaveBeenCalledTimes(1);
  });

  it("does not retry a raw request when the session vanished during refresh", async () => {
    const refresh = new Response(JSON.stringify({ access_token: "fresh-token" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(unauthorized())
      .mockResolvedValueOnce(refresh);
    session.updateSessionToken.mockResolvedValueOnce(false);
    vi.stubGlobal("fetch", fetchMock);

    await expect(edgeGetRaw("/v1/git/raw")).rejects.toMatchObject({ name: "Unauthorized" });

    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(session.updateSessionToken).toHaveBeenCalledWith("fresh-token");
    expect(session.clearCurrentSession).toHaveBeenCalledTimes(1);
  });

  it("clears the session when a refreshed raw request is still unauthorized", async () => {
    const refresh = new Response(JSON.stringify({ access_token: "fresh-token" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(unauthorized())
      .mockResolvedValueOnce(refresh)
      .mockResolvedValueOnce(unauthorized());
    vi.stubGlobal("fetch", fetchMock);

    await expect(edgeGetRaw("/v1/git/raw")).rejects.toMatchObject({ name: "Unauthorized" });

    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(session.updateSessionToken).toHaveBeenCalledWith("fresh-token");
    expect(session.clearCurrentSession).toHaveBeenCalledTimes(1);
  });

  it("does not persist a malformed token from a successful refresh response", async () => {
    const refresh = new Response(JSON.stringify({ access_token: 42 }), { status: 200 });
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(unauthorized())
      .mockResolvedValueOnce(refresh);
    vi.stubGlobal("fetch", fetchMock);

    await expect(edgeGet("/v1/issues")).rejects.toMatchObject({ name: "Unauthorized" });

    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(session.updateSessionToken).not.toHaveBeenCalled();
    expect(session.clearCurrentSession).toHaveBeenCalledTimes(1);
  });

  it("treats a control-bearing refresh token as authentication failure", async () => {
    const refresh = new Response(JSON.stringify({ access_token: "fresh\r\nsmuggled" }), {
      status: 200,
    });
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(unauthorized())
      .mockResolvedValueOnce(refresh);
    vi.stubGlobal("fetch", fetchMock);

    await expect(edgeGet("/v1/issues")).rejects.toMatchObject({ name: "Unauthorized" });

    expect(session.updateSessionToken).not.toHaveBeenCalled();
    expect(session.clearCurrentSession).toHaveBeenCalledTimes(1);
  });
});

describe("edgeLoginWithOidc", () => {
  it("accepts only the bounded human-session response shape", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({
      access_token: "signed-session",
      token_type: "Bearer",
      scheme: "session",
      expires_at: 1_800_000_000,
    }), { status: 200, headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(edgeLoginWithOidc("id-token", "nonce")).resolves.toEqual({
      accessToken: "signed-session",
      scheme: "session",
      expiresAt: 1_800_000_000,
    });
    expect(JSON.parse(String(fetchMock.mock.calls[0]?.[1]?.body))).toEqual({
      scheme: "oidc",
      material: "id-token",
      nonce: "nonce",
    });
    expect(fetchMock.mock.calls[0]?.[1]?.redirect).toBe("error");
  });

  it("rejects a superficially successful response with the wrong signed-token scheme", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({
      access_token: "wrong-purpose",
      token_type: "Bearer",
      scheme: "agent",
      expires_at: 1_800_000_000,
    }), { status: 200 })));

    await expect(edgeLoginWithOidc("id-token", "nonce")).rejects.toMatchObject({
      name: "Unauthorized",
    });
  });

  it("rejects a control-bearing human-session token", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(new Response(JSON.stringify({
      access_token: "signed\nsession",
      token_type: "Bearer",
      scheme: "session",
      expires_at: 1_800_000_000,
    }), { status: 200 })));

    await expect(edgeLoginWithOidc("id-token", "nonce")).rejects.toMatchObject({
      name: "Unauthorized",
    });
  });
});
