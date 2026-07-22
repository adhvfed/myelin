import { timingSafeEqual } from "node:crypto";

import type { OidcTransaction } from "./oidc-transaction-store";

const STATE_PATTERN = /^[A-Za-z0-9_-]{43}$/;
const CODE_PATTERN = /^[\x20-\x7e]{1,8192}$/;

export interface OidcCallbackInput {
  states: string[];
  codes: string[];
  cookieState: string;
  providerError: boolean;
  redirectUri: string;
}

export interface OidcCallbackDependencies {
  consume(state: string): Promise<OidcTransaction | null>;
  exchange(code: string, codeVerifier: string): Promise<string>;
  establish(idToken: string, nonce: string): Promise<void>;
}

function equal(left: string, right: string): boolean {
  const a = Buffer.from(left);
  const b = Buffer.from(right);
  return a.length === b.length && timingSafeEqual(a, b);
}

/** Fail-closed callback decision. The state transaction is consumed before any external exchange. */
export async function runOidcCallback(
  input: OidcCallbackInput,
  dependencies: OidcCallbackDependencies,
): Promise<boolean> {
  const state = input.states.length === 1 ? input.states[0]! : "";
  const code = input.codes.length === 1 ? input.codes[0]! : "";
  if (
    input.providerError ||
    !STATE_PATTERN.test(state) ||
    !STATE_PATTERN.test(input.cookieState) ||
    !equal(state, input.cookieState) ||
    !CODE_PATTERN.test(code)
  ) {
    return false;
  }
  try {
    const transaction = await dependencies.consume(state);
    if (!transaction || transaction.redirectUri !== input.redirectUri) return false;
    const idToken = await dependencies.exchange(code, transaction.codeVerifier);
    await dependencies.establish(idToken, transaction.nonce);
    return true;
  } catch {
    return false;
  }
}
