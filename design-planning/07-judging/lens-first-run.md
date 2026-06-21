# Phase-7 Judging — First-run-delight lens (owns D2, D5)

> Lens: **First-run delight / approachability** (cognitive walkthrough, method #20) + **dual-/tri-audience**
> (method #18). Judged **silent-first** — scored independently in writing before any panel discussion, no
> other lens consulted. Persona stance: a **non-engineer** (PM P6 / exec P11 / DPO P13) lands cold; and the
> dual-audience-surface test — does **one component** serve engineers AND PM/corporate without a quiet fork
> or a starved lens. Self-scores in the finalist READMEs were **ignored**; scores below are formed from the
> rendered artifacts. Status date **2026-06-20**. Tags: **PROVEN** = inspectable in the artifact / a measured
> mechanism · **JUDGEMENT** = my read against the anchors.
>
> **Carried caveat (UNVALIDATED, applies to D2 + D5 for every finalist):** the *persona-adaptive vocabulary*
> bet — that the same record reads as "issue"↔"work item"↔"deliverable"↔"outcome" per lens and that a
> non-engineer actually comprehends the empty-state copy — is **`[DEFERRED-UNTIL-USERS]`** (R-16 §6 falsifier:
> PMs may reject coarse same-schema records and demand a separate narrative tool). All comprehension/warmth
> claims are expert-judged, not user-tested. This caps the achievable score and is not finalist-specific.

Anchors (from `rubric.md` Part 2): **D2** 0 = hostile/expert-only · 4 = a PM is productive in minutes, the
empty state *teaches*, warmth without sacrificing precision. **D5** 0 = forks into two UIs or one lens is
starved · 4 = same component, both lenses excellent, switchable.

---

## D2 — First-run delight / approachability (non-engineer lands, understands, acts)

| Finalist | Score | Rationale (file + what I saw) |
|---|:--:|---|
| **A Instrument** | **2** | **JUDGEMENT.** Empty state *does* teach — `7-states-and-rtl.html` "No issues in this cycle yet · Create the first issue, or let FixAgent triage incoming reports" + New-issue / Import buttons; error blames the system ("Your data is fine — this is on us"). But the whole skin is a **midnight-utilitarian command-deck** (`1-shell-pr-context.html`: hairlines, mono load-bearing, `⌘K to act · g d → diff` hints) — reads *cold and expert-first* to a non-engineer. Productive-in-minutes is doubtful for P6/P11/P13. Competent states, hostile-leaning tone. |
| **B Workshop** | **4** | **JUDGEMENT (PROVEN copy).** Best first-run in the set. `6-states.html` empty: "Your roadmap is empty — let's place the first outcome · A roadmap lane is just your real work read over time… **No second copy to maintain**" — the empty state teaches the *mental model*, not just a button. Editorial-warm serif headings, ~66ch prose editor (`4-knowledge-unfurl.html`), respectful agent ("I will not edit it without approval"). Warm **without** toylike (no emoji-as-UI, no sparkle). A PM lands and acts. |
| **C Wayfinding** | **2** | **JUDGEMENT.** `states.html` empty teaches ("Start one to see it on the wayfinding nav") and the tone is clear, but `01-shell-chat.html` is **engineer-native** ("cache thrash under burst", "panic at lru.rs:142", channel▸topic▸message addressing). The empty copy presumes the user already grasps *why* topics "keep agent volume contained." Approachable to a *technical* PM; cool/expert-leaning for an exec/DPO. Signage-utilitarian, not inviting. |
| **D Civic** | **3** | **JUDGEMENT.** Plain-as-authority reads genuinely well for the **DPO/exec** persona this finalist targets. `01-shell-exec-dashboard.html`: outcome vocabulary ("Payments reliability", "Termintreue", "Gefährdet"), ambient non-pushy ForecastAgent footnote. `07-states.html` empty *teaches the legal path*: "When a subject exercises a GDPR right, start here. One subject → one inventory → one verifiable receipt"; `06-dsr-console.html` explains consequences in plain language **before** action. Guide-tone, not cheerleader; slightly sober/cool for a first-time non-corporate user, so not a 4. |

## D5 — Dual-/tri-audience (one component, many lenses, neither starved)

| Finalist | Score | Rationale (file + what I saw) |
|---|:--:|---|
| **A Instrument** | **4** | **PROVEN.** The cleanest one-component proof. `4-board-roadmap.html` is a single screen with a `role="tablist"` audience-lens toggle ("▦ Engineer · Board" ↔ "▤ PM · Roadmap", `aria-selected` wired) that switches **in place** (`lens()` JS, `.board`↔`.roadmap` over the SAME ISS-377 data, shared atoms `.idc/.sg/.pri/.lbl`). Vocabulary adapts (Todo/In-Progress/Review/Done ↔ Now/Next/Later). Neither lens starved (PM keeps progress/points; engineer keeps WIP/keyboard). Same component, both lenses, switchable = the 4 anchor literally. |
| **B Workshop** | **3** | **JUDGEMENT.** Three-lens segmented control (Engineer/PM/Exec) over the same ISS-377/PR-412 rows, full-fidelity PM roadmap, vocabulary adapts ("Outcome · Reliable payments under load"). But the switch is a **navigation fork to separate HTML files** (`1-shell-roadmap-lens.html` → `2-engineer-pr-diff.html`), not an in-place retune of one component; and the **Exec lens is named but not rendered** (only PM+engineer shown). Neither shown lens starved, but the "one component" claim is asserted via shared tokens, not demonstrated as a single switchable surface. Vocabulary bet caps it. |
| **C Wayfinding** | **2** | **JUDGEMENT.** Lens nav (Engineer/Roadmap/Exec) references the same ISS-377, shares status-glyph/ref-chip/agent grammar — but the surfaces are **architecturally distinct layouts that reframe into different data models**: `02-ci-run.html` = DAG/log/triage 3-column flow; `03-roadmap.html` = Now/Next/Later lanes + canvas. It's "two surfaces citing the same backend," not one component density-tuned; Exec is an un-rendered link. Closer to the 0-anchor's quiet-fork risk than to a switchable single component. |
| **D Civic** | **3** | **JUDGEMENT (PROVEN tuning).** Strong shared-substrate proof: `01-shell-exec-dashboard.html` (comfortable exec KPIs) ↔ `02-roadmap-dense.html` (dense transit-timetable Gantt) are the **same shell / status grammar / ref chip / token set over the same initiatives**, genuinely density-tuned (font 12→11.5px, 30px rows, Gantt vs bars, j/k nav) — not a fork; plus a real DPO↔subject pair in the DSR console. Capped at 3 because the lens change is a **rail link between two screens**, not an in-place switch, and the engineer lens is competent-not-flagship. |

---

## Per-dimension comparison

**D2 winner — B Workshop (4).** It is the only finalist whose empty state teaches the *model* ("a roadmap
lane is just your real work read over time — no second copy to maintain") rather than just offering a button,
and whose tone is warm-without-toylike across the editorial serif + prose editor + respectful agent copy. D
(3) is the strong runner-up and arguably *wins for the DPO/exec specifically* (legal-path empty state,
consequence-before-action), but its sober palette is cooler at true first contact. A and C (both 2) have
competent, well-designed state sets but skins that read expert-first/cold to a non-engineer — A is a
midnight command-deck, C is engineer-native chat ("panic at lru.rs:142").

**D5 winner — A Instrument (4).** A is the only finalist that *demonstrates* the rubric's literal 4: one
component, both lenses, switchable **in place** (a `role="tablist"` toggle re-rendering the same ISS-377 data
as engineer board ↔ PM roadmap, shared atoms, neither lens starved). D (3) has the best *shared-substrate*
proof (same shell/tokens/status grammar genuinely density-tuned, plus a DPO↔subject pair) but switches via a
rail link between two screens. B (3) has the richest *vocabulary* adaptation and three named lenses but forks
into separate files with the Exec lens unrendered. C (2) reframes into divergent data models across distinct
layouts — the weakest "one component" claim, edging toward a quiet fork.

## Lens verdict (1 paragraph)

A **PM/corporate user adopts B Workshop fastest** — it is the only finalist that makes a non-engineer feel
*invited and oriented* on landing (empty states that teach the mental model, warm editorial tone, a prose
surface where a PM actually thinks), and its three-lens framing speaks the PM/exec vocabulary directly; **D
Civic is the close second and the better fit for a regulated DPO/exec** specifically. The quiet-fork / starved-
lens risks: **C** forks hardest — its "lens" is navigation between architecturally different surfaces with
divergent data models (DAG/log vs Now/Next/Later), so "one component" is not demonstrated and the non-engineer
roadmap, while clean, is a reframe rather than a tuned view. **B and D** technically fork into separate screens
too (B leaves its Exec lens unrendered; D uses a rail link), but both keep a genuinely shared component
substrate, so neither *starves* a lens. **A** is the inverse trade: it owns D5 outright with a true in-place
switch where neither lens is starved, but its PM lens is the *less-loved half* and its cold command-deck skin
makes it the slowest for a non-engineer to warm to (D2 = 2). Over my two owned dimensions, **A wins D5, B
wins D2**, and no finalist clears both — the warm-approachable pole (B) and the one-switchable-component pole
(A) are, in this set, in tension. **Caveat carried as unvalidated:** every D2/D5 score rests on the
persona-adaptive-vocabulary bet and expert-judged comprehension/warmth, both `[DEFERRED-UNTIL-USERS]`.
