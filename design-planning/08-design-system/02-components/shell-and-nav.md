# Navigation Shell — primary nav · contextual sidebar · content · context pane

> **Tier 2.** The persistent outer frame every subsystem composes into, so switching subsystems never feels
> like switching apps (P1). **File date: 2026-06-20. Direction A "Instrument".**
>
> **Implements:** design-language §5.1 (the navigation shell) + §8b.4 (layout-containment & mobile bug
> checklist, all PROVEN bugs) + the finalist-A shell screen (`screens/1-shell-pr-context.html`).
>
> **Tagging:** **PROVEN** = a cited layout bug class / WCAG landmark requirement / architecture contract.
> **HOUSE STYLE** = direction-A character (48px rail, hairline grouping, compact default).

---

## 1. Purpose + the spec it implements

The shell owns the **layout grid and breakpoints**; subsystems own only their **sidebar + content**.
Four regions (design-language §5.1):

```
┌───────────────────────────── topbar (40px) ──────────────────────────────┐
│ brand · scope/residency cue · ⌘K trigger · search · inbox · identity menu  │
├──────┬───────────────┬─────────────────────────────┬──────────────────────┤
│ rail │ contextual    │  main content               │ context pane         │
│ 48px │ sidebar 224px │  (subsystem-owned)          │ 332px (optional)     │
│      │ (subsystem)   │                             │ the wedge assembles  │
└──────┴───────────────┴─────────────────────────────┴──────────────────────┘
```

- **Primary nav (rail):** the subsystem/area switcher — Code · CI · Issues · Knowledge · Chat · (sep) · Inbox ·
  Search/Admin. One shell invariant across all directions.
- **Contextual sidebar:** the current subsystem's tree/list (repo tree, run list, issue views, page tree,
  channel list) — subsystem-owned content in a shell-owned frame.
- **Main content:** subsystem-owned.
- **Context pane (optional, right):** where cross-artifact references and details surface (§5.3/§5.5) — the
  wedge felt; hosts reference chips/unfurls and the agent panel ([R] content in an [F] frame).
- **Constant chrome:** the ⌘K palette trigger, global search, inbox entry, the current `Principal`'s identity
  menu, and the **tenant/space scope indicator that doubles as a residency/visibility cue** (P9).
- **Deep-linkable URLs** for every artifact down to sub-artifact granularity (a diff line, a doc block, a CI
  step — ADR-13 `ArtifactRef`), because those links are what chat/issues/docs reference.

---

## 2. Anatomy + the layout-containment rules (§8b.4 — PROVEN bug classes)

The shell is the component most exposed to the §8b.4 bugs; these are **binding**:

- **Pin the shell to the viewport** (`height:100vh` / `overflow:hidden` on the shell root); **each region owns
  its own scroller.** *(PROVEN bug class.)*
- **A flex/grid child that scrolls needs `min-height:0`** + `overscroll-behavior:contain` — without
  `min-height:0`, overflow leaks up the tree and pushes a pinned composer below the fold. *(PROVEN.)* Every
  scrolling region (sidebar, content, context pane) sets `min-height:0` and `overflow:auto`.
- **`width:100%` is not a takeover:** a full-width mobile panel laid out *beside* a still-present column is
  clipped off-screen → **collapse the other column at the breakpoint** (see §6). *(PROVEN.)*
- Grid: `grid-template-rows: 40px 1fr` (topbar/body); body `grid-template-columns: 48px 224px 1fr 332px`
  (rail/sidebar/content/pane) — the pane column drops when absent.
- **One `<main id="main">`** + a **skip-link** as the first focusable element ("Skip to content").

---

## 3. Variants + the parameterization variant flags

| Variant | Values | Effect |
|---|---|---|
| **context pane** | present / absent | the 4th column shows only when a surface has cross-artifact context |
| **sidebar** | tree / list / hidden | subsystem chooses; the frame is constant |
| **layout** | desktop / tablet / mobile | §6 responsive behaviour |

**Parameterization variant flags (00-plan §2.2) — the shell is the primary reader of these:**

| Flag | What it changes in the shell |
|---|---|
| **`nav`** (`rail` ↔ `palette-led` ↔ `contextual`) | which navigation surface is **emphasised** — **all three exist always.** `rail`: the 48px rail is the primary mover. `palette-led` (A's default): the ⌘K trigger is foregrounded in the topbar, the rail is present but secondary. `contextual`: the sidebar tree leads. **This sets emphasis/default, not presence** — never a `switch(direction)`. |
| **`density`** (`compact` ↔ `comfortable`) | row heights and paddings in the sidebar/pane (compact: 32px rows, A default). |
| **`sovereigntyVisibility`** (`on-demand` ↔ `always-on`) | whether the residency/lawful-basis band is **always rendered** in the topbar scope cue + near data, or summoned. The DSR/residency cue is built **once** and gated behind this flag (A = on-demand; D = always-on). |
| **`tone`** | empty-state voice in subsystem regions only; no chrome change. |
| **`agentPresence`** | the context pane's agent panel default verbosity (ambient inline vs foregrounded) — the panel is the same. |

---

## 4. ALL states

| State | Behaviour |
|---|---|
| **default** | all regions rendered; the current rail item carries `aria-current="page"` and reads selected via a **`--surface-hover` fill + `--text-primary`** (brighter than the resting `--text-subtle`) — and may tint its glyph with `--accent`. **No colored side-bar / inset accent marker** (user feedback 2026-06-20: no rounded colored side borders — see [`../REFINEMENTS.md`](../REFINEMENTS.md)). The selected state is a non-colour difference (fill + brightness), so it never relies on colour alone. |
| **hover** | rail/sidebar item → `--text-primary` + `--surface-hover`. |
| **focus** | every interactive shell element shows the one `--focus-ring` (2px, logical offset). |
| **active** | pressed nav item. |
| **disabled** | a nav target the viewer can't reach is **absent**, not greyed (no teasing; permission-pre-filter). |
| **loading** | each region loads independently with **structure-skeleton + `aria-busy`** (ghost rows/cards matching the region), never a full-shell spinner — the shell renders, regions fill (§8b.6). |
| **empty** | a region with no items shows an onboarding-forward empty (subsystem copy, `tone`-shaped), never a blank panel. |
| **error** | a region **fails static**: one quiet system-blaming line + retry **scoped to that region**; the rest of the shell stays live (§8b.6). |
| **permission-denied** | a region the viewer can't access shows a graceful no-access state; the shell chrome stays. |
| **erased / tombstoned** | a sidebar/pane reference to an erased artifact renders the tombstone chip ([R]), never a dangling title. |
| **agent-pending** | the context pane's agent panel shows the quiet "an agent is working / awaiting you" state (R-14 §5; content is [R], frame is here). |
| **live-update** | a sidebar status (a PR going green, a check flipping) transitions subtly without scroll-jump or losing the user's selection; announced politely only if the viewer is watching (R-17 §6.1). |

---

## 5. Keyboard + ARIA model (PROVEN — WCAG landmarks)

- **Landmarks:** topbar `role="banner"`; rail `<nav aria-label="Surfaces">`; sidebar `<aside aria-label=…>`;
  content `<main id="main">`; context pane `<aside aria-label=…>`. One `lang` on `<html>`.
- **Skip-link** first in tab order → `#main` (PROVEN — 2.4.1 bypass blocks).
- **Tab order** follows visual/logical order; each region is keyboard-traversable; **no trap** (you can always
  Tab out of a region).
- **Focus** uses the one `--focus-ring` token in all three themes.
- **Live region:** the shell hosts **one global polite `role="status"` region** (the substrate live region per
  R-17 §6.1) — debounced, never per-background-tick.
- **React Aria mapping:** the shell is mostly layout (no single RAC primitive). The **identity menu** in the
  topbar uses `MenuTrigger`/`Menu` ([F] overlays); the **⌘K trigger** is a `Button` opening the palette
  `DialogTrigger`; the **theme/scope controls** are `Button`/`ToggleButton`. Drawers (§6) use `Modal`/`Popover`.

---

## 6. Responsive / touch drawer behaviour (§8b.4 mandate) — PROVEN

- **Breakpoints (HOUSE STYLE over the §8b.4 rules):** desktop (4 columns), tablet (rail + content + pane-as-
  drawer), mobile (content only; rail + sidebar + pane all become **toggled overlay drawers**).
- **Mobile drawer pattern (verbatim §8b.4):** rail / secondary-nav → **toggled overlays with backdrop +
  Escape + route-change auto-close.** Drawers reuse the [F] overlay substrate (portal, focus-trap, scroll-lock).
- **`width:100%` collapse rule:** at each breakpoint, **collapse the column the drawer replaces** so the
  full-width panel is not clipped beside a still-present column. *(PROVEN bug.)*
- **Hover is not touch-reachable (§8b.4):** any hover-revealed shell affordance (row actions in the sidebar)
  gets an **explicit mobile affordance** (a visible action button / overflow menu), not hover-only.
- **`min-height:0` + `overscroll-contain`** on every drawer scroller (same containment rule).
- Targets ≥ 24×24 CSS px (WCAG 2.5.8) on touch.

---

## 7. Semantic tokens consumed

`--surface` (page), `--surface-raised` (topbar/rail/sidebar/pane), `--surface-overlay` (drawers),
`--surface-hover`; `--text-primary/muted/subtle`; `--border` (hairline grouping), `--border-strong`;
`--accent` (the 2px active-nav marker only — `--c-rail-active`); `--focus-ring`; `--success`/`--warning`/
`--danger` (+ glyph+label) for sidebar status hints; `--overlay-scrim` (drawer backdrop); spacing/radius/
motion/z scales; `--z-chrome` (shell) and `--z-modal`/`--z-popover` (drawers).

## 8. Motion

Region content uses `--dur-fast` for live-update transitions (no scroll-jump). Drawers slide in at `--dur-base`
(180ms) with `--ease-enter`; reduced-motion → instant (the state still flips and announces). Pages render,
they don't animate in (§8b.6).

## 9. Usage do / don't

- **Do** pin the shell and give each region its own `min-height:0` scroller; **do** keep the chrome constant
  across subsystems; **do** gate the residency band behind `sovereigntyVisibility` (built once).
- **Do** read the `nav` flag to set emphasis — never branch on the finalist name.
- **Don't** let a region's overflow leak up the tree; **don't** rely on hover for touch; **don't** show a
  full-shell spinner (regions skeleton independently); **don't** clip a full-width mobile panel beside a
  still-present column.

## 10. The trap (review catches)

A subsystem that ships its own shell/layout has fractured the product (design-language §8.3 rule 1). A
component reads `nav`/`density`/`sovereigntyVisibility` flags + semantic tokens — **never `switch(direction)`**.
The classic §8b.4 failures (composer pushed below the fold; clipped mobile panel; dialog clipped in the
sidebar — that last one is forbidden by the [F] overlay portal-always rule) are the review checklist.
