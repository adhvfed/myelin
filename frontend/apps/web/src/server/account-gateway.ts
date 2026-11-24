// Server-only client for the edge's account endpoints: password sign-in and invitation links.
// These run before a session exists, so they never touch the session store. Edge error bodies are
// discarded; callers get a small closed set of reasons to turn into copy.

import { edgeOrigin } from "./edge-origin";
import { readLimitedText } from "./bounded-response";
import { edgePost, GatewayError, gatewayRequestSignal } from "./gateway";
import { validSessionToken } from "./session-store";

/** A freshly minted human session capability. */
export interface EdgeSessionGrant {
  accessToken: string;
  scheme: "session";
  expiresAt: number;
}

export type AccountFailure =
  | "credentials" // unknown email, wrong password, or an account that may not sign in
  | "rate_limited"
  | "link_invalid" // unknown, expired, or already-used invitation
  | "weak_password"
  | "account_exists"
  | "invalid_email"
  | "invalid_name"
  | "unavailable";

export class AccountError extends Error {
  constructor(readonly reason: AccountFailure) {
    super(`account request failed: ${reason}`);
    this.name = "AccountError";
  }
}

export interface EdgeInvitation {
  kind: "join" | "reset";
  email: string;
  organization: string;
  expiresAt: number;
}

export interface EdgeCreatedInvitation {
  url: string;
  email: string;
  expiresAt: number;
}

/** Invitation codes travel in a URL path; anything outside this alphabet is not one of ours. */
export function validInvitationCode(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9_-]{16,256}$/.test(value);
}

const MAX_ACCOUNT_RESPONSE_BYTES = 64 * 1024;

async function publicRequest(
  method: "GET" | "POST",
  path: string,
  body?: unknown,
): Promise<{ status: number; json: unknown }> {
  let response: Response;
  let text: string;
  try {
    response = await fetch(`${edgeOrigin()}${path}`, {
      method,
      headers: {
        accept: "application/json",
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      redirect: "error",
      signal: gatewayRequestSignal(),
    });
    text = await readLimitedText(response, MAX_ACCOUNT_RESPONSE_BYTES);
  } catch {
    throw new AccountError("unavailable");
  }
  let json: unknown = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = null;
  }
  return { status: response.status, json };
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function futureSeconds(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) > Math.floor(Date.now() / 1_000) &&
    (value as number) <= Math.floor(Number.MAX_SAFE_INTEGER / 1_000);
}

function sessionGrant(json: unknown): EdgeSessionGrant {
  const grant = record(json);
  if (
    !grant || !validSessionToken(grant.access_token) || grant.scheme !== "session" ||
    grant.token_type !== "Bearer" || !futureSeconds(grant.expires_at)
  ) {
    throw new AccountError("unavailable");
  }
  return { accessToken: grant.access_token, scheme: "session", expiresAt: grant.expires_at };
}

/** Exchange an email and password for a human session. */
export async function edgePasswordLogin(email: string, password: string): Promise<EdgeSessionGrant> {
  const { status, json } = await publicRequest("POST", "/v1/auth/password/login", { email, password });
  if (status === 200) return sessionGrant(json);
  if (status === 401 || status === 400) throw new AccountError("credentials");
  if (status === 429) throw new AccountError("rate_limited");
  throw new AccountError("unavailable");
}

/** Look up an invitation link. `null` means the link can no longer be used. */
export async function edgeGetInvitation(code: string): Promise<EdgeInvitation | null> {
  if (!validInvitationCode(code)) return null;
  const { status, json } = await publicRequest("GET", `/v1/auth/invitations/${code}`);
  if (status === 404) return null;
  if (status === 429) throw new AccountError("rate_limited");
  const invitation = record(json);
  if (
    status !== 200 || !invitation ||
    (invitation.kind !== "join" && invitation.kind !== "reset") ||
    typeof invitation.email !== "string" || invitation.email.length === 0 ||
    invitation.email.length > 320 ||
    typeof invitation.organization !== "string" || invitation.organization.length === 0 ||
    invitation.organization.length > 256 ||
    !futureSeconds(invitation.expires_at)
  ) {
    throw new AccountError("unavailable");
  }
  return {
    kind: invitation.kind,
    email: invitation.email,
    organization: invitation.organization,
    expiresAt: invitation.expires_at,
  };
}

/** Accept an invitation: create the account (join) or replace the password (reset). */
export async function edgeAcceptInvitation(
  code: string,
  input: { displayName?: string; password: string },
): Promise<EdgeSessionGrant> {
  if (!validInvitationCode(code)) throw new AccountError("link_invalid");
  const body = input.displayName === undefined
    ? { password: input.password }
    : { display_name: input.displayName, password: input.password };
  const { status, json } = await publicRequest("POST", `/v1/auth/invitations/${code}/accept`, body);
  if (status === 200) return sessionGrant(json);
  if (status === 404) throw new AccountError("link_invalid");
  if (status === 422 || status === 400) throw new AccountError("weak_password");
  if (status === 409) throw new AccountError("account_exists");
  if (status === 429) throw new AccountError("rate_limited");
  throw new AccountError("unavailable");
}

/** Create an invitation link for a teammate, as the signed-in member. */
export async function edgeCreateInvitation(email: string): Promise<EdgeCreatedInvitation> {
  let json: unknown;
  try {
    json = await edgePost<unknown>("/v1/invitations", { email });
  } catch (error) {
    if (error instanceof GatewayError) {
      if (error.status === 409) throw new AccountError("account_exists");
      if (error.status === 400 || error.status === 422) throw new AccountError("invalid_email");
      if (error.status === 429) throw new AccountError("rate_limited");
      throw new AccountError("unavailable");
    }
    throw error;
  }
  const created = record(json);
  if (
    !created || typeof created.url !== "string" || !/^https?:\/\//.test(created.url) ||
    created.url.length > 2_048 || typeof created.email !== "string" ||
    !futureSeconds(created.expires_at)
  ) {
    throw new AccountError("unavailable");
  }
  return { url: created.url, email: created.email, expiresAt: created.expires_at };
}
