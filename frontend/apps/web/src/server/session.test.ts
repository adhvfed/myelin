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

import { clearCurrentSession, clearRefusedSession, issueSession, SESSION_COOKIE } from "./session";

const priorId = `sess_${"a".repeat(32)}`;
const record = {
  token: "access-one",
  refreshToken: "refresh-one",
  scheme: "agent",
  credentialExpiresAtMs: Number.MAX_SAFE_INTEGER,
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

  it("clears the cookie of a refused session that is still the browser's", async () => {
    store.delete.mockResolvedValueOnce(true);

    await clearRefusedSession(priorId);

    expect(store.delete).toHaveBeenCalledWith(priorId);
    expect(http.deleteCookie).toHaveBeenCalledWith(
      SESSION_COOKIE,
      expect.objectContaining({ httpOnly: true, path: "/", sameSite: "lax" }),
    );
  });

  it("leaves the cookie alone when the refused session was already replaced", async () => {
    // A request that began before a password change rotated the session: its record is gone, and
    // the browser holds the fresh session's cookie instead.
    store.delete.mockResolvedValueOnce(false);

    await clearRefusedSession(priorId);

    expect(store.delete).toHaveBeenCalledWith(priorId);
    expect(http.deleteCookie).not.toHaveBeenCalled();
  });

  it("still expires the cookie when the store cannot be reached", async () => {
    store.delete.mockRejectedValueOnce(new Error("Valkey unavailable"));

    await expect(clearRefusedSession(priorId)).rejects.toThrow("Valkey unavailable");

    expect(http.deleteCookie).toHaveBeenCalled();
  });

  it("does nothing without a session id", async () => {
    await clearRefusedSession(null);

    expect(store.delete).not.toHaveBeenCalled();
    expect(http.deleteCookie).not.toHaveBeenCalled();
  });
});
