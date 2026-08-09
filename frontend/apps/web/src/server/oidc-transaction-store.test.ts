import { describe, expect, it } from "vitest";

import { MemoryOidcTransactionStore } from "./oidc-transaction-store";

const transaction = {
  codeVerifier: "v".repeat(43),
  nonce: "n".repeat(43),
  redirectUri: "https://myelin.example/auth/oidc/callback",
  returnTo: "/cli/auth?code=ABCD-EFGH",
};
const state = "s".repeat(43);

describe("MemoryOidcTransactionStore", () => {
  it("atomically consumes a transaction once", async () => {
    const store = new MemoryOidcTransactionStore();
    expect(await store.issue(state, transaction)).toBe(true);
    expect(await store.issue(state, transaction)).toBe(false);
    expect(await store.consume(state)).toEqual(transaction);
    expect(await store.consume(state)).toBeNull();
  });

  it("rejects malformed transaction material and projects extra fields", async () => {
    const store = new MemoryOidcTransactionStore();
    await expect(store.issue("short", transaction)).rejects.toThrow(/state/);
    await expect(store.issue(state, { ...transaction, nonce: "short" })).rejects.toThrow(/invalid/);
    await expect(store.issue(state, {
      ...transaction,
      redirectUri: "https://user:secret@myelin.example/callback",
    })).rejects.toThrow(/invalid/);
    await expect(store.issue(state, {
      ...transaction,
      returnTo: "https://outside.example/steal",
    })).rejects.toThrow(/invalid/);

    const withExtra = { ...transaction, untrusted: "drop" };
    expect(await store.issue(state, withExtra)).toBe(true);
    expect(await store.consume(state)).toEqual(transaction);
    expect(await store.consume("short")).toBeNull();
  });
});
