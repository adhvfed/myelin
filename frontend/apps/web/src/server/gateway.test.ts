import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const session = vi.hoisted(() => ({
  clearCurrentSession: vi.fn(),
  getSessionRecord: vi.fn(),
  updateSessionToken: vi.fn(),
}));

vi.mock("./session", () => session);

import { edgeGet, edgeGetRaw, gatewayRequestSignal } from "./gateway";

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

describe("gateway refresh revocation races", () => {
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
});
