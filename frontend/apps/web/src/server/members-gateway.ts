// Server-only client for the edge's people endpoints: the roster, role changes, removals and
// revoking invitations. Edge error bodies are discarded; callers get a small closed set of reasons.

import { redirect } from "@solidjs/router";
import { loginAfter } from "../lib/auth-return";
import { orgRole, type OrgRole } from "../lib/member-rules";
import { edgeDelete, edgeGet, edgePost, GatewayError, isUnauthorized } from "./gateway";

export interface Member {
  id: string;
  displayName: string;
  email: string;
  role: OrgRole;
  joinedAt: number;
}

export interface PendingInvitation {
  id: string;
  /** `null` for an invitation made before addresses were kept for the roster. */
  email: string | null;
  role: OrgRole;
  expiresAt: number;
}

export interface Roster {
  role: OrgRole;
  people: Member[];
  /** Present only for owners and admins. */
  invitations?: PendingInvitation[];
}

export type MemberFailure =
  | "role_required" // the viewer's role does not allow it
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

/** Ids travel in a URL path; anything else is not one the edge handed out. */
export function validMemberId(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9_.:-]{1,200}$/.test(value);
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function text(value: unknown, max: number): value is string {
  return typeof value === "string" && value.length <= max;
}

function member(value: unknown): Member {
  const person = record(value);
  const role = orgRole(person?.role);
  if (
    !person || !role || !validMemberId(person.id) || !text(person.display_name, 256) ||
    !text(person.email, 320) || !Number.isSafeInteger(person.joined_at)
  ) {
    throw new MemberError("unavailable");
  }
  return {
    id: person.id,
    displayName: person.display_name,
    email: person.email,
    role,
    joinedAt: person.joined_at as number,
  };
}

function invitation(value: unknown): PendingInvitation {
  const pending = record(value);
  const role = orgRole(pending?.role);
  if (
    !pending || !role || !validMemberId(pending.id) ||
    !(pending.email === null || text(pending.email, 320)) || !Number.isSafeInteger(pending.expires_at)
  ) {
    throw new MemberError("unavailable");
  }
  return { id: pending.id, email: pending.email, role, expiresAt: pending.expires_at as number };
}

function failure(error: unknown): never {
  if (error instanceof GatewayError) {
    if (error.status === 403 && error.code === "role_required") throw new MemberError("role_required");
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
  const roster = record(json);
  const role = orgRole(roster?.role);
  if (!roster || !role || !Array.isArray(roster.people) ||
    !(roster.invitations === undefined || Array.isArray(roster.invitations))) {
    throw new MemberError("unavailable");
  }
  return {
    role,
    people: roster.people.map(member),
    ...(Array.isArray(roster.invitations) ? { invitations: roster.invitations.map(invitation) } : {}),
  };
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
