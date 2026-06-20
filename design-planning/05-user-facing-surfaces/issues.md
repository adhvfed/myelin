# Surface group: Issue tracker (§7.3)

> Phase 5 surface map · group **I** · maps [`design-language §7.3`](../../planning/02-holistic-architecture/design-language.md)
> against the [§2 template](./README.md#2-the-per-surface-map-template). Pointer map; PROVEN / HOUSE
> STYLE tagged; date 2026-06-20. Cross-cutting obligations ([README §3](./README.md#3)) inherited.
> **This group carries the central dual-audience bet:** the **board (I-2) ↔ roadmap (I-3)** pair is
> the D1 same-data proof and the funnel's binding Axis-3 spread point ([README §5](./README.md#5)).

---

## I-2 — Issue board / cycle *(engineer lens of D1; funnel dense surface)*
1. **Jobs:** D1 engineer lens (≈E3/E4 plane — burn down a cycle on a keyboard board). Flow **F-PM-2** (the D1 pair). 2. **IA + shell:** `Issues → <project> → view`; content; contextual sidebar = the **views tree** (R-06 §3.2, shared shape with Knowledge).
3. **Components:** **it IS the §5.6 views component (R-10 §2)** — board projection; chip/unfurl, editor (issue body), palette.
4. **Density:** **0.55** — earns via J3 (board drag, WIP, swimlanes — a *projection*, not a board engine) + J2 (engineer density tier). **Persona lens (R-16 D1/L1):** projection=board, density=compact, vocab=`Issue`, fields=engineer set, landing=cycle board.
5. **Agent:** agent-suggested actions on cards (R-14); triage feeds it (I-7). 6. **Sovereignty:** visibility chip on confidential cards (rolled up as count, not opened — R-04 §5).
7. **State set (R-21 §2d):** empty = "drag from backlog or ask the planning agent"; optimistic card-move (B2); conflict (two people move same card → CAS, R-21 col 10); live-update when a teammate transitions. 8. **A11y (R-17 §5.2 board-drag hard component):** keyboard pick-up (Space/Enter → arrows → Space/Enter drop, Esc cancel; roving-tabindex, one Tab-stop); SR live-region "Picked up / Column X position Y / Dropped". **G2:** board columns mirror in RTL; German column labels expand without clipping.
9. **Device (MOB-1):** card hover-actions touch-reachable; board scrolls horizontally on mobile; keyboard-drag has no mobile equivalent → tap-to-move sheet. 10. **Wedge/motion:** `motion.move` on card-to-column (where-did-it-go), `motion.settle` on cross-column reflow (R-12 §3.1). 11. **DoD + switch:** the board is a *projection of the shared views component* (same chip/editor as the roadmap); an engineer burns down a cycle on the keyboard without the roadmap suffocating it (R-07 §2.1 trap-avoidance).

## I-3 — Roadmap / timeline *(PM lens of D1; funnel approachable surface)*
1. **Jobs:** M1 (communicate roadmap reflecting *real* delivery), M3. Flow **F-PM-2**. 2. **IA + shell:** `Issues → <project> → roadmap`; content; **same views tree** as the board.
3. **Components:** **same §5.6 views component (R-10 §2), same records** — timeline projection; same chip/editor.
4. **Density:** **0.5** — earns via J1 (temporal ranges + dependencies) + J2 (calm/spacious tier). **Persona lens (R-16 D1/L2):** projection=timeline, density=comfortable, vocab=`Roadmap`/`Work item`, fields=PM set (outcomes, now/next/later), landing=roadmap. **This is the same component over the same issue records as I-2 — the dual-audience proof (D5).**
5. **Agent:** planning-agent suggestions (empty-state CTA). 6. **Sovereignty:** "1 restricted item in this rollup" without leaking it (R-04 §5).
7. **State set (R-21 §2d):** stale ("updating…" then settles when an engineer transitions live — F-PM-2); empty = "No work scheduled — drag from backlog or ask the planning agent"; permission-as-count. 8. **A11y/i18n:** timeline range-bars keyboard-resizable; **G2 — timeline mirrors in RTL** (linear-time, R-18 §4.2; Hebrew exception: timelines stay LTR in Hebrew); locale-aware dates on the time axis (`Intl`, G2). 9. **Device:** **read-forward on mobile** (MOB-6) — a stakeholder reads "what's shipping" on a phone; range-editing desktop-mainly.
10. **Wedge/motion:** the roadmap **updates live** when delivery data changes (`motion.liveUpdate`) — "report maintains itself" (R-20 D-U1 delight). 11. **DoD + switch:** the report **is** the delivery data — a PM stops maintaining a Productboard/slide parallel reality (F-PM-2 🔪); **neither lens degraded** (D5) — the roadmap is not the board "calmed down," it is its own legitimate projection (R-02 R-MID-2). **Vocabulary T2 bet** (`Roadmap`/`Work item`) demonstrated here, carried as unvalidated ([README §4.4](./README.md#44)).

## I-1 — Issue detail view
1. **Jobs:** E1 (see the why), M2 (intent linked to delivery). 2. **IA:** `Issues → <project> → <issue>`; content + properties sidebar + context pane (linked PRs/commits/runs/docs). 3. **Components:** editor (R-10 §3, rich body), chip/unfurl (relations/linked artifacts), comment thread (R-10 §5.5, anchored), views (sub-issue checklist).
4. **Density:** 0.45. 5. **Agent:** agent-suggested actions (R-14); agent commenter with treatment. 6. **Sovereignty:** per-artifact visibility chip (R-19 §1.2); fields redacted per role (R-04 §4.2).
7. **State set (R-21 §2d):** SLA timers (locale-aware, G2); erased linked-issue → tombstone chip; rollup-recompute pending. 8. **A11y/i18n:** properties sidebar landmark; **SLA timer = `Intl` + business-calendar** (R-18 §5.2, load-bearing); humanised state strings (no `merge_request merged`). 9. **Device (MOB-2):** properties → drawer; body read-friendly; comment authoring via composer sheet.
10. **Wedge/motion:** **W5 backlinks** (linked PRs/commits appear automatically); born-linked issue (R-20 D-S2). 11. **DoD + switch:** the issue shows its full live reference graph (PRs/commits/runs/docs as chips) without a Jira-to-everything-else tab dance.

## I-7 — Triage inbox (agent-assisted)
1. **Jobs:** E11 (curated queue), M6 (promote chat report to tracked work). Flow F-PM-1 (convert-to-issue). 2. **IA:** `Issues → <project> → Triage`; content. 3. **Components:** views (list), plan card (agent labels), chip.
4. **Density:** 0.55. 5. **Agent (R-14 §6.2):** agent has labelled/deduped/routed; **suggest-not-auto** (human confirms); low-confidence flagged (HAX G10). 6. **Sovereignty:** scope-filtered.
7. **State set (R-21 §2d):** agent-pending; storm (incident floods → dedup by `origin_event`, R-04 §4.2). 8. **A11y:** single-key triage actions keyboard-reachable; agent labels as text. **G2:** humanised. 9. **Device:** read + triage on mobile (one-action). 10. **Wedge/motion:** agent proposal `motion.agentEnter`; convert-to-issue optimistic (W7 from chat). 11. **DoD + switch:** a noisy firehose becomes a curated queue the human *confirms* — never an auto-applied agent edit (R-14 doctrine-beats-HAX, never silent).

## I-4 / I-5 / I-6 / I-8 / I-9 / I-10 / I-12 — portfolio · list/table/calendar · cycle · My Work · dashboards · saved views · team page
- **I-4 Portfolio / exec rollup** (G1 exec lens of D1): `Issues → portfolio`; density 0.45 (read-forward, chart-forward calm tier, R-16 D1/L3 — roadmap's records rolled up, *not a third app*). Locale-aware. Desktop/tablet read-forward.
- **I-5 List / table / calendar views** (R-10 §2): same views component, other projections. Table inline-edit is the **R-17 §5.3 views-inline-edit hard component** (grid one Tab-stop, Enter/F2 edit, Tab commit, Esc revert; `role=grid` SR). Density 0.55. Calendar drag-to-reschedule.
- **I-6 Cycle (sprint) view** (M3): capacity, burndown chart. Density 0.5. Burndown mirrors LTR-progress in RTL (R-18 §4.2).
- **I-8 "My Work" hub** (M5): **lives at `[G] Home`** (cross-subsystem; overlaps the inbox S-4). Density 0.3. Per-role landing target (R-06 §7).
- **I-9 Dashboards** (M4/D5): `Issues → Dashboards`; configurable widgets, SLA gauges, the **one charting language** (§3.7). Density 0.4. **D5 dual-audience** (my flow / team / org rollup, R-16 D5). **Desktop-mainly (MOB-6)** — read-only tiles on mobile. Locale-aware numbers (G2).
- **I-10 Saved views management** (R-10 §2): first-class shareable/permissioned views. Density 0.3.
- **I-12 Team page** (M4): team-scoped work + health. Density 0.4.

## I-11 — Workflow / SLA / field-scheme admin *(admin; flow-orphaned — job-linked)*
1. **Job link ([README §4.2](./README.md#42)):** **G8** (P15 — "one admin surface for … policy") + **M4** (P7 — trustworthy SLA/flow analytics needs the SLA + workflow *defined*). Used when an admin configures a team's workflow/SLA. 2. **IA:** `Issues → [A] admin`; **one layer down** (P4 — progressive disclosure governance, R-02 R-CFG).
3. **Components:** views/forms, overlays. 4. **Density:** 0.4. 5. **Agent:** which agents may transition (links S-9). 6. **Sovereignty:** RBAC-adjacent.
7. **State set (R-21 §2d):** validation errors; permission. 8. **A11y/i18n:** form a11y; **G2** German expansion on long workflow/field labels (R-18 §2); **status categories are a fixed shared vocabulary** (R-02 R-CONS-2 — one configured status maps to a small fixed semantic set so "what does this column mean" reads identically across projects). 9. **Device:** **desktop-mainly (MOB-6)**.
10. **Motion:** settle-on-save. 11. **DoD + switch:** **anti-config-maze (R-02 R-CFG-1/2/3)** — adding custom fields/workflows never changes the *default* issue surface; no "Jira-admin-as-a-full-time-job"; the interaction grammar stays invariant (R-CONS-1, customisation changes *what data*, never *how the board is navigated*). This is the single biggest trap this group must not re-commit.

---

## Routed seam (flagged for Phase 6, [README §6 item 5](./README.md#6))
**Non-diff anchored-comment relocation.** R-09 §5.9 owns diff-anchored relocation (detach-to-pill); but
issue/comment anchoring to a sub-artifact (R-10 §5.5 "anchored comments survive/relocate") is **not
fully owned** — is anchored-comment relocation diff-only, or a general content-anchor the editor/views
also owe? **Carried as a Phase-6 question** (critic §4.3), not resolved here.

**Group invariants reminder:** I-2 and I-3 are the **same §5.6 component, same records, two
projections** — the open-the-same-chip-in-both test (R-07 §3) is the D4/D5 proof this group exists to
make. Density (`0.5`–`0.55`) is *projection + density-token tuning*, never a fork.
