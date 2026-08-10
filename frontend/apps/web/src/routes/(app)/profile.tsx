import { Icon, useToast, type IconName } from "@myelin/design-system";
import { Title } from "@solidjs/meta";
import { useAction } from "@solidjs/router";
import { createSignal, For, onMount, type JSX } from "solid-js";
import { useAppViewer } from "~/components/AppShell";
import { logout } from "~/lib/auth";
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
        <ProfileCard icon="approve" title="Session security" copy="Your capability stays in Myelin’s encrypted server-side session. Browser JavaScript never receives it."><ul class="profile-security-list"><li><Icon name="check-pass" /> Edge identity verified for this session</li><li><Icon name="check-pass" /> Tenant and region bound to every request</li><li><Icon name="check-pass" /> Unsafe requests require same-origin verification</li></ul><button type="button" class="profile-signout" disabled={!interactive()} onClick={() => void signOut()}><Icon name="close" /> Sign out of Myelin</button></ProfileCard>
      </div>
    </div>
  </>;
}

function ProfileCard(props: { icon: IconName; title: string; copy: string; children?: JSX.Element }) {
  return <section class="profile-card"><header><Icon name={props.icon} /><div><h2>{props.title}</h2><p>{props.copy}</p></div></header><div class="profile-card-content">{props.children}</div></section>;
}
