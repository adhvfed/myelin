import { afterEach, describe, expect, it, vi } from "vitest";

const http = vi.hoisted(() => ({
  deleteCookie: vi.fn(),
  getCookie: vi.fn(),
  setCookie: vi.fn(),
}));
const store = vi.hoisted(() => ({
  delete: vi.fn(),
  get: vi.fn(),
  issue: vi.fn(),
  ready: vi.fn(),
  rotate: vi.fn(),
  updateToken: vi.fn(),
}));

vi.mock("vinxi/http", () => http);
vi.mock("./session-backend", () => ({
  createSessionStore: () => store,
  sessionBackend: () => ({ kind: "memory" }),
}));

import { clearCurrentSession, issueSession, SESSION_COOKIE } from "./session";

const priorId = `sess_${"a".repeat(32)}`;
const record = {
  token: "access-one",
  refreshToken: "refresh-one",
  scheme: "agent",
  principalId: "principal-one",
  displayName: "Operator",
  region: "fr-par",
  tenant: "acme",
};

afterEach(() => {
  vi.clearAllMocks();
});

describe("request session lifecycle", () => {
  it("uses one atomic store operation for reauthentication", async () => {
    http.getCookie.mockReturnValue(priorId);
    store.rotate.mockResolvedValue(undefined);

    const replacementId = await issueSession(record);

    expect(replacementId).toMatch(/^sess_[A-Za-z0-9_-]{32}$/);
    expect(store.rotate).toHaveBeenCalledWith(priorId, replacementId, record);
    expect(store.delete).not.toHaveBeenCalled();
    expect(store.issue).not.toHaveBeenCalled();
    expect(http.setCookie).toHaveBeenCalledWith(
      SESSION_COOKIE,
      replacementId,
      expect.objectContaining({ httpOnly: true, path: "/", sameSite: "lax" }),
    );
  });

  it("does not replace the browser cookie when atomic rotation fails", async () => {
    http.getCookie.mockReturnValue(priorId);
    store.rotate.mockRejectedValueOnce(new Error("Valkey unavailable"));

    await expect(issueSession(record)).rejects.toThrow("Valkey unavailable");

    expect(http.setCookie).not.toHaveBeenCalled();
  });

  it("expires the browser cookie even when server-side logout revocation fails", async () => {
    http.getCookie.mockReturnValue(priorId);
    store.delete.mockRejectedValueOnce(new Error("Valkey unavailable"));

    await expect(clearCurrentSession()).rejects.toThrow("Valkey unavailable");

    expect(http.deleteCookie).toHaveBeenCalledWith(
      SESSION_COOKIE,
      expect.objectContaining({ httpOnly: true, path: "/", sameSite: "lax" }),
    );
  });
});
