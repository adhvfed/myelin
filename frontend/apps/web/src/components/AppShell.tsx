// THE APP SHELL (doc 10 §7 / doc 08 §7) — the slot-based layout frame the whole UI hangs in.
//
// Chrome it owns, ONCE: the brand + the ⌘K command-palette trigger + the residency cue + the inbox
// affordance + the identity menu in the header; the fixed icon NAV rail; a secondary-nav slot; and the
// fluid main slot (the `min-height:0` scroll container). Global ⌘K opens the palette from anywhere.
// Built from the MR-016 design-system (<Icon>, semantic tokens) + the MR-017 overlays (Dialog/Menu/
// Toast); semantic-tokens-only; a11y per the design manual (landmarks, skip link, aria-current).
import { For, Show, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { A, useAction, useLocation, useNavigate } from "@solidjs/router";
import { Icon, Menu, Dialog, useToast, type IconName, type MenuItemSpec } from "@myelin/design-system";
import { logout, type Viewer } from "../lib/auth";
import { CommandPalette, type Command } from "./CommandPalette";

interface NavItem {
  href: string;
  icon: IconName;
  label: string;
  /** An unbuilt destination (R3.4 honest rail): muted + a neutral "soon" dot + a title tooltip, still
   *  a real keyboard-reachable link that lands on the teaching NotAvailable. Never disabled, never
   *  accent, never colour-alone (the tooltip + destination copy carry the meaning). */
  soon?: boolean;
}

// The primary subsystem rail. Only Code is wired to a real screen; the rest are the shell's declared
// destinations (their surfaces land with each subsystem track) — present, HONEST about being unbuilt:
// reachable "soon" links, not dead/disabled icons (the rail-honesty argument, §6).
const NAV: NavItem[] = [
  { href: "/git/repos", icon: "nav-code", label: "Code" },
  { href: "/issues", icon: "nav-issues", label: "Issues", soon: true },
  { href: "/chat", icon: "nav-chat", label: "Chat", soon: true },
  { href: "/ci", icon: "nav-ci", label: "CI", soon: true },
  { href: "/knowledge", icon: "nav-knowledge", label: "Knowledge", soon: true },
];

const THEMES = ["dark", "light", "high-contrast"] as const;

export interface AppShellProps {
  viewer: Viewer;
  /** The page's secondary navigation (rendered in the secondary-nav slot). */
  secondaryNav?: JSX.Element;
  children?: JSX.Element;
}

export function AppShell(props: AppShellProps) {
  const location = useLocation();
  const navigate = useNavigate();
  const toast = useToast();
  const doLogout = useAction(logout);

  // The repo slug of the current per-repo route (`/git/repos/{repo}/…`), for the repo-scoped palette
  // entry. `undefined` off a repo route.
  const currentRepo = (): string | undefined => {
    const m = /^\/git\/repos\/([^/]+)/.exec(location.pathname);
    return m?.[1];
  };

  const [paletteOpen, setPaletteOpen] = createSignal(false);
  const [inboxOpen, setInboxOpen] = createSignal(false);

  const cycleTheme = () => {
    const el = document.documentElement;
    const current = (el.dataset.theme as (typeof THEMES)[number]) ?? "dark";
    const next = THEMES[(THEMES.indexOf(current) + 1) % THEMES.length] ?? "dark";
    el.dataset.theme = next;
    toast.show({ title: `Theme: ${next}`, variant: "info" });
  };

  // Global ⌘K (and Ctrl+K) — open the palette from anywhere in the shell.
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen(true);
      }
    };
    document.addEventListener("keydown", onKey);
    onCleanup(() => document.removeEventListener("keydown", onKey));
  });

  const commands = (): Command[] => [
    ...NAV.map((n) => ({
      id: `nav:${n.href}`,
      label: `Go to ${n.label}`,
      icon: n.icon,
      run: () => {
        window.location.assign(n.href);
      },
    })),
    // R3.1 — PR palette entries (client-side navigation via useNavigate, never window.location).
    ...(currentRepo()
      ? [
          {
            id: "pr:repo",
            label: "Go to pull requests",
            icon: "pull-request" as IconName,
            run: () => navigate(`/git/repos/${currentRepo()}/prs`),
          },
        ]
      : []),
    { id: "pr:needs-review", label: "Pull requests needing my review", icon: "pull-request", run: () => navigate("/prs?bucket=needs-review") },
    { id: "pr:mine", label: "My pull requests", icon: "pull-request", run: () => navigate("/prs?bucket=yours") },
    { id: "inbox", label: "Open inbox", icon: "inbox", run: () => setInboxOpen(true) },
    { id: "theme", label: "Toggle theme", icon: "settings", run: cycleTheme },
    { id: "logout", label: "Sign out", icon: "human", run: () => void doLogout() },
  ];

  const identityItems: MenuItemSpec[] = [
    { label: "Profile", icon: "human", onSelect: () => toast.show({ title: "Profile is a later surface", variant: "info" }) },
    { label: "Toggle theme", icon: "settings", onSelect: cycleTheme },
    { label: "Sign out", icon: "close", onSelect: () => void doLogout() },
  ];

  return (
    <>
      <a class="skip-link" href="#main">Skip to content</a>
      <div
        class="app-shell"
        style={{
          display: "grid",
          "grid-template-columns": "auto 1fr",
          "grid-template-rows": "auto 1fr",
          "min-height": "0",
        }}
      >
        {/* Header (spans both columns): brand · ⌘K trigger · residency cue · inbox · identity. */}
        <header
          style={{
            "grid-column": "1 / 3",
            display: "flex",
            "align-items": "center",
            gap: "var(--space-3)",
            padding: "var(--space-2) var(--space-3)",
            "border-block-end": "var(--hairline) solid var(--border)",
            background: "var(--surface-raised)",
            "z-index": "var(--z-chrome)",
          }}
        >
          <strong style={{ "font-size": "var(--fs-h3)", "letter-spacing": "0.02em" }}>Myelin</strong>

          {/* The ⌘K command-palette trigger — looks like search, opens the palette. */}
          <button
            type="button"
            class="cmdk-trigger"
            onClick={() => setPaletteOpen(true)}
            aria-keyshortcuts="Meta+K Control+K"
            aria-haspopup="dialog"
            style={{
              display: "flex",
              "align-items": "center",
              gap: "var(--space-2)",
              flex: "1",
              "max-width": "28rem",
              padding: "var(--space-2) var(--space-3)",
              border: "var(--hairline) solid var(--border)",
              "border-radius": "var(--radius-1)",
              cursor: "pointer",
              "text-align": "start",
            }}
          >
            <Icon name="search" />
            <span style={{ flex: "1" }}>Search or run a command</span>
            <kbd style={{ "font-family": "var(--font-mono)", "font-size": "var(--fs-caption)" }}>⌘K</kbd>
          </button>

          <div style={{ flex: "1" }} />

          <ResidencyCue region={props.viewer.region} tenant={props.viewer.tenant} />

          {/* Inbox affordance — glyph + visible unread count (never color-only; WCAG 1.4.1). */}
          <button
            type="button"
            class="inbox-button"
            onClick={() => setInboxOpen(true)}
            aria-haspopup="dialog"
            aria-label="Inbox, 2 unread"
            style={{
              display: "inline-flex",
              "align-items": "center",
              gap: "var(--space-1)",
              padding: "var(--space-2)",
              border: "var(--hairline) solid var(--border)",
              "border-radius": "var(--radius-1)",
              cursor: "pointer",
            }}
          >
            <Icon name="inbox" />
            <span
              aria-hidden="true"
              style={{
                "min-width": "1.1rem",
                "text-align": "center",
                "font-size": "var(--fs-caption)",
                background: "var(--accent)",
                color: "var(--on-accent)",
                "border-radius": "var(--radius-pill)",
                padding: "0 var(--space-1)",
              }}
            >
              2
            </span>
          </button>

          <Menu
            label="Account menu"
            placement="bottom-end"
            triggerLabel={
              <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-2)" }}>
                <Icon name="human" />
                <span>{props.viewer.displayName}</span>
                <Icon name="chevron" />
              </span>
            }
            items={identityItems}
          />
        </header>

        {/* The fixed icon NAV rail. */}
        <nav
          aria-label="Primary"
          style={{
            "grid-row": "2",
            display: "flex",
            "flex-direction": "column",
            gap: "var(--space-1)",
            padding: "var(--space-2)",
            "border-inline-end": "var(--hairline) solid var(--border)",
            background: "var(--surface-raised)",
            // doc 10 §1 names the nav aside as a transformed ancestor — proving the overlays MUST
            // portal to body to anchor correctly.
            transform: "translateZ(0)",
          }}
        >
          <For each={NAV}>
            {(item) => {
              const isActive = () =>
                location.pathname === item.href || location.pathname.startsWith(item.href + "/");
              return (
                <A
                  href={item.href}
                  class={item.soon ? "nav-rail-item soon" : "nav-rail-item"}
                  aria-label={item.soon ? `${item.label} (coming soon)` : item.label}
                  title={item.soon ? `${item.label} — coming soon` : undefined}
                  aria-current={isActive() ? "page" : undefined}
                  // Colour/active/hover come from the .nav-rail-item class (surface-hover fill +
                  // brighter text, no accent fill — R1 binding). `.soon` mutes it + shows a neutral
                  // dot. Only layout stays inline.
                  style={{
                    position: "relative",
                    display: "flex",
                    "align-items": "center",
                    "justify-content": "center",
                    width: "2.25rem",
                    height: "2.25rem",
                    "border-radius": "var(--radius-1)",
                  }}
                >
                  <Icon name={item.icon} title={item.label} />
                  <Show when={item.soon}>
                    <span class="soon-dot" aria-hidden="true" />
                  </Show>
                </A>
              );
            }}
          </For>
        </nav>

        {/* Secondary-nav slot + the fluid main slot (the min-height:0 scroll container). */}
        <div
          style={{
            "grid-row": "2",
            display: "grid",
            "grid-template-columns": props.secondaryNav ? "14rem 1fr" : "1fr",
            "min-height": "0",
            "min-width": "0",
          }}
        >
          <Show when={props.secondaryNav}>
            <aside
              aria-label="Secondary"
              style={{
                "border-inline-end": "var(--hairline) solid var(--border)",
                padding: "var(--space-3)",
                overflow: "auto",
              }}
            >
              {props.secondaryNav}
            </aside>
          </Show>
          <main
            id="main"
            tabindex="-1"
            style={{ "min-height": "0", "min-width": "0", overflow: "auto", padding: "var(--space-5)" }}
          >
            {props.children}
          </main>
        </div>
      </div>

      <CommandPalette open={paletteOpen()} onClose={() => setPaletteOpen(false)} commands={commands()} />

      <Dialog
        open={inboxOpen()}
        onClose={() => setInboxOpen(false)}
        title="Inbox"
        description="Notifications arrive here. Live delivery (SSE) is a later wiring."
        size="sm"
      >
        <ul style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
          <li style={{ display: "flex", gap: "var(--space-2)", "align-items": "center" }}>
            <Icon name="pull-request" />
            {/* R3.1 — the inbox PR row is now a real link into the cross-repo "needs review" bucket.
                (A per-item deep-link to the specific PR lands when the inbox is data-driven — floor.) */}
            <A href="/prs?bucket=needs-review" onClick={() => setInboxOpen(false)} style={{ color: "var(--text-primary)" }}>
              A pull request needs your review
            </A>
          </li>
          <li style={{ display: "flex", gap: "var(--space-2)", "align-items": "center" }}>
            <Icon name="check-pass" />
            <span>CI passed on acme/myelin</span>
          </li>
        </ul>
      </Dialog>
    </>
  );
}

function ResidencyCue(props: { region: string; tenant: string }) {
  // The data-region indicator the design manual specifies — glyph + TEXT label (never color-only), so
  // the operator always knows which residency region their data lives in (a sovereignty-as-UX cue).
  return (
    <span
      title={`Tenant ${props.tenant}`}
      style={{
        display: "inline-flex",
        "align-items": "center",
        gap: "var(--space-1)",
        padding: "var(--space-1) var(--space-2)",
        border: "var(--hairline) solid var(--border)",
        "border-radius": "var(--radius-pill)",
        color: "var(--text-muted)",
        "font-size": "var(--fs-caption)",
      }}
    >
      <Icon name="database" />
      <span>Data region:</span>
      <strong style={{ color: "var(--text-primary)" }}>{props.region}</strong>
    </span>
  );
}
