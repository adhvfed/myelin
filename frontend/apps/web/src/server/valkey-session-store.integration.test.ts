import { randomBytes } from "node:crypto";
import { afterAll, describe, expect, it } from "vitest";
import Redis from "ioredis";

import type { SessionRecord } from "./session-store";
import { SESSION_KEY_PREFIX, ValkeySessionStore } from "./valkey-session-store";

const redisUrl = process.env.REDIS_URL;
const sessionKey = Buffer.alloc(32, 7).toString("base64url");
const stores: ValkeySessionStore[] = [];

function store(): ValkeySessionStore {
  const instance = new ValkeySessionStore(redisUrl!, sessionKey);
  stores.push(instance);
  return instance;
}

afterAll(async () => {
  await Promise.all(stores.map((instance) => instance.close()));
});

describe.runIf(Boolean(redisUrl))("ValkeySessionStore integration", () => {
  it("shares issue, idle touch, token rotation, and revocation across server replicas", async () => {
    const firstReplica = store();
    const secondReplica = store();
    const id = `integration_${randomBytes(12).toString("hex")}`;
    const replacementId = `integration_${randomBytes(12).toString("hex")}`;
    const record: SessionRecord = {
      token: "access-one",
      refreshToken: "refresh-one",
      scheme: "agent",
      credentialExpiresAtMs: Date.now() + 60_000,
      principalId: "principal-one",
      displayName: "Operator",
      region: "fr-par",
      tenant: "acme",
    };

    try {
      await firstReplica.issue(id, record);
      const admin = new Redis(redisUrl!);
      const stored = await admin.get(`${SESSION_KEY_PREFIX}${id}`);
      admin.disconnect(false);
      expect(stored).not.toContain(record.token);
      expect(stored).not.toContain(record.refreshToken);
      expect(stored).not.toContain(record.principalId);
      expect(stored).not.toContain(record.tenant);
      expect(await secondReplica.get(id)).toEqual(record);

      expect(await secondReplica.updateToken(id, "access-two")).toBe(true);
      expect(await firstReplica.get(id)).toEqual({ ...record, token: "access-two" });

      const replacement = { ...record, token: "access-three" };
      await firstReplica.rotate(id, replacementId, replacement);
      expect(await secondReplica.get(id)).toBeNull();
      expect(await secondReplica.get(replacementId)).toEqual(replacement);

      expect(await firstReplica.delete(replacementId)).toBe(true);
      expect(await secondReplica.get(replacementId)).toBeNull();
    } finally {
      await firstReplica.delete(id);
      await firstReplica.delete(replacementId);
    }
  });

  it("deletes corrupt records instead of raising an authentication-path error", async () => {
    const sessionStore = store();
    const admin = new Redis(redisUrl!);
    const corruptRecords = ["{", "{}", JSON.stringify({ record: "bad" })];

    try {
      for (const [index, value] of corruptRecords.entries()) {
        const readId = `corrupt_read_${randomBytes(8).toString("hex")}_${index}`;
        const readKey = `${SESSION_KEY_PREFIX}${readId}`;
        await admin.set(readKey, value);
        expect(await sessionStore.get(readId)).toBeNull();
        expect(await admin.exists(readKey)).toBe(0);

        const updateId = `corrupt_update_${randomBytes(8).toString("hex")}_${index}`;
        const updateKey = `${SESSION_KEY_PREFIX}${updateId}`;
        await admin.set(updateKey, value);
        expect(await sessionStore.updateToken(updateId, "replacement")).toBe(false);
        expect(await admin.exists(updateKey)).toBe(0);
      }
    } finally {
      admin.disconnect(false);
    }
  });

  it("bounds the server-side key by the credential expiry", async () => {
    const sessionStore = store();
    const admin = new Redis(redisUrl!);
    const id = `credential_expiry_${randomBytes(8).toString("hex")}`;
    const credentialExpiresAtMs = Date.now() + 10_000;
    const record: SessionRecord = {
      token: "short-access",
      refreshToken: "",
      scheme: "session",
      credentialExpiresAtMs,
      principalId: "principal-short",
      displayName: "Short Session",
      region: "fr-par",
      tenant: "acme",
    };

    try {
      await sessionStore.issue(id, record);
      // Sample the wall clock before PTTL. Sampling after the network round trip can make the
      // comparison bound 1–2 ms smaller than the already-returned integer TTL and flake while the
      // stored expiry is still correct.
      const observedAtMs = Date.now();
      const ttl = await admin.pttl(`${SESSION_KEY_PREFIX}${id}`);
      expect(ttl).toBeGreaterThan(0);
      expect(ttl).toBeLessThanOrEqual(credentialExpiresAtMs - observedAtMs);
    } finally {
      await sessionStore.delete(id);
      admin.disconnect(false);
    }
  });

  it("rejects a valid encrypted record transplanted to a different session id", async () => {
    const sessionStore = store();
    const admin = new Redis(redisUrl!);
    const sourceId = `bound_source_${randomBytes(8).toString("hex")}`;
    const targetId = `bound_target_${randomBytes(8).toString("hex")}`;
    const sourceKey = `${SESSION_KEY_PREFIX}${sourceId}`;
    const targetKey = `${SESSION_KEY_PREFIX}${targetId}`;
    const record: SessionRecord = {
      token: "access-bound",
      refreshToken: "refresh-bound",
      scheme: "agent",
      credentialExpiresAtMs: Date.now() + 60_000,
      principalId: "principal-bound",
      displayName: "Bound Operator",
      region: "fr-par",
      tenant: "acme",
    };

    try {
      await sessionStore.issue(sourceId, record);
      const stored = await admin.get(sourceKey);
      expect(stored).not.toBeNull();
      await admin.set(targetKey, stored!);

      expect(await sessionStore.get(targetId)).toBeNull();
      expect(await admin.exists(targetKey)).toBe(0);
      expect(await sessionStore.get(sourceId)).toEqual(record);
    } finally {
      await sessionStore.delete(sourceId);
      await sessionStore.delete(targetId);
      admin.disconnect(false);
    }
  });

  it("fails readiness when an ACL permits PING/EVAL but denies session primitives", async () => {
    const admin = new Redis(redisUrl!);
    const username = `ready_${randomBytes(8).toString("hex")}`;
    const password = randomBytes(18).toString("base64url");
    const restrictedUrl = new URL(redisUrl!);
    restrictedUrl.username = username;
    restrictedUrl.password = password;
    let restricted: Redis | undefined;
    let restrictedStore: ValkeySessionStore | undefined;

    try {
      await admin.call(
        "ACL",
        "SETUSER",
        username,
        "reset",
        "on",
        `>${password}`,
        `~${SESSION_KEY_PREFIX}*`,
        "+ping",
        "+info",
        "+eval",
      );
      restricted = new Redis(restrictedUrl.toString());
      restrictedStore = new ValkeySessionStore(restrictedUrl.toString(), sessionKey);
      expect(await restricted.ping()).toBe("PONG");
      await expect(restrictedStore.ready()).rejects.toThrow();
    } finally {
      restricted?.disconnect(false);
      restrictedStore?.close();
      await admin.call("ACL", "DELUSER", username);
      admin.disconnect(false);
    }
  });
});
