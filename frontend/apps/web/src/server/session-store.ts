export const SESSION_ABSOLUTE_TTL_MS = 8 * 60 * 60 * 1_000;
export const SESSION_IDLE_TTL_MS = 30 * 60 * 1_000;

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

/** In-process implementation of the session lifecycle contract. The production transport can swap
 * this for Valkey while retaining identical absolute/idle expiry and token-rotation semantics. */
export class MemorySessionStore {
  readonly #sessions = new Map<string, StoredSession>();

  ready(): void {}

  issue(id: string, record: SessionRecord, nowMs = Date.now()): void {
    assertUsableCredentialExpiry(record.credentialExpiresAtMs, nowMs);
    this.#sessions.set(id, {
      record: { ...record },
      createdAtMs: nowMs,
      lastSeenAtMs: nowMs,
      expiresAtMs: Math.min(nowMs + SESSION_ABSOLUTE_TTL_MS, record.credentialExpiresAtMs),
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

export function assertUsableCredentialExpiry(expiresAtMs: number, nowMs = Date.now()): void {
  if (!Number.isSafeInteger(expiresAtMs) || expiresAtMs <= nowMs) {
    throw new Error("session credential expiry is invalid or elapsed");
  }
}
