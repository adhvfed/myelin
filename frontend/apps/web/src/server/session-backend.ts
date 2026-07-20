import type { SessionStore } from "./session-store";
import { MemorySessionStore } from "./session-store";
import { ValkeySessionStore } from "./valkey-session-store";

export type SessionBackend =
  | { kind: "memory" }
  | { kind: "valkey"; url: string };

export function sessionBackend(production: boolean, redisUrl: string | undefined): SessionBackend {
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
  return { kind: "valkey", url };
}

export function createSessionStore(backend: SessionBackend): SessionStore {
  return backend.kind === "valkey"
    ? new ValkeySessionStore(backend.url)
    : new MemorySessionStore();
}
