export const SESSION_ABSOLUTE_TTL_MS = 8 * 60 * 60 * 1_000;
export const SESSION_IDLE_TTL_MS = 30 * 60 * 1_000;
export const MAX_SESSION_TOKEN_BYTES = 32 * 1024;
export const MAX_SESSION_PRINCIPAL_BYTES = 512;
export const MAX_SESSION_IDENTITY_FIELD_BYTES = 128;
export const MAX_SESSION_DISPLAY_NAME_BYTES = 512;

export interface SessionRecord {
  token: string;
  refreshToken: string;
  scheme: string;
  /** Absolute expiry of the credential carried by this session. The browser session may end
   * earlier because of idle or platform limits, but it must never outlive this instant. */
  credentialExpiresAtMs: number;
  principalId: string;
  displayName: string;
  region: string;
  tenant: string;
}

export interface SessionStore {
  ready(): void | Promise<void>;
  issue(id: string, record: SessionRecord): void | Promise<void>;
  rotate(priorId: string, id: string, record: SessionRecord): void | Promise<void>;
  get(id: string): SessionRecord | null | Promise<SessionRecord | null>;
  updateToken(id: string, token: string): boolean | Promise<boolean>;
  delete(id: string): boolean | Promise<boolean>;
}

interface StoredSession {
  record: SessionRecord;
  createdAtMs: number;
  lastSeenAtMs: number;
  expiresAtMs: number;
}

function boundedIdentityField(value: unknown, maxBytes: number): value is string {
  if (typeof value !== "string" || value.length === 0 || value.length > maxBytes) return false;
  if (new TextEncoder().encode(value).byteLength > maxBytes) return false;
  for (const character of value) {
    const codePoint = character.codePointAt(0)!;
    if (codePoint <= 0x1f || codePoint === 0x7f) return false;
  }
  return true;
}

function validToken(value: unknown, allowEmpty: boolean): value is string {
  return typeof value === "string" &&
    ((allowEmpty && value.length === 0) ||
      (value.length <= MAX_SESSION_TOKEN_BYTES && /^[\x21-\x7e]+$/.test(value)));
}

/** Validate and project every runtime-created or decrypted session before it enters auth state. */
export function validatedSessionRecord(value: unknown, nowMs = Date.now()): SessionRecord {
  if (!value || typeof value !== "object") throw new Error("session record is invalid");
  const record = value as Record<string, unknown>;
  if (
    !validToken(record.token, false) ||
    !validToken(record.refreshToken, true) ||
    typeof record.scheme !== "string" ||
    !/^[a-z][a-z0-9_]{0,31}$/.test(record.scheme) ||
    !boundedIdentityField(record.principalId, MAX_SESSION_PRINCIPAL_BYTES) ||
    !boundedIdentityField(record.displayName, MAX_SESSION_DISPLAY_NAME_BYTES) ||
    !boundedIdentityField(record.region, MAX_SESSION_IDENTITY_FIELD_BYTES) ||
    !boundedIdentityField(record.tenant, MAX_SESSION_IDENTITY_FIELD_BYTES)
  ) {
    throw new Error("session record is invalid");
  }
  assertUsableCredentialExpiry(record.credentialExpiresAtMs, nowMs);
  return {
    token: record.token,
    refreshToken: record.refreshToken,
    scheme: record.scheme,
    credentialExpiresAtMs: record.credentialExpiresAtMs,
    principalId: record.principalId,
    displayName: record.displayName,
    region: record.region,
    tenant: record.tenant,
  };
}

export function assertValidSessionToken(token: unknown): asserts token is string {
  if (!validToken(token, false)) throw new Error("session access token is invalid");
}

/** In-process implementation of the session lifecycle contract. The production transport can swap
 * this for Valkey while retaining identical absolute/idle expiry and token-rotation semantics. */
export class MemorySessionStore {
  readonly #sessions = new Map<string, StoredSession>();

  ready(): void {}

  issue(id: string, record: SessionRecord, nowMs = Date.now()): void {
    const validated = validatedSessionRecord(record, nowMs);
    this.#sessions.set(id, {
      record: validated,
      createdAtMs: nowMs,
      lastSeenAtMs: nowMs,
      expiresAtMs: Math.min(nowMs + SESSION_ABSOLUTE_TTL_MS, validated.credentialExpiresAtMs),
    });
  }

  rotate(priorId: string, id: string, record: SessionRecord, nowMs = Date.now()): void {
    // This implementation is synchronous: no observer can interleave between replacement issue and
    // prior revocation. Production provides the equivalent atomicity in one Valkey Lua script.
    this.issue(id, record, nowMs);
    if (priorId !== id) this.#sessions.delete(priorId);
  }

  get(id: string, nowMs = Date.now()): SessionRecord | null {
    const session = this.#sessions.get(id);
    if (!session) return null;

    if (
      nowMs >= session.expiresAtMs ||
      nowMs - session.lastSeenAtMs >= SESSION_IDLE_TTL_MS
    ) {
      this.#sessions.delete(id);
      return null;
    }

    session.lastSeenAtMs = nowMs;
    return { ...session.record };
  }

  updateToken(id: string, token: string, nowMs = Date.now()): boolean {
    assertValidSessionToken(token);
    const record = this.get(id, nowMs);
    const session = this.#sessions.get(id);
    if (!record || !session) return false;
    session.record = { ...record, token };
    return true;
  }

  delete(id: string): boolean {
    return this.#sessions.delete(id);
  }

  size(): number {
    return this.#sessions.size;
  }
}

export function assertUsableCredentialExpiry(
  expiresAtMs: unknown,
  nowMs = Date.now(),
): asserts expiresAtMs is number {
  if (
    typeof expiresAtMs !== "number" ||
    !Number.isSafeInteger(expiresAtMs) ||
    expiresAtMs <= nowMs
  ) {
    throw new Error("session credential expiry is invalid or elapsed");
  }
}
