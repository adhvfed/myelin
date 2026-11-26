// Server-only client for the edge's people endpoints: the roster, role changes, removals and
// revoking invitations. Edge error bodies are discarded; callers get a small closed set of reasons.

import { redirect } from "@solidjs/router";
import { loginAfter } from "../lib/auth-return";
import type { OrgRole } from "../lib/member-rules";
import { parseRoster, validMemberId, type Roster } from "../lib/member-roster";
import { edgeDelete, edgeGet, edgePost, GatewayError, isUnauthorized } from "./gateway";

export type MemberFailure =
  | "role_required" // the viewer's role does not allow it
  | "forbidden" // this session may not use the people endpoints at all
  | "last_owner" // it would leave the organization without an owner
  | "not_found"
  | "unavailable";

export class MemberError extends Error {
  constructor(readonly reason: MemberFailure) {
    super(`member request failed: ${reason}`);
    this.name = "MemberError";
  }
}

export type MemberResult = { ok: true } | { ok: false; reason: MemberFailure };

/** Runs one change and says how it went. A refused session (removing yourself ends your own)
 * goes to sign-in. */
export async function memberResult(change: () => Promise<void>): Promise<MemberResult> {
  try {
    await change();
    return { ok: true };
  } catch (error) {
    if (isUnauthorized(error)) throw redirect(loginAfter(error));
    return { ok: false, reason: error instanceof MemberError ? error.reason : "unavailable" };
  }
}

function failure(error: unknown): never {
  if (error instanceof GatewayError) {
    if (error.status === 403) {
      throw new MemberError(error.code === "role_required" ? "role_required" : "forbidden");
    }
    if (error.status === 409 && error.code === "last_owner") throw new MemberError("last_owner");
    if (error.status === 404) throw new MemberError("not_found");
    throw new MemberError("unavailable");
  }
  throw error;
}

/** The organization's people, and its pending invitations when the viewer manages people. */
export async function edgeRoster(): Promise<Roster> {
  let json: unknown;
  try {
    json = await edgeGet<unknown>("/v1/members");
  } catch (error) {
    failure(error);
  }
  const roster = parseRoster(json);
  if (!roster) throw new MemberError("unavailable");
  return roster;
}

export async function edgeSetRole(id: string, role: OrgRole): Promise<void> {
  if (!validMemberId(id)) throw new MemberError("not_found");
  try {
    await edgePost<unknown>(`/v1/members/${encodeURIComponent(id)}/role`, { role });
  } catch (error) {
    failure(error);
  }
}

export async function edgeRemoveMember(id: string): Promise<void> {
  if (!validMemberId(id)) throw new MemberError("not_found");
  try {
    await edgeDelete<unknown>(`/v1/members/${encodeURIComponent(id)}`);
  } catch (error) {
    failure(error);
  }
}

export async function edgeRevokeInvitation(id: string): Promise<void> {
  if (!validMemberId(id)) throw new MemberError("not_found");
  try {
    await edgeDelete<unknown>(`/v1/invitations/${encodeURIComponent(id)}`);
  } catch (error) {
    failure(error);
  }
}
