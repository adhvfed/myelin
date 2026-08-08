import type { SessionStore } from "./session-store";
import { MemorySessionStore } from "./session-store";
import { ValkeySessionStore } from "./valkey-session-store";
import { decodeSessionKey } from "./session-cipher";

export type SessionBackend =
  | { kind: "memory" }
  | { kind: "valkey"; url: string; encryptionKey: string };

export function sessionBackend(
  production: boolean,
  redisUrl: string | undefined,
  encryptionKey: string | undefined,
): SessionBackend {
  const url = redisUrl?.trim();
  if (!url) {
    if (!production) return { kind: "memory" };
    throw new Error("REDIS_URL is required for durable production web sessions");
  }

  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    throw new Error("REDIS_URL must be an absolute redis:// or rediss:// URL");
  }
  if ((parsed.protocol !== "redis:" && parsed.protocol !== "rediss:") || !parsed.hostname) {
    throw new Error("REDIS_URL must be an absolute redis:// or rediss:// URL");
  }
  if (production && parsed.protocol !== "rediss:") {
    throw new Error("REDIS_URL must use rediss:// TLS in production");
  }
  // Construction validates exact key length/encoding before the process accepts traffic. Requiring
  // the key for every Valkey deployment also prevents a multi-replica dev stack from silently using
  // per-process ephemeral keys.
  const key = encryptionKey?.trim();
  if (!key) throw new Error("MYELIN_WEB_SESSION_KEY is required for encrypted Valkey sessions");
  decodeSessionKey(key);
  return { kind: "valkey", url, encryptionKey: key };
}

export function createSessionStore(backend: SessionBackend): SessionStore {
  return backend.kind === "valkey"
    ? new ValkeySessionStore(backend.url, backend.encryptionKey)
    : new MemorySessionStore();
}
