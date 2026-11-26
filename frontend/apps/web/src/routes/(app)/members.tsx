import { ConfirmDialog, Icon, useToast } from "@myelin/design-system";
import { Title } from "@solidjs/meta";
import { createAsync, useAction, useSubmission } from "@solidjs/router";
import { createSignal, For, Match, onMount, Show, Switch } from "solid-js";
import { useAppViewer } from "~/components/AppShell";
import { ProfileCard } from "~/components/ProfileCard";
import { getAuthConfig } from "~/lib/auth";
import { grantableRoles, mayActOn, ROLE_LABEL, roleChoices, type OrgRole } from "~/lib/member-rules";
import {
  changeRole,
  createInvitation,
  getRoster,
  removeMember,
  revokeInvitation,
  type InvitationResult,
} from "~/lib/members";
import type { Member, MemberResult, PendingInvitation, Roster } from "~/server/members-gateway";

const longDate = (unix: number) =>
  new Date(unix * 1_000).toLocaleDateString(undefined, { day: "numeric", month: "long", year: "numeric" });
const untilDate = (unix: number) =>
  new Date(unix * 1_000).toLocaleDateString(undefined, { day: "numeric", month: "long" });

const MEMBER_ERRORS: Record<Exclude<MemberResult, { ok: true }>["reason"], string> = {
  role_required: "Your role doesn't allow that.",
  last_owner: "An organization needs an owner. Make someone else an owner first.",
  not_found: "They're no longer here. The list has been refreshed.",
  unavailable: "That didn't work just now. Try again in a moment.",
};

export default function Members() {
  const viewer = useAppViewer();
  const roster = createAsync(() => getRoster());
  const [interactive, setInteractive] = createSignal(false);
  onMount(() => setInteractive(true));
  return <>
    <Title>Members · Myelin</Title>
    <div class="profile-screen">
      <header class="profile-hero members-hero"><div class="profile-avatar" aria-hidden="true"><Icon name="team" size={30} /></div><div><p>Organization</p><h1>Members</h1><span><code>{viewer.tenant}</code></span></div></header>
      <Switch>
        <Match when={roster()?.state === "not_member"}>
          <p class="members-note" data-testid="members-not-member">Only members of this organization can see its people.</p>
        </Match>
        <Match when={roster()?.state === "unavailable"}>
          <p role="alert" class="members-note" data-testid="members-unavailable">The members list couldn't be loaded. Try again in a moment.</p>
        </Match>
        <Match when={roster()?.state === "ready" ? (roster() as { roster: Roster }).roster : undefined} keyed>
          {(ready) => <RosterView roster={ready} viewerId={viewer.principalId} interactive={interactive()} />}
        </Match>
      </Switch>
    </div>
  </>;
}

function RosterView(props: { roster: Roster; viewerId: string; interactive: boolean }) {
  const manages = () => props.roster.role !== "member";
  return (
    <div class="members-grid">
      <Show
        when={manages()}
        fallback={<p class="members-note" data-testid="invite-owners-only">Owners and admins invite people. Ask one of them to invite a teammate.</p>}
      >
        <InviteCard role={props.roster.role} />
      </Show>
      <PeopleCard roster={props.roster} viewerId={props.viewerId} interactive={props.interactive} />
      <Show when={props.roster.invitations}>
        {(invitations) => <InvitationsCard invitations={invitations()} interactive={props.interactive} />}
      </Show>
    </div>
  );
}

/** Shows a member action's failure as a toast; success is visible in the refreshed list. */
function useMemberAction(run: (form: FormData) => Promise<MemberResult>, done: string) {
  const toast = useToast();
  return async (fields: Record<string, string>) => {
    const form = new FormData();
    for (const [name, value] of Object.entries(fields)) form.set(name, value);
    const result = await run(form);
    toast.show(result.ok
      ? { title: done, variant: "success" }
      : { title: MEMBER_ERRORS[result.reason], variant: "danger" });
    return result.ok;
  };
}

function PeopleCard(props: { roster: Roster; viewerId: string; interactive: boolean }) {
  const setRole = useMemberAction(useAction(changeRole), "Role changed");
  const remove = useMemberAction(useAction(removeMember), "Removed from the organization");
  const [removing, setRemoving] = createSignal<Member | null>(null);
  const [stepping, setStepping] = createSignal<{ member: Member; role: OrgRole } | null>(null);
  const choose = (member: Member, role: OrgRole, select: HTMLSelectElement) => {
    // Changing your own role takes powers away from you, so it is confirmed first.
    if (member.id === props.viewerId) {
      select.value = member.role;
      setStepping({ member, role });
      return;
    }
    void setRole({ id: member.id, role }).then((ok) => { if (!ok) select.value = member.role; });
  };
  return (
    <ProfileCard
      icon="team"
      title="People"
      copy="Owners manage everyone. Admins invite and manage members. Members see who's here."
    >
      <ul class="members-list" data-testid="members-people">
        <For each={props.roster.people}>
          {(member) => {
            const isSelf = member.id === props.viewerId;
            const choices = roleChoices(props.roster.role, member.role, isSelf);
            const label = member.displayName || member.email || "Unnamed person";
            return (
              <li data-testid="member-row" data-member-email={member.email}>
                <div class="members-who">
                  <strong>{label}{isSelf ? " (you)" : ""}</strong>
                  <span>{member.email}</span>
                </div>
                <span class="members-meta">Joined {longDate(member.joinedAt)}</span>
                <Show
                  when={choices.length > 1}
                  fallback={<span class="members-role" data-testid="member-role-text">{ROLE_LABEL[member.role]}</span>}
                >
                  <select
                    class="members-role-select"
                    aria-label={`Role for ${label}`}
                    data-testid="member-role"
                    disabled={!props.interactive}
                    value={member.role}
                    onChange={(event) => choose(member, event.currentTarget.value as OrgRole, event.currentTarget)}
                  >
                    <For each={choices}>{(role) => <option value={role}>{ROLE_LABEL[role]}</option>}</For>
                  </select>
                </Show>
                <Show when={mayActOn(props.roster.role, member.role, isSelf)}>
                  <button
                    type="button"
                    class="members-danger"
                    disabled={!props.interactive}
                    data-testid="member-remove"
                    aria-label={`Remove ${label}`}
                    onClick={() => setRemoving(member)}
                  >
                    Remove
                  </button>
                </Show>
              </li>
            );
          }}
        </For>
      </ul>
      <ConfirmDialog
        open={removing() !== null}
        variant="destructive"
        title={`Remove ${removing()?.displayName || removing()?.email || "this person"}?`}
        description={removing()?.id === props.viewerId
          ? "You'll be signed out everywhere and won't be able to sign in to this organization again."
          : "They'll be signed out everywhere at once and won't be able to sign in to this organization again."}
        confirmLabel="Remove"
        onCancel={() => setRemoving(null)}
        onConfirm={() => {
          const member = removing();
          setRemoving(null);
          if (member) void remove({ id: member.id });
        }}
      />
      <ConfirmDialog
        open={stepping() !== null}
        title={`Make yourself ${stepping() ? ROLE_LABEL[stepping()!.role].toLowerCase() : ""}?`}
        description="You'll lose what your current role lets you do, and only an owner can give it back."
        confirmLabel="Change my role"
        onCancel={() => setStepping(null)}
        onConfirm={() => {
          const change = stepping();
          setStepping(null);
          if (change) void setRole({ id: change.member.id, role: change.role });
        }}
      />
    </ProfileCard>
  );
}

function InvitationsCard(props: { invitations: PendingInvitation[]; interactive: boolean }) {
  const revoke = useMemberAction(useAction(revokeInvitation), "Invitation revoked");
  const [revoking, setRevoking] = createSignal<PendingInvitation | null>(null);
  const address = (invitation: PendingInvitation | null) => invitation?.email ?? "an earlier invitation";
  return (
    <ProfileCard
      icon="inbox"
      title="Pending invitations"
      copy="Links sent but not yet used. Revoking one stops its link working, and stops its email if it hasn't gone out."
    >
      <Show
        when={props.invitations.length > 0}
        fallback={<p class="members-empty" data-testid="invitations-empty">No invitations are waiting.</p>}
      >
        <ul class="members-list" data-testid="members-invitations">
          <For each={props.invitations}>
            {(invitation) => (
              <li data-testid="invitation-row" data-invitation-email={invitation.email ?? ""}>
                <div class="members-who">
                  <strong>{invitation.email ?? "Address not recorded"}</strong>
                  <span>Joins as {ROLE_LABEL[invitation.role].toLowerCase()}</span>
                </div>
                <span class="members-meta">Until {untilDate(invitation.expiresAt)}</span>
                <button
                  type="button"
                  class="members-danger"
                  disabled={!props.interactive}
                  data-testid="invitation-revoke"
                  aria-label={`Revoke the invitation for ${address(invitation)}`}
                  onClick={() => setRevoking(invitation)}
                >
                  Revoke
                </button>
              </li>
            )}
          </For>
        </ul>
      </Show>
      <ConfirmDialog
        open={revoking() !== null}
        variant="destructive"
        title={`Revoke the invitation for ${address(revoking())}?`}
        description="Its link stops working at once. You can invite them again later."
        confirmLabel="Revoke"
        onCancel={() => setRevoking(null)}
        onConfirm={() => {
          const invitation = revoking();
          setRevoking(null);
          if (invitation) void revoke({ id: invitation.id });
        }}
      />
    </ProfileCard>
  );
}

const INVITE_ERRORS: Record<string, string> = {
  account_exists: "Someone with that email already has an account here.",
  invalid_email: "That doesn't look like an email address.",
  rate_limited: "Too many invitations just now. Wait a minute, then try again.",
  role_required: "Your role doesn't allow inviting someone as that.",
};

/** The edge's wait, rounded up to whole minutes, in plain words. */
function waitCopy(seconds: number | undefined): string {
  if (!seconds) return "later";
  const minutes = Math.ceil(seconds / 60);
  return minutes === 1 ? "in a minute" : `in ${minutes} minutes`;
}

function inviteError(result: InvitationResult): string {
  if (result.ok) return "";
  if (result.reason === "recipient_rate_limited") {
    return `That address has had several invitations recently. Try again ${waitCopy(result.retryAfterSecs)}.`;
  }
  return INVITE_ERRORS[result.reason] ?? "The link couldn't be created. Try again in a moment.";
}

/** Invite a teammate. With mail on Myelin emails them the link; otherwise it is copied by hand. */
function InviteCard(props: { role: OrgRole }) {
  const inviting = useSubmission(createInvitation);
  const config = createAsync(() => getAuthConfig());
  const emailed = () => config()?.mail_delivery_enabled === true;
  const grantable = () => grantableRoles(props.role);
  const toast = useToast();
  const copy = async (url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      toast.show({ title: "Invitation link copied", variant: "success" });
    } catch {
      toast.show({ title: "Couldn't copy. Select the link and copy it by hand.", variant: "danger" });
    }
  };
  return (
    <ProfileCard
      icon="team"
      title="Invite a teammate"
      copy={emailed()
        ? "Myelin emails one person a link to join your organization."
        : "Create a link that lets one person join your organization. Send it to them yourself; Myelin doesn't email it."}
    >
      <form action={createInvitation} method="post" class="profile-invite-form">
        <div>
          <label>
            <span>Their email</span>
            <input name="email" type="email" required autocomplete="off" data-testid="invite-email" />
          </label>
          <Show when={grantable().length > 1} fallback={<input type="hidden" name="role" value="member" />}>
            <label class="members-invite-role">
              <span>Joins as</span>
              <select name="role" data-testid="invite-role">
                <For each={[...grantable()].reverse()}>{(role) => <option value={role}>{ROLE_LABEL[role]}</option>}</For>
              </select>
            </label>
          </Show>
          <button type="submit" data-testid="invite-submit" aria-busy={inviting.pending ? "true" : undefined}>
            {inviting.pending ? (emailed() ? "Sending…" : "Creating…") : emailed() ? "Send invitation" : "Create link"}
          </button>
        </div>
      </form>
      <Show when={inviting.result} keyed>
        {(result) => (
          <Show
            when={result.ok ? result : undefined}
            keyed
            fallback={
              <p role="alert" class="profile-invite-error" data-testid="invite-error">
                {inviteError(result)}
              </p>
            }
          >
            {(created) => (
              <div role="status" class="profile-invite-result" data-testid="invite-result">
                <Show
                  when={created.delivery === "link" ? created : undefined}
                  keyed
                  fallback={<p data-testid="invite-sent">We're emailing an invitation to <strong>{created.email}</strong> to join as {ROLE_LABEL[created.role].toLowerCase()}. The link in it works once, until {untilDate(created.expiresAt)}.</p>}
                >
                  {(link) => (
                    <>
                      <p>Link for <strong>{link.email}</strong> to join as {ROLE_LABEL[created.role].toLowerCase()}. It works once, until {untilDate(link.expiresAt)}.</p>
                      <div>
                        <input readonly aria-label="Invitation link" value={link.url} data-testid="invite-url" onFocus={(event) => event.currentTarget.select()} />
                        <button type="button" onClick={() => void copy(link.url)}>Copy</button>
                      </div>
                    </>
                  )}
                </Show>
              </div>
            )}
          </Show>
        )}
      </Show>
    </ProfileCard>
  );
}
