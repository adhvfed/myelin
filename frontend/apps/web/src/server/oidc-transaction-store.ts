import { randomUUID } from "node:crypto";

import Redis from "ioredis";

import { SessionCipher } from "./session-cipher";

export const OIDC_TRANSACTION_TTL_MS = 10 * 60 * 1_000;
export const OIDC_TRANSACTION_KEY_PREFIX = "myelin:web-oidc:v1:";

export interface OidcTransaction {
  codeVerifier: string;
  nonce: string;
  redirectUri: string;
}

export interface OidcTransactionStore {
  issue(state: string, transaction: OidcTransaction): Promise<boolean>;
  consume(state: string): Promise<OidcTransaction | null>;
  ready(): Promise<void>;
}

interface StoredTransaction {
  transaction: OidcTransaction;
  expiresAtMs: number;
}

function validTransaction(value: unknown): value is OidcTransaction {
  if (!value || typeof value !== "object") return false;
  const transaction = value as Record<string, unknown>;
  return (
    typeof transaction.codeVerifier === "string" &&
    typeof transaction.nonce === "string" &&
    typeof transaction.redirectUri === "string"
  );
}

export class MemoryOidcTransactionStore implements OidcTransactionStore {
  readonly #transactions = new Map<string, StoredTransaction>();

  async issue(state: string, transaction: OidcTransaction): Promise<boolean> {
    this.#purge();
    if (this.#transactions.has(state)) return false;
    this.#transactions.set(state, {
      transaction: { ...transaction },
      expiresAtMs: Date.now() + OIDC_TRANSACTION_TTL_MS,
    });
    return true;
  }

  async consume(state: string): Promise<OidcTransaction | null> {
    const stored = this.#transactions.get(state);
    this.#transactions.delete(state);
    if (!stored || Date.now() >= stored.expiresAtMs) return null;
    return { ...stored.transaction };
  }

  async ready(): Promise<void> {}

  #purge(): void {
    const now = Date.now();
    for (const [state, transaction] of this.#transactions) {
      if (now >= transaction.expiresAtMs) this.#transactions.delete(state);
    }
  }
}

const CONSUME_SCRIPT = `
local value = redis.call("GET", KEYS[1])
if not value then return nil end
redis.call("DEL", KEYS[1])
return value
`;

const READY_SCRIPT = `
redis.call("SET", KEYS[1], ARGV[1], "PX", 5000)
local value = redis.call("GET", KEYS[1])
redis.call("DEL", KEYS[1])
return value
`;

export class ValkeyOidcTransactionStore implements OidcTransactionStore {
  readonly #redis: Redis;
  readonly #cipher: SessionCipher;
  #outageReported = false;

  constructor(url: string, encryptionKey: string) {
    this.#cipher = new SessionCipher(encryptionKey);
    this.#redis = new Redis(url, {
      connectTimeout: 5_000,
      maxRetriesPerRequest: 1,
      retryStrategy: (attempt) => Math.min(attempt * 100, 2_000),
    });
    this.#redis.on("error", () => {
      if (this.#outageReported) return;
      this.#outageReported = true;
      console.error("[web-oidc] Valkey connection failed; login transactions are unavailable");
    });
    this.#redis.on("ready", () => {
      this.#outageReported = false;
    });
  }

  async issue(state: string, transaction: OidcTransaction): Promise<boolean> {
    const sealed = this.#cipher.encrypt(
      state,
      "oidc_transaction",
      JSON.stringify(transaction),
    );
    return (await this.#redis.set(
      this.#key(state),
      sealed,
      "PX",
      OIDC_TRANSACTION_TTL_MS,
      "NX",
    )) === "OK";
  }

  async consume(state: string): Promise<OidcTransaction | null> {
    const sealed = await this.#redis.eval(CONSUME_SCRIPT, 1, this.#key(state));
    if (typeof sealed !== "string") return null;
    try {
      const value: unknown = JSON.parse(this.#cipher.decrypt(state, "oidc_transaction", sealed));
      return validTransaction(value) ? value : null;
    } catch {
      return null;
    }
  }

  async ready(): Promise<void> {
    const probe = `ready:${randomUUID()}`;
    const value = randomUUID();
    const result = await this.#redis.eval(READY_SCRIPT, 1, this.#key(probe), value);
    if (result !== value) throw new Error("Valkey OIDC transaction probe failed");
  }

  close(): void {
    this.#redis.disconnect(false);
  }

  #key(state: string): string {
    return `${OIDC_TRANSACTION_KEY_PREFIX}${state}`;
  }
}
