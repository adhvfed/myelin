# The Sketch Funnel — Two-Stage Plan with Named Axes of Variation

> Phase: `design-planning/02-research-roadmap`. Plans Phase 6 (the divergent-then-convergent sketch
> phase). Pairs with [`rubric.md`](./rubric.md) (sketches are designed *toward* it) and the Phase-1
> Double-Diamond frame (#5): Phase 6 is the **Develop** diamond — deliberate divergence, then
> convergence to finalists. The point is a **decision-ready spread**: the finalists must span the design
> space so the final human can choose — and overrule the recommendation — without a restart. Status
> date: **2026-06-20**.

The failure mode this plan exists to prevent: an autonomous pipeline converging early on **one
instinct** and producing four variations of the same idea. We defeat that by (a) naming **axes of
variation** up front so divergence is *structured* across the space, and (b) culling finalists for
**merit AND spread**, never four near-duplicates.

---

## Part 1 — The axes of variation (named up front)

Each axis is a spectrum with two named poles. Phase 6's divergent concepts (stage 6a) are deliberately
**scattered across these axes** so the space is actually covered; the finalists (6b) are chosen to
**occupy different positions** on the most decision-relevant axes. The first four are required; the
fifth and sixth are valuable and should be covered if budget allows.

### Axis 1 — Information density: **dense ↔ calm**
- **Dense pole:** maximal information per screen; tight spacing; many columns; Linear/Sourcehut-grade
  compactness as the *default*, breathing room earned.
- **Calm pole:** generous whitespace; fewer things on screen; Notion-grade approachability as the
  default, density behind a toggle.
- **Why it matters for Myelin:** the dual-audience mandate (§2) and "density is earned" (P5) mean the
  *default* density is a real, contestable choice — engineers want dense, PM/corporate want calm, and
  the product must pick a default and a toggle behaviour. The final human should see both instincts
  realized to decide where the default sits.

### Axis 2 — Navigation paradigm: **persistent rail ↔ command-palette-led ↔ contextual**
- **Persistent-rail pole:** a always-visible primary nav rail + contextual sidebar is the spine
  (GitLab/Slack-like); discoverability via visible structure.
- **Command-palette-led pole:** minimal chrome; `Cmd/Ctrl-K` is the primary way to move (Linear-like);
  the rail is thin or collapsible; discoverability via search.
- **Contextual pole:** the shell adapts to the current artifact/flow (the PR context-pane, the incident
  channel) and surfaces what's relevant *now* rather than a fixed global structure.
- **Why it matters:** P3 keyboard-first vs. P6/P11 approachability pull in opposite directions here, and
  the "one shell" (§5.1) is the central problem's physical embodiment. How much the user *navigates*
  vs. *summons* is a defining UX bet.

### Axis 3 — Surface unification: **highly-unified-one-skin ↔ distinct-per-surface-identity**
- **Highly-unified pole:** every surface looks/behaves almost identically; maximum muscle-memory
  transfer; minimal per-surface personality (one skin, density-tuned only).
- **Distinct-per-surface pole:** each surface keeps a recognisable identity (code feels like code, the
  roadmap feels like a roadmap, chat feels like chat) within shared tokens/components.
- **Why it matters:** **this axis IS the central design problem** ("one product, five surfaces" — the
  unification-vs-distinctness tension). Phase 6 must produce finalists at *different points* on this
  axis so the human can decide how much distinctness Myelin tolerates before it fractures, or how much
  uniformity it tolerates before a diff is starved or a roadmap suffocates.

### Axis 4 — Emotional tone: **utilitarian-precise ↔ warm-approachable**
- **Utilitarian-precise pole:** restrained, neutral, fast, no-nonsense; the tool gets out of the way
  (Linear/Sourcehut tone).
- **Warm-approachable pole:** friendlier, more human copy, softer shapes, more guidance and
  encouragement (Notion tone) — without becoming toylike (the §2 anti-pattern: no emoji-as-UI, no
  sparkle).
- **Why it matters:** "loved" is partly emotional (#13 visual direction is HOUSE STYLE), and the
  three-audience span means tone is contested — engineers distrust warmth, corporate/governance distrust
  whimsy, PMs reward approachability. The tone decision is a deliberate, reviewable choice, not an
  accident of sketch #1.

### Axis 5 (valuable) — Agent presence: **ambient/background ↔ foregrounded-collaborator**
- **Ambient pole:** agents are quiet — their output lives in threads/inbox/collapsible summaries;
  calm-by-default (P8/§6.5); you summon them.
- **Foregrounded pole:** agents are visible participants in the main flow (a reviewer in the PR, a
  triager in the queue), proposing inline.
- **Why it matters:** agent-native is a defining bet (P7/§6); how *present* agents are by default is a
  trust-and-calm trade-off worth seeing realized at both poles.

### Axis 6 (valuable) — Sovereignty visibility: **always-on cues ↔ on-demand consoles**
- **Always-on pole:** residency/visibility/lawful-basis cues are persistently present near the data
  (a region badge on the scope indicator, a visibility chip on every artifact).
- **On-demand pole:** sovereignty lives in dedicated, excellent consoles (the DSR/RoPA/residency
  surfaces, §7.6) reached when needed, keeping the daily UI clean.
- **Why it matters:** P9 says sovereignty must be *felt*, but P8 says attention is sacred; how much
  sovereignty is ambient vs. summoned is a real design choice for the regulated/public-sector buyer.

---

## Part 2 — Stage 6a: diverge cheap

**Goal:** many quick, single-screen concepts deliberately scattered across the axes, so the spread is
real before any deepening cost is spent.

- **Target count: 16–20 single-screen concepts.** (Cheap, throwaway-grade; one screen each; not yet a
  coherent system.) This breadth substitutes for the group-ideation ceremony Phase 1 SKIPped (README
  §3) — the breadth *is* the divergence.
- **Coverage requirement (so the spread is real, not random):** the 16–20 concepts must, between them,
  cover:
  - **Each pole of Axes 1–4 at least twice** (e.g. ≥2 dense and ≥2 calm; ≥2 highly-unified and ≥2
    distinct-per-surface; etc.), so no axis collapses to one instinct.
  - **A range of screens, not all the same screen.** At minimum the set must include concepts for:
    the **shell** (the one-product frame), a **dense engineer surface** (PR/diff or board or CI run), an
    **approachable PM/corporate surface** (roadmap or dashboard or a governance console), an **agent/HITL
    moment** (the approval card or an agent-reviewed PR), and the **command palette / reference-unfurl**
    wedge moment. A given concept screen is picked *and* placed at a deliberate axis position (e.g.
    "PR view, dense × command-palette-led × highly-unified × utilitarian").
  - **At least one concept that pushes a pole to its honest extreme** (e.g. a maximally-calm engineer
    surface, a maximally-distinct-per-surface shell) so the human sees the edges of the space, not just
    the safe middle.
- **Fidelity:** low — enough to read the idea and its axis position. Tokens may be rough. No need for
  all states yet (states come in 6c).
- **Output:** a divergence board: each concept tagged with its screen + its position on each axis + a
  one-line rationale. This board is the evidence that the space was actually spanned.

---

## Part 3 — Stage 6b: cull to 3–4 finalists (merit AND spread)

**Goal:** reduce to **3–4 finalists** chosen for BOTH merit and spread — never four near-duplicates.

- **Merit screen:** score each concept against the rubric ([`rubric.md`](./rubric.md)) at concept
  fidelity (the hard gates can only be *directionally* assessed here; full gate conformance is required
  at 6c). Drop concepts with fatal flaws (e.g. an instinct that can't ever clear G1/G2, or that forks
  the product on D4).
- **Spread rule (binding):** the 3–4 finalists **must occupy materially different positions on Axis 3
  (surface unification)** — the central-problem axis — and should also differ on at least one of Axes
  1, 2, or 4. Four finalists clustered at the same axis positions is a **cull failure** and must be
  redone. Concretely: if two surviving finalists are near-duplicates, replace the weaker one with the
  best concept from an *unrepresented* axis position, even if its raw merit score is slightly lower —
  spread is worth a small merit cost because it protects the human's ability to overrule.
- **Output:** a cull rationale naming, per finalist, (a) why it has merit and (b) which axis positions
  it represents that the others don't. The set as a whole must demonstrably span the space.

---

## Part 4 — Stage 6c: deepen each finalist into a coherent mini-system

**Goal:** each finalist becomes a coherent multi-screen mini-system, realized enough to actually
evaluate against the full rubric — and realized enough that **choosing a non-recommended finalist is NOT
a restart** (the decision-ready-spread requirement).

Each finalist must deliver:

- **3–5 key screens forming one coherent system** (same tokens, same shell, same components across them
  — proving the finalist's *own* internal coherence, which is also D4 evidence).
- **PLUS its design tokens** authored in **DTCG-conformant structure** (so G1 contrast is measurable and
  the tokens are portable into Phase 8) — primitive → semantic → component tiers, light + dark at
  minimum, with the focus token derived AA-safe (§8b.3).
- **The hard-gate demonstrations** (G1/G2) on the required screens: measured-contrast tokens; the
  long-word (German) + non-Latin (Greek/Cyrillic) strings; at least one **mirrored RTL** state; visible
  focus; status-not-by-colour-alone.
- **The unglamorous states** (README §9) for at least one surface: empty / loading-skeleton / error /
  permission-denied / erased-tombstone / agent-pending.

### The comparable screen set (every finalist MUST include — for side-by-side judging)

So Phase 7 can score the same things across finalists, **every finalist must include all of:**

1. **The shell** — the one-product navigation frame (primary nav + contextual sidebar + content +
   context pane), showing how the five surfaces compose into one skeleton (D4, central problem).
2. **At least one dense engineer surface** — a PR/diff *or* an issue board *or* a CI run view, at the
   finalist's chosen density (D1, D7).
3. **At least one approachable PM/corporate surface** — a roadmap/now-next-later *or* an executive
   dashboard *or* a governance/sovereignty console (D2, D5, D9).
4. **At least one agent / HITL moment** — the plan-then-apply approval card (Approve/Edit/Reject) in
   context, with the agent treatment and attribution (D6).
5. **One wedge moment** — the command palette *or* a reference chip/unfurl in action (the cross-artifact
   thesis made tangible; R-22).

A finalist that omits any of these cannot be scored on the dimension it would have demonstrated, and is
therefore **incomplete**, not merely weaker. Screens 2 and 3 must use the **same shared components**
(views/editor/chip) tuned differently — that *is* the dual-audience / one-product proof.

---

## Part 5 — The funnel in one line

**6a:** 16–20 single-screen concepts scattered across Axes 1–4 (≥2 per pole) covering shell + dense + 
approachable + agent + wedge screens → **6b:** cull to **3–4 finalists** for merit AND spread (must 
differ on Axis 3 + one other) → **6c:** deepen each into a **3–5 screen mini-system + DTCG tokens + 
hard-gate demos + unglamorous states**, all including the comparable screen set → Phase 7 judges against
[`rubric.md`](./rubric.md); the human picks (and may overrule) from a genuinely-spread set.
