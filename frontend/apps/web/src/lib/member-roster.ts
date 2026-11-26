// The edge's `GET /v1/members` answer, checked field by field. Anything malformed is refused whole.
import { orgRole, type OrgRole } from "./member-rules";

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
  /** Active owners; zero only when the last was erased or revoked outside the roster. */
  owners: number;
  people: Member[];
  /** Present only for owners and admins. */
  invitations?: PendingInvitation[];
}

/** Ids travel in a URL path segment, so `.` and `..` are never ids. */
export function validMemberId(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9_.:-]{1,200}$/.test(value) &&
    value !== "." && value !== "..";
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function text(value: unknown, max: number): value is string {
  return typeof value === "string" && value.length <= max;
}

function count(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function member(value: unknown): Member | null {
  const person = record(value);
  const role = orgRole(person?.role);
  if (
    !person || !role || !validMemberId(person.id) || !text(person.display_name, 256) ||
    !text(person.email, 320) || !count(person.joined_at)
  ) {
    return null;
  }
  return { id: person.id, displayName: person.display_name, email: person.email, role, joinedAt: person.joined_at };
}

function invitation(value: unknown): PendingInvitation | null {
  const pending = record(value);
  const role = orgRole(pending?.role);
  if (
    !pending || !role || !validMemberId(pending.id) || !("email" in pending) ||
    !(pending.email === null || text(pending.email, 320)) || !count(pending.expires_at)
  ) {
    return null;
  }
  return { id: pending.id, email: pending.email, role, expiresAt: pending.expires_at };
}

/** The roster, or `null` when the answer is not one. */
export function parseRoster(json: unknown): Roster | null {
  const roster = record(json);
  const role = orgRole(roster?.role);
  if (!roster || !role || !count(roster.owners) || !Array.isArray(roster.people)) return null;
  const people = roster.people.map(member);
  if (people.some((person) => person === null)) return null;
  if (roster.invitations === undefined) {
    return { role, owners: roster.owners, people: people as Member[] };
  }
  if (!Array.isArray(roster.invitations)) return null;
  const invitations = roster.invitations.map(invitation);
  if (invitations.some((pending) => pending === null)) return null;
  return {
    role,
    owners: roster.owners,
    people: people as Member[],
    invitations: invitations as PendingInvitation[],
  };
}
