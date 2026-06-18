# Phase 2 — The Myelin Shared Design Language

> Phase: `02-holistic-architecture`. Canonical brief: [`VISION.md`](../../VISION.md)
> (single source of truth; never contradicted). Companion Phase-2 docs:
> [`architecture-decisions.md`](./architecture-decisions.md) (the ADR register) and
> [`system-overview.md`](./system-overview.md) (how the parts interact). Phase-1 inputs:
> [`personas.md`](../01-research/personas.md), [`use-cases.md`](../01-research/use-cases.md),
> [`competitive-landscape.md`](../01-research/competitive-landscape.md),
> [`technical-structuring.md`](../01-research/technical-structuring.md),
> [`agent-native-design.md`](../01-research/agent-native-design.md),
> [`gdpr-eu-sovereignty.md`](../01-research/gdpr-eu-sovereignty.md), and the five
> [`subsystem-deep-dives/`](../01-research/subsystem-deep-dives/).

This document is the **coherence backbone for every Myelin frontend**. "Top-of-the-line UX and
design" is a VISION §3 non-negotiable, and VISION §3/§5.2 forbid frontend code without a design
sketch behind it. The shared design language is what keeps five subsystems — built by different
Phase-4 agents, possibly in different stacks — feeling like **one product**, and what makes the
two platform-defining ideas (the cross-artifact wedge and agent-native interaction) *visible and
trustworthy* in the interface.

It is deliberately **opinionated**. Where the ADRs decided backend boundaries, this doc decides
the UX point of view and the recurring interaction patterns, and names what Phase-4 design
sketches must produce. It stays at principles+structure altitude — it gives a *direction* for
tokens and a *catalogue* of components and views, not a finished design system (that is a Phase-4
deliverable per subsystem, reviewed before UI code).

---

## 0. How to read this document

| § | What it gives you |
|---|---|
| §1 | **Design principles** — the product's point of view; what it must feel like. |
| §2 | **The dual-audience problem** — serving keyboard-first engineers *and* approachable PM/corporate at once. |
| §3 | **Design tokens direction** — color/theming/dark mode, typography, spacing, elevation, motion (principles, not a dump). |
| §4 | **Accessibility & i18n baseline** — WCAG target, keyboard, focus, contrast, RTL, multilingual. |
| §5 | **Shared component & interaction patterns** — the navigation shell, command palette, reference chip/unfurl, agent/HITL card, comments/mentions, the tables/boards/views component, the block editor, the notifications inbox. |
| §6 | **How agents are surfaced** — labeling, plan-then-apply, HITL gates, attribution/audit. The agent-native UX contract. |
| §7 | **The consolidated catalogue of views** — every primary screen across all five subsystems + shared/admin, for Phase-4 sketching. |
| §8 | **Frontend stack direction** — recommendation, rationale, and how it stays coherent. |
| §9 | **Open questions carried forward.** |

---

## 1. Design principles — Myelin's point of view

These nine principles are the lens every screen is judged against. They derive directly from the
positioning ("Linear-and-Notion-grade UX with Jira-grade depth, EU-sovereign, agent-native" —
`competitive-landscape.md §7`) and the personas (`personas.md`).

### P1 — One product, not five tools (coherence is the feature)
The differentiator *is the integration* (VISION §1; `competitive-landscape.md §6`). The UI must
read as one platform: one navigation shell, one command palette, one identity/avatar treatment,
one reference chip, one comment thread, one editor, one views component — shared across git, CI,
issues, knowledge, chat. A user must never feel they "left one app and entered another." This is
the UI embodiment of the three glue contracts (ADR-13): the same `ArtifactRef` renders the same
chip everywhere; the same `Principal` renders the same identity badge everywhere. Avoid the
Atlassian "stitched-together" feel (`competitive-landscape.md §6.1`) at the *visual and
interaction* layer, not just the backend.

### P2 — Speed is a feature; the UI must feel instant
Linear is the explicit UX North Star and the reason teams switch is raw speed
(`competitive-landscape.md §3`; `issue-tracker.md`). Optimistic updates, local-first interaction
where feasible, sub-100ms perceived response on common actions, no full-page reloads, live
updates pushed via the event bus (`git-hosting.md §4`). Latency is a design defect, not a backend
detail. (Note the tension with the residency trade-off, ADR-11/§6.2 of the system overview:
no non-EU CDN for personal data — so perceived speed is bought with optimistic UI, in-region
edge, and prefetch, *not* global replication.)

### P3 — Keyboard-first, mouse-complete
Every primary action is reachable by keyboard (`Cmd/Ctrl-K` command palette everywhere, `j/k`
navigation, single-key actions on focused items, full keyboard nav of diffs, boards, tables, and
the editor). Engineers (P1–P5) live on the keyboard. *But* the same actions are fully reachable
by mouse/touch with discoverable affordances for PMs and corporate users (P6–P11) who do not
memorise shortcuts. Keyboard-first is never keyboard-only.

### P4 — Progressive disclosure: simple by default, powerful on demand
The startup founder (P1) and the regulated-enterprise admin (P15) use the *same* product
(`personas.md §6`). Sensible, opinionated defaults are visible; depth (custom workflows, SLA
config, permission schemes, governance) is one layer down, never in the newcomer's face. This is
how we get "Jira's power with Linear's speed" (`competitive-landscape.md §3`) — the power is
present but not *imposed*. Configurable governance "scales from invisible to fully controlled
without forcing the startup through enterprise complexity" (`personas.md §6`).

### P5 — Density is earned, not default
Engineers want information density (a diff, a board, a log); novices want breathing room. Default
to *comfortable* density with a global **compact mode** toggle (and per-view density where it
matters: diffs, tables, logs). Density must never come at the cost of touch targets or
readability for the PM/corporate audience.

### P6 — Reference everything, everywhere (the wedge, made visible)
Any artifact (commit, PR, issue, doc, doc-block, CI run, CI step, chat message) is addressable as
an `ArtifactRef` (ADR-13) and therefore *mentionable and unfurlable from anywhere*. The reference
chip and the rich unfurl are **the most important shared components in the platform** (§5.3):
they are how the cross-artifact graph becomes tangible. A reference is always live,
permission-aware per viewer, and backlinked. "Cross-references rot" is the #1 pain we kill
(`personas.md` P1).

### P7 — Agents are visible, labeled, and trustworthy (never magic, never hidden)
Agent-native is a VISION non-negotiable, and the security/DPO personas' (P12/P13) deepest fear is
ungoverned automation (`personas.md`). The UI's answer: agents are **always visibly labeled as
agents** (AI Act, ADR-08); they **propose before they act** (plan-then-apply, ADR-08); consequential
actions pass through a **human-in-the-loop approval card** (ADR-08/ADR-09); every agent action is
**attributed and audit-linked** like a human's. Trust is built by making the agent's reasoning,
proposed effects, scope, and provenance *legible* — never by hiding them behind a "magic" button.
This is §6, the agent-native UX contract.

### P8 — Calm by default; attention is sacred
Notification overload is the universal incumbent failure (`competitive-landscape.md §1/§5`;
`personas.md` P7). Myelin defaults to **calm**: one prioritised "what needs *me*" inbox
(Notifications, §5.8), storm-control and dedup (ADR-12 / Notifications), agent verbosity kept out
of the main timeline (`chat.md §2.5`). Quiet is the default; the user opts *into* more, never out
of a firehose.

### P9 — Trust through transparency (sovereignty & GDPR are UX, not fine print)
The EU-sovereign/GDPR promise (VISION §3) must be *felt* in the product, not buried in a settings
page. Data residency, lawful basis, who/what can see a thing, agent scope, audit trails, and
data-subject-rights tooling are **first-class, legible surfaces** (the DPO P13, security P12, and
admin P15 personas are gatekeepers, `personas.md §4`). "Where does this data live?", "who
processed this?", "show me everything about this subject" are answerable *in the UI*. Privacy-by-
default (ADR-12): private visibility, opt-in telemetry, minimal retention — all reflected in
honest defaults.

---

## 2. The dual-audience problem (and how the design language resolves it)

The single hardest UX mandate in Myelin: the issue tracker — and to a lesser degree knowledge and
chat — must serve **engineers and PMs/corporate as co-equal audiences** (VISION §2;
`personas.md` P6; `issue-tracker.md §4`). The market's defining failure is the "engineering tool
vs. management tool" split (Jira/Linear for engineers, Productboard/Notion for PMs), which forces
PMs to maintain a parallel reality (`competitive-landscape.md §3`; `personas.md` P6). Myelin's
wedge is **one schema, many views** — the same data presented as a fast engineering board *and* a
PM roadmap, never two systems (ADR-06).

The design language resolves this with **persona-adaptive views over shared primitives**, not
separate products:

- **Same data, different lens.** The shared database/views component (§5.6, ADR-06) renders the
  *same* records as: a keyboard-driven list/board for engineers; a timeline/roadmap with
  outcomes/now-next-later for PMs; a portfolio rollup for executives. The lens is a view, not a
  fork.
- **Default lens by role, switchable by anyone.** A PM lands on roadmap; an engineer lands on
  their cycle board; both can switch. No one is locked out of the other's view.
- **Vocabulary that translates.** The same object is "issue" to an engineer and "work item" /
  "deliverable" in a roadmap context; terminology is a presentation choice over one model, surfaced
  per-space configuration — never a schema fork.
- **Density and chrome adapt.** Engineer surfaces are dense and keyboard-forward (P3/P5); PM/exec
  surfaces are more spacious, chart-forward, and pointer-friendly — *the same components* tuned by
  density tokens and default layout, not different code.
- **Approachability is a hard requirement, not a nice-to-have.** The Sourcehut lesson
  (`competitive-landscape.md §1`): developer-purist ergonomics that alienate non-engineers are a
  failure here, because PMs/corporate are *half the mandate*.

This principle generalises: **wherever a surface serves both audiences, build one component over
shared primitives and adapt presentation by role/density — never split the product.**

---

## 3. Design tokens — the direction (principles + structure, not a full dump)

Tokens are the atomic, themeable design decisions every component consumes. Phase 2 sets the
*system and direction*; the concrete palette/scale values are a Phase-4 design-system deliverable,
reviewed before UI code (VISION §3). The mandate: **one token system, consumed by every subsystem,
so coherence (P1) is mechanical, not a matter of discipline.**

### 3.1 Token architecture — three tiers
1. **Primitive tokens** — raw values (a color ramp `gray-50…gray-950`, a type scale, a spacing
   scale). Never used directly by components.
2. **Semantic tokens** — intent-named, theme-aware (`surface`, `surface-raised`, `text-primary`,
   `text-muted`, `border`, `accent`, `success`, `warning`, `danger`, `agent`, `focus-ring`).
   Components consume *only* these. This is what makes dark mode and theming a token-table swap,
   not a component rewrite.
3. **Component tokens** — optional per-component overrides bound to semantics.

This three-tier structure is what lets the same components render in light/dark/high-contrast and
(later) tenant-branded themes without touching component code.

### 3.2 Color & theming
- **Neutral-led, accent-restrained.** A long, carefully-spaced neutral ramp carries 90% of the UI
  (surfaces, text, borders); a single brand accent + a small set of *functional* colors
  (success/warning/danger/info) do the rest. This keeps dense engineering surfaces calm (P8) and
  approachable surfaces clean (P4).
- **Dark mode is first-class and co-designed, not derived.** Engineers live in dark mode; it is
  not an afterthought tint. Both themes are designed against the same semantic tokens from day one.
  A **high-contrast** theme variant is part of the accessibility baseline (§4), not a separate
  effort.
- **A reserved "agent" semantic color/treatment.** Agent-attributed content, agent proposals, and
  HITL cards share a *consistent, distinct, non-alarming* visual treatment (a dedicated `agent`
  semantic token family) so "this came from / awaits an agent" is recognisable at a glance
  everywhere (P7, §6). It must be distinguishable without relying on color alone (icon + label;
  §4).
- **Functional status colors are shared across subsystems.** CI green/red, PR open/merged/closed,
  issue state categories, SLA breach — drawn from one functional palette so "red means trouble"
  reads identically in CI, issues, and chat unfurls (P1).
- **Tenant theming (later) is bounded.** Tenant branding (logo, accent) is supported via the
  semantic layer but constrained so it can never break contrast or coherence — accessibility tokens
  are not tenant-overridable.

### 3.3 Typography
- **Two families: a UI sans + a monospace.** A highly-legible variable sans for UI/content (good
  at small sizes and high density), and a monospace for code, diffs, logs, SHAs, and inline code —
  monospace is load-bearing across git/CI/knowledge/chat. **EU-multilingual coverage is a
  selection criterion** (broad Latin-extended, Greek, Cyrillic at minimum; §4): the type stack must
  render European languages cleanly, not just English.
- **One modular type scale** shared platform-wide (e.g. a ~1.2 ratio set of named steps:
  `display / h1 / h2 / h3 / body / body-sm / caption / code`). Long-form knowledge content gets a
  reading-optimised measure and line-height; dense surfaces (tables, logs, diffs) get a tighter set
  from the *same* scale.
- **Variable fonts** preferred for weight range without payload cost; self-hosted (no third-party
  font CDN — a sovereignty/GDPR consideration, ADR-11/ADR-12: no personal data or request logs
  leaving the cell to a font host).

### 3.4 Spacing, layout & density
- **One spacing scale** (a 4px base grid: `0,1,2,3,4,6,8,12,16,24…`). Every margin/padding/gap is
  a scale step — no magic numbers. This is the mechanical basis of cross-subsystem rhythm (P1).
- **Density modes** (§P5): `comfortable` (default) and `compact` are token sets over the same
  scale, toggled globally and per-density-sensitive view (diffs, tables, logs, boards).
- **A shared responsive layout grid** and consistent breakpoints; the navigation shell (§5.1) owns
  the outer frame so every subsystem composes into the same skeleton.

### 3.5 Elevation, surfaces & borders
- **Borders-and-surfaces first, shadow sparingly.** Flat, layered surfaces (`surface`,
  `surface-raised`, `surface-overlay`) with restrained elevation read as modern and fast and keep
  dense screens calm. Shadow/elevation is reserved for genuinely floating layers (command palette,
  menus, popovers, the unfurl hovercard, toasts, the HITL card when it overlays).
- **Consistent radius scale** and one focus-ring treatment (§4) shared everywhere.

### 3.6 Motion
- **Motion is functional, fast, and interruptible** — it communicates state change (optimistic
  update settling, a card moving columns, a panel opening), never decoration. Short durations
  (≈120–200ms), standard easing tokens, and **`prefers-reduced-motion` honoured** as a first-class
  path (§4), not a degraded one. Live event-driven updates (a PR going green, an issue moving) get a
  subtle, non-jarring transition so the user notices without being interrupted (P2/P8). Agent
  proposals appearing/resolving get a consistent, recognisable motion (P7).

### 3.7 Iconography & data viz
- **One icon set**, consistent stroke/weight, with a stable mapping of icon→meaning across
  subsystems (the same "merge", "branch", "run", "doc", "channel", "agent" glyphs everywhere, P1).
- **One charting/data-viz language** for analytics (issue burndown, CI health, delivery metrics,
  SLA gauges, usage/quota) drawn from the functional palette — so a chart in CI reads like a chart
  in issues (P1; used by P7/P8/P11 reporting surfaces).

---

## 4. Accessibility & internationalisation baseline (non-negotiable)

Accessibility and EU-multilingual support are **baseline requirements**, not enhancements —
consistent with "top-of-the-line UX" (VISION §3) and the EU-sovereign mandate (Myelin serves EU
public-sector buyers, `personas.md §6`, for whom accessibility is frequently a legal procurement
requirement — EN 301 549 / EAA).

- **WCAG 2.2 AA is the platform target**, with AAA-level contrast pursued where feasible on
  primary reading and code surfaces. Public-sector readiness (EN 301 549) is the bar the design
  system is built to clear.
- **Full keyboard operability.** Everything actionable by mouse is actionable by keyboard (P3):
  command palette, navigation, diffs, boards, tables, the block editor, comment threads, the HITL
  approval card. No keyboard traps; logical tab order; documented shortcuts surfaced via a
  discoverable `?` cheat-sheet.
- **Visible, consistent focus.** One `focus-ring` semantic token, always visible on keyboard
  focus, meeting contrast minimums on every surface (light/dark/high-contrast).
- **Contrast as a token constraint.** Semantic text/background pairs are validated to meet AA
  contrast *in the token system itself*; the `agent` and functional-status treatments never rely on
  color alone (always icon + text label) so they pass for color-blind users.
- **Screen-reader correctness is a component contract.** Shared components ship correct semantics/
  ARIA once (the editor, tables, boards, unfurl cards, the HITL card, the notifications inbox),
  so every subsystem inherits accessible behaviour rather than re-implementing it. Live regions
  announce event-driven updates and agent proposals appropriately (without spamming).
- **Internationalisation & multilingual (EU-first).** Full i18n from the start: externalised
  strings, locale-aware dates/numbers/calendars (business-calendar awareness matters for SLAs,
  `issue-tracker.md`), and **first-class support for the major EU languages**. Search is
  multilingual (ADR-10/ADR-14). The product is designed to be *operated and read* in a user's own
  EU language, which is part of the sovereignty value proposition.
- **RTL support is built in, not bolted on.** Layout uses logical (start/end) properties
  throughout so right-to-left renders correctly; the navigation shell, editor, and views all
  mirror properly. (Flagged in `chat.md §3`.)
- **Reduced motion & other preferences** (`prefers-reduced-motion`, `prefers-contrast`, text
  scaling/zoom to 200% without loss) are honoured paths, not degradations.

---

## 5. Shared component & interaction patterns

These are the recurring surfaces that appear across multiple subsystems. **Each is built once,
against the shared tokens and (where relevant) the glue contracts, and reused** — this reuse is
the mechanical guarantee of coherence (P1). For each: what it is, where it recurs, and the key
interaction rules.

### 5.1 The navigation shell (the outer frame)
The persistent skeleton every subsystem composes into — so switching subsystems never feels like
switching apps (P1).
- **Structure:** a primary nav (subsystem/area switcher: Code · CI · Issues · Knowledge · Chat ·
  Inbox · Search), a contextual sidebar (the current subsystem's tree/list — repo tree, run list,
  issue views, space/page tree, channel list), the main content area, and an optional right-hand
  **context pane** (where cross-artifact references and details surface — §5.3, §5.5).
- **Constant elements:** the command palette trigger, global search, the notifications inbox entry
  (§5.8), the current `Principal`'s identity menu, tenant/space context, and the org/team/project
  scope indicator (which doubles as a residency/visibility cue, P9).
- **Rules:** the shell owns the layout grid and breakpoints; subsystems own only their sidebar +
  content; deep-linkable URLs for every artifact down to sub-artifact granularity (a diff line, a
  doc block, a CI step — ADR-13 `ArtifactRef`), because those links are what chat/issues/docs
  reference (`git-hosting.md §4.4`).

### 5.2 The command palette (`Cmd/Ctrl-K`)
The keyboard nerve-centre (P3), present on **every** screen, Linear/Notion-grade
(`competitive-landscape.md §3/§4`).
- **Unifies:** navigation (jump to any repo/issue/doc/channel/run), actions (create issue, open
  PR, transition issue, insert block, start review), and search entry (full-text + structured,
  permission-pre-filtered via `list-objects`, ADR-03 — you can only find what you may see, §5.7).
- **Composes the query AST (ADR-07):** structured filters typed in the palette build the *same*
  query AST as saved views and agent triggers — one query language, one parser, surfaced humanly.
- **Agent-invocable surface symmetry:** the actions reachable in the palette map to the same typed
  `ToolDef`s agents use (ADR-08) — humans and agents act through the same catalogue, which keeps
  capabilities consistent and auditable.

### 5.3 The reference chip + the artifact unfurl (the wedge component)
**The most important shared component in the platform** (P6; the literal embodiment of the wedge,
`competitive-landscape.md §6`; `chat.md §2.4` calls unfurls "the differentiator"). Two coupled
forms of rendering an `ArtifactRef` (ADR-13):
- **Reference chip** — the inline, compact form (an `@mention` of a person/agent, a `#issue`, a
  linked doc, a commit/PR/run). Shows type icon + current title/state + a status hint; click to
  open, hover to peek.
- **Unfurl card** — the rich, expanded projection: a PR with checks/reviewers, an issue with
  state/assignee, a doc section, a CI run with status, a chat thread. Includes **inline actions**
  where permitted (re-run a CI job, transition an issue, approve a PR) — unfurls are an action
  surface, not just a preview (`chat.md §3`).
- **Hard rules (these are platform law, ADR-13/ADR-03/ADR-12):**
  - **Live, not snapshot** by default — the unfurl is a *current* projection fetched/cached via
    the target subsystem's projection API and kept fresh by bus update events (cache invalidation
    per `ArtifactRef`). (Live-vs-snapshot is flagged `chat.md §2.4`; default live for correctness
    and erasure-safety.)
  - **Permission-aware per viewer** — resolved through Id's per-viewer check; if you can't see the
    target, you get a graceful "no access" card, never a leaked title (`chat.md §2.4`; ADR-03).
    This is the same correctness invariant as permission-aware reads (system-overview §5.2).
  - **Tombstones gracefully** — on erasure/deletion the chip/unfurl degrades to a tombstone, never
    a dangling leak (ADR-12; Refs tombstoning).
- **Recurs in:** chat messages (densest, `chat.md`), issue descriptions/comments, knowledge
  blocks, PR descriptions, CI annotations, notifications — *everywhere content lives*, because the
  reference/mention nodes are part of the shared content model (ADR-05).

### 5.4 The agent / HITL approval card
The trust-bearing surface of agent-native (P7, §6, ADR-08/ADR-09). Detailed in §6; summarised
here as a shared component: a visually distinct (the `agent` treatment) card that shows an agent's
**proposed effects** (plan-then-apply), its scope/identity/delegation, and — for consequential
actions — **Approve / Edit / Reject** controls that resolve a durable HITL gate. It surfaces
primarily *in chat* (the approval-card surface, system-overview §8.2) but the component is shared
and can appear inline on a PR, an issue, or in the notifications inbox.

### 5.5 Comments, threads, mentions & reactions
One conversation primitive across PR review, issue discussion, doc comments, and chat — so
"discuss an artifact" feels identical everywhere (P1).
- **One comment/thread model** over the shared content model (ADR-05): rich text, `@mentions`
  (people *and* agents, rendered as reference chips §5.3), `#artifact` references, code blocks,
  reactions.
- **Review batching** where it matters (start review → batch inline comments → submit verdict, à
  la GitHub — `git-hosting.md §4.2` calls this strongly preferred for review quality).
- **Mentions are notification + trigger surfaces:** an `@mention` of a person routes to their inbox
  (§5.8); an `@mention` of an agent is a trigger into the agent fabric (ADR-08; `chat.md §8`). The
  UI makes both legible.
- **Anchored comments:** comments can anchor to a diff line, a doc block, or a sub-artifact
  (`ArtifactRef#sub`), and survive/relocate sensibly (diff-anchoring is a P4 problem, TE-22).

### 5.6 The tables / boards / views component (the shared "database/views" surface)
One component renders the shared structured-collection primitive (ADR-06) — used by **both the
issue tracker and knowledge databases**, the platform's biggest reuse boundary
(`competitive-landscape.md §3/§4`; ADR-06).
- **View types (one component, multiple projections):** **table** (inline-edit, resize/reorder
  columns, add field), **board/kanban** (drag between columns, WIP limits, swimlanes), **calendar**
  (drag to reschedule), **list** (grouped/sorted/filtered, keyboard-navigable), **gallery** (card
  grid), **timeline/Gantt/roadmap** (ranges, dependencies). Enumerated in `issue-tracker.md §5.2`
  and `knowledge-platform.md §2.4/§3`.
- **A view = a query AST + grouping + sort + visible fields** (ADR-06/ADR-07), permission-aware by
  construction (ADR-03/ADR-07 — a view can never show rows the viewer can't see).
- **Saved views are first-class objects:** shareable, permissioned, per-user-overridable-vs-shared
  (`issue-tracker.md §5.4`; `knowledge-platform.md §2.4`).
- **Field-definition UI** (typed fields: text/number/select/date/person/relation/formula) is
  shared (ADR-06); the *engines* underneath (issue workflow/SLA vs knowledge formula/collab) are
  subsystem-owned and surface their own controls — but the table/field UX is one component.
- **Persona-adaptive (per §2):** the same component serves the engineer board and the PM roadmap.

### 5.7 Search & the find experience
Permission-aware, multilingual, cross-artifact search is a shared surface (ADR-03/ADR-10).
- **Two entry points, one engine:** the **command palette** (§5.2) for quick navigate/jump, and a
  full **search view** for query-building, filtering by type/subsystem/field, and results.
- **Permission-pre-filtered (never post-filtered)** via Id's `list-objects` (ADR-03) — "a user must
  never find or see what they cannot access" (system-overview §5.2). This is a *correctness and
  GDPR* property surfaced as a UX guarantee (P9).
- **Cross-artifact by default, scopable:** one search spans commits, issues, docs, runs, messages,
  with type facets; scopable to a subsystem/space/repo. Multilingual + (later) semantic/vector
  results (ADR-14).

### 5.8 The notifications inbox ("what needs *me*")
The one prioritised cross-subsystem inbox — the antidote to notification overload (P8;
`personas.md` P7; the universal incumbent failure, `competitive-landscape.md §1/§5`).
- **One inbox** aggregating mentions, review requests, assignments, SLA warnings, HITL approvals,
  CI failures on my work, agent proposals awaiting me — across all five subsystems (consumes the
  bus, ADR-04/ADR-12).
- **Prioritised, deduped, storm-controlled** (Notifications shared system): grouped by artifact/
  thread, with clear "why am I getting this" provenance and one-action triage (done/snooze/mute/go).
- **Tunable, calm by default** (P8): granular per-type/per-scope preferences; the default is quiet.
  Agent-generated volume is kept out of the primary stream (`chat.md §2.5`).
- **HITL approvals appear here too:** the inbox is a second home for the §5.4 approval card, so a
  human gate is never missed.

### 5.9 The rich-text / block editor surface
One editor component over the shared content model (ADR-05) — the writing surface for knowledge
pages, issue descriptions/comments, PR descriptions, and chat composition (each with the *same*
node taxonomy; concurrency differs per subsystem, ADR-05).
- **Block-based, Notion-class** (`knowledge-platform.md §2/§3`): slash-command menu (`/` to insert
  any block: heading, list, table, code, callout, image, embed, database-view…), drag-to-reorder,
  `@mentions` and `#artifact` references as **first-class inline nodes** (ADR-05 — the same
  mention/ref node everywhere, rendered as §5.3 chips), inline equations, code blocks with syntax
  highlighting.
- **One editor, many concurrency models:** knowledge gets full collaborative editing (CRDT/OT, a
  P4 decision TE-15); chat messages are small/mostly-immutable; issue descriptions are
  single-author-at-a-time — but they share the *editor component and AST* (ADR-05, "share the AST,
  not the editor engine"). The user experiences one writing surface.
- **Embeds:** a knowledge page can embed a live issue board (via the §5.6 views component over an
  `ArtifactRef`), an incident runbook can reference a CI run — embeds are reference nodes rendered
  inline (`knowledge-platform.md §1`).
- **Sanitisation + safe rendering** is a component responsibility (ADR-05), inherited by all
  consumers.

### 5.10 Cross-cutting state patterns (empty / loading / error / permission / erased)
VISION §3 explicitly requires empty/loading/error states in design sketches. The design language
standardises these as **shared patterns every component and view must implement**, so Phase-4
sketches have a checklist:
- **Empty** — onboarding-forward (first repo, first issue, first doc, first channel), guiding the
  next action; especially important for the low-friction startup persona (P1, `personas.md §6`).
- **Loading** — optimistic/skeleton, never a blocking spinner where optimistic UI is possible (P2).
- **Error** — honest, actionable, recoverable (retry, contact, what-happened); never a dead end.
- **Permission-denied** — the graceful "no access" treatment (§5.3), never a leak (P9/ADR-03).
- **Erased/tombstoned** — the GDPR-aware degraded state for deleted/erased artifacts (P9/ADR-12).
- **Agent-pending** — the "an agent is working / awaiting your approval" state (§6).

### 5.11 Identity, presence & attribution
One treatment for *who* (or *what*) across the platform (P1/P7).
- **One avatar/identity badge** for every `Principal` (human/agent/service), with the **agent
  treatment** (§3.2/§6) making agents unmistakable.
- **Presence/typing** (chat, collaborative editing) share one indicator language; these ride the
  firehose transport (ADR-04), not the durable bus.
- **Attribution everywhere:** every action shows its actor (including on-behalf-of/delegation for
  agents, ADR-08) and links to the audit trail (P9/ADR-12, §6).

---

## 6. How agents are surfaced in the UI — the agent-native UX contract

This is the visible half of ADR-08 (plan-then-apply) and the answer to P12/P13's deepest fears
(`personas.md §4/§5`). **Agent-native must be visible and trustworthy**; the UI is where trust is
won or lost. The contract every subsystem implements:

### 6.1 Agents are always labeled as agents
Every agent `Principal` (kind `Agent`, ADR-08) renders with the distinct **agent treatment**
(§3.2/§5.11) — a consistent badge/color/icon — wherever it appears: as a PR reviewer/author
(`git-hosting.md §4`), an issue commenter, a chat participant, a doc editor. This is an AI-Act duty
(ADR-08) and a trust primitive (P7). Agents are never disguised as humans; "an agent did this" is
always legible. Color is never the only signal (§4).

### 6.2 Plan-then-apply: agents propose, the UI shows the plan before the effect
Agents emit *proposed effects*, never direct side effects (ADR-08). The UI surfaces the **plan**:
"FixAgent proposes: open PR #88, link issue ENG-412, post to #incidents." The proposed effects are
shown as concrete, reviewable items (the §5.4 card) — what will change, on which artifacts, under
whose delegated authority. This makes both mock and real agents legible and is the same UX whether
the runtime is `MockAgentRuntime` today or `LlmAgentRuntime` later (the strategy-pattern payoff,
ADR-08).

### 6.3 Human-in-the-loop gates: the approval card
Consequential actions (merge, close, delete, deploy, anything on a protected surface, anything the
tenant policy marks sensitive — ADR-08; GDPR Art. 22 / AI Act) pass through a **HITL approval
card** backed by a durable workflow gate (ADR-09) that can wait minutes or days
(system-overview §8.2):
- **Surfaces:** primarily in **chat** (the approval-card surface, system-overview §8.2) and in the
  **notifications inbox** (§5.8), and can appear inline on the affected artifact.
- **Controls: Approve / Edit / Reject.** "Edit" lets a human amend the proposed effect before
  applying — the human stays in control of the *content* of the action, not just a yes/no.
- **Suggest-by-default:** the platform default is *propose*, with autonomy granted per-action,
  per-scope by policy owners (P12/P15) — never autonomous-by-default on consequential actions
  (ADR-08; `personas.md §5`).
- **Durable & non-blocking:** the gate persists; the human is reminded via the inbox; the agent
  run resumes on signal (ADR-09). A pending gate is visible, never silently lost.

### 6.4 Attribution & audit affordances
Every agent action is **attributed and audit-linked** like a human's (ADR-08/ADR-12). The UI
provides:
- **Per-action provenance:** who/what acted, on-behalf-of-whom (delegation), under which trigger,
  with the `correlation_id` threading a multi-step agent flow (system-overview §8.2). "Why did this
  happen?" is answerable inline.
- **A link to the tamper-evident audit trail** (ADR-12) from any agent (or human) action — the
  security/DPO/admin personas (P12/P13/P15) can trace any action to its origin.
- **Scope legibility:** an agent's current permissions/delegation and budget are inspectable, and
  org-level **agent governance surfaces** (which agents exist, what they may touch, kill switches)
  are first-class admin views (§7.6; `personas.md` P15).

### 6.5 Calm agent volume
Agents generate volume (review comments, triage updates, status posts). Per P8 and `chat.md §2.5`,
agent verbosity is kept **out of the main timeline** by default (threads, collapsible summaries,
the inbox) — agents are present and legible without drowning humans. Zulip-style topic threading is
considered specifically because agent participation raises volume (`competitive-landscape.md §5`).

> **The strategy-pattern UX payoff:** because the agent contract is plan-then-apply and the UI
> renders *proposed effects + gates + attribution*, the **exact same agent UI works for mock agents
> today and real agents later** — swapping the runtime changes nothing in the frontend (ADR-08).
> Building this UI now, against mocks, is how agent-native is "designed for, not bolted on" (VISION
> §3).

---

## 7. The consolidated catalogue of views (for Phase-4 design sketches)

This is the **complete list of primary screens across all five subsystems plus the shared/admin
surfaces**, consolidated from the subsystem deep-dives so Phase-4 design agents have a single
checklist. VISION §3/§5.2 requires design sketches (information architecture, key flows, and
wireframes of primary screens **with empty/loading/error states**, §5.10) for every screen before
UI code. Each view inherits the shared components (§5), tokens (§3), accessibility (§4), and the
agent surfaces (§6).

> Legend: every view also needs its **empty / loading / error / permission-denied / erased**
> states (§5.10) and is **keyboard-operable + accessible + i18n/RTL-ready** (§3–§4). Cross-refs
> point to the originating deep-dive.

### 7.1 Git hosting & code review (`git-hosting.md §4`)
- **Repository home** — overview, README render, branches/tags, clone/fork actions, activity.
- **File tree & file view** — fast navigation, syntax-highlighted view, permalink-by-SHA, **blame**
  (with ignore-rev), raw view, image/binary/LFS-aware rendering, large-file graceful handling.
- **History / commit views** — commit list per path, commit detail (diff, parents, signed-status),
  signature verification.
- **Compare view** — arbitrary ref/SHA-to-ref/SHA diff.
- **Code search** — symbol/semantic/text (permission-pre-filtered, §5.7).
- **PR overview** — description, linked issues/docs/runs (the **PR context pane**, the wedge
  flagship, system-overview §8.1), participants, status, required-checks summary, merge readiness,
  event timeline.
- **Diff / files-changed view** — unified + split, syntax highlight, per-file collapse/viewed
  state, whitespace toggles, rename/move handling, large-diff handling, **inline + batched review
  comments**.
- **Review surface** — verdicts (approve/request-changes/comment), required-reviewer & CODEOWNERS,
  commit-by-commit + incremental ("changes since you last reviewed"), and the **agent-aware review
  surface** (§6.1 — visually distinct agent reviewers/authors, dismiss/override, audit trail).
- **Checks / CI integration panel** — live status from CI events, required vs optional, re-run
  actions.
- **Branch protection / ruleset editor** — ref patterns, required approvals, dismiss-stale, status
  gates (a deep, progressive-disclosure admin surface, P4).
- **Repo settings** — collaborators/teams (authz, §7.6), webhooks/triggers, SSH keys.

### 7.2 CI/CD (`continuous-integration.md` Appendix A)
- **Run list / dashboard** — per-repo and cross-repo, filterable (branch/status/actor/trigger),
  live status — the "is main green?" view.
- **Single-run view** — DAG/stage visualization, per-job/per-step status + timing, artifacts,
  re-run controls.
- **Live log view** — streaming tail (firehose transport, ADR-04), collapsible per step,
  search-in-log, secret-masked, downloadable.
- **Matrix view** — the fan-out grid, partial-failure highlighting.
- **Pipeline / definition editor + validator** — config-as-code editing with schema validation
  (avoid YAML-sprawl, `competitive-landscape.md §2` — typed/validated config is a differentiator).
- **Environments & deployments view** — what's deployed where, history, **approvals queue** (a HITL
  surface, §6.3).
- **Secrets management** — scoped, audited (P12; supply-chain provenance).
- **Usage / quota / billing view** — minutes/credits by repo/runner class.
- **Agent-surfaced triage view** — failures formatted for agent/human triage; an agent's proposed
  fix as a plan (§6.2).

### 7.3 Issue tracker (`issue-tracker.md §5`)
- **Issue detail view** — rich-text body (§5.9), properties sidebar (type/status/priority/assignee/
  labels/cycle/project/custom fields), relations & hierarchy panel, linked PRs/commits/runs/docs
  (§5.3), activity/comment timeline (§5.5), sub-issue checklist with progress, SLA timers,
  agent-suggested actions (§6).
- **List / board / table / timeline / calendar views** — the shared views component (§5.6):
  grouped/sortable/filterable list, kanban board (WIP limits, swimlanes), spreadsheet table,
  roadmap/Gantt (dependencies), calendar.
- **Cycle (sprint) view** — capacity, burndown.
- **Roadmap / portfolio view** — initiatives/epics over time, OKR linkage, executive rollups (the
  **PM/exec lens**, §2; `personas.md` P6/P8/P11).
- **Triage inbox** — Linear-style incoming queue with agent-assisted dedup/labelling (§6; P5).
- **"My Work" hub** — personal cross-subsystem work (overlaps the notifications inbox, §5.8).
- **Dashboards** — configurable widgets (charts, counts, SLA gauges) (P7/P11; §3.7 data viz).
- **Saved views management** — first-class shareable/permissioned views (§5.6).
- **Workflow / SLA / field-scheme admin** — custom workflows, SLA config, custom-field/scheme
  editors (deep progressive-disclosure governance, P4/P15).
- **Team page** — team-scoped work, members, health.

### 7.4 Knowledge platform (`knowledge-platform.md §3`)
- **The block editor** — Notion-class WYSIWYG (§5.9): slash menu, drag-reorder, all block types,
  mentions/refs, equations, code, embeds (incl. live database views and artifact embeds).
- **Database views** — table (inline-edit, resize/reorder, add property), board (drag cards),
  calendar (drag-reschedule), list, gallery, timeline; per-view filter/sort/group; **row peek /
  open-as-page** (the shared views component, §5.6).
- **Navigation / sidebar tree** — spaces → pages → sub-pages, favorites/pins, recent, breadcrumb;
  quick-switcher palette (§5.2).
- **Backlinks & references panel** — "linked references / mentioned in" on every page, hover-peek
  of referenced artifacts (§5.3) — the reference graph made visible (P6).
- **Page history UI** — version timeline, diff view, restore.
- **Templates UI** — page/database templates, "new from template", template gallery.
- **Sharing & permissions UI** — page-tree ACL inheritance with overrides, share-with-link, guest,
  public-publish (page published to web) (§7.6; ADR-03 page-tree inheritance).
- **Export UI** — per-page/space/workspace export to Markdown/open formats (portability, P14/ADR-12).
- **Search palette** — knowledge-scoped + cross-artifact (§5.7).

### 7.5 Chat (`chat.md §3`)
- **Channel / conversation list (sidebar)** — sections (channels, DMs, threads, mentions),
  unread/mention markers, sort.
- **Message timeline view** — virtualized infinite scroll (large-channel scale), grouped messages,
  inline unfurls (§5.3), reactions.
- **Composer** — rich text (§5.9) with slash-commands, `@mention` autocomplete (humans + agents +
  artifacts), paste-URL-to-unfurl, code blocks, file upload.
- **Thread pane** — side-by-side/overlay; where most agent/incident detail lives (calm-by-default,
  §6.5).
- **Unfurl cards** — the rich, live, permission-aware previews with inline actions (§5.3) — chat is
  the densest consumer (`chat.md`).
- **Mentions / "Activity" inbox** — everything aimed at me across channels (feeds the unified inbox,
  §5.8).
- **Search view** — messages + artifact-scoped, permission-filtered (§5.7).
- **Incident / "canvas" view** `[UNCERTAIN/DEFER, chat.md §3]` — pinned structured summary atop an
  incident channel.
- **The HITL approval-card surface** — chat is the primary home of agent approval cards (§5.4/§6.3;
  system-overview §8.2).

### 7.6 Shared, identity, admin & GDPR surfaces (cross-cutting)
These are platform-level views owned by the shared systems, surfaced through the shell (§5.1). They
make sovereignty/GDPR/agent governance *legible* (P9; `personas.md` P12/P13/P14/P15).
- **The unified notifications inbox** (§5.8) — "what needs *me*", cross-subsystem.
- **Global / cross-artifact search view** (§5.7).
- **Identity & profile** — the user's profile, preferences (theme/density/locale/notification
  prefs), sessions, tokens, SSH keys, MFA.
- **Org / team / project / space administration** — the hierarchy + membership surfaces; SSO/SCIM
  integration (P15).
- **Permission / role management (RBAC face over ReBAC)** — assign roles, view effective access,
  the authoring surface that compiles to relationship tuples (ADR-03). "Who can see/do what" is
  inspectable.
- **Agent governance console** (§6.4) — which agents exist, their identities/scopes/delegation/
  budgets, autonomy policy, kill switches, agent audit (P12/P15; ADR-08).
- **Audit log explorer** — the tamper-evident, searchable record of every human + agent action
  (P12; ADR-12), with provenance/correlation threading.
- **GDPR / data-rights console** — the **DSR orchestrator UI**: locate/export/rectify/restrict/
  erase for a subject across *all* holders, deadline tracking, verifiable receipts; operable by
  Myelin *and by tenants* (Art. 28) (ADR-12; system-overview §8.3). The DPO's (P13) primary surface.
- **Data map / RoPA & residency console** — the generated data inventory, residency/region view
  ("where does this tenant's data live"), lawful-basis/consent/sub-processor registries (ADR-12;
  P13/P14). Makes sovereignty visible (P9).
- **Tenant / cell & residency settings** — region binding, isolation tier, retention policy,
  self-host/sovereign-deployment surfaces (ADR-11; P15).
- **Onboarding & empty-platform flows** — first-run for the low-friction startup (P1) and the
  enterprise admin (P15); the empty states (§5.10) tied together into a guided start.
- **Billing / usage / export & exit** — usage, cost, and **data-portability/exit** surfaces (the
  anti-lock-in promise, P14/`competitive-landscape.md §6.2`).

### 7.7 The CLI as a first-class "view" (one design surface, two renderings)
Myelin is keyboard/CLI-first for engineers (P1–P5). The **CLI is a peer surface to the web UI**,
not an afterthought — every primary capability has a CLI verb, consistent across subsystems
(`myelin <subsystem> <verb>`). Consolidated from the deep-dives:
- **Git/PR:** `myelin repo create|clone|fork|list|view`, `myelin pr create|list|view|checkout|
  diff|review`, `myelin agent review request` (`git-hosting.md §4`).
- **CI:** run/list/view/retry, log tail, config validate (`continuous-integration.md`).
- **Issues:** `myelin issue create|transition`, `myelin view list|show|save` (the same query AST,
  §5.2/ADR-07) (`issue-tracker.md §5`).
- **Knowledge:** `myelin kb page list|get|create|edit|move|history|restore|export`, `myelin kb db
  row/view …`, `myelin kb search` (`knowledge-platform.md`).
- **Chat:** `myelin chat post|ref` and server-side slash-commands in the composer (`chat.md §3`).
- **Design coherence rule:** CLI output, error states, and reference rendering follow the *same*
  vocabulary and `ArtifactRef` scheme as the UI (`myelin://…`), so the chip you see in the UI and
  the handle you paste in the CLI are the same identity (P1/P6). The CLI is in scope for the
  consistency the design language enforces, even though its "tokens" are textual.

---

## 8. Frontend stack direction (open per VISION §4 — recommendation + rationale)

VISION §4 leaves the frontend stack open, decided in the design-language deliverable and refined in
Phase 4. ADR-02 notes "TS/React-class is the expected baseline but not mandated." Here is the
**recommendation with rationale**, framed so it stays coherent and can be refined per-subsystem.

### 8.1 Recommendation
- **TypeScript + a modern React-class component framework** as the default frontend stack for all
  web surfaces, with a **single shared component library + design-token package** in the monorepo
  (ADR-01) that every subsystem consumes. This is where the shared components of §5 physically live.
- **One design-system package** implementing the §3 tokens and §5 components, versioned in lockstep
  with the backend contracts (ADR-01 keeps API/types and the design language in the same workspace).
- **Type-safe, generated API/types** from the platform's wire contracts (envelope, `ArtifactRef`,
  query AST, `ToolDef`) so the frontend can't drift from the backend (ADR-01/ADR-02 rationale —
  contracts that can't silently drift).
- **WASM-Rust at the edges where it earns it:** performance-critical, logic-heavy client pieces
  that already have a canonical Rust implementation (the `myelin-content` AST + sanitiser, the
  `myelin-query` AST parser/validator, diff rendering) are candidates to **compile to WASM and share
  the *exact* Rust logic** between server and client — eliminating a class of client/server drift
  bugs and honouring the Rust-default ethos (VISION §4) without forcing the whole UI into a less
  mature Rust-UI ecosystem.
- **Self-hosted assets, no third-party CDNs** for fonts/scripts/analytics — a sovereignty/GDPR
  constraint (ADR-11/ADR-12: no personal data or request metadata leaving the cell, §3.3).

### 8.2 Rationale
- **TS/React-class maximises the talent pool and ecosystem** for the most design-intensive,
  fastest-moving layer, where Rust UI frameworks are still immature — a pragmatic divergence from
  the Rust default exactly where VISION §4 invites it (the frontend is explicitly open).
- **A single shared component library is the mechanical enforcement of coherence (P1)** — the same
  reason ADR-01 puts the glue in shared crates: components imported by every subsystem *cannot*
  visually drift; a token change is one PR that updates every frontend.
- **The dual-audience and accessibility mandates (§2/§4) are best served by a mature component
  ecosystem** with battle-tested accessible primitives, rather than reinventing accessible tables/
  comboboxes/dialogs in a young stack.
- **WASM-Rust sharing** is the principled answer to "the editor AST and query AST must behave
  identically on client and server" — share the implementation, not just the spec (ADR-05/ADR-07).

### 8.3 How it stays coherent (the rules)
1. **No subsystem ships its own design system.** All consume the one token package + component
   library. New components are contributed *to the shared library*, reviewed against §1–§6.
2. **Design sketches precede UI code** (VISION §3): every screen in §7 gets IA + flows + wireframes
   (with the §5.10 states) reviewed against this document before implementation (Phase 4 / execution).
3. **The glue contracts render through shared components only:** any `ArtifactRef` → the §5.3 chip/
   unfurl; any `Principal` → the §5.11 identity/agent badge; any structured collection → the §5.6
   views component; any content → the §5.9 editor/renderer. Subsystems don't re-render these.
4. **A subsystem may diverge in stack only with written justification** (mirroring ADR-02): if a
   surface genuinely needs a different rendering approach (e.g. a heavy code-diff canvas, a
   high-throughput chat virtualization), it still consumes the token package and renders the glue
   contracts via the shared components, so coherence holds across the seam.
5. **Tokens are the contract:** even a divergent surface theming via the semantic token layer (§3.1)
   stays visually coherent and supports dark/high-contrast/tenant-theming for free.

---

## 9. Open questions carried forward

Honest about uncertainty (VISION §3). The design language commits the *direction*; these resolve in
Phase 4 (per-subsystem design sketches + design-system build) or are genuinely undecided.

- **[OPEN → P4]** The concrete token *values* — exact palettes (light/dark/high-contrast), the type
  family selections (EU-multilingual coverage validated), the spacing/radius numbers. Phase 2 sets
  the system; Phase 4 designs and reviews the values.
- **[OPEN → P4]** The full **block taxonomy completeness** and extension mechanism for the editor
  (ADR-05 defers this; Knowledge leads, Chat/Issues consume).
- **[OPEN → P4]** **Unfurl live-vs-snapshot** edge cases and per-viewer unfurl resolution at scale
  (`chat.md §2.4/§5.4`) — default is live+permission-aware (§5.3); the caching/precompute strategy is
  a Chat/Refs P4 concern.
- **[OPEN → P4]** **Collaboration concurrency UX** for knowledge (CRDT vs OT, presence, conflict
  surfacing — TE-15) — the editor *component* is shared (§5.9); the live-collab interaction layer is
  a Knowledge P4 decision.
- **[OPEN → P4]** **Designer-persona depth** (P9): how far the UI goes into native design/canvas
  authoring vs *referencing* external tools (Figma) — a product-scope decision deferred per
  `personas.md §7`. Affects whether a "canvas" view joins the catalogue (§7.4/§7.5 flag it
  `[UNCERTAIN]`).
- **[OPEN → P4]** **Offline scope** (`issue-tracker.md §4`) — how far local-first/offline-tolerant
  the engineer surfaces go; impacts the optimistic-UI strategy (P2).
- **[OPEN → P4]** **Mobile / native-app scope** — VISION §3 says "web or apps"; the deep-dives
  assume "mobile-reasonable read views" (`git-hosting.md §4`). Whether full native apps are in scope
  (and which surfaces) is undecided; the responsive web design language (§3.4) is the baseline.
- **[OPEN → P4]** **Frontend stack divergences** — the §8.4 escape hatch may be exercised by Chat
  (high-throughput virtualization) or Git (diff canvas); to be justified in those subsystems' sketches.
- **[OPEN → P4/legal]** The **persona-adaptive vocabulary** mapping (§2) — how far per-tenant
  terminology customization goes without fracturing the shared model.

---

## 10. Cross-references
- [`VISION.md`](../../VISION.md) — §3 (top-tier UX, agent-native, GDPR/sovereign non-negotiables;
  design-before-implementation), §4 (frontend stack open), §5.2 (this deliverable's mandate).
- [`architecture-decisions.md`](./architecture-decisions.md) — ADR-03 (permission-aware reads →
  §5.3/§5.7), ADR-05 (shared content model → §5.9), ADR-06 (shared views primitive → §5.6), ADR-07
  (query AST → §5.2/§5.6), ADR-08 (plan-then-apply/agents → §6), ADR-09 (HITL durable gates → §5.4/
  §6.3), ADR-11 (residency → P2/§3.3/§8), ADR-12 (GDPR/audit → §6.4/§7.6/§9), ADR-13 (`ArtifactRef`/
  `Principal` → §5.3/§5.11), ADR-01/ADR-02 (monorepo/Rust-default → §8).
- [`system-overview.md`](./system-overview.md) — §5.2 (the two correctness invariants surfaced as
  §5.3/§5.7 UX guarantees), §8.1 (PR context pane → §7.1), §8.2 (agent flagship + HITL → §6),
  §8.3 (DSAR fan-out → §7.6 GDPR console).
- [`01-research/personas.md`](../01-research/personas.md) — the audiences §1/§2/§6/§7 serve.
- [`01-research/competitive-landscape.md`](../01-research/competitive-landscape.md) — the UX North
  Stars (Linear/Notion/Slack) and traps (Jira/Atlassian/Teams) §1/§2 steal-and-avoid.
- [`01-research/use-cases.md`](../01-research/use-cases.md) — UC-X-3 (PR pane), UC-X-4 (agent flow)
  exercised by the catalogue (§7).
- [`01-research/subsystem-deep-dives/`](../01-research/subsystem-deep-dives/) — the source of the
  view catalogue (§7): each subsystem's "Key UX / views required" section.
- **Seeds Phase 4:** the §7 catalogue is the per-subsystem screen checklist; the §9 open questions
  are each design agent's starting decisions; §8.3 is the coherence contract every frontend obeys.
