import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";

const CIPHER_VERSION = "v1";
const IV_BYTES = 12;
const TAG_BYTES = 16;
const KEY_BYTES = 32;
const BASE64URL_KEY = /^[A-Za-z0-9_-]{43}$/;
const BASE64_KEY = /^[A-Za-z0-9+/]{43}=$/;

export type SessionSecretField = "token" | "record";

/** Decode the deployment key without accepting Node's permissive, partially decoded base64 input. */
export function decodeSessionKey(encoded: string | undefined): Buffer {
  const value = encoded?.trim() ?? "";
  let key: Buffer;
  if (BASE64URL_KEY.test(value)) key = Buffer.from(value, "base64url");
  else if (BASE64_KEY.test(value)) key = Buffer.from(value, "base64");
  else {
    throw new Error(
      "MYELIN_WEB_SESSION_KEY must be exactly 32 random bytes encoded as base64 or base64url",
    );
  }
  if (key.length !== KEY_BYTES) {
    throw new Error(
      "MYELIN_WEB_SESSION_KEY must be exactly 32 random bytes encoded as base64 or base64url",
    );
  }
  return key;
}

/** AES-GCM envelope for credentials held in the shared browser-session backend. */
export class SessionCipher {
  readonly #key: Buffer;

  constructor(encodedKey: string | undefined) {
    this.#key = decodeSessionKey(encodedKey);
  }

  encrypt(sessionId: string, field: SessionSecretField, plaintext: string): string {
    const iv = randomBytes(IV_BYTES);
    const cipher = createCipheriv("aes-256-gcm", this.#key, iv);
    cipher.setAAD(this.#aad(sessionId, field));
    const ciphertext = Buffer.concat([cipher.update(plaintext, "utf8"), cipher.final()]);
    const tag = cipher.getAuthTag();
    return [
      CIPHER_VERSION,
      iv.toString("base64url"),
      ciphertext.toString("base64url"),
      tag.toString("base64url"),
    ].join(".");
  }

  decrypt(sessionId: string, field: SessionSecretField, envelope: string): string {
    try {
      const parts = envelope.split(".");
      if (parts.length !== 4 || parts[0] !== CIPHER_VERSION) throw new Error("invalid envelope");
      const iv = decodeCanonicalBase64Url(parts[1]!, IV_BYTES);
      const ciphertext = decodeCanonicalBase64Url(parts[2]!, undefined);
      const tag = decodeCanonicalBase64Url(parts[3]!, TAG_BYTES);
      const decipher = createDecipheriv("aes-256-gcm", this.#key, iv);
      decipher.setAAD(this.#aad(sessionId, field));
      decipher.setAuthTag(tag);
      const plaintext = Buffer.concat([decipher.update(ciphertext), decipher.final()]);
      return new TextDecoder("utf-8", { fatal: true }).decode(plaintext);
    } catch {
      // Authentication failure, malformed storage, and a wrong deployment key are deliberately
      // indistinguishable. The caller deletes the unusable session and treats it as logged out.
      throw new Error("stored session credential could not be decrypted");
    }
  }

  #aad(sessionId: string, field: SessionSecretField): Buffer {
    // Bind ciphertext to both its session and field so a datastore writer cannot swap the sealed
    // record into the access-token slot or transplant credentials between browser sessions.
    return Buffer.from(`myelin:web-session:v1\0${sessionId}\0${field}`, "utf8");
  }
}

function decodeCanonicalBase64Url(value: string, expectedBytes: number | undefined): Buffer {
  if (!/^[A-Za-z0-9_-]*$/.test(value)) throw new Error("invalid base64url");
  const decoded = Buffer.from(value, "base64url");
  if (decoded.toString("base64url") !== value) throw new Error("non-canonical base64url");
  if (expectedBytes !== undefined && decoded.length !== expectedBytes) {
    throw new Error("invalid decoded length");
  }
  return decoded;
}
