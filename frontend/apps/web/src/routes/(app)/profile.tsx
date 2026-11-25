import { Icon, useToast, type IconName } from "@myelin/design-system";
import { Title } from "@solidjs/meta";
import { createAsync, useAction, useSubmission } from "@solidjs/router";
import { createSignal, For, onMount, Show, type JSX } from "solid-js";
import { useAppViewer } from "~/components/AppShell";
import { createInvitation, getAuthConfig, logout } from "~/lib/auth";
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
        <ProfileCard icon="approve" title="Session security" copy="Your capability stays in Myelin’s encrypted server-side session. Browser JavaScript never receives it."><ul class="profile-security-list"><li><Icon name="check-pass" /> Edge identity verified for this session</li><li><Icon name="check-pass" /> Tenant and region bound to every request</li><li><Icon name="check-pass" /> Unsafe requests require same-origin verification</li></ul><button type="button" class="profile-signout" disabled={!interactive()} onClick={() => void signOut()}><Icon name="close" /> Sign out of Myelin</button></ProfileCard>
      </div>
    </div>
  </>;
}

const INVITE_ERRORS: Record<string, string> = {
  account_exists: "Someone with that email already has an account here.",
  invalid_email: "That doesn't look like an email address.",
  rate_limited: "Too many invitations just now. Wait a minute, then try again.",
};

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
                {INVITE_ERRORS[result.ok ? "" : result.reason] ?? "The link couldn't be created. Try again in a moment."}
              </p>
            }
          >
            {(created) => (
              <div role="status" class="profile-invite-result" data-testid="invite-result">
                <Show
                  when={created.delivery === "link" ? created : undefined}
                  keyed
                  fallback={<p data-testid="invite-sent">Invitation sent to <strong>{created.email}</strong>. The link in it works once, until {untilDate(created.expiresAt)}.</p>}
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

function ProfileCard(props: { icon: IconName; title: string; copy: string; children?: JSX.Element }) {
  return <section class="profile-card"><header><Icon name={props.icon} /><div><h2>{props.title}</h2><p>{props.copy}</p></div></header><div class="profile-card-content">{props.children}</div></section>;
}
