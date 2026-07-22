import { randomBytes } from "node:crypto";

import Redis from "ioredis";

import {
  SESSION_ABSOLUTE_TTL_MS,
  SESSION_IDLE_TTL_MS,
  assertUsableCredentialExpiry,
  type SessionRecord,
  type SessionStore,
} from "./session-store";
import { SessionCipher } from "./session-cipher";

export const SESSION_KEY_PREFIX = "myelin:web-session:v1:";

const VALID_SESSION_LUA = `
local function valid_record(record)
  return type(record) == "table"
    and type(record.token) == "string"
    and type(record.sealed) == "string"
end
local function valid_session(session)
  return type(session) == "table"
    and valid_record(session.record)
    and type(session.createdAtMs) == "number"
    and type(session.lastSeenAtMs) == "number"
    and type(session.expiresAtMs) == "number"
end
`;

const ISSUE_SCRIPT = `
local clock = redis.call("TIME")
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
local expiresAtMs = math.min(now + tonumber(ARGV[2]), tonumber(ARGV[3]))
if expiresAtMs <= now then return 0 end
local session = {
  record = cjson.decode(ARGV[1]),
  createdAtMs = now,
  lastSeenAtMs = now,
  expiresAtMs = expiresAtMs
}
local ttl = math.floor(math.min(tonumber(ARGV[4]), expiresAtMs - now))
redis.call("SET", KEYS[1], cjson.encode(session), "PX", ttl)
return 1
`;

const ROTATE_SCRIPT = `
local clock = redis.call("TIME")
local now = tonumber(clock[1]) * 1000 + math.floor(tonumber(clock[2]) / 1000)
local expiresAtMs = math.min(now + tonumber(ARGV[2]), tonumber(ARGV[3]))
if expiresAtMs <= now then return 0 end
local session = {
  record = cjson.decode(ARGV[1]),
  createdAtMs = now,
  lastSeenAtMs = now,
  expiresAtMs = expiresAtMs
}
local encoded = cjson.encode(session)
local ttl = math.floor(math.min(tonumber(ARGV[4]), expiresAtMs - now))
redis.call("SET", KEYS[1], encoded, "PX", ttl)
redis.call("DEL", KEYS[2])
return 1
`;

const GET_SCRIPT = `${VALID_SESSION_LUA}
local raw = redis.call("GET", KEYS[1])
if not raw then return nil end
local decoded, session = pcall(cjson.decode, raw)
if not decoded or not valid_session(session) then
  redis.call("DEL", KEYS[1])
  return nil
end
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

const UPDATE_TOKEN_SCRIPT = `${VALID_SESSION_LUA}
local raw = redis.call("GET", KEYS[1])
if not raw then return 0 end
local decoded, session = pcall(cjson.decode, raw)
if not decoded or not valid_session(session) then
  redis.call("DEL", KEYS[1])
  return 0
end
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

const READY_SCRIPT = `
local clock = redis.call("TIME")
local encoded = cjson.encode({ checkedAt = clock[1] })
redis.call("SET", KEYS[1], encoded, "PX", 5000)
local raw = redis.call("GET", KEYS[1])
local decoded, value = pcall(cjson.decode, raw)
redis.call("DEL", KEYS[1])
if not decoded or type(value) ~= "table" or value.checkedAt ~= clock[1] then return 0 end
return 1
`;

type StoredRecord = { token: string; sealed: string };

function validStoredRecord(value: unknown): value is StoredRecord {
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return typeof record.token === "string" && typeof record.sealed === "string";
}

function validRecord(value: unknown): value is SessionRecord {
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return (
    ["token", "refreshToken", "scheme", "principalId", "displayName", "region", "tenant"].every(
      (field) => typeof record[field] === "string",
    ) && Number.isSafeInteger(record.credentialExpiresAtMs)
  );
}

export class ValkeySessionStore implements SessionStore {
  readonly #redis: Redis;
  readonly #cipher: SessionCipher;
  #outageReported = false;

  constructor(url: string, encryptionKey: string | undefined) {
    this.#cipher = new SessionCipher(encryptionKey);
    this.#redis = new Redis(url, {
      connectTimeout: 5_000,
      maxRetriesPerRequest: 1,
      retryStrategy: (attempt) => Math.min(attempt * 100, 2_000),
    });
    // Report once per outage without logging the URL or credentials; the operation itself still
    // fails closed, and a successful reconnect resets the report latch.
    this.#redis.on("error", () => {
      if (this.#outageReported) return;
      this.#outageReported = true;
      console.error("[web-session] Valkey connection failed; session readiness is unavailable");
    });
    this.#redis.on("ready", () => {
      this.#outageReported = false;
    });
  }

  async issue(id: string, record: SessionRecord): Promise<void> {
    assertUsableCredentialExpiry(record.credentialExpiresAtMs);
    const issued = await this.#redis.eval(
      ISSUE_SCRIPT,
      1,
      this.#key(id),
      JSON.stringify(this.#seal(id, record)),
      SESSION_ABSOLUTE_TTL_MS,
      record.credentialExpiresAtMs,
      SESSION_IDLE_TTL_MS,
    );
    if (issued !== 1) throw new Error("session credential expired during issuance");
  }

  async rotate(priorId: string, id: string, record: SessionRecord): Promise<void> {
    assertUsableCredentialExpiry(record.credentialExpiresAtMs);
    const rotated = await this.#redis.eval(
      ROTATE_SCRIPT,
      2,
      this.#key(id),
      this.#key(priorId),
      JSON.stringify(this.#seal(id, record)),
      SESSION_ABSOLUTE_TTL_MS,
      record.credentialExpiresAtMs,
      SESSION_IDLE_TTL_MS,
    );
    if (rotated !== 1) throw new Error("session credential expired during rotation");
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
      if (validStoredRecord(record)) return this.#open(id, record);
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
      this.#cipher.encrypt(id, "token", token),
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
    const probeId = `ready:${randomBytes(12).toString("hex")}`;
    const response = await this.#redis.eval(READY_SCRIPT, 1, this.#key(probeId));
    if (response !== 1) throw new Error("Valkey session backend failed its read/write/script probe");
  }

  #key(id: string): string {
    return `${SESSION_KEY_PREFIX}${id}`;
  }

  #seal(id: string, record: SessionRecord): StoredRecord {
    const { token, ...rest } = record;
    return {
      token: this.#cipher.encrypt(id, "token", token),
      sealed: this.#cipher.encrypt(id, "record", JSON.stringify(rest)),
    };
  }

  #open(id: string, record: StoredRecord): SessionRecord {
    const rest: unknown = JSON.parse(this.#cipher.decrypt(id, "record", record.sealed));
    const opened = {
      ...(rest as Omit<SessionRecord, "token">),
      token: this.#cipher.decrypt(id, "token", record.token),
    };
    if (!validRecord(opened)) throw new Error("stored session record has an invalid shape");
    // Project only the declared fields even after authenticated decryption; future schema fields or
    // a mistakenly sealed extra property must not flow into request-local identity objects.
    return {
      token: opened.token,
      refreshToken: opened.refreshToken,
      scheme: opened.scheme,
      credentialExpiresAtMs: opened.credentialExpiresAtMs,
      principalId: opened.principalId,
      displayName: opened.displayName,
      region: opened.region,
      tenant: opened.tenant,
    };
  }
}
