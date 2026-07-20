import { randomBytes } from "node:crypto";
import { afterAll, describe, expect, it } from "vitest";

import type { SessionRecord } from "./session-store";
import { ValkeySessionStore } from "./valkey-session-store";

const redisUrl = process.env.REDIS_URL;
const stores: ValkeySessionStore[] = [];

function store(): ValkeySessionStore {
  const instance = new ValkeySessionStore(redisUrl!);
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
      principalId: "principal-one",
      displayName: "Operator",
      region: "fr-par",
      tenant: "acme",
    };

    try {
      await firstReplica.issue(id, record);
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
});
