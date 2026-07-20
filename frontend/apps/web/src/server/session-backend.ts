import type { SessionStore } from "./session-store";
import { MemorySessionStore } from "./session-store";
import { ValkeySessionStore } from "./valkey-session-store";

export type SessionBackend =
  | { kind: "memory" }
  | { kind: "valkey"; url: string };

export function sessionBackend(production: boolean, redisUrl: string | undefined): SessionBackend {
  const url = redisUrl?.trim();
  if (url) return { kind: "valkey", url };
  if (production) {
    throw new Error("REDIS_URL is required for durable production web sessions");
  }
  return { kind: "memory" };
}

export function createSessionStore(backend: SessionBackend): SessionStore {
  return backend.kind === "valkey"
    ? new ValkeySessionStore(backend.url)
    : new MemorySessionStore();
}
