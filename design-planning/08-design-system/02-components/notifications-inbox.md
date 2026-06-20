# Component spec — Notifications inbox ("what needs *me*")

> **Phase 8b · `02-components/` · Tier-2 shared component (shell-owned global surface).** Direction = finalist
> **A "Instrument"** (consumes [`../01-tokens/tokens.css`](../01-tokens/tokens.css)). **File date: 2026-06-20.**
> Stack: TS + React (function components) + **React Aria Components**. **Not committed.**
>
> **Implements:** design-language **§5.8** (the one prioritised cross-subsystem inbox) + **§6.5** (calm agent
> volume) + **§5.10** (states). Research it renders:
> [`shared-patterns.md`](../../04-research/interaction/shared-patterns.md) (R-10 §4 — one store, why-it-fired,
> ranking, dedup, the storm state) · [`attribution-and-calm.md`](../../04-research/agent-ux/attribution-and-calm.md)
> (R-15 §5 — calm-volume patterns, the surge) · [`state-craft.md`](../../04-research/craft/state-craft.md)
> (R-21 §1.13 — the storm experience render, which this surface OWNS).
>
> **Tagging:** **PROVEN** = a cited standard / vendor baseline, or an existing contract surfaced
> (notifications.md: one store §1.3, `origin_event`+`reason` §2.1, deterministic ranking §3.1, dedup §3.2,
> humanise §3.3, storm shed-budget §5.2, firehose resume §7). **HOUSE STYLE** = synthesis. `[DEFERRED-UNTIL-USERS]`
> = the storm felt-calm + ranking-trust hypotheses (flagged below).
>
> **Reuse:** the item **subject is a [`<ReferenceChip>`](./reference-chip-and-unfurl.md)**; the humanised
> "why-it-fired" string renders via the **[`<BlockEditor>`](./block-editor.md) render path** (one templating
> surface → one render path); an **HITL approval** item docks the **[`<AgentHitlCard>`](./agent-hitl-card.md)**
> as a row; the inbox-item-row molecule shares shape with the palette row + list-view row.
> **This surface invents no new mechanism — it surfaces the existing notifications architecture as UX.**

---

## 1. Name + purpose

**`<NotificationsInbox>`** — the ONE prioritised cross-subsystem inbox: the antidote to notification overload
(P8; the universal incumbent failure). The design-language one-liner, carried verbatim: ***"there is one
inbox; everything else is a saved filter on it."*** It aggregates mentions, review requests, assignments, SLA
warnings, HITL approvals, CI failures on my work, and agent proposals — across all five subsystems — into one
ranked, deduped, calm "what needs me" surface. *(PROVEN contracts; presentation HOUSE STYLE.)*

---

## 2. The binding rules surfaced from notifications.md (PROVEN — not new)

- **One store, one read-state truth (§1.3, C-9)** — exactly **one** inbox. "My Work", "Activity/Mentions",
  "Review requests" are **scoped `filter`s over one store**, never separate inboxes. *Read it in chat → it's
  read in the unified inbox* (one `state` column across every view).
- **"Why am I getting this" provenance on every item (§2.1, NOTIF-2)** — every item carries `origin_event` +
  structured `reason` (`mentioned`/`assigned`/`review_requested`/`sla`/`approval_requested`/`watched`/`replied`/
  `agent_proposal`/`state_changed`/`fyi`), rendered as a one-line humanised provenance ("Review requested by
  @ana on PR #88"). Answerable inline — not a new mechanism (beats the GitHub `reason` label / Linear's narrow
  set).
- **Deterministic, explainable ranking (§3.1)** — `priority 0..100` from `reason → base → class`
  (approval/escalated/sla = 90/critical; review/assigned/mentioned = 70/direct; replied/agent_proposal =
  55/participating; watched/state_changed = 35/watching; fyi = 15). **Deterministic-first** because an
  unpredictable ranking erodes trust faster than no ranking; "why ranked here?" must be answerable. ML-tuned
  ranking is a named follow-on behind the same interface — **the UI must not assume it**.
- **Dedup / storm-control is write-time (§3.2)** — N identical events collapse to one item with a "+N more"
  coalesce-count (`UNIQUE(tenant, recipient, dedup_key)`); thread/subject coalescing; self-notifications
  dropped. The UI shows **groups, not a firehose**.
- **Humanised strings, not raw ids (§3.3 / §8b.5)** — rendered per-viewer at read time by resolving each
  `ArtifactRef` through `resolve(Display)`: a confidential subject → "Alice updated a restricted issue" (title
  never leaks); an erased actor → "[erased user]". The frontend owns **no humanisation lookup** — same
  permission-safe / erasure-safe / always-current property as the chip.

---

## 3. Anatomy

```
┌──────────────────────────────────────────────────────────────────┐
│ {filter tabs: What needs me · Mentions · Review · …}   {⚙ tune}    │  ← saved filters over ONE store
├──────────────────────────────────────────────────────────────────┤
│ ◆ {kind} {subject = ReferenceChip}        {triage actions}  {when} │  ← item row
│   why: Review requested by @ana on PR #88                          │  ← provenance line (always)
│ ◆ {kind} {subject} · +23 more                              {when}  │  ← deduped group ("+N more")
│ ⬠ {AGENT} FixAgent · approval requested  [Approve][Edit][Reject]   │  ← HITL card docked as a row (critical)
└──────────────────────────────────────────────────────────────────┘
```
- **Item row** — kind glyph + label · subject `<ReferenceChip>` (live, permission-aware) · the **always-present
  provenance line** · one-action triage · timestamp (tabular-nums, locale tz). Unread indicator by
  glyph/weight + position, **not colour alone**.
- **Deduped group** — one row + "+N more", expandable to the N underlying events.
- **HITL row** — `reason=approval_requested` docks the `<AgentHitlCard>` at critical priority (a human gate is
  never missed).

---

## 4. Interaction spec

- **One-action triage** — each item: **done / snooze / mute / go (open)** — single-action, keyboard-first
  (`E` done, `H` snooze, `Enter`/`G` go, à la Linear; pointer-complete for PMs). `mark_all_read` on a filter.
- **Prioritised + grouped, calm by default** — default view is "what needs me" ranked; deduped groups collapse
  with "+N more"; **agent-generated volume routed OUT of the main stream** (threaded / collapsible summaries —
  the agent-mention lane is shed-budgeted separately so humans never queue behind agent runs). Quiet is the
  default; the user opts *into* more.
- **HITL approvals appear here** — the inbox is the durable gate's **second home** (R-14 §4).
- **Live updates** — new items stream over the firehose resume-cursor protocol; a reconnect loses zero items; a
  new item transitions in subtly (never a jarring jump).
- **Tunable** — per-type/per-scope preferences over the frozen `QueryAst` matcher; **quiet-hours** in the
  recipient tz; **critical pierces** (you cannot silence an on-call page). Default = quiet.

---

## 5. Variants + parameterization variant flags

- **`agentPresence` flag (`ambient`↔`foregrounded`)** — sets how agent volume surfaces: `ambient` (A's
  default) → the storm-grouped collapsed agent-activity line + gates inbox-first; `foregrounded` → agent
  proposals more present as visible rows. **Legibility/attribution/gating are constant** across the flag
  (Axis 5 invariant).
- **`density` flag** — item-row height / provenance-line size via `--row-h`, `--space-*`; compact is A's default.
- **`tone` flag** — the empty/inbox-zero copy voice only ("You're all caught up").
- **NOT affected:** `nav`, `surfaceUnification`, `sovereigntyVisibility`. **No `switch(direction)`.**

---

## 6. ALL states

| State | Behaviour | Source |
|---|---|---|
| **Empty (inbox zero)** | a calm, *rewarding* "You're all caught up" — not an onboarding nag; the calm-by-default payoff. **No confetti.** Voice per `tone`. | §5.10 / R-21 §1.1 |
| **Loading** | item-row skeletons matching the final list, `aria-busy` + polite live region; never a spinner; suppress flash <~1s. | §8b.6 |
| **Error** | one quiet **system-blaming** line + retry; **fails static** — already-materialised items still render on a hiccup. | notifications.md §5.3 |
| **Permission-changed subject** | an item whose subject the viewer just lost access to humanises to a tombstone / "restricted", **never a leaked title** (D-N4). | ADR-03 / §3.3 |
| **Erased actor/subject** | "[erased user]" / restricted tombstone — references-not-payloads makes this free (D-N6). | ADR-12 |
| **Deduped group** | "+N more" coalesce; expandable to the N underlying events. | §3.2 |
| **Agent-pending** | "an agent is working / awaiting your approval" — the gate-awaiting HITL row at critical priority (R-14). | §5.10 / §6 |
| **Degraded** | a subject chip that can't refresh shows last-known + "can't refresh" dot; the inbox stays live (fails static). | R-09 §5.10 |
| **Stale / reconnecting (firehose drop)** | resume from `last_seq` (backfill then live); `resync_required` → full `list_inbox` reload, **named not silent**; last-known items stay visible. | notifications.md §7 / D-N11 |
| **Storm / 30×-agent-surge (THIS SURFACE OWNS IT)** | the **agent lane sheds first** (`429 + Retry-After`); the **human-direct inbox stays in budget**; the UI shows (a) human-direct items at top, untouched; (b) a **single coalesced agent-activity group** ("42 agent updates — expand") not 42 rows; (c) a calm, **non-alarming** surge indicator (no red firehose). **The render is the proof of the agent-native calm claim.** | notifications.md §5.2 / D-N5; R-21 §1.13 |
| **No-results (filtered)** | "No items match this filter" + clear-filter path; **permission-honest** (never reveals hidden matches exist). | R-21 §1.14 |

---

## 7. Keyboard + ARIA model (a named G1 surface)

- **List structure** — React Aria **`GridList`** (rows with focusable triage actions) or **`ListBox`**;
  filter tabs = **`Tabs`**; per-item triage = **`Button`**/**`MenuTrigger`**; tune = a **`Popover`**/**`Dialog`**.
- **Keyboard-first triage** — `j/k` move, single-key actions on focus, `Enter` to open; **pointer-complete**.
  Roving tabindex within the list; one Tab stop; `Tab` exits to the next shell region (no trap).
- **Live-region announcements** of **new high-priority items without spamming** — announce **critical/direct**,
  not every `fyi` (the polite-region discipline). The storm surge announces the *group*, not 42 rows.
- **Status / priority not by colour alone** — glyph + label + position; unread by weight/indicator + position.
- **The HITL row** inherits the `<AgentHitlCard>` keyboard/ARIA model (approve/edit/reject reachable).
- **Reflow / RTL** — rows + provenance lines reflow at 200%/320px; logical properties mirror RTL; triage
  actions on inline-end.
- **Cognitive walkthrough:** a new PM can tell *why* an item is here (the provenance line) and *what to do*
  (the visible triage actions) — not memorised.

---

## 8. Semantic tokens consumed

| Purpose | Token(s) |
|---|---|
| Inbox surface / row dividers | `--surface`, `--surface-raised` (group headers), `--border` |
| Row hover / unread | `--surface-hover`; unread indicator `--accent` (identity) + weight + position |
| Subject / provenance / when | `--text-primary` (subject), `--text-subtle` (why-line, when) |
| Subject chip | `<ReferenceChip>` tokens (`--c-chip-*`) |
| Priority class (critical/direct/…) | glyph + label; critical accented `--danger`/`--warning` **with label**, never colour-alone |
| **Agent** (agent-activity group, HITL row) | **`--agent`** / `--on-agent` / `--agent-subtle` / `--c-agent-mark` |
| Triage actions | neutral buttons; `go` primary `--c-btn-primary-bg` |
| Surge indicator | **calm** — `--text-muted` + `--agent-subtle`, **never** a red firehose banner |
| Focus | `--focus-ring` |

Binds only to semantics / chip + agent handles.

---

## 9. Motion (token-based, reduced-motion first-class)

- **New item enters** — `--dur-base` `--ease-enter`, subtle, no scroll-jump while reading.
- **Triage (done/snooze)** — the row leaves with `--dur-fast` `--ease-exit`; an **undo toast** (Tier-1 Toast,
  never steals focus) carries the reversal.
- **Group expand / surge coalesce** — `--dur-fast` `--ease-standard`.
- **Inbox-zero** — a **quiet** settle, **no confetti / no celebration burst** (calm even at the payoff).
- **No bounce/sparkle.** **`prefers-reduced-motion`** → 0; items appear/leave instantly + announce.

---

## 10. Usage do / don't

**Do**
- Keep **one store**; every view a **saved filter** over it (one read-state truth).
- Put the **why-it-fired** provenance line on **every** item, always.
- Rank **deterministically + explainably**; keep "why ranked here?" answerable; don't assume ML ranking.
- Dedup into groups ("+N more"); route agent volume **out of the main stream**.
- Under storm, keep human-direct items unburied at top + collapse agents into one calm group.
- Let critical pierce quiet-hours; keep triage one-action + keyboard-first + pointer-complete.

**Don't**
- Don't give any subsystem its own inbox/store (the exact failure the platform exists to fix).
- Don't leak a subject title the viewer lost access to (humanise to tombstone/restricted).
- Don't ship a black-box ranking that buries a critical item under `fyi`.
- Don't flood the human inbox under agent load (fail the one thing incumbents can't do).
- Don't use an alarming red surge banner; don't celebrate inbox-zero with confetti.
- Don't `switch(direction)`.

---

## 11. Honesty — PROVEN vs HOUSE STYLE vs deferred

- **PROVEN:** one store / one read-state truth; `origin_event`+`reason` provenance; deterministic explainable
  ranking; write-time dedup + storm shed-budget (humans never queue behind agents); humanise-at-source
  (permission-/erasure-safe); firehose lossless resume; critical-pierces. Every behaviour maps to
  notifications.md / drills (D-N1/D-N4/D-N5/D-N6/D-N11) — **invents nothing**.
- **HOUSE STYLE:** the row layout + triage choreography; the storm *experience render* (the single coalesced
  agent group + non-alarming surge indicator); the calm inbox-zero.
- **`[DEFERRED-UNTIL-USERS]`:** is the **storm render felt as calm** under real surge — can users still
  find/act on a human-direct item, and does the coalesced agent group read as calm vs alarming? (the
  shed-budget is PROVEN; *felt-calm* is the hypothesis). Does the **deterministic ranking** surface the right
  things — `important_buried_rate` on real traffic (ML ranking is gated behind this measured signal). Does the
  **erased / restricted** subject read as lawful vs broken (inherits R-09 §11). Method: per-segment RITE +
  a simulated 30× surge on the Phase-6 finalist that ships this.

*End. Component spec HOUSE STYLE over the PROVEN notifications.md contracts + design-language §5.8/§6.5;
subjects are `<ReferenceChip>`s, why-strings render via `<BlockEditor>`, HITL docks `<AgentHitlCard>`. OWNS the
storm/30×-surge state. Consumes the finalist-A token set incl. the reserved `--agent` token. Not committed.*
