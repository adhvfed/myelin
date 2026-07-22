import { randomBytes } from "node:crypto";

import Redis from "ioredis";
import { describe, expect, it } from "vitest";

import {
  MAX_STORED_OIDC_TRANSACTION_BYTES,
  OIDC_TRANSACTION_KEY_PREFIX,
  ValkeyOidcTransactionStore,
} from "./oidc-transaction-store";

const redisUrl = process.env.REDIS_URL;
const encryptionKey = Buffer.alloc(32, 9).toString("base64url");

describe.runIf(Boolean(redisUrl))("ValkeyOidcTransactionStore integration", () => {
  it("encrypts a transaction and atomically consumes it once across replicas", async () => {
    const first = new ValkeyOidcTransactionStore(redisUrl!, encryptionKey);
    const second = new ValkeyOidcTransactionStore(redisUrl!, encryptionKey);
    const admin = new Redis(redisUrl!);
    const state = randomBytes(32).toString("base64url");
    const transaction = {
      codeVerifier: randomBytes(32).toString("base64url"),
      nonce: randomBytes(32).toString("base64url"),
      redirectUri: "https://myelin.example/auth/oidc/callback",
    };

    try {
      expect(await first.issue(state, transaction)).toBe(true);
      const stored = await admin.get(`${OIDC_TRANSACTION_KEY_PREFIX}${state}`);
      expect(stored).not.toContain(transaction.codeVerifier);
      expect(stored).not.toContain(transaction.nonce);
      expect(await second.consume(state)).toEqual(transaction);
      expect(await first.consume(state)).toBeNull();
    } finally {
      await admin.del(`${OIDC_TRANSACTION_KEY_PREFIX}${state}`);
      admin.disconnect(false);
      first.close();
      second.close();
    }
  });

  it("atomically deletes an oversized stored transaction before decryption", async () => {
    const store = new ValkeyOidcTransactionStore(redisUrl!, encryptionKey);
    const admin = new Redis(redisUrl!);
    const state = randomBytes(32).toString("base64url");
    const key = `${OIDC_TRANSACTION_KEY_PREFIX}${state}`;

    try {
      await admin.set(key, "x".repeat(MAX_STORED_OIDC_TRANSACTION_BYTES + 1));
      expect(await store.consume(state)).toBeNull();
      expect(await admin.exists(key)).toBe(0);
    } finally {
      await admin.del(key);
      admin.disconnect(false);
      store.close();
    }
  });
});
