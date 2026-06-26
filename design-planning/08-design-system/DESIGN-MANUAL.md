# The Myelin Design Manual — direction A "Instrument"

> ⚠️ **STACK SUPERSEDED (2026-06-26) — read before building any UI.** The visual system, tokens, icons, component
> *specs*, principles, and the WCAG/APG accessibility bar in this manual **remain canonical**. But the
> **implementation stack** named below (TypeScript + React + React Aria — §1.1/§1.2 of
> `00-framework-and-buildout-plan.md`) is **superseded**: the build is **SolidJS + SolidStart + Tauri 2**, with
> **hand-built overlay primitives** (not React Aria) and a **per-block `contenteditable`** editor. For ANY UI
> surface, read **`planning/system-reviews/2026-06-26/08-frontend-foundation.md`** (the stack) and
> **`…/10-frontend-component-patterns.md`** (the component behaviour) ALONGSIDE the relevant spec here. Wherever
> this manual says "React" / "React Aria", read "Solid / hand-built" per docs 08 + 10.

> **The single authoritative handbook for building a Myelin UI surface.** Open this first; it is the
> through-line that ties the principles, tokens, components, patterns, and pipeline together and points
> you at the detailed spec or the live styleguide for each. **Status date: 2026-06-21.**
> **Direction: Finalist A "Instrument"** (highly-unified · dense · palette-led · utilitarian) —
> Phase-7 recommendation, **confirmed by the user**. The whole system is **parameterized**: another
> direction is a token-set swap + a flag flip, never a rewrite (§8).
>
> **Tagging (the VISION §3 honesty rule, carried everywhere):** **PROVEN** = a cited standard
> (WCAG 2.2 / WAI-ARIA APG / EN 301 549 / AI Act), a measured artifact value, or a surfaced
> architecture contract (an ADR). **HOUSE STYLE** = our taste/synthesis (the character of direction A).
> **`[DEFERRED-UNTIL-USERS]` / `[UNDER-EVIDENCED]`** = a reception/comprehension/trust hypothesis no
> expert pass can settle; this is expert design work, **not user-validated** (§9).

---

## Table of contents

1. [Introduction](#1-introduction) — what this is, who it's for, "Instrument" in a paragraph, how to use it + the styleguide, provenance.
2. [Principles](#2-principles) — P1–P9 distilled into the rules every screen is judged against.
3. [Foundations](#3-foundations) — colour & theming · type · spacing/density · elevation/borders · motion · iconography.
4. [Components](#4-components) — the navigable inventory; one entry each, linking to its spec.
5. [Patterns](#5-patterns) — the agent-native contract · dual-audience lenses · unglamorous states · perceived performance · sovereignty-as-UX.
6. [Accessibility & i18n](#6-accessibility--i18n) — the WCAG/EN bar, keyboard, focus, status-not-by-colour, reduced motion, i18n/RTL.
7. [Voice & do/don'ts](#7-voice--dodonts) — the concise rules, incl. REFINEMENTS R1.
8. [From design to code](#8-from-design-to-code) — the tokens→Style-Dictionary→CSS/TS pipeline, React + React Aria, the strok icon pipeline, the live styleguide, parameterization.
9. [Extending the system & governance](#9-extending-the-system--governance) — adding a token/icon/component, the quality gates, and the honesty register.

**The source-of-truth map** (this manual synthesizes; these are canonical — go here to build):

| Layer | Canonical source |
|---|---|
| Principles & view catalogue | [`../../planning/02-holistic-architecture/design-language.md`](../../planning/02-holistic-architecture/design-language.md) |
| Stack · parameterization · buildout | [`00-framework-and-buildout-plan.md`](./00-framework-and-buildout-plan.md) |
| Binding user feedback | [`REFINEMENTS.md`](./REFINEMENTS.md) |
| Tokens (DTCG + contrast tables) | [`01-tokens/tokens.md`](./01-tokens/tokens.md) · [`tokens.json`](./01-tokens/tokens.json) · [`tokens.css`](./01-tokens/tokens.css) |
| Component specs | [`02-components/`](./02-components/) (+ its [`README.md`](./02-components/README.md)) |
| Live styleguide (no-drift reference) | [`03-styleguide/index.html`](./03-styleguide/index.html) |
| Icons | [`04-icons/ICONS-README.md`](./04-icons/ICONS-README.md) · [`USAGE-MAP.md`](./04-icons/USAGE-MAP.md) |
| Why A; runner-up D; the hybrid | [`../07-judging/decision-brief.md`](../07-judging/decision-brief.md) |
| Research provenance | [`../04-research/`](../04-research/) |

---

## 1. Introduction

### What this is, and who it's for
This is the handbook a **designer or engineer opens to build a Myelin surface** — a screen, a component,
a state. Myelin is one platform that fuses git hosting, CI/CD, an issue tracker, a knowledge base, and
chat, EU-sovereign and agent-native. Its central bet is **coherence**: five subsystems must read as
*one product*. This manual is how that bet is kept mechanical rather than a matter of discipline — it
distills the principles, names the foundations, catalogues the shared components, and codifies the
patterns, so that any surface built from it is coherent, accessible, and on-character by construction.

It is **deliberately opinionated and concrete**. It does not re-document the specs; it is the through-line
that tells you *which* spec to open, *why* a decision was made (with its PROVEN/HOUSE-STYLE provenance),
and *how* the pieces compose. When this manual and a spec ever disagree, the spec is canonical for detail
and [`REFINEMENTS.md`](./REFINEMENTS.md) overrides both.

### The "Instrument" direction, in one paragraph
**A "Instrument" is a midnight command-deck.** Neutral-led surfaces carry ~90% of the UI; **hairline
borders do nearly all grouping** (shadow is reserved for genuinely floating layers); a **single rationed
electric-blue accent** is the only chroma, and the focus/primary affordance rides a *derived, higher-contrast*
token distinct from it. **Monospace is load-bearing** (refs, SHAs, code). Radius is near-zero (sharp
reads fast); **density is compact by default**; motion is crisp, fast, and interruptible (no spring, no
sparkle). The thesis is **maximum muscle-memory transfer, minimum per-surface personality** — one skin
across all five subsystems, with the dual-audience PM lens reached by *tuning the same component*, not
forking it. *(HOUSE STYLE character; the floors it sits on are PROVEN — §3, §6.)*

### How to use this manual + the live styleguide
- **Read §2 (principles) once** — they are the lens every screen is judged against, including yours.
- **Build foundations-first.** §3 is the vocabulary; §8 is the build order (tokens → overlay primitives
  → shared components → surfaces). Never skip the overlay substrate (§4.1) — it is the most expensive
  retrofit. *(PROVEN sequencing — design-language §8b.1.)*
- **Reach for a component before inventing one** (§4). New components are contributed *down* into the
  shared library, never re-implemented per surface.
- **Open the live styleguide as your reference for the real thing.**
  [`03-styleguide/index.html`](./03-styleguide/index.html) renders from the **real generated `tokens.css`**,
  runs **with the stack down** (static, no build, no CDN), and has **live theme + RTL toggles**. Its
  contrast ratios are **measured live in JS** from the resolved tokens and recompute on theme switch —
  so the styleguide *cannot drift from the app* and the measured-contrast gate is visible. Sections:
  Colour/semantic tokens · Type scale · Spacing & radius · Elevation · Motion · **Icon library (all 42,
  theme-aware)** · Component showcase. *(PROVEN-as-mandate — design-language §8b.6.)*

### Provenance — chosen direction, swappable by design
Direction **A** is the [decision-brief](../07-judging/decision-brief.md) recommendation (weighted **2.98/4**,
winning the two highest-weighted bets: **D4 one-product coherence = 4** and **D5 dual-audience = 4**) and
is **confirmed by the user**. The runner-up was **D "Civic"** (2.89, owning **D9 sovereignty = 4**), and
the panel's likely best end-state is a **synthesized hybrid** (A's chassis + B's warm reading lens + C's
rendered agent contract + D's sovereignty console). Everything in this manual is **parameterized** so
that picking D — or the hybrid — is a *re-run*, not a rebuild (§8.4). Read every "direction" choice as
carrying the brief's `[DEFERRED-UNTIL-USERS]` flag.

---

## 2. Principles

The nine principles from [`design-language.md` §1](../../planning/02-holistic-architecture/design-language.md#1-design-principles--myelins-point-of-view)
are the product's point of view and the lens every screen is judged against. Here they are as **actionable
design rules** — what each *forces you to do*.

- **P1 — One product, not five tools.** Coherence *is the feature.* Use the shared shell, palette,
  identity badge, reference chip, comment thread, editor, and views component — never a subsystem-local
  copy. The same `ArtifactRef` renders the same chip everywhere; the same `Principal` renders the same
  badge everywhere (ADR-13). A user must never feel they "left one app." *Test:* would this component
  render identically if dropped into another subsystem?
- **P2 — Speed is a feature; the UI must feel instant.** Optimistic updates, structure-skeletons (never
  blank spinners), sub-100ms perceived response on common actions, no full-page reloads. Latency is a
  design defect. The residency trade-off (no global CDN of personal data) means speed is bought with
  optimistic UI + in-region edge + prefetch (§5.4), not replication.
- **P3 — Keyboard-first, mouse-complete.** Every primary action is reachable by keyboard (⌘K palette
  everywhere, `j/k`, single-key actions, full keyboard nav of diffs/boards/tables/editor) — *and* fully
  reachable by mouse/touch with discoverable affordances. **Keyboard-first is never keyboard-only.**
- **P4 — Progressive disclosure.** Opinionated defaults are visible; depth (custom workflows, SLA config,
  permission schemes, governance) is one layer down, never in the newcomer's face. Power present, not imposed.
- **P5 — Density is earned, not default.** Default to the chosen direction's density (A = **compact**) with
  a global density toggle and per-view density where it matters. Density never costs touch targets or
  readability.
- **P6 — Reference everything, everywhere.** Any artifact is an `ArtifactRef` and therefore mentionable and
  unfurlable from anywhere. The **reference chip + unfurl is the most important shared component** — it is
  the cross-artifact wedge made tangible. Always live, permission-aware per viewer, backlinked.
- **P7 — Agents are visible, labeled, trustworthy — never magic, never hidden.** Agents are always labeled
  as agents; they **propose before they act** (plan-then-apply); consequential actions pass a **HITL
  approval card**; every agent action is attributed and audit-linked. Trust is built by legibility, never
  a "magic" button. *(AI Act duty — PROVEN; the full contract is §5.1.)*
- **P8 — Calm by default; attention is sacred.** One prioritised "what needs *me*" inbox; dedup +
  storm-control; agent volume kept out of the main timeline. **Quiet is the default; the user opts into
  more, never out of a firehose.**
- **P9 — Trust through transparency (sovereignty & GDPR are UX, not fine print).** "Where does this data
  live?", "who/what processed this?", "show me everything about this subject" are answerable *in the UI*,
  by the right role. Privacy-by-default: private visibility, opt-in telemetry, minimal retention. *(The
  treatment is §5.5; it is the most `[UNDER-EVIDENCED]` area — §9.)*

---

## 3. Foundations

The atomic design decisions. Vocabulary canonicalised in [`01-tokens/tokens.md`](./01-tokens/tokens.md);
see them live in the [styleguide](./03-styleguide/index.html). Components consume the **semantic tier only**.

### 3.1 Colour & theming
- **Three-tier DTCG tokens** *(PROVEN architecture — design-language §3.1):* **Primitive** (raw ramps —
  never used directly) → **Semantic** (intent-named, theme-aware: `surface*`, `text-*`, `border*`,
  `accent`/`on-accent`, `focus-ring`, `success`/`warning`/`danger`/`info`, `agent`/`on-agent`/`agent-subtle`
  — **the only tier components touch**) → **Component** (optional handles, each bound to a semantic, never
  a primitive). This indirection is what makes theming a *table swap, not a rewrite* — the same mechanism
  the direction-parameter rides (§8.4).
- **Three authored themes:** `dark` (default), `light`, `high-contrast` — each a semantic override map
  re-pointing at a different primitive ramp. Switching theme = setting `data-theme` on the root; no
  component change.
- **Measured contrast — never claimed** *(PROVEN — WCAG 1.4.3/1.4.11; the gate is §8.1):* every semantic
  text/UI pair is recomputed from the actual hex against AA (4.5:1 text, 3:1 large/UI) in all three themes.
  **Every text and `on-*` pair passes AA in every theme; the lowest is 5.05:1.** See the full tables in
  [`tokens.md` §2](./01-tokens/tokens.md#2-measured-contrast-tables-every-semantic-textui-pair-all-three-themes).
- **Focus-ring ≠ identity accent** *(PROVEN — the carve-out rule; tokens.md §2.4):* the identity `accent`
  may carry brand at its natural chroma even near the AA floor, but **focus and the primary-action fill
  must ride a *derived*, higher-contrast `focus-ring` token** so the affordance a keyboard user depends on
  never rests on a sub-AA colour. (In light, `accent` lands *exactly* at 4.50:1 — the very case this
  exists for; `button-primary-bg → focus-ring` (6.55:1), not `→ accent`.) One `focus-ring` covers **every**
  interactive element, including the autofocused palette input.
- **The agent colour is a fourth neutral semantic axis** (a reserved violet), **never a status colour** —
  "the screen is not a traffic light." Agent legibility never relies on colour alone (§5.1).
- **Functional status is shared across subsystems** and **never by colour alone** — always glyph + label +
  position; no saturated traffic-light fills. *(PROVEN — WCAG 1.4.1.)*

### 3.2 Typography
- **Two self-hosted variable families** *(CDN banned — sovereignty/GDPR; design-language §3.3):* a UI sans +
  a **load-bearing monospace** (refs, SHAs, diffs, logs, inline code). **EU-multilingual coverage is a
  selection gate** — Latin-ext + Greek + Cyrillic at minimum; Arabic/Hebrew for RTL is a `[VERIFY]` before build.
- **One ~1.2 type scale**, shared platform-wide: `caption 12 · body-sm 13 · body 14 · h3 16 · h2 20 ·
  h1 24 · display 30 · code 13`. Line-heights `tight 1.3 / body 1.45 / reading 1.6` — **body ≥1.45 for
  diacritic headroom** (1.0 clips Greek tonos / Cyrillic breve / Latin-ext caron).
- **Hierarchy from weight & colour before size** *(HOUSE STYLE; the "big size = amateur tell" rule).*

### 3.3 Spacing, layout, grid & density
- **One 4px spacing ramp:** `0,4,8,12,16,24,32,48,64`. Every margin/padding/gap is a step — **off-ramp
  magic numbers (5/7/13px) are the amateur tell.**
- **Density modes** are token sets over the same scale: **A defaults to `compact`**; the global density
  toggle + per-view density (diffs, tables, logs, boards) are first-class (P5).
- **The shell owns the layout grid + breakpoints** (§4.2). Subsystems supply only their sidebar + content.

### 3.4 Elevation & borders
- **Borders-and-surfaces first; shadow reserved** *(direction-A character; PROVEN benefit at high contrast):*
  hairline (1px) borders do nearly all grouping. Shadow exists only for genuinely floating layers —
  `shadow.popover` (menus/dropdowns/hovercards) and `shadow.overlay` (palette/dialog). In high-contrast,
  shadow is inert and a border carries the layer.
- **Radius near-zero:** `0, 3 (default), 6, 10, pill`. **One z-index scale** (a single token):
  `base < chrome(100) < popover(200) < modal(300) < toast(400)`; per-component magic z-index is banned.
  All overlays portal to the document root.

### 3.5 Motion
The token set + the five laws *(R-12 [motion-microinteractions](../04-research/visual/motion-microinteractions.md);
mandates PROVEN, values HOUSE STYLE):*
- **Durations** `instant 0 · micro 90 · fast 140 · base 180 · deliberate 240` (all ≤240ms). **Easings**
  `standard / enter / exit / emphasized (reserved agent) / linear` — **no spring/bounce/overshoot primitive
  exists.** Semantic roles: `motion.feedback/settle/move/enter/exit/liveUpdate/agentEnter/agentResolve`.
- **L1** motion communicates a state change or doesn't ship. **L2** fast + interruptible (functional band
  120–200ms; only the reserved live/agent cue reaches 240ms; new input interrupts in-flight motion).
  **L3 pages render, they don't animate in.** **L4** `prefers-reduced-motion` is a *first-class spelling*,
  not a removal — every role has a no-movement spelling that conveys the same state change (instant flip +
  ≤80ms cross-fade/static marker + live-region for background changes); **removing motion removes
  animation, never information.** **L5** one motion = one meaning; the agent signature is reserved.

### 3.6 Iconography
The 42-icon system is a first-class, wired-in part of the system. Registry + spec:
[`ICONS-README.md`](./04-icons/ICONS-README.md); two-way binding + gaps: [`USAGE-MAP.md`](./04-icons/USAGE-MAP.md);
the live set: styleguide **Icon library** section.
- **42 core icons, one monochrome outline style, 24×24 grid**, grouped: nav (8), git/CI (10), issue/work
  (5), objects (8), principals (3), agent/HITL (4), chrome (4).
- **`currentColor`, theme-inheriting** *(PROVEN no-CDN benefit):* every icon is authored with
  `stroke="currentColor"`; one drawing serves dark/light/high-contrast **and** the `--agent` token — no
  per-theme files. Recolour via the wrapping element's CSS `color`; size 16/20/24 via CSS `width`/`height`.
- **Pipeline:** `strok/*.strok` (sole hand-edited source) → `bash build.sh` (`strok batch`) → `svg/*.svg`
  (shipped, themeable) + `preview/*.png` + `dist/sprite.svg` (`<symbol>` sheet for `<use>`) +
  `dist/manifest.json` (machine registry, drives icon pickers/search). The build **fails loudly** if any
  SVG lacks `currentColor` or bakes a hex, and is idempotent (two runs → empty `diff`).
- **Consume by registry name** — inline SVG (primary) or `<use href="sprite.svg#name">`. Never by
  appearance. Each component spec carries an **Icons** line naming its canonical glyphs by registry name;
  bind to those.
- **The rails** *(PROVEN duties; the geometry HOUSE STYLE):* **the agent-mark rail** — `agent` is a *plain
  geometric mark* (rounded square + centered dot), the shape channel of the four-channel agent signature
  (label + mark + `--agent` + attribution); never sparkle/star/wand/emoji, never colour-alone. **The
  status rail** — the **ring is reserved exclusively for the CI verdict trio** (`check-pass`/`check-fail`/
  `check-pending`); a bare ✓/✗ is an *action* (`approve`/`reject`/`close`), not a verdict. `chevron` is
  one glyph CSS-rotated for all four directions.
- **Certified 10/10** ([`ACCEPTANCE.md`](./04-icons/ACCEPTANCE.md)); the backlog (projection-switcher
  mini-family `view-board/table/list/gallery`, `service`, `react`, `snooze`, `mute`, `attachment`, `add`,
  `copy`, `filter`) is in `USAGE-MAP.md` §C — **no gap touches the agent or status rails**.

---

## 4. Components

The shared inventory — built **once** against the tokens and reused (this reuse is the mechanical
guarantee of P1). Full template (anatomy · variants + the 6 flags · ALL states · keyboard+ARIA →
React Aria · tokens · motion · do/don't) lives in each spec; directory index + the ownership/seam map:
[`02-components/README.md`](./02-components/README.md). Build order is **overlays → shared → surfaces** (§8).

> Every component also implements the unglamorous states (§5.3) and is keyboard-operable + accessible +
> i18n/RTL-ready (§6). Icons below are by **registry name** (§3.6).

### 4.1 Tier 1 — Overlay primitives (build these first)
**[`overlays.md`](./02-components/overlays.md)** — the shared substrate: **Dialog · ConfirmDialog · Popover ·
Dropdown/Menu · Tooltip · Toast.** Focus-trap + return-focus, scroll-lock, Escape/backdrop dismiss,
portal-to-root, the one z-index scale, and correct ARIA live **once** here and are inherited free —
*consumers never re-implement them.* Build first: it is the most expensive UX retrofit (design-language §8b.1).
ConfirmDialog (`alertdialog`, default-focus the *safe* action) is reserved for irreversible/GDPR/HITL.
Toast hosts the **undo** for the optimistic-rollback contract (§5.4) and never steals focus.
*Icons: `close`, `chevron`, `check-pass`, `check-fail`. React Aria: `Modal`/`Dialog`, `Popover`,
`Menu`, `Tooltip`, `ToastQueue`.*

### 4.2 Tier 2 — Shared components

| Component | Purpose / when to use | Key states & rules | Icons |
|---|---|---|---|
| **[Navigation shell](./02-components/shell-and-nav.md)** | The persistent four-region frame (topbar · 48px rail · 224px contextual sidebar · content · optional 332px context pane) every subsystem composes into. Use as the outer frame, always. | Active rail item = **`--surface-hover` fill + brighter text, no colored side-bar marker** (R1, §7). Each region skeletons independently (no full-shell spinner); a region fails *static* with scoped retry; live-update flips without scroll-jump or losing selection. Pin to viewport, each region owns its scroller (`min-height:0`). | `nav-code/ci/issues/knowledge/chat`, `search`, `inbox`, `settings`, `human`, `chevron`, object glyphs, `check-*` |
| **[Command palette](./02-components/command-palette.md)** | The ⌘K nerve-centre — a **real `<input>` combobox** to Navigate / Act / Search / Build-query over one IA, one query AST, one permission pre-filter; A's default nav surface. Use for all cross-cutting nav/action/search/filter entry. | Idle shows recents + suggested actions (never blank); **no-access row is indistinguishable from not-found** (the anti-oracle rule, never leaks); error degrades to the local pool; consequential verbs carry a `gate` marker routing into ConfirmDialog/HITL. Roving listbox via `aria-activedescendant`. | `repo`/`pull-request`/`issue`/`doc`/`channel`/`run`/`human`/`agent`/`settings`, `search`, `priority`, `gate`, `chevron`, `external-link` |
| **[Reference chip + unfurl](./02-components/reference-chip-and-unfurl.md)** | The two densities (compact chip / rich card) of one `ArtifactRef`, **resolved live per-viewer every render** — the wedge (P6). Use anywhere a reference is placed; must render identically across board cell, editor mention, inbox subject, PR pane, chat unfurl. | **No-access = `{icon} Restricted` with no title** (non-leaking by construction); Moved/Outdated pills; Tombstoned/erased; rebase-orphan detaches honestly (never jumps to the wrong line); degraded freezes last-known. Unfurl is an *action* surface (inline re-run / transition / approve where permitted). | type glyphs `pull-request`/`issue`/`sub-issue`/`doc`/`run`/`commit`/`branch`/`tag`/`message`/`repo`/`file`/`folder`/`database`/`human`/`agent`/`link`; status `check-*`/`merge`/`priority`/`rerun`; actions `external-link`/`kebab`/`edit` |
| **[Agent / HITL approval card](./02-components/agent-hitl-card.md)** | `<AgentHitlCard>` — renders an agent's concrete **proposed effects** before `EffectApi::apply` runs, resolving a durable HITL gate (plan-then-apply, §5.1). Use for any agent proposal needing review. Three homes: chat (primary), inbox (never-missed), inline on the artifact. | gate-awaiting · gate-edited (proposed→amended **diff**, attributed `human-edited-agent-proposal`) · agent-error mid-chain (saga: completed steps stand, no half-open PR) · budget-exceeded / loop-guard-tripped · denied/cross-cell (never leaks target). **Approve / Edit / Reject**; cost + scope always shown, never on a toggle. | `agent`, `gate`, `approve`, `edit`, `reject`, `link`, `external-link`; per-effect targets reuse chip icons |
| **[Identity / agent badge](./02-components/identity-and-agent-badge.md)** | One badge per `Principal` (human/agent/service). The atom embedded wherever a Principal appears — specced once, inherited by chip, HITL card, comments, views cells, mention node, inbox, audit log. | Loading skeletons the name, renders mark immediately; permission-denied → "Restricted"/"[hidden user]" (never leaks a name); erased → "[erased user]"; **agent treatment is four-channel** (label "Agent" + plain mark + `--agent` + attribution), never disguised as human. | `human`, `agent` (plain mark), `team` |
| **[Forms & controls](./02-components/forms-and-controls.md)** | The control atoms — Button, Input, Select/Combobox, Checkbox/Radio/Switch, Field+Validation. The atoms inside dialogs, the HITL Edit form, views inline-edit, the comment composer, field-definition UI. | Focus uses `--focus-ring` (distinct from `--accent`); loading via in-button spinner / skeleton + `aria-busy`; error = `aria-invalid` + danger border + glyph + message, **input never cleared**; an unpickable option is *absent*, not greyed. | `search`, `edit`, `external-link`, `close`, `check-pass`, `chevron`, `approve` (checked), `check-fail` (error) |
| **[Comments / threads / mentions / reactions](./02-components/comments-mentions.md)** | The one conversation primitive across PR review, issue discussion, doc comments, chat. Bodies render via the BlockEditor render path; mentions/refs are `<ReferenceChip>`; agent proposals are `<AgentHitlCard>`. | Optimistic saving (typed text never lost); permission-denied = read-only, **composer absent not greyed**; erased = "[erased user]"/tombstone with thread integrity preserved; conflict = CRDT merge or keep-yours/take-theirs, **never silent overwrite**; review batching. | `human`/`agent`, chip glyphs, `link`, `message`, `kebab`, `edit`, `approve`/`reject` (verdicts) |
| **[Views (table/board/calendar/list/gallery/timeline)](./02-components/views.md)** | `<Views>` — six projections of **one query AST**; doubly load-bearing as the issues↔knowledge reuse boundary **and** the engineer↔PM dual-audience lens (§5.2). Use for any structured collection; switching projection re-renders the same rows (not navigation). | empty vs filtered-to-nothing (distinct); permission-denied rows **absent by pre-filter** (never greyed); optimistic-pending drag/edit settles or honest-rollback; conflict = CAS keep-yours/take-theirs (never silently lose an edit); stale/live-update subtle, no scroll-jump. | `roadmap` (timeline), `cycle` (calendar), `search`, `priority`, `settings`, `check-*`, `chevron`, `kebab`, `agent` *(board/table/list/gallery glyphs are a backlogged gap — §3.6)* |
| **[Block editor](./02-components/block-editor.md)** | `<BlockEditor>` — one writing surface for knowledge pages, issue/PR descriptions, comments, chat. Governed by the **one-render-path law** (read and edit run the same parser; `render(parse(md))===md` is a hard CI gate). | error/save-failed (typed content never lost); permission-denied = read-only via the *same* render path; conflict/collab = presence + CRDT/OT merge, never silently dropped (deep model `[OPEN → P4]`); offline buffers locally, lossless re-sync. Controlled `contenteditable`, structured mention/ref nodes; parse/serialize is shared WASM-Rust. | `kebab` (drag/convert), slash-menu `doc`/`database`/`file`/`link`/`issue`, `chevron`, chip glyphs *(attachment/`+` are gaps)* |
| **[Notifications inbox](./02-components/notifications-inbox.md)** | `<NotificationsInbox>` — the one prioritised cross-subsystem inbox; "there is one inbox; everything else is a saved filter on it." The HITL gate's second home. | **Owns the storm**: under a 30× agent surge the **agent lane sheds first**, human-direct items stay at top, a single coalesced agent group + non-alarming surge indicator; deduped "+N more"; agent-pending HITL row at critical priority; permission-changed subject humanises to a tombstone (never leaks); inbox-zero is a *calm* reward (no confetti). | `inbox`, `settings`, kind glyphs `message`/`pull-request`/`issue`/`check-fail`/`gate`/`agent`/`priority`, `chevron`, HITL `approve`/`edit`/`reject`, triage `check-pass`/`external-link`/`close` *(snooze/mute glyphs are gaps)* |

**The seam (so reuse holds):** the **identity/agent badge** and the **overlay substrate** and the **form
controls** are foundational atoms consumed *inside* the richer organisms — the rich components build *on*
them and must not redefine the agent treatment or re-implement focus-trap/validation. The **palette row /
dropdown item / slash-menu item / inbox row share one row shape** (icon + label + kbd-hint). The **diff /
files-changed viewer** is the hardest a11y component (R-17 §5.1) and is owned by the rich-components track.

---

## 5. Patterns

The recurring cross-component contracts. These are *how* the components above combine into trustworthy
behaviour.

### 5.1 The agent-native UX contract
The visible half of plan-then-apply (ADR-08) — the answer to the security/DPO personas' deepest fear.
Provenance: [legibility-and-hitl](../04-research/agent-ux/legibility-and-hitl.md) (R-14),
[attribution-and-calm](../04-research/agent-ux/attribution-and-calm.md) (R-15). The thesis:
**trustworthiness is a property of the contract, not the runtime** — so the *exact same UI* works for mock
agents today and real LLM agents later.
- **Label** *(PROVEN — AI Act + WCAG 1.4.1):* every agent renders with the **four-channel treatment**
  (label "Agent" + plain geometric mark + reserved `--agent` token + attribution string). Never
  colour-alone, never sparkle/wand, never disguised as human.
- **Plan-then-apply:** agents emit *proposed* effects, never direct side effects. The card shows concrete
  effects + per-effect target chip + per-effect authority/`gate` marker + scope (the intersection
  `agent.policy ∩ delegation ∩ tenant.policy`) + **live budget — cost/scope always shown, never on a toggle.**
- **HITL:** **Approve / Edit / Reject**, with **Edit mandatory and load-bearing** (the human amends the
  proposed effect within schema+scope, sees a proposed→amended diff, cannot escalate authority; Reject
  requires a reason). Consequential actions back a durable gate that can wait minutes or days.
- **Attribution & audit:** the **five-field provenance envelope** (Who · What · On-behalf-of · Trigger ·
  Correlation), each a real backend field, humanised never raw; an inline **"Why?"** affordance scaling to
  stakes; one click to the **tamper-evident audit log** (a hash-chain + Merkle / RFC-6962 model, *not*
  blockchain). **Confidence is shown only when the runtime supplies it as categorical/N-best — never a
  fabricated number** (mock supplies none → v1 shows capability/scope statements).
- **Calm volume** *(P8):* agent verbosity is kept out of the main timeline (threading, collapsible
  summaries tied by `correlation_id`, inbox routing). The **two-surge model**: an infra surge sheds the
  *agent lane's* budget (humans never queue behind agents — PROVEN); an attention surge groups in the inbox.
- *Carried:* the trust-calibration study and the Edit-path UX are the **least-evidenced**
  `[DEFERRED-UNTIL-USERS]`; over-trust-from-fluency must be re-tested against the real LLM runtime.

### 5.2 Dual-/tri-audience — "one component, many lenses"
Provenance: [persona-adaptive](../04-research/dual-audience/persona-adaptive.md) (R-16). **A lens is a named
bundle of configuration over one component + one record set — never a fork.** Five config axes:
(1) projection, (2) density, (3) vocabulary, (4) visible fields/grouping/sort/filter, (5) default-landing/chrome.
- **The falsifiable D5 invariant:** two lenses differ *only* by values on axes 1–5; a difference not
  expressible as an axis value **is a fork = the failure.** The engineer board and the PM roadmap are the
  same `<Views>` component, a few config values apart (the literal D4=4 / D5=4 proof of direction A).
- **Two failure modes to catch:** the fork (two components), and the **averaged "serves-neither"
  compromise** — *a lens that is only acceptable to its persona has already failed; each lens must be
  excellent for its persona, not an average.*
- **Vocabulary is bounded** (the fracturing control): **T1** schema/refs/API/CLI/search is **FROZEN** (one
  canonical term); **T2** lens label is a bounded curated synonym set; **T3** free per-tenant rename is
  discouraged/audited. It is a presentation label-map, never a schema fork.
- *Carried:* whether coarse same-schema records satisfy PMs (vs a separate narrative tool) is the rank-1
  `[DEFERRED-UNTIL-USERS]` falsifier; every dual-audience surface must be sketched in *all* its lenses over
  the same data or it cannot be scored on D5.

### 5.3 The unglamorous-states system
Provenance: [state-craft](../04-research/craft/state-craft.md) (R-21). The six required states
(design-language §5.10) — **empty · loading · error · permission-denied · erased · agent-pending** — plus
eight the happy-path bias skips (degraded · stale/reconnecting · optimistic-rollback · conflict ·
moved/outdated · cross-cell · storm · no-results). **Every component and view implements its applicable set.**
- **The four-word house rule:** *never blank, never blame, never leak, never lie.* (blank→loading/empty;
  blame→error; leak→permission/erased; lie→optimistic-rollback/conflict/stale.) Fail at the **smallest scope.**
- **Empty = onboarding-forward** (next action front-and-center, ~2:1 instruction:delight; first-use vs
  cleared vs filtered are distinct). **Loading = a structure-matching skeleton — there is no spinner
  token in the system.** **Error = blame the *system* in one quiet line + a path, scoped, input never
  lost.** **Permission-denied = Restricted (403-analogue) vs Absent (404-analogue), policy's choice not the
  frontend's, never leaks.** **Erased/tombstoned = dignified, GDPR-aware, 0 recoverable PII, erased actor →
  "[erased user]"** (the highest-stakes sovereignty state). **Storm** is owned by the inbox; **conflict** by
  the collab editor; **rebase-orphan** by the diff.
- *Carried:* the correctness invariants (no-leak / lossless-resume / fails-static / never-silent-overwrite /
  honest-revert) are **PROVEN**, but whether each degraded state *reads* as lawful vs broken is HYPOTHESIS.

### 5.4 Perceived performance
Provenance: [perceived-performance](../04-research/visual/perceived-performance.md) (R-13). **Speed and calm
are the same discipline — both protect attention.** Every rule is checkable against a budget.
- **Latency budgets** *(PROVEN — RAIL/Nielsen):* keyboard action <~100ms; optimistic action paints <~100ms
  (server-ack reconciled async); **suppress flash-of-spinner <~1s — show a structure-skeleton, never a
  blank spinner**; **pages render, they don't animate in**; watched live-update ≤~240ms in-place (no
  scroll-jump/re-sort); a degraded surface **fails static**.
- **The three-state optimistic contract** + four binding rules: optimism **never hides failure**
  (rollback is *more* visible than settle); reversibility over confirmation **except** irreversible /
  consequential / GDPR-erase / agent-HITL still Confirm; never clobber an in-flight edit (→ conflict);
  idempotent under retry. (Implemented via Toast undo-host + button loading state + the settle/rollback
  motion.) This is the cross-cutting gap **the design system must ship** (§9) — finalist sketches proved
  read-time perf but not write-time.
- **Residency caveat** (P2/ADR-11): perceived speed is bought by optimistic UI + in-region edge + prefetch
  — **never global CDN replication of personal data.** Whether in-region edge buys "enough" felt speed for
  a worst-case cross-region collaborator is the **largest open question** here.

### 5.5 Sovereignty-as-UX
Provenance: [sovereignty-as-ux](../04-research/sovereignty/sovereignty-as-ux.md) (R-19). **This is the most
under-evidenced area in the corpus — no external design playbook exists.** Legal/architectural claims are
PROVEN; the UX choices are HOUSE STYLE, `[UNDER-EVIDENCED]` (§9).
- **The P9 three-question heuristic** (the D9 probe): "Where does this data live?" / "Who/what processed
  this and may see it?" / "Show me everything about this subject" — all answerable in-UI, by the right
  role, without leaving the product.
- **The four-tier residency cue ladder** (escalates with *risk*, never by default): T0 ambient quiet region
  token (*not* a flag emoji) → T1 hover detail (operator + key-control) → T2 inline cross-boundary transfer
  warning → T3 cross-cell provenance tag. **Residency ≠ sovereignty** (who can decrypt, not just where).
- **The DSR console** (DPO side): five tabs = the five `PersonalDataHolder` ops (locate/export/rectify/
  restrict/erase = GDPR Arts. 15/20/16/18/17), **per-holder rows** (the legal completeness unit), a
  failure-isolated saga (never silent partial), an *explanatory* erase-consequence dialog, a **verifiable
  Merkle receipt** ("proof, not promise"), an escalating deadline clock. Plus the read+request-only
  **data-subject view** (power asymmetry visible as affordance) and the **audit-log explorer** (one log for
  human and agent actions, `correlation_id` causal walk). Direction A surfaces these **on-demand**; direction
  D makes them **always-on** (the `sovereigntyVisibility` flag, §8.4).
- **Tombstone honesty:** a tombstone states erasure happened (to a permitted viewer) but never reveals
  what/about-whom beyond the date; **no-access is deliberately indistinguishable from erased to an
  unauthorised viewer** (distinguishing would leak existence).
- *Carried:* the regulated-buyer (P13/P14) review is the validation plan; **"a DPO trusts it at a glance"
  is the unproven keystone.** `[OPEN — LEGAL]` residuals (third-party free-text PII, the audit carve-out,
  Art. 17 reach into immutable git) are surfaced honestly, not resolved.

---

## 6. Accessibility & i18n

**Not enhancements — baseline.** Provenance: [audit-method](../04-research/accessibility/audit-method.md)
(R-17), [i18n-rtl-patterns](../04-research/accessibility/i18n-rtl-patterns.md) (R-18).

- **The bar** *(PROVEN):* the **hard gate is WCAG 2.1 AA via EN 301 549**, legally enforceable under the
  **EAA (in force 2025-06-28)** — *market eligibility, not polish*. The **house target is WCAG 2.2 AA**
  (2.2 ⊇ 2.1). Our market is EU public-sector procurement, where this is a legal eligibility requirement.
- **Two-pass audit:** automated CI (axe-class) is a regression net catching only ~30–40% of issues; the
  **manual expert pass is the real gate**; evidence is demonstrated, not claimed. **G1-pass is necessary,
  not sufficient — AT-user testing is `[DEFERRED-UNTIL-USERS]`.**
- **Keyboard:** everything actionable by mouse is actionable by keyboard (P3); no traps; logical tab order;
  a discoverable `?` cheat-sheet. The seven hard components (diff · board-drag · views-inline-edit ·
  block-editor · HITL-card · command-palette · nested-overlays) each carry a keyboard + screen-reader entry.
- **Visible focus:** the **one derived `focus-ring` token, on every interactive element** (incl. the
  autofocused palette input), ≥3:1 in light/dark/high-contrast. `focus-ring === accent < 3:1` is a Blocker.
- **Status never by colour alone** *(WCAG 1.4.1):* glyph + label + position; no saturated fills. **The
  agent treatment is four-channel** for the same reason (§5.1).
- **Reduced motion** is a first-class spelling, never a degradation (§3.5, L4). **`forced-colors` /
  high-contrast** is a token-layer default (`tokens.css` ships the `@media (forced-colors: active)`
  fallback), inherited by every component. Every **skeleton sets `aria-busy` + announces via one debounced
  polite live region** — a system default, not opt-in.
- **i18n / l10n / RTL** *(an English-only or LTR-only build is ineligible for the EU market):* externalised
  strings; **text-expansion budget** (German +30–40%, short strings up to 2×; pseudo-localization as the
  pre-flight test); **no fixed-width text containers, no truncation of essential labels.** **Full RTL via
  CSS logical properties** (inline-start/end) — **physical left/right is banned (lint rule)**; RTL correct
  by construction, tested with a **real Arabic/Hebrew string with a mixed-direction run**, not a flipped
  mockup; bidi-isolate LTR runs (code/SHAs/`ArtifactRef` stay LTR in RTL prose); mirror layout +
  directional icons, **never** mirror logos/clocks/checkmarks. **Never hand-format dates/numbers — use
  `Intl`/CLDR**; SLA/business-calendar awareness is load-bearing (display in the viewer's locale, **compute
  breach on the policy's calendar**). Machine strings are humanised **at the backend** (templates, not
  concatenation; never a frontend id→name map) — §7.

---

## 7. Voice & do/don'ts

Concise rules. Where a rule has a source, it carries its tag.

**Do:**
- **Express selected/active as `--surface-hover` fill + brighter text** (optionally an accent-tinted glyph)
  — a non-colour difference. *(REFINEMENTS [R1](./REFINEMENTS.md), **binding**: the user dislikes rounded
  colored side borders. **No colored side-bar / inset accent edge marker** as the selected/active indicator.
  If an edge marker is ever genuinely needed it must be **square-cut, no border-radius** — the exception,
  not the default.)*
- **Make agents look like agents** — the four-channel treatment, a plain geometric mark, attribution.
- **Blame the system, never the user**, in one quiet line + a path; preserve typed input through every error.
- **Hierarchy from weight & colour before size**; spacing on the 4px ramp; one rationed accent.
- **Render, don't animate in**; keep motion functional, fast, interruptible.
- **Show structure while loading** (skeleton matching the final layout) — never a blank spinner.

**Don't:**
- **No sparkle / shimmer / magic-wand / star AI iconography. No emoji as UI** (it can't inherit
  `currentColor` or re-theme). *(HOUSE STYLE; the legibility duty is PROVEN — AI Act.)*
- **No status by colour alone**, no saturated traffic-light fills ("the screen is not a traffic light").
- **No off-ramp spacing** (5/7/13px), no big-type-for-hierarchy — the amateur tells.
- **No colour via inline style on an interactive element** (inline beats `:hover`/`:focus` specificity) —
  interactive colour comes from tokens/utility classes only. *(PROVEN.)*
- **No physical left/right in CSS** (breaks RTL), no fixed-width text containers (breaks expansion).
- **No leaking** a title/name/existence through a permission-denied or erased state; **no lying** with
  optimism that hides failure.
- **No spring/bounce/overshoot, no confetti, no looping/ambient motion, no decorative hover-scale/parallax.**
- **No `switch(direction)` in a component** — read the flags + semantic tokens (§8.4).

---

## 8. From design to code

Stack frozen in [`00-framework-and-buildout-plan.md`](./00-framework-and-buildout-plan.md) §1; the
implementation track consumes this without re-litigating it.

### 8.1 The token pipeline (the contract)
**DTCG `tokens.json` (source) → Style Dictionary v4+ → `tokens.css` (CSS custom properties, the runtime
components consume) + TS constants.** Three tiers; **components consume the semantic tier only.** Two CI
gates guard it: the **measured-contrast gate** (recomputes AA over the *generated* table — a brand accent
at ~2.8:1 fails) and the **focus≠identity derivation rule.** A token change is **one PR that updates every
frontend.** Self-hosted assets, **no CDN.**

### 8.2 React + React Aria
**TypeScript + React (function components + hooks).** **React Aria Components (Adobe)** supplies headless
behaviour + ARIA + focus management (the strictest available a11y, the deciding factor on a legal floor;
first-class i18n/RTL) — **and decides nothing about the look; the token layer decides every pixel**, which
is what keeps the primitive choice orthogonal to the direction choice. **WASM-Rust at the edges that earn
it** (`myelin-content` AST + sanitiser, `myelin-query` AST, diff render) so the *exact* Rust logic is
shared client↔server. **Build order: foundations → overlay primitives → shared components → surfaces** —
the overlay substrate first (§4.1) or you pay the most expensive retrofit.

### 8.3 The strok icon / emit pipeline
`strok/*.strok` → `bash build.sh` (`strok batch`) → `svg/` + `dist/sprite.svg` + `dist/manifest.json` +
`preview/`. Inline-SVG `currentColor` (no per-theme files); consume by registry name; the build fails loud
on a baked hex and is idempotent. The `manifest.json` registry drives icon pickers/search. *(Full detail §3.6;
do **not** edit `.strok`-derived SVGs by hand or touch the pipeline as part of UI work.)*

### 8.4 The live styleguide & the parameterization model
- **The styleguide is the no-drift reference** ([`03-styleguide/index.html`](./03-styleguide/index.html)):
  it renders from the **real generated `tokens.css`**, runs **stack-down**, and toggles theme + RTL live
  with **live-measured** contrast — so the reference cannot drift from the app, and the direction swap is
  *demoable* (re-point the `<link>`, watch every component re-skin).
- **The direction-parameter is the semantic token set + six variant flags** — **never a `switch(direction)`.**
  The flags: `density` · `nav` · `surfaceUnification` · `tone` · `agentPresence` · `sovereigntyVisibility`,
  each a prop/config with a token-backed default. Direction A's row is `compact · palette-led · one-skin ·
  utilitarian · ambient · on-demand`. **"Pick D instead of A" = point the build at D's `tokens.json` + flip
  the six flags to D's row** (the contrast gate re-validates; `always-on` lights up the sovereignty band
  built once behind the flag) — **no component code changes.** The hybrid is the same: A's token base +
  `one-skin · warm (serif on reading only) · foregrounded · always-on`. *(This is the load-bearing
  requirement of the whole phase — 00-plan §2.)*

---

## 9. Extending the system & governance

### Adding a token
Edit the **DTCG `tokens.json`** (the right tier — primitive for a new raw value, semantic for a new intent,
component only for a genuine per-component handle bound to a semantic). Regenerate via Style Dictionary.
**The measured-contrast + focus≠identity CI gates must pass** before it ships. Never hardcode a hex or add
an inline interactive colour. *(tokens.md §6.)*

### Adding an icon
`strok new strok/<name>.strok --profile icon` → draw inside the 2,2→22,22 live area reusing the family
vocabulary → add the registry row + `@meaning`/`@tags` header → `bash build.sh` → verify
`grep -L currentColor svg/*.svg` is empty → add it to the contact sheet → wire it into `USAGE-MAP.md`.
Kebab-case, meaning-named, subsystem-agnostic; one canonical glyph per meaning; **never a second style, never
the agent or status rails violated.** *(ICONS-README "add a new icon".)*

### Adding a component
Contribute it **down into the shared library** (no subsystem ships its own design system —
design-language §8.3 rule 1). Spec it against the fixed template (anatomy · variants + the 6 flags · ALL
states incl. the §5.3 set · keyboard+ARIA → React Aria primitive · semantic tokens · motion + reduced-motion
· do/don't), build on the overlay substrate, and reuse the badge / chip / form atoms rather than
re-implementing them.

### Quality gates (the acceptance bar)
- **Tokens:** measured-contrast + focus≠identity (§8.1).
- **A11y:** the two-pass audit; the seven hard components' keyboard + SR checklists; `forced-colors`
  fallback; `aria-busy` + one polite live region on every skeleton (R-17).
- **Editor:** `render(parse(md)) === md` over a corpus (a hard CI gate).
- **i18n/RTL:** the D-G2.1–D-G2.6 demonstration set (German, Greek/Cyrillic, mirrored RTL, `Intl` dates +
  one SLA surface, no machine strings, inspectable logical CSS); the no-physical-left/right lint (R-18).
- **Parameterization:** no `switch(direction)` in any component (the falsifiable review test, 00-plan §2.2).
- **The done-bar (the switch test):** a surface is done only when, by driving the **real UI in a browser**,
  a team could move to it without hitting a wall the old tool didn't have. The real palette + optimistic
  rollback are prerequisites to *ever* clearing it. *(design-language §8b.7.)*

### The cross-cutting capabilities the system must still ship
From decision-brief §7 — aspirational in the finalist sketches, **required** here: (1) the **command palette
as a real primitive** (real input · fuzzy-filter · run · focus-trap+Esc · roving `aria-activedescendant`);
(2) **optimistic-update + honest rollback** (§5.4); (3) **`forced-colors`** support; (4) **`aria-busy` +
live-regions on skeletons**; (5) the **focus token covers the autofocused palette control**.

### The honesty register — what is NOT user-validated
This is **expert design work, not user-validated.** The deferred-until-users research track is the
validation plan. Carried flags:
- **`[DEFERRED-UNTIL-USERS]` (system-wide):** correctness of contrast / RTL / i18n / state-invariants is
  **PROVEN-by-construction**; **comprehension, warmth, "loved," "trusted," and "calm-felt" are HYPOTHESES.**
  G1-pass ≠ usable-with-assistive-technology (AT-user testing deferred).
- **Sovereignty-as-UX is the most `[UNDER-EVIDENCED]` area** (§5.5) — no external playbook; "a DPO trusts it
  at a glance" is the unproven keystone (P13/P14 review pending); `[OPEN — LEGAL]` residuals surfaced, not
  resolved.
- **The agent contract's trust-calibration and the Edit-path UX are the least-evidenced** agent items;
  over-trust-from-fluency must be re-tested against the **real LLM runtime** (mock-agent trust may not
  predict real-LLM trust).
- **Dual-audience rests on the persona-adaptive-vocabulary bet** — whether coarse same-schema records
  satisfy PMs (vs a separate narrative tool) is the rank-1 falsifier.
- **The visual direction itself is recommended-not-chosen** (confirmed by the user, but the *reception* of A
  vs B/C/D is unvalidated) — which is exactly why the parameterization model (§8.4) keeps the choice cheap
  to revisit.

---

*End of the Myelin Design Manual. Direction A "Instrument," confirmed; runner-up D "Civic"; parameterized so
direction is swappable. Synthesizes the design-language, the 08 design system (tokens · components ·
styleguide · icons), the decision brief, and the 04 research corpus. Open the
[live styleguide](./03-styleguide/index.html) for the real thing. Not committed — the orchestrator commits.*
