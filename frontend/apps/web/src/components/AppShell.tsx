// THE APP SHELL (doc 10 §7 / doc 08 §7) — the slot-based layout frame the whole UI hangs in.
//
// Chrome it owns, ONCE: the brand + the ⌘K command-palette trigger + the residency cue + the inbox
// affordance + the identity menu in the header; the fixed icon NAV rail; a secondary-nav slot; and the
// fluid main slot (the `min-height:0` scroll container). Global ⌘K opens the palette from anywhere.
// Built from the MR-016 design-system (<Icon>, semantic tokens) + the MR-017 overlays (Dialog/Menu/
// Toast); semantic-tokens-only; a11y per the design manual (landmarks, skip link, aria-current).
import {
  For,
  Show,
  createContext,
  createEffect,
  createSignal,
  onCleanup,
  onMount,
  useContext,
  type JSX,
} from "solid-js";
import { A, useAction, useLocation, useNavigate } from "@solidjs/router";
import { Icon, Menu, Dialog, useToast, type IconName, type MenuItemSpec } from "@myelin/design-system";
import { logout, type Viewer } from "../lib/auth";
import { createInbox } from "../lib/notifications";
import { CommandPalette, type Command } from "./CommandPalette";

/** The shell context a nested route uses to fill the shell-owned context-pane region (§1b). A route
 *  calls `setContextPane(() => <Pane/>)` in an effect (with `onCleanup(() => setContextPane(null))`);
 *  the shell renders it as its 4th region / drawer. This keeps the frame/breakpoint/drawer/landmark in
 *  the shell ONCE while letting any surface supply the content — without threading a prop through the
 *  pathless layout. */
interface ContextPaneApi {
  /** Supply the pane as a render THUNK (called fresh per location) so the inline column and the narrow
   *  drawer never share a DOM node. Pass `null` (in `onCleanup`) to drop the pane. */
  setContextPane: (render: (() => JSX.Element) | null) => void;
  setContextPaneLabel: (label: string) => void;
}
const ContextPaneContext = createContext<ContextPaneApi>();

/** Consume the shell's context-pane slot from a nested route (no-op setters off the shell). */
export function useContextPane(): ContextPaneApi {
  return (
    useContext(ContextPaneContext) ?? {
      setContextPane: () => {},
      setContextPaneLabel: () => {},
    }
  );
}

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

/** The MOB-2 breakpoint: at ≤ 1280px the context-pane column drops and the pane becomes a drawer. */
const PANE_DRAWER_MAX = 1280;

/** The body grid columns: [secondaryNav?] main [contextPane?]. The pane column is added only when a
 *  pane is present AND wide — otherwise it DROPS entirely (content never renders beside an empty
 *  gutter, §1b). */
/** True when a keystroke target is a text field / editable — bare-key shortcuts must not fire there. */
function isTypingTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  const tag = el.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || el.isContentEditable;
}

function paneColumns(hasSecondary: boolean, hasPaneColumn: boolean): string {
  const cols: string[] = [];
  if (hasSecondary) cols.push("14rem");
  cols.push("minmax(0, 1fr)");
  if (hasPaneColumn) cols.push("332px");
  return cols.join(" ");
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
  // MOB-2: wide (> 1280px) renders the pane as a 4th column; narrow renders it in a drawer. Default
  // wide so SSR renders the column; onMount installs the real matchMedia listener.
  const [paneWide, setPaneWide] = createSignal(true);
  const [paneDrawerOpen, setPaneDrawerOpen] = createSignal(false);
  onMount(() => {
    const mq = window.matchMedia(`(min-width: ${PANE_DRAWER_MAX + 1}px)`);
    const sync = () => setPaneWide(mq.matches);
    sync();
    mq.addEventListener("change", sync);
    onCleanup(() => mq.removeEventListener("change", sync));
  });
  // Route change auto-closes the drawer (NOTES §1b).
  createEffect(() => {
    location.pathname;
    setPaneDrawerOpen(false);
  });
  // The pane content a nested route supplies via `useContextPane()` (or the direct prop). The prop
  // wins when present; otherwise the context signal drives the region.
  const [ctxPaneThunk, setCtxPaneThunk] = createSignal<(() => JSX.Element) | null>(null);
  const [ctxPaneLabel, setCtxPaneLabel] = createSignal("Context");
  const paneApi: ContextPaneApi = {
    setContextPane: (render) => setCtxPaneThunk(() => render),
    setContextPaneLabel: (label) => setCtxPaneLabel(label),
  };
  const paneThunk = (): (() => JSX.Element) | null => ctxPaneThunk();
  const hasPane = () => paneThunk() != null;
  const paneLabel = () => ctxPaneLabel();
  // The inbox binds to REAL notification state (empty today — the honest floor), never a hardcoded
  // count or demo rows.
  const inbox = createInbox();
  const inboxLabel = () => {
    const n = inbox.unreadCount();
    if (n === 0) return "Inbox, no unread notifications";
    return `Inbox, ${n} unread notification${n === 1 ? "" : "s"}`;
  };

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
        return;
      }
      // `x` toggles the context-pane drawer (NOTES §4) — only when narrow + a pane exists, and never
      // while typing into a field or with a modifier held.
      if (
        e.key.toLowerCase() === "x" &&
        !e.metaKey &&
        !e.ctrlKey &&
        !e.altKey &&
        hasPane() &&
        !paneWide() &&
        !isTypingTarget(e.target)
      ) {
        e.preventDefault();
        setPaneDrawerOpen((v) => !v);
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
          "grid-template-rows": "auto minmax(0, 1fr)",
          // R3.3 §1b — the shell owns the viewport height (was 100vh; dvh survives mobile URL bars).
          height: "100dvh",
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

          {/* MOB-2 — the "Context" drawer trigger. Only rendered when a surface supplies a pane AND
              the viewport is narrow (≤ 1280px); at wider widths the pane is the 4th column. */}
          <Show when={hasPane() && !paneWide()}>
            <button
              type="button"
              class="context-trigger"
              onClick={() => setPaneDrawerOpen(true)}
              aria-haspopup="dialog"
              aria-keyshortcuts="x"
              style={{
                display: "inline-flex",
                "align-items": "center",
                gap: "var(--space-1)",
                padding: "var(--space-2)",
                border: "var(--hairline) solid var(--border)",
                "border-radius": "var(--radius-1)",
                background: "var(--surface-raised)",
                color: "var(--text-primary)",
                cursor: "pointer",
              }}
            >
              <Icon name="link" />
              <span>{paneLabel()}</span>
            </button>
          </Show>

          {/* Inbox affordance — glyph + a visible unread COUNT badge (never color-only; WCAG 1.4.1).
              Badge + aria-label are driven by REAL notification state: ZERO unread → NO badge, and
              the accessible name says "no unread notifications" (dissolves firstrun #3 fake inbox). */}
          <button
            type="button"
            class="inbox-button"
            onClick={() => setInboxOpen(true)}
            aria-haspopup="dialog"
            aria-label={inboxLabel()}
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
            <Show when={inbox.unreadCount() > 0}>
              <span
                data-testid="inbox-badge"
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
                {inbox.unreadCount()}
              </span>
            </Show>
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

        {/* Secondary-nav slot · the fluid main slot · the shell-owned context-pane column (R3.3 §1b).
            The pane column is present only when a surface supplies `contextPane` AND the viewport is
            wide (> 1280px) — otherwise it DROPS (content never renders beside an empty gutter) and the
            pane moves to the header-triggered drawer below. */}
        <div
          style={{
            "grid-row": "2",
            display: "grid",
            "grid-template-columns": paneColumns(Boolean(props.secondaryNav), hasPane() && paneWide()),
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
            <ContextPaneContext.Provider value={paneApi}>
              {props.children}
            </ContextPaneContext.Provider>
          </main>
          {/* The context pane — a `complementary` landmark that owns its own scroller (§1b). Rendered
              inline only when wide; when narrow it lives in the drawer (below), never in both places. */}
          <Show when={hasPane() && paneWide()}>
            <aside
              aria-label={paneLabel()}
              data-testid="context-pane"
              style={{
                "border-inline-start": "var(--hairline) solid var(--border)",
                padding: "var(--space-4)",
                overflow: "auto",
                "min-height": "0",
                "overscroll-behavior": "contain",
                background: "var(--surface-raised)",
              }}
            >
              {paneThunk()?.()}
            </aside>
          </Show>
        </div>
      </div>

      {/* MOB-2 — the narrow-viewport context drawer (portal + scrim + focus-trap + Esc via Dialog). */}
      <Show when={hasPane() && !paneWide()}>
        <Dialog
          open={paneDrawerOpen()}
          onClose={() => setPaneDrawerOpen(false)}
          title={paneLabel()}
          size="sm"
        >
          <div data-testid="context-pane-drawer" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-4)" }}>
            {paneThunk()?.()}
          </div>
        </Dialog>
      </Show>

      <CommandPalette open={paletteOpen()} onClose={() => setPaletteOpen(false)} commands={commands()} />

      <Dialog
        open={inboxOpen()}
        onClose={() => setInboxOpen(false)}
        title="Inbox"
        description="Things that need you land here."
        size="sm"
      >
        {/* Bound to REAL notification state. Empty today (the honest floor — the durable inbox
            endpoint + its SSE delivery are a later wiring); NEVER fabricated rows. */}
        <Show
          when={inbox.items().length > 0}
          fallback={
            <div
              role="status"
              data-testid="inbox-empty"
              style={{ display: "flex", "flex-direction": "column", "align-items": "center", gap: "var(--space-2)", padding: "var(--space-5) var(--space-3)", "text-align": "center", color: "var(--text-muted)" }}
            >
              <Icon name="inbox" />
              <strong style={{ color: "var(--text-primary)" }}>You're all caught up</strong>
              <span style={{ "font-size": "var(--fs-body-sm)" }}>
                When a pull request needs your review or someone mentions you, it'll show up here.
              </span>
            </div>
          }
        >
          <ul style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
            <For each={inbox.items()}>
              {(item) => (
                <li style={{ display: "flex", gap: "var(--space-2)", "align-items": "center" }}>
                  <Icon name="inbox" />
                  <Show
                    when={item.href}
                    fallback={<span>{item.title}</span>}
                  >
                    {(href) => (
                      <A href={href()} onClick={() => setInboxOpen(false)} style={{ color: "var(--text-primary)" }}>
                        {item.title}
                      </A>
                    )}
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Show>
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
