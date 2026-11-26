// The organization's people as SolidStart server functions. Every action is a POST, so the
// middleware's origin check guards it; the roster query is a server-function GET, which the CSRF
// guard admits only from the app's own client.
import { action, query, redirect } from "@solidjs/router";
import { isUnauthorized } from "../server/gateway";
import { AccountError, edgeCreateInvitation } from "../server/account-gateway";
import type { AccountFailure, EdgeCreatedInvitation } from "../server/account-gateway";
import {
  edgeRemoveMember,
  edgeRevokeInvitation,
  edgeRoster,
  edgeSetRole,
  MemberError,
  memberResult,
} from "../server/members-gateway";
import type { MemberResult, Roster } from "../server/members-gateway";
import { loginAfter } from "./auth-return";
import { orgRole, type OrgRole } from "./member-rules";

export type RosterView =
  | { state: "ready"; roster: Roster }
  | { state: "not_member" }
  | { state: "unavailable" };

export const getRoster = query(async (): Promise<RosterView> => {
  "use server";
  try {
    return { state: "ready", roster: await edgeRoster() };
  } catch (error) {
    if (isUnauthorized(error)) throw redirect(loginAfter(error));
    if (error instanceof MemberError && error.reason === "role_required") return { state: "not_member" };
    return { state: "unavailable" };
  }
}, "roster");

export type InvitationResult =
  | ({ ok: true; role: OrgRole } & EdgeCreatedInvitation)
  | { ok: false; reason: AccountFailure; retryAfterSecs?: number };

/** Invite a teammate, as a member unless the form names another role. With mail on the edge
 * emails the link; otherwise it is shown once, to be passed on by hand. */
export const createInvitation = action(async (formData: FormData): Promise<InvitationResult> => {
  "use server";
  const email = String(formData.get("email") ?? "").trim();
  const role = orgRole(formData.get("role") ?? "member");
  if (!email || email.length > 320 || !email.includes("@") || !role) {
    return { ok: false, reason: "invalid_email" };
  }
  try {
    return { ok: true, role, ...(await edgeCreateInvitation(email, role)) };
  } catch (error) {
    if (isUnauthorized(error)) throw redirect(loginAfter(error));
    const retryAfterSecs = error instanceof AccountError ? error.retryAfterSecs : undefined;
    const reason = error instanceof AccountError ? error.reason : "unavailable";
    return { ok: false, reason, ...(retryAfterSecs ? { retryAfterSecs } : {}) };
  }
}, "create-invitation");

export const changeRole = action(async (formData: FormData): Promise<MemberResult> => {
  "use server";
  const role = orgRole(formData.get("role"));
  if (!role) return { ok: false, reason: "not_found" };
  return memberResult(() => edgeSetRole(String(formData.get("id") ?? ""), role));
}, "change-member-role");

export const removeMember = action(async (formData: FormData): Promise<MemberResult> => {
  "use server";
  return memberResult(() => edgeRemoveMember(String(formData.get("id") ?? "")));
}, "remove-member");

export const revokeInvitation = action(async (formData: FormData): Promise<MemberResult> => {
  "use server";
  return memberResult(() => edgeRevokeInvitation(String(formData.get("id") ?? "")));
}, "revoke-invitation");
