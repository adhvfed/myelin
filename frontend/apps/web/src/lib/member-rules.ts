// What the viewer's role lets them do to people, mirrored from the edge (accounts/roles.rs) so the
// page offers only what the edge would accept. The edge still decides every request.
export type OrgRole = "owner" | "admin" | "member";

export function orgRole(value: unknown): OrgRole | null {
  return value === "owner" || value === "admin" || value === "member" ? value : null;
}

export const ROLE_LABEL: Record<OrgRole, string> = {
  owner: "Owner",
  admin: "Admin",
  member: "Member",
};

/** The roles `viewer` may hand out, by invitation or by a change. */
export function grantableRoles(viewer: OrgRole): OrgRole[] {
  if (viewer === "owner") return ["owner", "admin", "member"];
  if (viewer === "admin") return ["member"];
  return [];
}

/** Whether `viewer` may change or remove someone holding `target`. */
export function mayActOn(viewer: OrgRole, target: OrgRole, isSelf: boolean): boolean {
  if (viewer === "owner") return true;
  if (viewer === "admin") return target === "member" || isSelf;
  return false;
}

/** The roles to offer for one person: their own, and what the viewer may change it to. */
export function roleChoices(viewer: OrgRole, target: OrgRole, isSelf: boolean): OrgRole[] {
  if (!mayActOn(viewer, target, isSelf)) return [target];
  const choices = new Set<OrgRole>([target, ...grantableRoles(viewer)]);
  return (["owner", "admin", "member"] as const).filter((role) => choices.has(role));
}
