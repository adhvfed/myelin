import Redis from "ioredis";

import {
  SESSION_ABSOLUTE_TTL_MS,
  SESSION_IDLE_TTL_MS,
  type SessionRecord,
  type SessionStore,
} from "./session-store";

const KEY_PREFIX = "myelin:web-session:v1:";

const ISSUE_SCRIPT = `
local clock = redis.call("TIME")
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
local session = {
  record = cjson.decode(ARGV[1]),
  createdAtMs = now,
  lastSeenAtMs = now,
  expiresAtMs = now + tonumber(ARGV[2])
}
redis.call("SET", KEYS[1], cjson.encode(session), "PX", ARGV[3])
return 1
`;

const GET_SCRIPT = `
local raw = redis.call("GET", KEYS[1])
if not raw then return nil end
local session = cjson.decode(raw)
local clock = redis.call("TIME")
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
local idle = tonumber(ARGV[1])
if now >= session.expiresAtMs or now - session.lastSeenAtMs >= idle then
  redis.call("DEL", KEYS[1])
  return nil
end
session.lastSeenAtMs = now
local ttl = math.floor(math.min(idle, session.expiresAtMs - now))
redis.call("SET", KEYS[1], cjson.encode(session), "PX", ttl)
return cjson.encode(session.record)
`;

const UPDATE_TOKEN_SCRIPT = `
local raw = redis.call("GET", KEYS[1])
if not raw then return 0 end
local session = cjson.decode(raw)
local clock = redis.call("TIME")
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
local idle = tonumber(ARGV[2])
if now >= session.expiresAtMs or now - session.lastSeenAtMs >= idle then
  redis.call("DEL", KEYS[1])
  return 0
end
session.record.token = ARGV[1]
session.lastSeenAtMs = now
local ttl = math.floor(math.min(idle, session.expiresAtMs - now))
redis.call("SET", KEYS[1], cjson.encode(session), "PX", ttl)
return 1
`;

function validRecord(value: unknown): value is SessionRecord {
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return [
    "token",
    "refreshToken",
    "scheme",
    "principalId",
    "displayName",
    "region",
    "tenant",
  ].every((field) => typeof record[field] === "string");
}

export class ValkeySessionStore implements SessionStore {
  readonly #redis: Redis;

  constructor(url: string) {
    this.#redis = new Redis(url, {
      connectTimeout: 5_000,
      maxRetriesPerRequest: 1,
      retryStrategy: (attempt) => Math.min(attempt * 100, 2_000),
    });
    // Connection errors surface to the operation that is already failing closed. Registering a
    // listener prevents ioredis from treating background reconnect errors as unhandled events.
    this.#redis.on("error", () => {});
  }

  async issue(id: string, record: SessionRecord): Promise<void> {
    await this.#redis.eval(
      ISSUE_SCRIPT,
      1,
      this.#key(id),
      JSON.stringify(record),
      SESSION_ABSOLUTE_TTL_MS,
      Math.min(SESSION_IDLE_TTL_MS, SESSION_ABSOLUTE_TTL_MS),
    );
  }

  async get(id: string): Promise<SessionRecord | null> {
    const raw = await this.#redis.eval(
      GET_SCRIPT,
      1,
      this.#key(id),
      SESSION_IDLE_TTL_MS,
    );
    if (typeof raw !== "string") return null;
    try {
      const record: unknown = JSON.parse(raw);
      if (validRecord(record)) return record;
    } catch {
      // Corrupt session data is authentication failure, never a partially trusted record.
    }
    await this.delete(id);
    return null;
  }

  async updateToken(id: string, token: string): Promise<boolean> {
    const updated = await this.#redis.eval(
      UPDATE_TOKEN_SCRIPT,
      1,
      this.#key(id),
      token,
      SESSION_IDLE_TTL_MS,
    );
    return updated === 1;
  }

  async delete(id: string): Promise<boolean> {
    return (await this.#redis.del(this.#key(id))) > 0;
  }

  close(): void {
    this.#redis.disconnect(false);
  }

  async ready(): Promise<void> {
    const response = await this.#redis.ping();
    if (response !== "PONG") throw new Error("Valkey session backend did not answer PING");
  }

  #key(id: string): string {
    return `${KEY_PREFIX}${id}`;
  }
}
