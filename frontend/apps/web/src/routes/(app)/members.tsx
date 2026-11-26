import { ConfirmDialog, Icon, useToast } from "@myelin/design-system";
import { Title } from "@solidjs/meta";
import { createAsync, useAction, useSubmission } from "@solidjs/router";
import { createEffect, createSignal, For, Match, onMount, Show, Switch } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { useAppViewer } from "~/components/AppShell";
import { ProfileCard } from "~/components/ProfileCard";
import { getAuthConfig } from "~/lib/auth";
import type { Member, PendingInvitation, Roster } from "~/lib/member-roster";
import { grantableRoles, mayActOn, ROLE_LABEL, roleChoices, type OrgRole } from "~/lib/member-rules";
import {
  changeRole,
  createInvitation,
  getRoster,
  removeMember,
  revokeInvitation,
  type InvitationResult,
} from "~/lib/members";
import type { MemberResult } from "~/server/members-gateway";

const longDate = (unix: number) =>
  new Date(unix * 1_000).toLocaleDateString(undefined, { day: "numeric", month: "long", year: "numeric" });
const untilDate = (unix: number) =>
  new Date(unix * 1_000).toLocaleDateString(undefined, { day: "numeric", month: "long" });

const MEMBER_ERRORS: Record<Exclude<MemberResult, { ok: true }>["reason"], string> = {
  role_required: "Your role doesn't allow that.",
  forbidden: "This session can't manage people. Sign out and sign in again.",
  last_owner: "An organization needs an owner. Make someone else an owner first.",
  not_found: "They're no longer here. The list has been refreshed.",
  unavailable: "That didn't work just now. Try again in a moment.",
};

export default function Members() {
  const viewer = useAppViewer();
  const view = createAsync(() => getRoster());
  // Each refresh is reconciled into one store, so rows, focus and half-typed forms survive it.
  const [roster, setRoster] = createStore<{ current?: Roster }>({});
  createEffect(() => {
    const latest = view();
    if (latest?.state === "ready") setRoster("current", reconcile(latest.roster, { key: "id" }));
  });
  const [interactive, setInteractive] = createSignal(false);
  onMount(() => setInteractive(true));
  return <>
    <Title>Members · Myelin</Title>
    <div class="profile-screen">
      <header class="profile-hero"><div class="profile-avatar" aria-hidden="true"><Icon name="team" size={30} /></div><div><p>Organization</p><h1>Members</h1><span><code>{viewer.tenant}</code></span></div></header>
      <Switch>
        <Match when={view()?.state === "not_member"}>
          <p class="members-note" data-testid="members-not-member">Only members of this organization can see its people.</p>
        </Match>
        <Match when={view()?.state === "forbidden"}>
          <p role="alert" class="members-note" data-testid="members-forbidden">This session can't see the organization's people. Sign out and sign in again.</p>
        </Match>
        <Match when={view()?.state === "unavailable"}>
          <p role="alert" class="members-note" data-testid="members-unavailable">The members list couldn't be loaded. Try again in a moment.</p>
        </Match>
        <Match when={view()?.state === "ready" && roster.current}>
          <RosterView roster={roster.current!} viewerId={viewer.principalId} interactive={interactive()} />
        </Match>
      </Switch>
    </div>
  </>;
}

/** A pending confirmation. It stays set while the dialog closes, so the dialog's words don't
 * change under the closing animation. */
interface Confirmation {
  title: string;
  description: string;
  confirmLabel: string;
  destructive: boolean;
  run: () => Promise<boolean>;
}

/** One confirm dialog for a card, and a guard so a double click acts once. */
function useConfirmation() {
  const [pending, setPending] = createSignal<Confirmation | null>(null);
  const [open, setOpen] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const ask = (confirmation: Confirmation) => {
    setPending(confirmation);
    setOpen(true);
  };
  const dialog = () => (
    <ConfirmDialog
      open={open()}
      variant={pending()?.destructive ? "destructive" : "confirm"}
      title={pending()?.title ?? ""}
      description={pending()?.description ?? ""}
      confirmLabel={pending()?.confirmLabel}
      onCancel={() => setOpen(false)}
      onConfirm={() => {
        const confirmation = pending();
        setOpen(false);
        if (!confirmation || busy()) return;
        setBusy(true);
        void confirmation.run().finally(() => setBusy(false));
      }}
    />
  );
  return { ask, busy, dialog };
}

/** Runs a member action and reports a failure as a toast; success shows in the refreshed list. */
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

function RosterView(props: { roster: Roster; viewerId: string; interactive: boolean }) {
  return (
    <div class="members-grid">
      <Show when={props.roster.owners === 0}>
        <p role="status" class="members-note members-warning" data-testid="members-no-owner">
          This organization has no owner. Ask the operator to run <code>edge role</code>.
        </p>
      </Show>
      <Show
        when={props.roster.role !== "member"}
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

const nameOf = (member: Member) => member.displayName || member.email || "Unnamed person";

function PeopleCard(props: { roster: Roster; viewerId: string; interactive: boolean }) {
  const setRole = useMemberAction(useAction(changeRole), "Role changed");
  const remove = useMemberAction(useAction(removeMember), "Removed from the organization");
  const confirmation = useConfirmation();
  let list: HTMLUListElement | undefined;
  const soleOwner = (member: Member) =>
    member.id === props.viewerId && member.role === "owner" && props.roster.owners <= 1;

  const confirmRole = (member: Member, role: OrgRole) => {
    const self = member.id === props.viewerId;
    confirmation.ask({
      title: self ? `Make yourself ${ROLE_LABEL[role].toLowerCase()}?` : `Make ${nameOf(member)} ${ROLE_LABEL[role].toLowerCase()}?`,
      description: self
        ? "You'll lose what your current role lets you do, and only an owner can give it back."
        : `${nameOf(member)} becomes ${ROLE_LABEL[role].toLowerCase()} from their next click.`,
      confirmLabel: "Change role",
      destructive: false,
      run: () => setRole({ id: member.id, role }),
    });
  };
  const confirmRemove = (member: Member) => {
    confirmation.ask({
      title: member.id === props.viewerId ? "Remove yourself?" : `Remove ${nameOf(member)}?`,
      description: member.id === props.viewerId
        ? "You'll be signed out everywhere and won't be able to sign in to this organization again."
        : "They'll be signed out everywhere at once and won't be able to sign in to this organization again.",
      confirmLabel: "Remove",
      destructive: true,
      run: async () => {
        const ok = await remove({ id: member.id });
        if (ok) list?.focus();
        return ok;
      },
    });
  };

  return (
    <ProfileCard
      icon="team"
      title="People"
      copy="Owners manage everyone. Admins invite and manage members. Members see who's here."
    >
      <ul class="members-list" data-testid="members-people" ref={list} tabIndex={-1} aria-label="People">
        <For each={props.roster.people}>
          {(member) => {
            const isSelf = () => member.id === props.viewerId;
            const choices = () => roleChoices(props.roster.role, member.role, isSelf());
            const [chosen, setChosen] = createSignal<OrgRole>(member.role);
            createEffect(() => setChosen(member.role));
            const hintId = `sole-owner-${member.id}`;
            return (
              <li data-testid="member-row" data-member-email={member.email}>
                <div class="members-who">
                  <strong title={nameOf(member)}>{nameOf(member)}{isSelf() ? " (you)" : ""}</strong>
                  <span title={member.email}>{member.email}</span>
                  <Show when={soleOwner(member)}>
                    <span id={hintId} class="members-hint" data-testid="sole-owner-hint">You're the only owner. Make someone else an owner before you step down.</span>
                  </Show>
                </div>
                <span class="members-meta">Joined {longDate(member.joinedAt)}</span>
                <Show
                  when={choices().length > 1}
                  fallback={<span class="members-role" data-testid="member-role-text">{ROLE_LABEL[member.role]}</span>}
                >
                  <div class="members-role-control">
                    <select
                      aria-label={`Role for ${nameOf(member)}`}
                      aria-describedby={soleOwner(member) ? hintId : undefined}
                      data-testid="member-role"
                      disabled={!props.interactive || soleOwner(member)}
                      value={chosen()}
                      onChange={(event) => setChosen(event.currentTarget.value as OrgRole)}
                    >
                      <For each={choices()}>{(role) => <option value={role}>{ROLE_LABEL[role]}</option>}</For>
                    </select>
                    <Show when={chosen() !== member.role}>
                      <button
                        type="button"
                        class="members-save"
                        data-testid="member-role-save"
                        disabled={confirmation.busy()}
                        onClick={() => confirmRole(member, chosen())}
                      >
                        Save
                      </button>
                    </Show>
                  </div>
                </Show>
                <Show when={mayActOn(props.roster.role, member.role, isSelf())}>
                  <button
                    type="button"
                    class="profile-signout"
                    disabled={!props.interactive || confirmation.busy() || soleOwner(member)}
                    aria-describedby={soleOwner(member) ? hintId : undefined}
                    data-testid="member-remove"
                    aria-label={`Remove ${nameOf(member)}`}
                    onClick={() => confirmRemove(member)}
                  >
                    Remove
                  </button>
                </Show>
              </li>
            );
          }}
        </For>
      </ul>
      {confirmation.dialog()}
    </ProfileCard>
  );
}

function InvitationsCard(props: { invitations: PendingInvitation[]; interactive: boolean }) {
  const revoke = useMemberAction(useAction(revokeInvitation), "Invitation revoked");
  const confirmation = useConfirmation();
  let region: HTMLDivElement | undefined;
  const confirmRevoke = (invitation: PendingInvitation) => {
    confirmation.ask({
      title: invitation.email ? `Revoke the invitation for ${invitation.email}?` : "Revoke this invitation?",
      description: "Its link stops working at once, and its email stops if it hasn't gone out. You can invite them again later.",
      confirmLabel: "Revoke",
      destructive: true,
      run: async () => {
        const ok = await revoke({ id: invitation.id });
        if (ok) region?.focus();
        return ok;
      },
    });
  };
  return (
    <ProfileCard
      icon="inbox"
      title="Pending invitations"
      copy="Links sent but not yet used. Revoking one stops its link working, and stops its email if it hasn't gone out."
    >
      <div ref={region} tabIndex={-1} aria-label="Pending invitations" class="members-region">
        <Show
          when={props.invitations.length > 0}
          fallback={<p class="members-empty" data-testid="invitations-empty">No invitations are waiting.</p>}
        >
          <ul class="members-list" data-testid="members-invitations">
            <For each={props.invitations}>
              {(invitation) => (
                <li class="members-invitation" data-testid="invitation-row" data-invitation-email={invitation.email ?? ""}>
                  <div class="members-who">
                    <strong title={invitation.email ?? undefined}>{invitation.email ?? "Address not recorded"}</strong>
                    <span>Joins as {ROLE_LABEL[invitation.role].toLowerCase()}</span>
                  </div>
                  <span class="members-meta">Until {untilDate(invitation.expiresAt)}</span>
                  <button
                    type="button"
                    class="profile-signout"
                    disabled={!props.interactive || confirmation.busy()}
                    data-testid="invitation-revoke"
                    aria-label={invitation.email ? `Revoke the invitation for ${invitation.email}` : "Revoke this invitation"}
                    onClick={() => confirmRevoke(invitation)}
                  >
                    Revoke
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>
      {confirmation.dialog()}
    </ProfileCard>
  );
}

const INVITE_ERRORS: Record<string, string> = {
  account_exists: "Someone with that email already has an account here.",
  invalid_email: "That doesn't look like an email address.",
  invalid_role: "Choose owner, admin or member.",
  invitation_exists: "That address already has an invitation only an owner can replace. Ask an owner, or revoke it first.",
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
          <button type="submit" data-testid="invite-submit" disabled={inviting.pending} aria-busy={inviting.pending ? "true" : undefined}>
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
