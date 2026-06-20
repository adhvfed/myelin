# Phase 6 — Stage 6b (CONVERGE): The Divergence Board & Cull to Finalists

> Phase: `design-planning/06-design-sketches`. Reduces the 20 divergent single-screen concepts of 6a to
> **3–4 finalists chosen for merit AND spread** (per [`sketch-funnel.md §Part 3`](../02-research-roadmap/sketch-funnel.md)),
> scored *directionally* against [`rubric.md`](../02-research-roadmap/rubric.md) at concept fidelity, with
> the comparable screen set targeted from [`05-user-facing-surfaces/README.md §5`](../05-user-facing-surfaces/README.md).
> **Honesty (VISION §3):** at 6b the hard gates G1/G2 can only be assessed *directionally* (does this
> instinct have a credible path to clearing them at 6c?) and the scored dims D1–D10 only at concept
> fidelity. Every judgement is **taste-laden and overruleable** — tagged **PROVEN** (an inspectable
> fact in the artifact / a cited rubric rule) vs **JUDGEMENT** (the judge's read). Status date: **2026-06-20**.
> Do not commit (orchestrator handles git). 6c builds the finalists; this file does not.

---

## 1. The divergence board (all 20 concepts)

Columns: **ID · screen · persona · the six axis positions · merit read (concept-fidelity, vs rubric dims)
· directional gate-risk note (could it EVER pass G1/G2?)**. Axis order = **density · nav · unification
(Axis 3, the central problem) · tone · agent · sovereignty**. All axis positions are **PROVEN** (read
straight from each file's CONCEPT block, cross-checked against the markup); merit/gate reads are **JUDGEMENT**.

| ID | Screen | Persona | D1 dens · A2 nav · **A3 unif** · A4 tone · A5 agent · A6 sov | Merit read (strengths / flaws vs rubric) | Gate-risk (G1/G2 directional) |
|---|---|---|---|---|---|---|
| **C-01** | Shell + PR overview/context pane (G-6, wedge flagship) | Rauno Freiberg — "Instrument" | dense · rail · **highly-unified** · utilitarian · ambient · on-demand | **Strong.** The clearest "one product" frame: hairline-only grouping, one rationed accent (also focus-derived per §8b.3), the context pane *assembles itself* (D4/wedge). High D1/D7; agent ambient+labelled+gated (D6 good). Flaw: a PM would find it cold (D2 low). | **Low.** Focus token = derived accent already; status carries glyph+label; German string already in the rail (`#404 Schwellenwert…`). Credible G1/G2 path. |
| **C-02** | Shell + roadmap now/next/later (I-3, PM lens) | Jessica Hische — "Workshop" | calm · contextual · **distinct-per-surface** · warm · ambient · on-demand | **Strong for the warm pole.** Cream/ink ramp (never #fff/#000), one *disciplined* serif on headings only, the roadmap **is** the delivery data (no parallel Productboard) — the D5/D2 bet. Has the lens-switch segmented control (engineer/roadmap/exec) — direct D5 evidence. Flaw: warmth could read soft to engineers. | **Low–med.** German + Greek strings present in cards; warm ramp must prove AA on cream (contrast is the watch-item, not a blocker). |
| **C-03** | Shell + exec dashboard (I-9/I-4) | Massimo Vignelli — "Civic" | medium · rail · **highly-unified** · sober · ambient · **always-on** | **Solid, sober.** One grid, one family, restrained single-hue charts, status by glyph+label+position, **always-on residency/lawful-basis band** near the data (D9 strong; Axis-6 represented). Flaw: as a *direction* it overlaps C-10 (also Tufte-ish exec/always-on) and is less distinctive than the diff/chat candidates. | **Low.** Designed around glyph+label status and measured authority-blue accent; strong G1 instinct. G2: German `Rate-Limiting-Schwellenwerte` shown. |
| **C-04** | Shell at **distinct-EXTREME** (honest edge) | Vernacular brutalism (Deville/suckless) | medium · contextual · **distinct-EXTREME** · utilitarian · foregrounded · on-demand | **Valuable as the edge, not as a finalist.** Three side-by-side material chromes (terminal/IRC/ledger) deliberately fracture coherence to *show the human the edge* — keeps shared invariants on purpose. Foregrounded inline HITL plan card is good. Flaw: by design it **half-fails D4** (it is the cautionary extreme); not a shippable direction. | **Med-high risk as shipped.** Three forked chromes multiply the G1 surface (3 focus systems, 3 status grammars) and the "show the machinery" ethos fights reflow. Useful donor, not finalist. |
| **C-05** | PR pane / **diff** | Rauno Freiberg / Vercel-Geist | dense · **palette-led** · **highly-unified** · utilitarian · ambient · on-demand | **Top-tier craft.** Real SVG status icons (not colour-alone), the why-pane skeleton→fill, palette-led nav (`⌘K` is the spine), agent reviewer marked `advisory`+gated, line-level keyboard hints (`F7 next change · . comment`). Highest D1. Same Instrument family as C-01 but at the densest surface + palette nav. | **Low.** The most rubric-aware artifact in the set on status-not-colour-alone and keyboard. G2 is the to-prove (RTL diff, mixed-dir code) — but that's a 6c task, not a flaw. |
| **C-06** | Diff (code-feels-like-code) | Wim Crouwel "Gridnik" | dense · rail · **distinct-per-surface** · utilitarian · ambient · on-demand | **Good distinct-surface argument.** One monospace does *everything incl. chrome* so the diff keeps terminal identity, while keeping shared shell/chip/⌘K (D4 across the seam). Flaw: mono-everywhere is a narrow aesthetic; as a *whole-product* direction it would suffocate the roadmap/knowledge surfaces (Axis-3 distinctness that doesn't generalise). | **Low.** Mono grid + hairlines is contrast-friendly. G2: mono-everywhere risks Greek/German rendering headroom — a watch-item. |
| **C-07** | CI run + live log | Edward Tufte — Data | dense · contextual · **highly-unified** · utilitarian · foregrounded · on-demand | **Distinctive & smart.** Data-ink maximised, sparklines carry flaky-history, a **foregrounded** agent that *annotates the data* (points at the failing line) rather than decorates — a fresh take on D6 + Axis-5 foregrounded. Flaw: it's a single specialised surface; hard to read as a whole-product identity. | **Med.** Sparkline-dense data-ink is the hardest G1 case (tiny targets, data-as-meaning). Honest path exists but it's the riskiest of the strong ones. |
| **C-08** | Issue board (engineer lens) | Karri Saarinen / Linear | dense · palette-led · **highly-unified** · utilitarian · ambient · on-demand | **Strong, but near-duplicate of C-01/C-05.** Board as a *projection of the one views component* (D4/D5 thesis), keyboard-first (j/k/x/c), one accent on focus only. Excellent — but it occupies the *same* Instrument×highly-unified×utilitarian cell as C-01/C-05/C-15. Merit high, marginal spread value. | **Low.** Linear-grade keyboard model is a G1 asset; board drag-as-not-only-input is the named hard component. |
| **C-09** | Roadmap now/next/later | Jonathan Hoefler | calm · contextual · **distinct-per-surface** · warm · ambient · on-demand | **A near-duplicate of C-02.** Same screen, same warm/distinct/calm cell, serif-display reading rhythm. Slightly more typographic, slightly less system-thinking (no lens-switch control shown). C-02 is the stronger twin. | Low (same profile as C-02). |
| **C-10** | Exec dashboard | Edward Tufte | calm · rail · **highly-unified** · sober · ambient · **always-on** | **A near-duplicate of C-03** on the governance/always-on cell. Quiet annual-report small-multiples, always-on residency. Clean but overlaps C-03 (sober/always-on/unified) and C-07 (Tufte). | Low. |
| **C-11** | **DSR / sovereignty console** (always-on EXTREME) | Otl Aicher — institutional | calm · rail · **distinct-per-surface** · sober · ambient · **always-on-EXTREME** | **The D9 flagship.** Pictogram system, per-holder completeness, a **verifiable inclusion-proof receipt**, and an *honest legal residual* (`[OPEN — LEGAL]`) instead of trust-theatre. Uniquely owns the regulated-buyer surface (S-11) the funnel says ≥1 finalist must carry. Flaw: as a *whole direction* it's a console aesthetic, not a daily-driver shell. | **Low for G1** (signage-grade legibility is its whole point). Best-in-set on D9. Donor of the sovereignty console to whichever finalist carries it. |
| **C-12** | Roadmap pushed **DENSE** (calm-pole stress) | Massimo Vignelli | dense · rail · **highly-unified** · sober · ambient · on-demand | **Valuable stress test, not a finalist.** Transit-timetable roadmap proving "density is tuning not a fork" (the inverse of C-09). Important *evidence* for the unification thesis; as a standalone direction it's a single screen, overlaps C-03/C-10's sober-grid family. | Low. |
| **C-13** | HITL approval card (agent **FOREGROUNDED**) | Jony Ive | calm · contextual · **highly-unified** · warm · **FOREGROUNDED** · on-demand | **Beautiful single-object D6 take.** The plan card as the one calm focal object, timeline recedes. Strong agent-legibility. Flaw: one moment, not a system; warm+foregrounded+calm is a *tone*, not a whole-product spine. Best used as a donor for the HITL moment. | Low (reduction aids G1). |
| **C-14** | HITL approval card (agent **AMBIENT**) | Otl Aicher | dense · rail · **highly-unified** · utilitarian · **AMBIENT** · on-demand | **The honest counter-pole to C-13.** The proposal as *one typed row in the unified inbox* — calm-by-default agent presence (P8). Good systemic argument; pairs conceptually with C-18 (Rams inbox). Donor for the ambient-agent inbox treatment. | Low. |
| **C-15** | Command palette (**palette-led EXTREME**) | Rauno Freiberg | dense · **palette-led-EXTREME** · highly-unified · utilitarian · ambient · on-demand | **The wedge/Axis-2 extreme.** Four modes (Navigate/Act/Search/Build-query), token→chip query AST, permission-pre-filtered rows, consequential verbs routing *into* the gate. Excellent D1 evidence and the cleanest wedge. Same Instrument family — best as the **palette donor** to the Instrument finalist. | Low. |
| **C-16** | Live unfurl in chat (**wedge W2**) | Erik Spiekermann | calm · contextual · **distinct-per-surface** · warm · ambient · on-demand | **The wedge made tangible.** A pasted `myelin://` link unfurls into one live permission-aware card with a real inline action (Re-run failed); shows the unglamorous seams (no-access, cross-cell) honestly, never leaking a title. Strong D6/D9 micro. Donor of the live unfurl to the chat/warm finalist. | Low (permission-aware no-leak is a G1/D9 asset). |
| **C-17** | **Diff at the CALM-EXTREME** | Massimo Vignelli | **calm-EXTREME** · contextual · **highly-unified** · warm · ambient · on-demand | **The honest edge of Axis-1.** A typeset diff that *breathes* — proves whether the densest surface survives the calm pole. Real tokenised code, agent suggested-fix with Apply/Edit, German copy inline. Smart and well-built; but as a *default* it sacrifices D1/D7 (the engineer's core need). Edge evidence + donor for the calm-reading treatment. | Low. |
| **C-18** | Notifications inbox ("what needs me") | Dieter Rams | calm · rail · **highly-unified** · sober · ambient · on-demand | **Restraint as craft.** Provenance-always-shown, deduped groups, agent lane collapsed-by-default and sheds first, inbox-zero without confetti. Strong D7 (the firehose antidote). One of the eight shell invariants (S-4); a donor screen more than a whole direction. | Low (restraint aids G1). |
| **C-19** | Knowledge page / block editor | Jan Tschichold | calm · contextual · **distinct-per-surface** · warm · ambient · on-demand | **The reading/writing pole.** A measured ~66ch column, true type scale, the ONE editor (same chip/mention/slash/backlinks) tuned for prose. Tests the distinct edge gently (more serif character than the diff, same components). DocBot suggests ambiently. Strong D4-across-the-seam + D2 warmth. | **Low–med.** Long-measure serif is a G2 asset for expansion if it holds Greek/German; watch line-length under +35% German. |
| **C-20** | Chat · Zulip topics + **FOREGROUNDED** agents | Erik Spiekermann | medium · rail · **distinct-per-surface** · utilitarian · **FOREGROUNDED** · on-demand | **Top-tier.** Channel▸topic▸message wayfinding (the §4.3 threading bet realised), agents foregrounded yet *contained in their topic* (calm + foregrounded reconciled), a full in-stream HITL card with effects/gate/budget/**provenance-walk** (`correlation: incident-#9 → … → audit ✓`). Best D6 in the set; strongest distinct-per-surface *that still feels like one product*. | **Low.** Status carries glyph+label; agent four-channel treatment present; German message inline. Strong G1/G2 instinct. |

### Coverage confirmation (the space was actually spanned)

**PROVEN** (counted from the axis lines above):

- **Axis 1 (density):** dense ×8 (01,05,06,07,08,12,14,15) · calm ×9 (02,09,10,11,13,16,17,18,19) · medium ×3 (03,04,20). Honest extremes present: **C-17 calm-EXTREME**, dense-pole stress **C-12**. ✅ ≥2 per pole.
- **Axis 2 (nav):** rail ×9 · command-palette-led ×3 (05,08,15) incl. **C-15 palette-EXTREME** · contextual ×8. ✅ ≥2 per pole.
- **Axis 3 (unification — the central problem):** highly-unified ×12 · distinct-per-surface ×7 (02,06,09,11,16,19,20) · **distinct-EXTREME ×1 (C-04)**. ✅ ≥2 per pole + an honest extreme.
- **Axis 4 (tone):** utilitarian ×9 · warm ×6 (02,09,13,16,17,19) · sober ×5 (03,10,11,12,18). ✅ ≥2 per pole.
- **Axis 5 (agent):** ambient (majority) · **foregrounded ×4** (04,07,13,20). ✅ both poles realised.
- **Axis 6 (sovereignty):** on-demand (majority) · **always-on ×3** (03,10,11) incl. **C-11 always-on-EXTREME**. ✅ both poles realised.

The set genuinely spans the space, including the edges (C-04, C-11, C-15, C-17), not just the safe middle.

---

## 2. The cull rationale — the finalists

**Binding spread rule applied** ([`sketch-funnel §Part 3`](../02-research-roadmap/sketch-funnel.md)): the
finalists **must occupy materially different positions on Axis 3** and differ on ≥1 of Axes 1/2/4. The
single biggest cluster in 6a is **Instrument × highly-unified × utilitarian** (C-01/05/08/15) — four
near-duplicates. Spread discipline means **one** survivor from that cluster, not three, even though
several score high on raw merit.

### Finalist A — "Instrument" · **Axis 3 = highly-unified** (one-skin, density-tuned)
*Carrier concept: **C-05** (diff), absorbing **C-01** (shell/PR pane), **C-08** (board), **C-15** (palette).*

- **(a) Merit:** the most rubric-aware family in the set — real status-not-colour-alone (SVG glyphs),
  focus-token-derived-from-accent (§8b.3), the densest surface kept *calm* (D1+D7 ceiling), palette-led
  navigation as the spine (D1/Axis-2), and the context pane that assembles the wedge (D4). This is the
  reference answer to "can a power user cross the flow faster than Linear?" **JUDGEMENT:** highest combined
  D1/D3/D4/D7 in the set.
- **(b) What it represents that others don't:** the **highly-unified / one-skin** pole of the central
  problem at the **dense** + **palette-led** + **utilitarian** corner. It is the "maximum muscle-memory
  transfer, minimum per-surface personality" answer the human must be able to choose.

### Finalist B — "Workshop" · **Axis 3 = distinct-per-surface (warm)**
*Carrier concept: **C-02** (roadmap, with the lens-switch control), absorbing **C-19** (knowledge editor), **C-16** (live unfurl), **C-09** (roadmap typography).*

- **(a) Merit:** the dual-audience bet made visible — the roadmap **is** the delivery data with an
  engineer/roadmap/exec lens switch over the *same* records (D5, the funnel's binding Axis-3 spread point),
  warm cream/ink restraint that invites a non-engineer (D2) without going toylike, and C-19 proves the ONE
  editor tuned for prose (D4-across-the-seam). **JUDGEMENT:** best D2/D5 in the set.
- **(b) What it represents that others don't:** the **distinct-per-surface + warm + calm + contextual-nav**
  corner — the opposite Axis-3 pole from A, *and* opposite on Axes 1, 2, and 4. Maximal spread against A.

### Finalist C — "Wayfinding" · **Axis 3 = distinct-per-surface (utilitarian, foregrounded-agent)**
*Carrier concept: **C-20** (chat topics + foregrounded HITL), absorbing **C-13** (foregrounded single-object card craft), **C-07** (foregrounded data-annotating agent), **C-04** (the honest seam discipline, dialled back from EXTREME).*

- **(a) Merit:** the best agent-legibility artifact in the set (D6) — foregrounded agents *contained by
  topic* so foregrounded ≠ firehose (reconciles Axis-5 with P8), the full in-stream plan-then-apply card
  with effects/gate/budget/provenance-walk, and the §4.3 threading recommendation realised. **JUDGEMENT:**
  highest D6; a *different* distinct-per-surface answer from B (utilitarian wayfinding, not warm editorial).
- **(b) What it represents that others don't:** the **foregrounded-agent (Axis 5)** pole and a
  **utilitarian** distinct-per-surface identity — distinct from B's *warm* distinctness and from A's
  unification. It is how the human sees "agents as visible participants" realised calmly.

### Finalist D — "Civic" · **Axis 3 = highly-unified (sober) + always-on sovereignty**
*Carrier concept: **C-03** (exec dashboard, sober grid, always-on residency band), absorbing **C-11** (the DSR console + inclusion-proof receipt — the S-11 flagship), **C-10** (small-multiples), **C-18** (Rams inbox restraint), **C-12** (dense-roadmap "density-is-tuning" proof).*

- **(a) Merit:** the governance/regulated-buyer direction — always-on residency/lawful-basis near the data
  (Axis-6) + the DSR console with a verifiable receipt and an *honest* legal residual (D9 flagship; the
  funnel explicitly wants ≥1 finalist to carry S-11). Plainness-as-authority a DPO and a VP both trust
  (D2 for the corporate lens, not the engineer lens). **JUDGEMENT:** best D9; unique sovereignty posture.
- **(b) What it represents that others don't:** the **always-on sovereignty (Axis 6)** pole and a **sober**
  tone — and crucially it sits at **highly-unified** Axis-3 *but a different corner from A* (sober/medium/
  rail rather than utilitarian/dense/palette), so it does not duplicate A.

### How the set spans the space (the spread proof)

| Finalist | **Axis 3 (central problem)** | Axis 1 dens | Axis 2 nav | Axis 4 tone | Axis 5 agent | Axis 6 sov |
|---|---|---|---|---|---|---|
| **A Instrument** | **highly-unified (one-skin)** | dense | palette-led | utilitarian | ambient | on-demand |
| **B Workshop** | **distinct-per-surface (warm)** | calm | contextual | warm | ambient | on-demand |
| **C Wayfinding** | **distinct-per-surface (utilitarian)** | medium | rail | utilitarian | **foregrounded** | on-demand |
| **D Civic** | **highly-unified (sober)** | medium | rail | sober | ambient | **always-on** |

- **Axis 3 spread (binding):** two poles represented (unified ×2 *in different corners*, distinct ×2 *in
  different tones*) — the human can choose how much distinctness Myelin tolerates. ✅
- **Axes 1/2/4 spread (binding ≥1):** A↔B differ on **all four** of A1/A2/A3/A4; C and D each differ from
  both A and B on ≥2 axes. ✅
- **Valuable axes 5/6 covered:** C carries **foregrounded agents**; D carries **always-on sovereignty +
  the DSR console**. ✅ No two finalists are near-duplicates.

### The single most debatable cull decision (flagged for orchestrator sanity-check)

**Keeping D (Civic/sovereignty) over a second Instrument variant (C-08 board or C-15 palette).** By *raw
concept merit* C-08/C-15 are excellent and arguably out-score C-03/C-11 on craft. They were culled because
they are **near-duplicates of Finalist A's cell** (Instrument × highly-unified × utilitarian), and the
spread rule explicitly says to replace the weaker near-duplicate with the best concept from an
*unrepresented* axis position — here, **always-on sovereignty (Axis 6) + the regulated-buyer DSR console
(S-11)**, which the funnel says ≥1 finalist must eventually carry and which NO other finalist occupies.
**This is a deliberate small-merit-for-spread trade (JUDGEMENT).** If the orchestrator/human judges
sovereignty better handled as a *shared cross-cutting layer inside A/B/C* rather than its own finalist,
the cleanest reversal is: **drop D, promote C-08 (board) into A's screen set, and require the always-on
residency band + DSR console as a mandatory comparable-screen inside finalist D's slot folded into A.**
Flagged so the spread decision is overruleable without a restart.

---

## 3. The deepening plan for 6c

Each finalist becomes a **3–5 screen coherent mini-system + DTCG tokens + hard-gate demos + unglamorous
states**, and **must include the full comparable screen set** ([`sketch-funnel §Part 4`](../02-research-roadmap/sketch-funnel.md):
shell · ≥1 dense engineer · ≥1 approachable PM/corporate · ≥1 agent/HITL · ≥1 wedge). Screens are mapped
to specific 6a concepts to evolve. **At least one finalist (D) carries the sovereignty/DSR console.**

### Finalist A — "Instrument" (highly-unified, dense, palette-led, utilitarian)
- **Identity / visual direction:** Rauno-Freiberg "Instrument" — hairline-only grouping, one rationed
  electric accent (= focus-derived token), monospace load-bearing and native, near-zero radius, compact
  default; midnight command-deck tone.
- **Persona + grafts:** lead = **C-05/C-01**. Graft the **palette four-mode query-AST from C-15**, the
  **board-as-projection keyboard model from C-08**, and the **ambient-agent-inbox-row from C-14**.
- **Comparable screen set (concept→evolve):** shell+PR pane ← **C-01**; dense engineer = **diff ← C-05**;
  approachable PM/corporate = **issue board↔roadmap one-component, two lenses ← C-08** (board) tuned to a
  calmer roadmap lens (the D5 proof inside one skin); agent/HITL = **ambient inbox card ← C-14**; wedge =
  **command palette ← C-15**.

### Finalist B — "Workshop" (distinct-per-surface, calm, warm, contextual)
- **Identity / visual direction:** Hische/Hoefler editorial-warm but *disciplined* — warm-grey/cream ramp
  (never #fff/#000), ONE serif on reading headings over a shared UI sans, generous measure, humane copy.
- **Persona + grafts:** lead = **C-02**. Graft the **serif reading rhythm of C-09**, the **block editor of
  C-19** as the second dense-ish surface, and the **live unfurl of C-16** as the wedge.
- **Comparable screen set (concept→evolve):** shell+roadmap ← **C-02** (keep the engineer/roadmap/exec
  lens switch); dense engineer = **knowledge block editor ← C-19** *plus a diff rendered in the warm skin*
  (to prove a warm distinct-surface can still host code — borrow structure from C-17's calm diff); PM/
  corporate = **roadmap now/next/later ← C-02/C-09**; agent/HITL = a **warm plan card ← C-13** (Ive single-
  object) recoloured to the Workshop palette; wedge = **live unfurl ← C-16**.

### Finalist C — "Wayfinding" (distinct-per-surface, medium, utilitarian, foregrounded agent)
- **Identity / visual direction:** Spiekermann signage/wayfinding — functional grotesk, address-grade
  labelling (channel▸topic▸message), density that stays navigable like a transit map, one rationed
  wayfinding accent.
- **Persona + grafts:** lead = **C-20**. Graft the **single-object HITL focal craft of C-13** for the card,
  the **data-annotating foregrounded agent of C-07** for the CI surface, and the **honest seam discipline
  of C-04** (dialled back from EXTREME) for the per-surface identity.
- **Comparable screen set (concept→evolve):** shell+chat ← **C-20**; dense engineer = **CI run + live log
  with the foregrounded annotating agent ← C-07**; PM/corporate = a **topic/incident summary "canvas"**
  (the thin pinned-summary version, §4.3) read as a status surface; agent/HITL = **in-stream plan card ←
  C-20** (with C-13's focal craft); wedge = **live unfurl / reference chip in chat ← C-16/C-20 composer**.

### Finalist D — "Civic" (highly-unified, sober, always-on sovereignty) — **carries the DSR console**
- **Identity / visual direction:** Vignelli/Aicher "Civic" — one rigid modular grid, one type family at a
  few steps, single low-chroma authority blue, status by glyph+label+position, plainness-as-authority,
  **always-on residency/lawful-basis band** near the data.
- **Persona + grafts:** lead = **C-03**. Graft the **DSR console + inclusion-proof receipt + honest legal
  residual from C-11** (the S-11 sovereignty flagship — the mandated console), the **small-multiples of
  C-10**, the **dense-roadmap "density-is-tuning" grid of C-12**, and the **Rams inbox restraint of C-18**.
- **Comparable screen set (concept→evolve):** shell+exec dashboard ← **C-03**; dense engineer = **dense
  roadmap/portfolio grid ← C-12** (proving density-is-tuning within the unified skin) *or* a sober diff in
  the same grid; PM/corporate = **exec dashboard ← C-03/C-10**; **sovereignty/DSR console ← C-11** (the
  always-on Axis-6 flagship); agent/HITL = a **sober inbox-row plan card ← C-14/C-18**; wedge = **command
  palette** in the Civic skin (shared invariant) *or* a residency-aware unfurl.

---

## 4. What each finalist must PROVE at 6c (gestured-at-only in 6a)

All four must, per the rubric, ship the things 6a could only sketch: **full DTCG tokens** (primitive→
semantic→component, light+dark, focus token derived AA-safe), the **hard gates G1/G2** *demonstrated on the
required screen set*, and the **unglamorous states** (empty/loading-skeleton/error/permission-denied/
erased-tombstone/agent-pending) on ≥1 surface. Specifics per finalist:

- **A "Instrument" must prove:** that **dense + dark** clears **G1 contrast measured** (the acid accent
  must be AA on `--g0`, and the *focus* ring must be a derived AA-passing token, not the identity accent);
  the **diff under RTL** (mixed LTR code inside RTL prose, bidi-isolated) and **German +35%** without
  gutter/line-number breakage — the hardest G2 case in the set. Plus the **rebase-orphan** diff state and
  the **agent-pending** review state. (6a showed the accent and one German rail string only.)
- **B "Workshop" must prove:** that the **warm cream/ink ramp** clears **G1 AA** (warm-on-cream is the
  contrast watch-item) and that the **serif holds Greek + long German compounds** at the ~66ch measure
  without clipping/reflow loss at 200%; one **mirrored RTL** roadmap/editor; and the *empty/first-run*
  state (the roadmap that teaches a PM the next step — D2). 6a asserted warmth; it must now measure it.
- **C "Wayfinding" must prove:** the **agent-volume storm state** (the inbox/topic owns the 30× surge —
  foregrounded must stay calm under load), the **stream-drop/resume** state (chat timeline owns it), the
  full **HITL gate states** (gate-awaiting → approved/edited/rejected + the failure set), and a **mirrored
  RTL chat** with a real Arabic/Hebrew run + bidi-isolated LTR. 6a showed one happy-path card.
- **D "Civic" must prove:** the **DSR erasure-outcome / tombstone** states (the DSR console owns erasure
  outcome; `sub_gone`/`root_gone`/`erased`), the **cross-cell / cross-boundary residency warning** (T2/T3),
  **permission-denied without leak** (Restricted vs Absent), German+Greek in the always-on band + tables,
  and one **mirrored RTL** governance surface. 6a showed the happy-path receipt and band only.

---

*End of 6b cull. The board confirms the space was spanned (≥2 per pole on Axes 1–4 + honest extremes on
1/2/3/6); the 3–4 finalists (A Instrument · B Workshop · C Wayfinding · D Civic) occupy materially
different Axis-3 positions and differ on ≥1 of Axes 1/2/4; the most debatable cull (D over a 2nd
Instrument) is flagged as overruleable. Concept-fidelity reads are JUDGEMENT; axis positions and
inspectable craft facts are PROVEN. Feeds 6c (deepen) → Phase 7 (judge against the rubric). Not committed.*
