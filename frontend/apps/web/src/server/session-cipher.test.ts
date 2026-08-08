import { describe, expect, it } from "vitest";

import { decodeSessionKey, SessionCipher } from "./session-cipher";

const key = Buffer.alloc(32, 7).toString("base64url");

describe("SessionCipher", () => {
  it("round-trips a credential without embedding its plaintext", () => {
    const cipher = new SessionCipher(key);
    const envelope = cipher.encrypt("sess_one", "token", "secret-access-token");

    expect(envelope).not.toContain("secret-access-token");
    expect(cipher.decrypt("sess_one", "token", envelope)).toBe("secret-access-token");
  });

  it("binds ciphertext to the session id and credential field", () => {
    const cipher = new SessionCipher(key);
    const envelope = cipher.encrypt("sess_one", "token", "secret-access-token");

    expect(() => cipher.decrypt("sess_two", "token", envelope)).toThrow(
      "stored session credential could not be decrypted",
    );
    expect(() => cipher.decrypt("sess_one", "record", envelope)).toThrow(
      "stored session credential could not be decrypted",
    );
  });

  it("rejects malformed and non-canonical deployment keys", () => {
    expect(() => decodeSessionKey(undefined)).toThrow(/exactly 32 random bytes/);
    expect(() => decodeSessionKey("not-base64")).toThrow(/exactly 32 random bytes/);
    expect(() => decodeSessionKey(Buffer.alloc(31).toString("base64url"))).toThrow(
      /exactly 32 random bytes/,
    );
  });
});
