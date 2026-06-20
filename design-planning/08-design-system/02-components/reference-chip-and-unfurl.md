# Component spec — Reference chip + Artifact unfurl (the wedge component)

> **Phase 8b · `02-components/` · Tier-2 shared component.** Direction = finalist **A "Instrument"**
> (consumes the token system in [`../01-tokens/tokens.css`](../01-tokens/tokens.css)). **File date: 2026-06-20.**
> Stack: TS + React (function components) + **React Aria Components**; styled entirely by the semantic
> token layer (00-plan §1.2). **Not committed.**
>
> **Implements:** design-language **§5.3** (the reference chip + unfurl — "the most important shared
> component in the platform"; the three hard rules) · **§5.10** (cross-cutting states). Research it renders:
> [`reference-unfurl.md`](../../04-research/interaction/reference-unfurl.md) (R-09, the full spec) ·
> [`wedge-moments.md`](../../04-research/craft/wedge-moments.md) (R-22, W1/W2/W4/W5 — this component *in a
> flow*) · [`state-craft.md`](../../04-research/craft/state-craft.md) (R-21 §1.4/§1.5/§1.11/§1.12 — the
> degraded renders).
>
> **Tagging:** **PROVEN** = a cited standard, or an existing architecture contract this spec *surfaces*
> (the `myelin-refs` resolver ladder, ADR-13/03/12). **HOUSE STYLE** = our component-design synthesis.
> `[DEFERRED-UNTIL-USERS]` = a comprehension/trust hypothesis no expert pass settles (carried from R-09 §11).
>
> **Scope.** This file specs *this component*. It does **not** redesign the resolver (surfaced, not invented)
> nor re-spec the overlay Popover substrate (the foundational agent owns Tier-1; this component *consumes* it)
> nor the HITL approval card (see [`agent-hitl-card.md`](./agent-hitl-card.md) — the agent-pending unfurl
> *references* it).

---

## 1. Name + purpose

**`<ReferenceChip>` / `<ReferenceUnfurl>`** — the two render densities of **one resolved `ArtifactRef`**
(ADR-13). You place a reference once (an `@mention`, a `#issue`, a pasted `myelin://` / web URL, a `/embed`);
the platform resolves it **live, per viewer, every render**, and paints it **compact (chip)** inline or
**rich (card)** when there's room or on demand. **One component**, parameterised by *density* (chip ↔ card),
*artifact type* (PR / issue / doc / run / commit / thread / principal), and *resolver state* (§5).

This is the embodiment of **P6 (reference everything)** and the coherence proof (rubric D4): the same chip
must render identically in a board cell, an editor mention, an inbox subject, a PR context pane, and a chat
unfurl (R-10 §1 reuse invariant). *(PROVEN reuse invariant; framing HOUSE STYLE over the PROVEN resolver.)*

---

## 2. Anatomy

### 2.1 Chip (inline form)
`[ {type-icon} {status-glyph?} {humanised title / display key} {state-pill?} ]` — one inline run,
baseline-aligned, **never taller than a line of body text** (the calm rule, P8). Parts:

- **type-icon** — the one canonical icon per type from the self-hosted set (§1.5 / §3.7), inline SVG
  inheriting `currentColor`. **Shared with views, palette, breadcrumb, identity badge** (P1). It is the
  *persistent identity* and is **known before resolution** (it is in the URN `<type>`), so loading is type-true.
- **status-glyph** — for stateful types only (PR open/merged/closed, issue state, run pass/fail). **Glyph +
  label/position, never colour alone** (WCAG 1.4.1; §8b.3). Status colour is a *redundant* channel.
- **humanised title / display key** — the *current* title or render-time display key (`#1421`, `@alice`,
  `~general`), projected **per viewer per locale at the backend** (`myelin-refs` §4.8; design-language §8b.5).
  **Never a raw URN, never a bare id.** Truncates with ellipsis (full text in `title` + the peek); truncation
  survives German +35% and non-Latin scripts (G2; logical properties so RTL mirrors free).
- **state-pill** — present *only* in non-live states (moved / outdated / no-access / erased) — §5.

### 2.2 Unfurl card (rich form) — shared shell, type-specialised body
```
┌────────────────────────────────────────────────────────────┐
│ {type-icon} {humanised title}             {status badge}    │  ← header (always, all types)
│ {scope breadcrumb: org › repo/space/channel}   {residency?} │  ← context line (always)
├────────────────────────────────────────────────────────────┤
│ {type-specific body — see §2.3}                             │  ← body (varies by type)
├────────────────────────────────────────────────────────────┤
│ {inline actions, permitted only — §4}        {open ↗}       │  ← action bar
└────────────────────────────────────────────────────────────┘
```
**Header + context line + action bar are identical across all types** (the D4 coherence test); only the
**body** varies. The card is **collapsible**; in dense streams it defaults **collapsed to the chip** (§7).
*(HOUSE STYLE over PROVEN §5.3.)*

### 2.3 Card body per type (renders the `project(ref, viewer)` projection — no new backend fields)
| Type | Body (current, live) |
|---|---|
| **PR** | branch → base; **checks roll-up** (pass/fail/pending + per-check rows on expand); reviewers + state; linked issue/doc chips; +N/−M; mergeability |
| **Issue** | state · assignee · priority · cycle; sub-issue progress; linked PR/doc chips; SLA timer; last activity |
| **Doc / block** | breadcrumb; the **anchored block/section excerpt** (`b<id>`/`h<id>`); backlinks count; last-edited |
| **CI run / step** | status + duration; step roll-up; **failing step + log tail when `step-<n>`-anchored** (jump-to-failure); re-run affordance |
| **Thread / message** | channel + participants; anchored message excerpt; reply count; unread |
| **Person / Agent** | name + role (person); **agent** → the agent treatment (label+mark+token+attribution), scope/authority summary, link to governance console |

### 2.4 The hovercard (peek) — a *bounded* unfurl
Header + a 1–3 line body summary only — **no action bar** (an action bar in a transient hovercard is a 1.4.13
hazard and a mis-click hazard). To act, open the full card or the artifact. *(HOUSE STYLE; WCAG 1.4.13.)*

---

## 3. The three affordances (the chip's whole interaction surface)

1. **Open (primary)** — click / `Enter` → navigate (or open in the context pane). The chip is a **link**.
2. **Peek (hover *or* focus)** — the bounded hovercard (§2.4), **after ~300ms on hover, immediately on
   keyboard focus**. **Dismissable / hoverable / persistent** per WCAG 2.2 SC 1.4.13 (PROVEN).
3. **Act (secondary, permission-gated)** — a `···`/overflow surfaces inline actions (§4) *iff* permitted;
   otherwise the affordance is **absent, not disabled-greyed** (don't advertise unavailable power, don't leak
   the gate exists). *(HOUSE STYLE.)*

---

## 4. Inline actions (act where permitted) — the unfurl is an action surface, not a preview

A curated, high-value, low-risk subset per type (PROVEN-direction — R-01 §3.1 / design-language §5.3):

| Type | Inline actions | Risk class |
|---|---|---|
| **CI run / step** | Re-run (all / failed only); view log | low |
| **Issue** | Transition state; assign to me; add to my cycle | low/medium |
| **PR** | Approve / Request changes / Comment; re-request review (merge only from the full PR) | medium → routes through HITL shape |
| **Doc** | comment on block; copy ref | low |
| **Thread** | reply; react; mark read | low |
| **Agent proposal** | Approve / Edit / Reject (this *is* the HITL card as an unfurl — see `agent-hitl-card.md`) | high — plan-then-apply gate |

**The four hard rules (PROVEN where cited):**
1. **Gated by *presence*, not greying** — an action the viewer can't perform is **absent** (pre-filtered via
   ADR-03 `list_objects`, same guarantee as the palette).
2. **Optimistic + honest rollback** — applies instantly; on backend reject the card **reverts + one quiet
   system-blaming line** ("Couldn't re-run — you have view-only access"). The optimistic contract is the
   shared primitive (00-plan §4 gap-2; R-13 OPT-1..4). State §5 covers the rollback render.
3. **Reversibility over confirmation, with the carve-out** — re-run/react/assign → no dialog. Approve-a-PR /
   transition-to-Done / approve-an-agent-effect → route to the **Confirm/HITL** shape. A human suggestion and
   an agent proposal are the **same "approve a proposed effect" shape** (P1).
4. **The action carries provenance** — acting from an unfurl is attributed + audit-linked identically to
   acting on the artifact's own surface (the action is on the `ArtifactRef`, not a copy).

---

## 5. ALL states (the resolver→UI contract — this component OWNS several "unglamorous" states)

Every chip/unfurl is the rendering of **exactly one** state `resolve(ref, viewer, mode)` returns; the UI
**must render every state it returns and invent none** (`myelin-refs` §4.6 — the correctness guarantee).
Each state renders at **both densities**.

| # | State | Chip | Card | Owns/source |
|---|---|---|---|---|
| 1 | **Live** (default) | icon + current title + status hint | full card (§2.2) | the differentiator; bus-kept-fresh (§6) |
| 2 | **Peeking** | — | bounded hovercard (§2.4) | WCAG 1.4.13 |
| 3 | **Loading** | type-icon pill + **shimmer where title lands** (icon known from URN) | header skeleton + type-shaped body lines | structure-skeleton, never spinner; suppress flash <~1s (00-plan §4 gap-4; **`aria-busy`** + polite live region) |
| 4 | **No-access** (`Tombstone{denied}`) | `{icon} Restricted` — **no title** | "You don't have access to this {type}." + request-access path | **OWNED**; non-leaking *by construction* (resolver collapsed it before the wire — ADR-03). Restricted (you may know it exists) vs **Absent** (not rendered at all) is a *policy* input, not a frontend guess (R-21 §1.4) |
| 5 | **Moved** (`Projection + moved`) | "moved" pill | "Relocated; showing the current version" banner | **OWNED**; Git 3-way context match (`myelin-refs` §3.5) — the reference *followed the content* |
| 6 | **Outdated** (`Projection(partial) + outdated`) | "outdated" pill | surviving part + "some content has changed" | **OWNED**; never silently re-anchor to wrong content |
| 7 | **Tombstoned / erased** | `sub_gone`: "{parent} — that part is gone" · `root_gone`: "{type} deleted" · `erased`: "Erased" | parent card (sub_gone) / "This {type} was deleted." / "This {type} was erased under a data-rights request." | **OWNED**; GDPR-aware, **root carried** so the ref degrades to context. Erased = **0 recoverable PII** (drill D-5). Dignified, never a broken-image icon |
| 8 | **Cross-cell** | normal chip + tiny **residency tag** (P9) | normal card + "lives in {region}" footnote | **OWNED**; resolved cell-locally — only the filtered projection or a tombstone crosses, never raw rows/PII (C-5; ADR-11). Else → the no-access render |
| 9 | **Rebase-orphan diff chip** | exact→live · rebased→"moved" · partial→"outdated" · content_gone→**detach to "outdated — was on former line N" pill, lift to file level** | same | **OWNED** (the hardest case); content-anchored BLAKE3 + 3-way match (`myelin-refs` §3.5). **Never silently jumps to a wrong line** (the GitHub anti-pattern) |
| 10 | **Degraded** (resolver unreachable) | last-known + "can't refresh" dot | frozen last projection + "showing last known — couldn't refresh"; inline actions disabled | **OWNED**; last-known from the bounded projection cache; **fails static for this chip only**, never a page error |
| 11 | **Agent-pending** | agent treatment + "awaiting your approval" / "agent working" | the HITL card as an unfurl (Approve/Edit/Reject) | named; **depth → [`agent-hitl-card.md`](./agent-hitl-card.md)**; agent-authored unfurls default out of the main timeline (P8) |

**The load-bearing rule (PROVEN):** the no-access and erased renders **never receive a title to leak** —
`resolve` collapsed them to a tombstone before content crossed the wire. Non-leaking *by construction*, not by
frontend discipline. This is the structural beat over Slack/Notion (R-01 §3.1/§2.4).

Cross-cutting states from the matrix that map here: **empty** = N/A (a chip is always *a reference to
something*; the empty *container* is the consuming surface's job); **error** = a *resolve* failure is the
**degraded** render (#10), not a red error (a permission *outcome* is #4, never "Error 403" — R-21 §1.3/§1.4).

---

## 6. Live-not-snapshot + humanised strings (surfaced, not redesigned)

- **Live, not snapshot (default; snapshot is never an option for the chip).** The unfurl is a *current
  projection* kept fresh by **bus update events** (per-`ArtifactRef` cache invalidation; `refs-projection-
  invalidator` busts on `*.updated`/`*.erased`). A PR chip flips red→green, an issue transitions, a doc edits —
  **the chip updates in place** (a live-update microinteraction, §8 motion). This is the correctness/erasure-
  safety reason live is the default: a snapshot can show content the target later restricted/erased (a GDPR
  leak); a live projection can't, because the next resolve returns the tombstone. *(PROVEN — §5.3 hard rule.)*
- **Humanised strings at the source.** The chip never shows a raw machine string; display-name resolution
  lives at the backend (`myelin-refs` §4.8 / design-language §8b.5). The frontend owns **no id→name map**.
  Locale-aware (G2). *(PROVEN.)*

---

## 7. Variants + parameterization variant flags

- **Density (chip ↔ card)** is the component's primary internal parameter (its own prop), independent of the
  global flags.
- **`density` flag (`comfortable`↔`compact`)** — sets chip row-height / card padding via the token set
  (`--space-*`, `--row-h`); compact is A's default. The chip stays one-line-tall in either.
- **`agentPresence` flag (`ambient`↔`foregrounded`)** — sets the *default surfacing* of the **agent-pending**
  unfurl (#11): ambient → collapsed to a chip / inbox-routed; foregrounded → the rendered card inline. **The
  component is identical**; the flag sets the default, not a branch.
- **`sovereigntyVisibility` flag (`on-demand`↔`always-on`)** — `always-on` renders the **residency tag**
  (#8 / context line) on *every* card near data, not only cross-cell ones.
- **NOT affected:** `nav`, `surfaceUnification`, `tone` (the chip/unfurl is chrome-invariant across directions
  — the bounded-distinctness bet, 00-plan §2.4). **No `switch(direction)` anywhere.** *(HOUSE STYLE rule.)*

---

## 8. Keyboard + ARIA model

- **Chip = a link** — React Aria **`Link`**; accessible name = humanised title + type ("Pull request: Fix
  login, merged"). `Tab` to it, `Enter` opens.
- **Peek = a non-modal hovercard** — built on the Tier-1 **Popover** (React Aria **`Tooltip`** semantics for
  the trigger pattern is wrong here because the peek is interactive/hoverable; use the **Popover/Dialog
  (non-modal)** primitive). Opens on **focus as well as hover** (keyboard + touch parity); **`Esc` dismisses
  without moving pointer/focus** and returns focus to the chip; **`Enter` opens**; pointer may move *onto* the
  card without it vanishing; no auto-timeout. *(PROVEN — WCAG 2.2 SC 1.4.13 dismissable/hoverable/persistent.)*
- **Inline actions** — a **`MenuTrigger` + `Menu`** (Tier-1 Dropdown) on the `···`; roving within, `Esc`
  closes + returns focus. Each action is keyboard-reachable in a logical order; the action set is
  permission-pre-filtered (absent items never appear).
- **Visible focus** — the one `--focus-ring` token on the chip and every inline action, every theme.
- **Live updates** — announce **state *changes* a viewer is watching** via a polite live region (a PR going
  green), **not every background refresh** (no spam). Skeletons set **`aria-busy`** (00-plan §4 gap-4).
- **Status never colour-alone** — glyph + label + position (§2.1).
- **Reflow/zoom + i18n** — chip truncation + card reflow at 200%/320px; survive German +35%, Greek/Cyrillic,
  **RTL mirroring via logical properties** (the whole card mirrors with no override sheet — G2). Residency tag
  + status pills must not clip under expansion.

---

## 9. Semantic tokens consumed

| Purpose | Token(s) |
|---|---|
| Chip surface / border | `--c-chip-bg` (→ `--surface-raised`), `--c-chip-border` (→ `--border`) |
| Title / muted meta | `--text-primary`, `--text-muted`, `--text-subtle` |
| Status glyphs | `--success` / `--warning` / `--danger` / `--info` (+ `-subtle`) — **always with glyph + label** |
| **Agent** treatment (#11, person/agent card) | **`--agent`** / `--on-agent` / `--agent-subtle`, `--c-agent-mark` — the reserved fourth axis, never a status colour |
| Focus | `--focus-ring` (the derived AA-safe token, distinct from `--accent`) |
| State pills (moved/outdated) | `--text-muted` on `--surface-overlay`; **no colour-coded pill** (label carries it) |
| Card elevation (peek/unfurl popover) | `--shadow-popover`; z-index `--z-popover` |
| Spacing / radius / motion | `--space-*`, `--radius-1`, the motion tokens below |

The chip binds **only** to semantic vars / the `--c-chip-*` handles (00-plan §2.1) — re-pointing the semantic
table re-skins it with no edit.

---

## 10. Motion (token-based, reduced-motion first-class)

- **Live-update flip** (red→green in place) — `--dur-deliberate` (240ms, the reserved notice-without-interrupt
  band) + `--ease-standard`; a colour/glyph cross-fade, **no scroll-jump, no layout shift** (R-12 `liveUpdate`).
- **Peek open/close** — `--dur-fast` (140ms) `--ease-enter` / `--ease-exit`; the card *appears*, it doesn't
  slide a paragraph (§8b.6 "pages render, they don't animate in").
- **Inline-action settle / rollback** — settle `--dur-micro`; rollback reverses the move so failure looks
  *different* from success (OPT-1).
- **No spring/bounce, no sparkle/shimmer** (§8b.3 / R-12 anti-list).
- **`prefers-reduced-motion`** — all durations → 0 (token-level, §3.5); the state still **flips and
  announces**. Reduced-motion loses the animation, never the information.

---

## 11. Usage do / don't

**Do**
- Render every degraded state as a *designed* state (never blank, never blame, never leak, never lie — R-21 §0).
- Default to **compact chip**; auto-expand at most **one card per message** (the primary ref) — the rest stay
  chips (the anti-Slack-noise rule, R-09 §7.2).
- Keep the type-icon, header, context line, action bar **identical across all types** (the D4 coherence test:
  open the same `#issue` in a board cell, an editor mention, an inbox row — it must be the identical chip).
- Show the residency tag on cross-cell refs (P9); label live embeds "live · updated 2m ago".

**Don't**
- Don't ever show a raw id / URN / `merge_request merged` raw string (the #1 "feels unfinished" tell).
- Don't grey-out an action the viewer lacks — make it **absent** (don't leak the gate).
- Don't put an action bar inside the transient hovercard (1.4.13 + mis-click hazard).
- Don't let a rebase-orphaned chip silently jump to a wrong line — detach honestly (#9).
- Don't snapshot — never cache a title across a permission/erasure change (the GDPR-leak class).
- Don't `switch(direction)`; read `density`/`agentPresence`/`sovereigntyVisibility` + semantic tokens.

---

## 12. Honesty — PROVEN vs HOUSE STYLE vs deferred

- **PROVEN:** the resolver→UI state contract (every render = one resolver state); non-leaking-by-construction
  for no-access/erased; live-not-snapshot for correctness/erasure-safety; humanised-at-source; WCAG 1.4.13
  hovercard, 1.4.1 status-not-colour, the focus token. The mechanisms are surfaced, not invented.
- **HOUSE STYLE:** the chip anatomy + the card shell shape; the inline-action *subset* per type (R-09 §13 #2 —
  which actions are "in-flow enough" for a chip is a taste call); the "moved" pill vs hard-detach choice for
  rebase-orphans; one-card-auto-expand.
- **`[DEFERRED-UNTIL-USERS]`** (carried from R-09 §11): does the **no-access** card read as "you lack access"
  (intended) vs "this is broken"? Is the **erased tombstone** dignified vs alarming (esp. with a DPO)? Is the
  **rebase-relocated** chip *trusted* (the "moved" pill) or do reviewers want a hard-detach? Method: per-segment
  RITE on the finalist that ships this, on F-ENG-1/F-PM-1/F-GOV-1. Until then: treat the no-leak/erasure-safety
  as PROVEN, the *comprehension/trust* of each degraded render as HYPOTHESIS.

*End. Component spec HOUSE STYLE over the PROVEN `myelin-refs` resolver + design-language §5.3 hard rules +
ADR-13/03/12; renders R-09, used by R-22's wedges; states from R-21. Consumes the finalist-A token set. Not
committed.*
