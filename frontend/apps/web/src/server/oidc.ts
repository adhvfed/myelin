import { randomBytes } from "node:crypto";

import { deleteCookie, getCookie, setCookie } from "vinxi/http";

import { safeAuthReturnTo } from "../lib/auth-return";
import { edgeLoginWithOidc, edgeWhoamiWithToken } from "./gateway";
import { oidcClientConfig, type OidcClientConfig } from "./oidc-config";
import { oidcAuthorizationUrl, oidcClientAuthorization } from "./oidc-core";
import {
  matchesOidcStateCookie,
  oidcStateCookieName,
  runOidcCallback,
  type OidcCallbackResult,
} from "./oidc-callback-core";
import {
  MemoryOidcTransactionStore,
  type OidcTransactionStore,
  ValkeyOidcTransactionStore,
} from "./oidc-transaction-store";
import { issueSession } from "./session";
import { sessionBackend } from "./session-backend";

const MAX_TOKEN_RESPONSE_BYTES = 64 * 1024;
const OIDC_TIMEOUT_MS = 15_000;
const production = process.env.NODE_ENV === "production";
// One host-only cookie per transaction keeps concurrent tabs independent. The callback state selects
// exactly one cookie, so an unsolicited or malformed callback cannot erase another login attempt.
const stateCookiePrefix = production ? "__Host-myelin_oidc_state_" : "myelin_oidc_state_";

const globalStore = globalThis as unknown as {
  __myelinOidcTransactionStore?: OidcTransactionStore;
};

function config(): OidcClientConfig | null {
  return oidcClientConfig(process.env, production);
}

function transactionStore(): OidcTransactionStore {
  return (globalStore.__myelinOidcTransactionStore ??= (() => {
    const backend = sessionBackend(
      production,
      process.env.REDIS_URL,
      process.env.MYELIN_WEB_SESSION_KEY,
    );
    return backend.kind === "valkey"
      ? new ValkeyOidcTransactionStore(backend.url, backend.encryptionKey)
      : new MemoryOidcTransactionStore();
  })());
}

function secret(bytes = 32): string {
  return randomBytes(bytes).toString("base64url");
}

export function interactiveOidcConfigured(): boolean {
  return config() !== null;
}

export async function oidcTransactionStoreReady(): Promise<void> {
  if (config()) await transactionStore().ready();
}

export async function beginOidcLogin(returnTo: unknown): Promise<string> {
  const oidc = config();
  if (!oidc) throw new Error("interactive OIDC is not configured");
  const store = transactionStore();
  let state = "";
  let issued = false;
  const codeVerifier = secret();
  const nonce = secret();
  for (let attempt = 0; attempt < 3 && !issued; attempt += 1) {
    state = secret();
    issued = await store.issue(state, {
      codeVerifier,
      nonce,
      redirectUri: oidc.redirectUri,
      returnTo: safeAuthReturnTo(returnTo),
    });
  }
  if (!issued) throw new Error("could not allocate an OIDC transaction");

  const cookieName = oidcStateCookieName(stateCookiePrefix, state);
  if (!cookieName) throw new Error("generated OIDC state has an invalid shape");
  setStateCookie(cookieName, state);
  return oidcAuthorizationUrl(oidc, state, nonce, codeVerifier);
}

function setStateCookie(name: string, value: string): void {
  setCookie(name, value, {
    httpOnly: true,
    sameSite: "lax",
    secure: production,
    path: "/",
    maxAge: 10 * 60,
  });
}

function clearStateCookie(name: string): void {
  deleteCookie(name, {
    httpOnly: true,
    sameSite: "lax",
    secure: production,
    path: "/",
  });
}

async function limitedText(response: Response): Promise<string> {
  const declared = Number(response.headers.get("content-length") ?? "0");
  if (declared > MAX_TOKEN_RESPONSE_BYTES) throw new Error("OIDC token response is too large");
  if (!response.body) return "";
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > MAX_TOKEN_RESPONSE_BYTES) {
      await reader.cancel();
      throw new Error("OIDC token response is too large");
    }
    chunks.push(value);
  }
  const body = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder("utf-8", { fatal: true }).decode(body);
}

async function exchangeCode(
  oidc: OidcClientConfig,
  code: string,
  codeVerifier: string,
): Promise<string> {
  const response = await fetch(oidc.tokenEndpoint, {
    method: "POST",
    headers: {
      accept: "application/json",
      authorization: oidcClientAuthorization(oidc.clientId, oidc.clientSecret),
      "content-type": "application/x-www-form-urlencoded",
    },
    body: new URLSearchParams({
      grant_type: "authorization_code",
      code,
      redirect_uri: oidc.redirectUri,
      code_verifier: codeVerifier,
    }),
    redirect: "error",
    signal: AbortSignal.timeout(OIDC_TIMEOUT_MS),
  });
  const contentType = response.headers
    .get("content-type")
    ?.split(";", 1)[0]
    ?.trim()
    .toLowerCase();
  const text = await limitedText(response);
  if (!response.ok || contentType !== "application/json") {
    throw new Error("OIDC token exchange failed");
  }
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    throw new Error("OIDC token response was invalid");
  }
  const idToken = (body as { id_token?: unknown })?.id_token;
  if (typeof idToken !== "string" || !idToken || idToken.length > 32 * 1024) {
    throw new Error("OIDC token response omitted a usable ID token");
  }
  return idToken;
}

/** Complete and consume the browser transaction. Any failure is intentionally opaque to the UI. */
export async function completeOidcLogin(requestUrl: string): Promise<OidcCallbackResult | null> {
  const oidc = config();
  const callback = new URL(requestUrl);
  if (!oidc) return null;
  const states = callback.searchParams.getAll("state");
  const state = states.length === 1 ? states[0]! : "";
  const cookieName = oidcStateCookieName(stateCookiePrefix, state);
  if (!cookieName) return null;
  const cookieState = getCookie(cookieName) ?? "";
  if (!matchesOidcStateCookie(state, cookieState)) return null;
  clearStateCookie(cookieName);
  return runOidcCallback(
    {
      states,
      codes: callback.searchParams.getAll("code"),
      cookieState,
      providerError: callback.searchParams.has("error"),
      redirectUri: oidc.redirectUri,
    },
    {
      consume: (state) => transactionStore().consume(state),
      exchange: (code, verifier) => exchangeCode(oidc, code, verifier),
      establish: async (idToken, nonce) => {
        const login = await edgeLoginWithOidc(idToken, nonce);
        const who = await edgeWhoamiWithToken(login.accessToken, login.scheme);
        await issueSession({
          token: login.accessToken,
          refreshToken: "",
          scheme: login.scheme,
          credentialExpiresAtMs: Math.min(login.expiresAt, who.expires_at) * 1_000,
          principalId: who.principal_id,
          displayName: who.principal_id,
          region: who.region,
          tenant: who.tenant,
        });
      },
    },
  );
}
