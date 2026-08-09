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

export interface OidcCallbackResult {
  authenticated: boolean;
  returnTo: string;
}

function equal(left: string, right: string): boolean {
  const a = Buffer.from(left);
  const b = Buffer.from(right);
  return a.length === b.length && timingSafeEqual(a, b);
}

export function oidcStateCookieName(prefix: string, state: string): string | null {
  return STATE_PATTERN.test(state) ? `${prefix}${state}` : null;
}

export function matchesOidcStateCookie(state: string, cookieState: string): boolean {
  return STATE_PATTERN.test(state) && STATE_PATTERN.test(cookieState) && equal(state, cookieState);
}

/** Fail-closed callback decision. The state transaction is consumed before any external exchange. */
export async function runOidcCallback(
  input: OidcCallbackInput,
  dependencies: OidcCallbackDependencies,
): Promise<OidcCallbackResult | null> {
  const state = input.states.length === 1 ? input.states[0]! : "";
  const code = input.codes.length === 1 ? input.codes[0]! : "";
  if (
    !STATE_PATTERN.test(state) ||
    !matchesOidcStateCookie(state, input.cookieState)
  ) {
    return null;
  }
  let transaction: OidcTransaction | null;
  try {
    transaction = await dependencies.consume(state);
  } catch {
    return null;
  }
  if (!transaction || transaction.redirectUri !== input.redirectUri) return null;
  if (input.providerError || !CODE_PATTERN.test(code)) {
    return { authenticated: false, returnTo: transaction.returnTo };
  }
  try {
    const idToken = await dependencies.exchange(code, transaction.codeVerifier);
    await dependencies.establish(idToken, transaction.nonce);
    return { authenticated: true, returnTo: transaction.returnTo };
  } catch {
    return { authenticated: false, returnTo: transaction.returnTo };
  }
}
