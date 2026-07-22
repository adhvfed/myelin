import { describe, expect, it } from "vitest";

import {
  MemorySessionStore,
  SESSION_ABSOLUTE_TTL_MS,
  SESSION_IDLE_TTL_MS,
  type SessionRecord,
} from "./session-store";

const record: SessionRecord = {
  token: "access-one",
  refreshToken: "refresh-one",
  scheme: "agent",
  credentialExpiresAtMs: Number.MAX_SAFE_INTEGER,
  principalId: "principal-one",
  displayName: "Operator",
  region: "fr-par",
  tenant: "acme",
};

describe("MemorySessionStore", () => {
  it("enforces absolute expiry even while the session stays active", () => {
    const store = new MemorySessionStore();
    store.issue("session-one", record, 1_000);

    expect(store.get("session-one", 1_000 + SESSION_IDLE_TTL_MS - 1)).toEqual(record);
    expect(store.get("session-one", 1_000 + SESSION_ABSOLUTE_TTL_MS)).toBeNull();
    expect(store.size()).toBe(0);
  });

  it("enforces idle expiry and removes the replayable record", () => {
    const store = new MemorySessionStore();
    store.issue("session-one", record, 5_000);

    expect(store.get("session-one", 5_000 + SESSION_IDLE_TTL_MS)).toBeNull();
    expect(store.get("session-one", 5_001)).toBeNull();
  });

  it("ends at credential expiry and refuses an elapsed credential", () => {
    const store = new MemorySessionStore();
    const expiring = { ...record, credentialExpiresAtMs: 15_000 };
    store.issue("session-one", expiring, 10_000);

    expect(store.get("session-one", 14_999)).toEqual(expiring);
    expect(store.get("session-one", 15_000)).toBeNull();
    expect(() => store.issue("session-two", expiring, 15_000)).toThrow(/expiry/);
  });

  it("refreshes idle activity without extending the absolute deadline", () => {
    const store = new MemorySessionStore();
    store.issue("session-one", record, 10_000);

    for (let now = 10_000; now < 10_000 + SESSION_ABSOLUTE_TTL_MS; now += SESSION_IDLE_TTL_MS - 1) {
      expect(store.get("session-one", now)).toEqual(record);
    }
    expect(store.get("session-one", 10_000 + SESSION_ABSOLUTE_TTL_MS)).toBeNull();
  });

  it("rotates access tokens without changing the other server-only facts", () => {
    const store = new MemorySessionStore();
    store.issue("session-one", record, 20_000);

    expect(store.updateToken("session-one", "access-two", 21_000)).toBe(true);
    expect(store.get("session-one", 21_001)).toEqual({ ...record, token: "access-two" });
    expect(store.updateToken("missing", "access-two", 21_000)).toBe(false);
  });

  it("atomically replaces a prior browser session", () => {
    const store = new MemorySessionStore();
    store.issue("session-one", record, 20_000);

    store.rotate("session-one", "session-two", { ...record, token: "access-two" }, 21_000);

    expect(store.get("session-one", 21_001)).toBeNull();
    expect(store.get("session-two", 21_001)).toEqual({ ...record, token: "access-two" });
    expect(store.size()).toBe(1);
  });
});
