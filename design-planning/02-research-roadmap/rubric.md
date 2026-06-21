# The Pre-Registered Judging Rubric (BINDING)

> Phase: `design-planning/02-research-roadmap`. **This rubric is written BEFORE any sketch exists.**
> Phase 6 sketches are designed *toward* it; Phase 7 judges *against* it. It is **binding**: Phase 7 may
> not invent new criteria mid-judging, and may not down-weight a hard gate. It operationalises the
> Phase-1 evaluation methods (#12 measured tokens, #15 HAX, #17 §6-critique, #18 dual-audience, #19
> P1–P9 heuristics, #20 cognitive walkthrough, #21 a11y audit, #22 structured critique, #24 switch test)
> into a single scoreable instrument. Tags: **PROVEN** (a cited standard / evidenced method) vs **HOUSE
> STYLE** (our taste). Status date: **2026-06-20**.

The order of operations is fixed: **(1) hard gates → (2) weighted score → (3) tie-break.** A sketch that
fails any hard gate is **non-conformant and cannot be ranked above a conforming sketch**, regardless of
how beautiful it is. This is the whole reason the rubric is pre-registered: it stops "the orchestrator
preferred sketch 7" and stops a gorgeous-but-inaccessible or English-only design from winning (VISION §3
honesty; design-language §4 makes a11y/i18n *requirements*, not polish, for an EU-sovereign product).

---

## Part 1 — Hard pass/fail gates (requirements, not polish)

These are **binary**. Each finalist must *demonstrate* conformance in the sketch artifact itself
(measured/shown, not claimed in prose). Because Phase-6 sketches are limited HTML, "demonstrate" means
the relevant state/markup/token is actually present and inspectable for at least the **required screen
set** defined in [`sketch-funnel.md`](./sketch-funnel.md) §"comparable screens".

### Gate G1 — Accessibility (PROVEN — WCAG 2.1 AA / EN 301 549 as the hard floor)

**The floor is WCAG 2.1 Level AA and EN 301 549** (the EU harmonised standard that gives a presumption
of conformity with the European Accessibility Act, enforceable since 2025-06-28). The **house target is
WCAG 2.2 AA** (design-language §4); since **WCAG 2.2 ⊇ 2.1** (2.2 adds nine success criteria and removes
none, except 4.1.1 Parsing which was obsoleted), meeting 2.2 AA satisfies the 2.1 floor automatically.
Judge against 2.1 AA as the *gate*; reward 2.2-AA-specific criteria (e.g. 2.4.11 focus-not-obscured,
2.5.8 target-size-minimum, 3.3.8 accessible authentication) in the scored dimension D3.

A sketch passes G1 only if it demonstrates **all** of:

- **Contrast measured, not claimed.** Every text/background and essential-UI pair in the sketch's token
  set meets AA (4.5:1 normal text, 3:1 large text/UI components), verified by the contrast checker, not
  asserted. The brand accent failing AA is explicitly allowed *only if* the focus/primary-action token
  is a derived, AA-passing token (the focus token ≠ the identity token, §8b.3).
- **Visible focus on every interactive element, in every theme** (light/dark/high-contrast), via one
  `focus-ring` token meeting contrast minimums; focus is never removed and never obscured.
- **Full keyboard operability with no traps** for every interactive element shown — including the
  command palette, the views table/board, the block editor, the diff, and the HITL approval card —
  with a logical tab order.
- **Status never by colour alone** — every status (CI green/red, PR state, SLA breach, the `agent`
  treatment, success/warning/danger) carries a glyph or text label or position in addition to colour.
- **Semantic structure** — correct roles/landmarks/headings; dialogs/menus/comboboxes use the right
  ARIA patterns; live regions announce event-driven updates appropriately (and do not spam).
- **Reflow & zoom** — content reflows at 200% / 320px-equivalent without loss of content or function on
  the dense surfaces shown; reduced-motion is honoured as a first-class path.

*Phase-7 note:* automated checks catch only ~30–40% of issues; G1 is a manual expert audit per R-17's
method, with assistive-technology *user* testing deferred to Phase 4 (carried as a deferred-until-users
flag — passing G1 is necessary, not sufficient, for "accessible-with-AT").

### Gate G2 — i18n / l10n / RTL (PROVEN — requirement for an EU-sovereign product)

For an EU-sovereign product, internationalisation is a **requirement**, not an enhancement
(design-language §4; the product must be *operated and read* in a user's own EU language). A sketch
passes G2 only if it demonstrates **all** of:

- **Multiple EU languages shown**, including at minimum **one long-word / long-compound language
  (German)** and **one non-Latin script (Greek or Cyrillic)** — to prove the layout survives text
  expansion (German labels run ~30–40% longer than English) and non-Latin rendering (font coverage,
  line-height, no clipping). English-only sketches fail G2.
- **No truncation / overflow / clipping** of the expanded strings on the required screen set;
  no fixed-width assumptions that break under expansion.
- **RTL-awareness** — layout uses logical (start/end) properties, not physical (left/right); the sketch
  shows **at least one mirrored state** (e.g. the shell + a content surface in RTL with a real RTL
  string such as Arabic or Hebrew), with the editor, the views component, and overlays mirroring
  correctly.
- **Locale-aware formatting shown** for at least dates/numbers (the SLA/business-calendar surfaces make
  this load-bearing).
- **No hard-coded machine strings** leaking into the UI (`merge_request merged`, raw ids) — humanised,
  per §8b.5.

*Why both gates are non-negotiable:* failing accessibility or i18n does not make Myelin a worse product
— it makes it **ineligible for its core market** (EU public-sector procurement, EN 301 549 / EAA;
multilingual operation as part of the sovereignty value proposition). A beautiful sketch that fails
either is disqualified before aesthetics are considered.

---

## Part 2 — Scored dimensions (weighted; 0–4 scale with anchors)

Ten dimensions. Each is scored **0–4** by the owning judge lens (Part 3) against the anchors below. The
weighted sum is the conformance score *among sketches that passed both hard gates*.

**Scale anchors (general):** **0** = absent / actively wrong · **1** = present but weak, would lose a
switch-test · **2** = competent, meets the incumbent bar · **3** = strong, meets a North Star ·
**4** = exemplary, *beats* the North Star / is a reason-to-love.

| # | Dimension | Weight | What it measures | 0 / 4 anchors (abbreviated) |
|---|---|---:|---|---|
| **D1** | **Power-user efficiency** (keyboard-first, density) | **12%** | Keyboard path to every primary action; `Cmd/Ctrl-K` palette; `j/k` nav; earned density on engineer surfaces; minimal chrome between intent and action (P3/P5). | 0: mouse-only, chrome-heavy. 4: a power user crosses the whole flow on the keyboard faster than Linear; density is high *and* legible. |
| **D2** | **First-run delight / approachability** (PM/corporate, onboarding) | **10%** | Can a non-engineer (P6/P11/P13) land, understand, and act without prior knowledge; empty/first-run guides the next step; approachable without being toylike (P4; §2). | 0: hostile, expert-only. 4: a PM is productive in minutes; the empty state teaches; warmth without sacrificing precision. |
| **D3** | **Visual craft & emotional tone** | **12%** | Token discipline (hierarchy from weight/colour before size; spacing on the ramp; borders-over-shadow); a coherent, intentional aesthetic with a clear tone; the absence of the amateur tells (§8b.3); 2.2-AA-specific wins. | 0: generic/amateur tells present. 4: a distinctive, intentional, loved look; every token earns its place. |
| **D4** | **One-product coherence** (the five-surfaces-as-one test) | **14%** | The *central problem*: one shell, one identity badge, one chip, one palette, one editor, one views component across surfaces; a user never feels they "left one app." Per-surface density is *tuning of shared components*, not a fork. | 0: five stitched-together looks. 4: indisputably one product; dense and calm surfaces are visibly the same system tuned by density. |
| **D5** | **Dual-/tri-audience** (one component, many lenses) | **10%** | A dual-audience surface (issue views, knowledge db, dashboard) serves engineers AND PM/corporate from **one component** adapted by role/density/vocabulary — never a quiet fork; neither lens is a degraded compromise (§2; method #18). | 0: forks into two UIs, or one lens is starved. 4: same component, both lenses excellent, switchable. |
| **D6** | **Agent legibility & trust** (the §6 plan-then-apply / HITL contract) | **12%** | Agents always labelled (not magic, not hidden); plan-then-apply shows proposed effects before they happen; HITL Approve/**Edit**/Reject card; attribution + audit link; consequences conveyed (P7; §6; HAX). | 0: agents act invisibly or as "magic." 4: every agent action is legible, gated where consequential, attributable; trust is *calibrated*, not blind. |
| **D7** | **Density-made-calm** | **8%** | Shows a lot without shouting; calm-by-default; attention is sacred; agent volume kept out of the main timeline; one prioritised inbox; no firehose (P8; doctrine §4). | 0: noisy, traffic-light screen. 4: dense yet calm; the eye knows where to go; quiet is the default. |
| **D8** | **Perceived performance** (skeletons / optimistic states designed) | **6%** | Loading shows *structure* (skeletons matching final layout), not blank spinners; optimistic updates with honest rollback designed; "pages render, they don't animate in"; the latency budgets are visibly respected (P2; §8b.6). | 0: spinners on blank pages, no optimistic states. 4: feels instant; every wait shows structure; rollback is honest. |
| **D9** | **Sovereignty / GDPR-as-UX legibility** | **8%** | Residency, lawful basis, who/what can see a thing, agent scope, audit trail, and DSR tooling are *legible first-class surfaces*, not fine print; "where does this data live / who processed this / show me everything about this subject" are answerable in the UI (P9; §7.6). | 0: sovereignty buried in settings. 4: sovereignty is *felt* — a DPO trusts it at a glance; residency/visibility cues are present where data is. |
| **D10** | **The switch test** (would a team move without hitting a wall) | **8%** | The doctrine done-bar applied as a thought-experiment + interaction walkthrough: could a team switch off Jira/Slack/Notion to this surface without regressing on something the old tool had? (method #24; §8b.7). | 0: obvious regressions / dead ends. 4: a team could switch today and gain, not lose. |

**Total weight = 100%.** (D4 one-product-coherence and D3 visual-craft and D1/D6 carry the most weight,
reflecting the central problem and the "loved" mandate; the hard gates carry *infinite* weight by being
gates.)

---

## Part 3 — Judge-lens ownership (the multi-lens panel, pre-wired)

Phase 7 runs as a **multi-lens panel** (structured critique, method #22; **silent-first** — each lens
scores independently in writing before discussion, to fight anchoring/groupthink). Each scored dimension
is *owned* by one lens (which proposes the score; the panel ratifies), so coverage is guaranteed and no
dimension is everyone's-and-therefore-no-one's. The hard gates are owned by the Accessibility lens but
**every lens must confirm** a sketch cleared them before scoring.

| Judge lens | Owns (proposes scores for) | Phase-1 method it embodies |
|---|---|---|
| **Power-user-efficiency lens** | D1, D7, D8, D10 | #19 P1–P9 heuristics (P2/P3/P5/P8), #24 switch test |
| **First-run-delight lens** | D2, D5 | #20 cognitive walkthrough, #18 dual-audience |
| **Accessibility lens** | **G1, G2 (gates)**, D9 | #21 a11y audit, #12 measured tokens, sovereignty blueprint |
| **Visual-craft lens** | D3, D4, D6 | #13 visual direction, #19 (P1 coherence/P7 agent), #15/#17 agent |

> Coverage check: every dimension D1–D10 and both gates have exactly one owning lens; D4/D6 sit with the
> visual-craft lens because one-product coherence and agent legibility are most visible in the rendered
> artifact, but the power-user and accessibility lenses cross-check them.

---

## Part 4 — Aggregation & tie-break

**Aggregation method:**
1. **Gate filter (binary).** Any finalist failing G1 or G2 is marked **non-conformant** and ranked below
   every conforming finalist. (It is still *kept* in the spread — the final human may choose to fix-then-
   adopt it — but it cannot be the *recommended* direction.)
2. **Weighted score (continuous).** For conforming finalists, `score = Σ (weightᵢ × scoreᵢ)` over D1–D10,
   normalised to 0–100. Each dimension's score is the panel-ratified value (median of the lenses if they
   diverge, with the owning lens's rationale recorded).
3. **Rank.** Finalists are ranked by weighted score; the top conforming finalist is the **recommended
   direction** (clearly labelled as recommended, not chosen — the human decides).

**Tie-break rule (in strict order):**
1. **Hard gates first** — a finalist that cleared gates with more margin (e.g. demonstrated 2.2 AA, more
   languages/RTL depth) outranks one that barely cleared them.
2. **Then the weighted score** — higher total wins.
3. **Then the central-problem dimensions** — if still tied, the higher **D4 (one-product coherence)**,
   then **D6 (agent legibility)**, then **D5 (dual-audience)** wins (these encode Myelin's defining
   bets).
4. **Pure aesthetics break ties ONLY at the end** — if two finalists are genuinely indistinguishable on
   gates and all weighted dimensions, *then* subjective aesthetic preference may decide, and the
   rationale must be written down (HOUSE STYLE, per README §5.6: P1–P9 + measured gates decide; pure
   aesthetics break ties only).

**Pre-registration discipline:** Phase 7 records, per finalist, every gate pass/fail with evidence,
every dimension score with the owning lens's one-line rationale, the weighted total, and the tie-break
path if used. No dimension may be added, removed, or re-weighted during judging. If Phase 6 surfaces a
genuinely missing dimension, it is logged as a roadmap defect and the human is told — it does **not**
get silently scored.

---

## Part 5 — How Phase 6 designs toward this (the contract back to the funnel)

So sketches are designed *toward* the rubric rather than judged *by surprise*:

- Every finalist **must include the comparable screen set** from [`sketch-funnel.md`](./sketch-funnel.md)
  (≥1 dense engineer surface, ≥1 approachable PM/corporate surface, ≥1 agent/HITL moment, plus the
  shared shell) — because D1/D2/D4/D5/D6 cannot be scored otherwise.
- Every finalist **must ship its token set** (DTCG-structured) so G1 contrast and D3 craft are
  measurable.
- Every finalist **must demonstrate the hard-gate states** (the long-word + non-Latin + one RTL state;
  the measured-contrast tokens) on the required screens — these are designed in from sketch #1, not
  retrofitted.
- Every finalist **must depict the unglamorous states** named in README §9 for at least one surface
  (empty/loading/error/permission/erased/agent-pending) — D8 and the switch test (D10) depend on them.
