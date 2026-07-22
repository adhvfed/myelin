import { describe, expect, it } from "vitest";

import { MemoryOidcTransactionStore } from "./oidc-transaction-store";

const transaction = {
  codeVerifier: "verifier",
  nonce: "nonce",
  redirectUri: "https://myelin.example/auth/oidc/callback",
};

describe("MemoryOidcTransactionStore", () => {
  it("atomically consumes a transaction once", async () => {
    const store = new MemoryOidcTransactionStore();
    expect(await store.issue("state", transaction)).toBe(true);
    expect(await store.issue("state", transaction)).toBe(false);
    expect(await store.consume("state")).toEqual(transaction);
    expect(await store.consume("state")).toBeNull();
  });
});
