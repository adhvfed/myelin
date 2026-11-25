import { Icon, useToast, type IconName } from "@myelin/design-system";
import { Title } from "@solidjs/meta";
import { createAsync, useAction, useSubmission } from "@solidjs/router";
import { createEffect, createSignal, For, onMount, Show, type JSX } from "solid-js";
import { useAppViewer } from "~/components/AppShell";
import {
  changePassword,
  createInvitation,
  getAuthConfig,
  logout,
  logoutEverywhere,
  type InvitationResult,
  type PasswordChangeResult,
} from "~/lib/auth";
import { applyTheme, restoreTheme, THEMES, type Theme } from "~/lib/theme";

const THEME_COPY: Record<Theme, { label: string; copy: string }> = {
  dark: { label: "Dark", copy: "Focused, low-glare workspace" },
  light: { label: "Light", copy: "Bright surfaces and crisp contrast" },
  "high-contrast": { label: "High contrast", copy: "Maximum edge and text separation" },
};

export default function Profile() {
  const viewer = useAppViewer();
  const signOut = useAction(logout);
  const toast = useToast();
  const [theme, setTheme] = createSignal<Theme>("dark");
  const [interactive, setInteractive] = createSignal(false);
  onMount(() => { setTheme(restoreTheme()); setInteractive(true); });

  const chooseTheme = (next: Theme) => {
    setTheme(applyTheme(next));
    toast.show({ title: `Appearance set to ${THEME_COPY[next].label}`, variant: "success" });
  };

  return <>
    <Title>Profile · Myelin</Title>
    <div class="profile-screen">
      <header class="profile-hero"><div class="profile-avatar" aria-hidden="true"><Icon name="human" size={30} /></div><div><p>Signed-in identity</p><h1>{viewer.displayName}</h1><span>Engineering workspace member</span></div></header>
      <div class="profile-grid">
        <ProfileCard icon="human" title="Identity" copy="The identity Myelin uses for attribution and authorization."><dl><div><dt>Display name</dt><dd>{viewer.displayName}</dd></div><div><dt>Principal</dt><dd><code>{viewer.principalId}</code></dd></div></dl></ProfileCard>
        <ProfileCard icon="team" title="Workspace" copy="Your current organization boundary. Data from another tenant is never mixed into this session."><dl><div><dt>Tenant</dt><dd><code>{viewer.tenant}</code></dd></div><div><dt>Role</dt><dd>Member</dd></div></dl></ProfileCard>
        <ProfileCard icon="gate" title="Data residency" copy="Myelin keeps execution and durable product data inside the assigned region."><dl><div><dt>Active region</dt><dd><strong>{viewer.region}</strong></dd></div><div><dt>Residency</dt><dd>EU sovereign boundary</dd></div></dl></ProfileCard>
        <ProfileCard icon="settings" title="Appearance" copy="Choose a mode for this browser. The setting persists without changing your organization profile."><div class="profile-theme-options" role="radiogroup" aria-label="Appearance"><For each={THEMES}>{(option) => <button type="button" role="radio" aria-checked={theme() === option} disabled={!interactive()} onClick={() => chooseTheme(option)}><span class={`profile-theme-swatch ${option}`} aria-hidden="true" /><strong>{THEME_COPY[option].label}</strong><small>{THEME_COPY[option].copy}</small></button>}</For></div></ProfileCard>
        <InviteCard />
        <PasswordCard interactive={interactive()} />
        <ProfileCard icon="approve" title="Session security" copy="Your capability stays in Myelin’s encrypted server-side session. Browser JavaScript never receives it."><ul class="profile-security-list"><li><Icon name="check-pass" /> Edge identity verified for this session</li><li><Icon name="check-pass" /> Tenant and region bound to every request</li><li><Icon name="check-pass" /> Unsafe requests require same-origin verification</li></ul><div class="profile-signout-actions"><button type="button" class="profile-signout" disabled={!interactive()} onClick={() => void signOut()}><Icon name="close" /> Sign out of Myelin</button><SignOutEverywhere disabled={!interactive()} /></div></ProfileCard>
      </div>
    </div>
  </>;
}

const INVITE_ERRORS: Record<string, string> = {
  account_exists: "Someone with that email already has an account here.",
  invalid_email: "That doesn't look like an email address.",
  rate_limited: "Too many invitations just now. Wait a minute, then try again.",
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

const untilDate = (unix: number) =>
  new Date(unix * 1_000).toLocaleDateString(undefined, { day: "numeric", month: "long" });

/** Invite a teammate. With mail on Myelin emails them the link; otherwise it is copied by hand. */
function InviteCard() {
  const inviting = useSubmission(createInvitation);
  const config = createAsync(() => getAuthConfig());
  const emailed = () => config()?.mail_delivery_enabled === true;
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
                  fallback={<p data-testid="invite-sent">We're emailing an invitation to <strong>{created.email}</strong>. The link in it works once, until {untilDate(created.expiresAt)}.</p>}
                >
                  {(link) => (
                    <>
                      <p>Link for <strong>{link.email}</strong>. It works once, until {untilDate(link.expiresAt)}.</p>
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

const MIN_PASSWORD_LENGTH = 10;

const PASSWORD_ERRORS: Record<string, string> = {
  wrong_password: "That isn't your current password.",
  weak_password: `The new password is too short. Use at least ${MIN_PASSWORD_LENGTH} characters.`,
  password_mismatch: "The two new passwords don't match.",
  rate_limited: "Too many attempts. Wait a few minutes, then try again.",
  no_password: "You sign in with single sign-on, so there's no Myelin password to change.",
  password_changed_elsewhere:
    "Your password was just changed somewhere else, so it wasn't changed here.",
};

/** The field each failure is about; the rest are about the form as a whole. */
const PASSWORD_ERROR_FIELD: Record<string, "current" | "new" | "confirm"> = {
  wrong_password: "current",
  weak_password: "new",
  password_mismatch: "confirm",
};

function passwordError(result: PasswordChangeResult): string {
  if (result.ok) return "";
  return PASSWORD_ERRORS[result.reason] ?? "The password couldn't be changed. Try again in a moment.";
}

/** Change the password. Every other session ends; this browser stays signed in. The form works
 * only once the page is interactive: submitted without JavaScript, SolidStart would echo the
 * passwords back in a cookie. */
function PasswordCard(props: { interactive: boolean }) {
  const changing = useSubmission(changePassword);
  const config = createAsync(() => getAuthConfig());
  const toast = useToast();
  const [length, setLength] = createSignal(0);
  let form: HTMLFormElement | undefined;
  const failure = () => {
    const result = changing.result;
    return result && !result.ok ? result.reason : undefined;
  };
  /** A failure about one field describes that field; a failure about the form describes all. */
  const describedBy = (field: "current" | "new" | "confirm", hint?: string) => {
    const reason = failure();
    const about = reason ? (PASSWORD_ERROR_FIELD[reason] ?? field) : undefined;
    return [hint, about === field ? "password-change-error" : undefined]
      .filter(Boolean)
      .join(" ") || undefined;
  };
  const invalid = (field: "current" | "new" | "confirm") => {
    const reason = failure();
    return reason && PASSWORD_ERROR_FIELD[reason] === field ? "true" : undefined;
  };
  createEffect(() => {
    if (changing.result?.ok && !changing.pending) {
      form?.reset();
      setLength(0);
      toast.show({ title: "Password changed. Your other sessions were signed out.", variant: "success" });
    }
  });
  return (
    <Show when={config()?.password_login_enabled}>
      <ProfileCard
        icon="gate"
        title="Password"
        copy="Changing it signs you out everywhere else. This browser stays signed in."
      >
        <form ref={form} action={changePassword} method="post" class="profile-password-form" data-testid="password-form">
          <label>
            <span>Current password</span>
            <input
              name="current_password"
              type="password"
              required
              autocomplete="current-password"
              aria-invalid={invalid("current")}
              aria-describedby={describedBy("current")}
              data-testid="current-password"
            />
          </label>
          <label>
            <span>New password</span>
            <input
              name="new_password"
              type="password"
              required
              minlength={MIN_PASSWORD_LENGTH}
              autocomplete="new-password"
              aria-describedby={describedBy("new", "new-password-hint")}
              aria-invalid={invalid("new")}
              data-testid="new-password"
              onInput={(event) => setLength([...event.currentTarget.value].length)}
            />
          </label>
          <p id="new-password-hint" class="profile-password-hint">
            <span>At least {MIN_PASSWORD_LENGTH} characters.</span>
            <span aria-hidden="true" data-met={length() >= MIN_PASSWORD_LENGTH ? "" : undefined}>
              {Math.min(length(), MIN_PASSWORD_LENGTH)} / {MIN_PASSWORD_LENGTH}
            </span>
          </p>
          <label>
            <span>Confirm new password</span>
            <input
              name="confirm_password"
              type="password"
              required
              autocomplete="new-password"
              aria-invalid={invalid("confirm")}
              aria-describedby={describedBy("confirm")}
              data-testid="confirm-password"
            />
          </label>
          <Show when={changing.result && !changing.result.ok ? changing.result : undefined} keyed>
            {(result) => (
              <p role="alert" id="password-change-error" class="profile-invite-error" data-testid="password-error">
                {passwordError(result)}
              </p>
            )}
          </Show>
          <button
            type="submit"
            disabled={!props.interactive || changing.pending}
            data-testid="password-submit"
            aria-busy={changing.pending ? "true" : undefined}
          >
            {changing.pending ? "Changing…" : "Change password"}
          </button>
        </form>
      </ProfileCard>
    </Show>
  );
}

/** Sign out of every device, this one included, after a short confirmation. */
function SignOutEverywhere(props: { disabled: boolean }) {
  const [confirming, setConfirming] = createSignal(false);
  const ending = useSubmission(logoutEverywhere);
  let opener: HTMLButtonElement | undefined;
  let confirm: HTMLButtonElement | undefined;
  const open = () => {
    ending.clear();
    setConfirming(true);
    confirm?.focus();
  };
  const cancel = () => {
    setConfirming(false);
    opener?.focus();
  };
  return (
    <Show
      when={confirming()}
      fallback={
        <button ref={opener} type="button" class="profile-signout" disabled={props.disabled} onClick={open} data-testid="signout-everywhere">
          <Icon name="close" /> Sign out of all devices
        </button>
      }
    >
      <form action={logoutEverywhere} method="post" class="profile-signout-confirm" data-testid="signout-everywhere-confirm" aria-labelledby="signout-everywhere-question">
        <p id="signout-everywhere-question">Sign out on every device, including this one?</p>
        <div>
          <button
            ref={confirm}
            type="submit"
            class="profile-signout"
            disabled={ending.pending}
            aria-busy={ending.pending ? "true" : undefined}
            data-testid="signout-everywhere-submit"
          >
            {ending.pending ? "Signing out…" : "Sign out everywhere"}
          </button>
          <button type="button" onClick={cancel}>Cancel</button>
        </div>
        <Show when={ending.result && !ending.result.ok}>
          <p role="alert" class="profile-invite-error" data-testid="signout-everywhere-error">
            Your other devices couldn't be signed out just now. Try again in a moment.
          </p>
        </Show>
      </form>
    </Show>
  );
}

function ProfileCard(props: { icon: IconName; title: string; copy: string; children?: JSX.Element }) {
  return <section class="profile-card"><header><Icon name={props.icon} /><div><h2>{props.title}</h2><p>{props.copy}</p></div></header><div class="profile-card-content">{props.children}</div></section>;
}
