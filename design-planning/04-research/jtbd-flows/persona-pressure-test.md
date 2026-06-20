# R-05 — Persona Pressure-Test & Validation-Priority Register

> **Phase 4 research corpus** · deliverable of prompt **R-05** (workstream
> [`ws-b-jtbd-and-flows.md`](../../02-research-roadmap/ws-b-jtbd-and-flows.md)).
> **File date: 2026-06-20.** No real users exist; personas P1–P15 are HYPOTHESES
> ([`personas.md`](../../../planning/01-research/personas.md) §0, §7). This file
> **pressure-tests the persona assumptions that R-03 ([jtbd-catalogue.md](./jtbd-catalogue.md))
> and R-04 ([cross-surface-flows.md](./cross-surface-flows.md)) silently inherited**, and
> produces the **register of what to validate first**. It does not re-derive the personas or
> the jobs — it interrogates the *load-bearing assumptions underneath them*. Depends on R-03;
> informed by R-04. Feeds **R-16** (dual-audience), the rubric (D4/D5/D6/D9), and the
> sketch-funnel axes.

## 0. How to read this file

The single most important fact about this corpus: **the personas are hypotheses, no user has
been interviewed** (`personas.md` §0, §7; VISION §3 honesty rule). Every job in R-03 and every
flow in R-04 is built on assumptions about *who these people are and what they actually need*.
This file's purpose is to **make those assumptions explicit, rate our confidence, say what
breaks if each is wrong, and rank which to validate first** — so the deferred real-persona
research (the load-bearing risk per roadmap README §6) attacks the riskiest assumptions first
rather than spreading thin.

Two methods, per the prompt:

1. **Proto-persona pressure-testing** *(method #3)* — for each persona we name its
   **load-bearing assumptions** (the ones the whole design rests on, not trivia), assign a
   **confidence** (H/M/L), and state **what-breaks-if-wrong**. **PROVEN** *(proto-persona /
   assumption-first discipline: J. Gothelf, Lean UX — proto-personas are explicitly
   provisional and must be treated as assumptions to validate, not facts;
   [Gothelf, "Using Proto-Personas for Executive Alignment"](https://jeffgothelf.com/blog/using-proto-personas-for-executive-alignment/);
   [NN/g, "Personas: Study Guide"](https://www.nngroup.com/articles/persona/) on assumption-based
   vs research-based personas).*

2. **Assumption / risk mapping** *(method #4)* — we plot the load-bearing assumptions on a
   **risk × certainty grid** to derive the validation order. **PROVEN** *(the assumption-mapping
   / riskiest-assumption-test prioritisation: Lean UX assumption mapping & the
   "test the riskiest assumption first" rule; [Gothelf & Seiden, Lean UX — assumptions
   prioritisation](https://jeffgothelf.com/blog/leanuxcontent/);
   [Strategyzer / D. Bland, "Assumptions Mapping"](https://www.strategyzer.com/library/test-business-ideas-assumptions-mapping)
   — prioritise by importance × evidence; high-importance / low-evidence is tested first).*

**Tags.** **PROVEN** = grounded in a cited method/standard. **HOUSE STYLE** = our synthesis /
taste. Every *specific* assumption, confidence rating, and ranking below is **HYPOTHESIS** in
the sense of R-03 (unvalidated, no users) — the **method** is PROVEN; the **instantiation** is
our reasoned judgement and is exactly what the deferred study (§5) must confirm or falsify.

**Confidence scale (HOUSE STYLE, used consistently):**
- **H (higher)** — strongly-evidenced *industry pattern* (the pains are well-attested even if
  *our personas* are not) → lower validation urgency.
- **M (medium)** — plausible but contested; depends on segment/archetype mix.
- **L (lower)** — a genuine bet with little external support; **these dominate the
  validation-priority register**.

> Confidence is **not** the same as importance. A high-confidence assumption that, if wrong,
> kills the product still ranks high to validate. The §4 register multiplies **blast-radius ×
> (1 − confidence)** (assumption-mapping logic), not confidence alone.

---

## 1. Per-persona load-bearing assumptions (pressure-test)

For each persona: the **load-bearing assumptions** (the ones design choices depend on), the
**confidence**, and **what breaks if wrong** — tied to the R-03 jobs and R-04 flows that would
collapse. We group by audience (mirrors R-03 §1) so corporate/governance is impossible to skip.

### 1.A Engineers (P1–P5)

| # | Persona | Load-bearing assumption (HYPOTHESIS) | Conf. | What breaks if wrong (jobs/flows endangered) |
|---|---|---|---|---|
| A-a | **P1 backend** | Engineers will *trade their incumbent muscle memory* (GitHub+Jira+Slack keyboard habits) for Myelin's palette/keyboard model **only if it is at least as fast** — speed is the price of entry, not a delighter. | M | E7 (palette flow), F-ENG-1 keyboard path; if false, the whole "wedge engineer flagship" loses its wedge — engineers tolerate a tab-switch they already know over a new system that is merely *unified but slower*. |
| A-b | **P1/P2** | The *why-without-leaving-flow* job (E1) is **high-importance AND low-satisfaction** — i.e. engineers are genuinely pained by context-scatter, not resigned to it. | M | E1, E5, F-ENG-2; if engineers have *already* internalised the tab-switch as free (satisfaction high), the live-backlink chain is a nice-to-have, not the differentiator R-04 stakes the seam register on. |
| A-c | **P3 platform/SRE** | P3 is the **strongest internal stakeholder for the shared backend** and *wants* first-class event triggers over webhook glue (E8). | H | E8, E9, F-PM-1 incident path; lower risk — this pain is very well-attested industry-wide. |
| A-d | **P4 staff** | Staff engineers' **leverage surface is search + reference graph + cross-cutting views**, and they will adopt for *that* even if daily git UX is a wash. | M | E5, M7, M8; if P4's real leverage is elsewhere (e.g. people/meetings), the "reference graph as senior-eng leverage" bet weakens. |
| A-e | **P5 OSS maintainer** | A **public, EU-hosted, GDPR-clean forge** is a *real adoption driver* for European OSS — not just a compliance checkbox. | L | E11, E12; **flagged in personas.md as positioning, not validated.** If false, the OSS/public-default surface investment (and the P5↔P12 conflict below) is mis-prioritised. |
| A-f | **P1–P5 (cross)** | Agents **save** engineer time rather than *create review/noise toil* — engineers will accept agent-pending states and HITL cards as net-positive. | L | E11, M9, F-AGT-1 entirely; this is the agent-native bet at the engineer level. Mock-agent acceptance ≠ real-LLM acceptance (R-15 caveat). If engineers find agent volume a tax, calm-volume (R-15) becomes existential, not polish. |

### 1.B PM / delivery (P6–P10)

| # | Persona | Load-bearing assumption (HYPOTHESIS) | Conf. | What breaks if wrong |
|---|---|---|---|---|
| B-a | **P6 PM** | A PM will **abandon the parallel reality** (Productboard/Notion/slides) and trust a roadmap that is *a view over the engineers' issue data* — i.e. the same-data roadmap is *enough* for stakeholder communication. | **L** | M1, F-PM-2, the **D1 dual-audience pair**, rubric D5. **This is the single most load-bearing PM assumption** — the entire "one product, no second system" thesis for the PM audience rests on it. If PMs still want a separate narrative/aspirational layer, D1 is *not* a same-data pair and the §2 "serves neither" trap reopens. |
| B-b | **P6 PM** | The PM-friendly lens (roadmap/now-next-later) can be a **configuration of the shared views component**, not a separate product — and won't feel like "an engineer tool with PM paint." | L | M1, M3, D1, D2; R-16 owns the per-lens critique. If the config-not-fork bet fails for PMs, the views component fractures. |
| B-c | **P7 EM** | EMs want delivery analytics as **insight, not surveillance**, and will trust metrics drawn from one event stream — *and their reports won't game them*. | M | M4, D5; the "fairness, not gaming" emotional job. If EMs (or their reports) perceive surveillance, the analytics surface is rejected or gamed → metrics disagree (the D5 failure). |
| B-d | **P8 PgM/TPM** | Cross-team dependency/portfolio management over shared data is **important enough** to PgMs that they'll adopt Myelin for it vs. a dedicated PPM tool. | M | M7, D5; PgM may be the *least* native fit (lowest agent appetite, most likely to keep a specialist tool). |
| B-e | **P9 designer** | Myelin **referencing** (not replacing) Figma is acceptable to designers — they'll keep design intent in Myelin's reference graph despite authoring elsewhere. | L | M8; **explicitly an open product question (personas.md §7, design-language §9).** If designers won't dual-home, the design↔code reference chain (M8) has no designer-side anchor. |
| B-f | **P10 tech-writer** | Writers want **docs-as-code coupled to Git change-signal** + an A4 curation agent, over a standalone wiki. | M | M9, M10; the stale-doc/freshness job. Plausible but the docs-as-code-vs-wiki tradeoff is "eternal & unhappy" (personas.md P10) — preference may split. |

### 1.C Corporate / governance (P11–P15)

| # | Persona | Load-bearing assumption (HYPOTHESIS) | Conf. | What breaks if wrong |
|---|---|---|---|---|
| C-a | **P13 DPO** | A DPO will **trust the DSR/sovereignty consoles at a glance** — "make sovereignty legible" actually lands as confidence, not as another dashboard to distrust. | **L** | G5, G6, G7, F-GOV-1, rubric D9, sketch Axis 6. **Sovereignty-as-UX has no external playbook** (flagged R-03 §4, R-04 §11). This is the corporate-side mirror of B-a: the highest-stakes governance bet, lowest evidence. |
| C-b | **P12 security** | An *inspectable* unified RBAC + agent-governance/kill-switch surface **answers a CISO's deepest fear of agent-native platforms** rather than amplifying it. | **L** | G2, G4, F-AGT-1 governance lens. If a CISO sees "first-class agents" as net-new attack surface no console allays, the agent-native thesis is *blocked at the gate* by the very persona who gatekeeps adoption. |
| C-c | **P11 CTO/VP** | Consolidation ("one platform, one bill, one identity, one audit") is a **decisive buying argument** — the integration value outweighs best-of-breed loss. | M | G1; the economic-buyer thesis. If CTOs prefer best-of-breed + integration tax, the "one product" pitch loses its buyer. |
| C-d | **P14 procurement** | EU-sovereignty + anti-lock-in + clean exit is a **real differentiator that wins deals**, not just a hygiene factor. | M | G9; tied to A-e and the public-sector archetype bet (personas.md §6 — "strategy hypothesis, not a fact"). |
| C-e | **P15 IT-admin** | Self-host / sovereign-cloud operability is **demanded** (not just nice) by the sovereignty-sensitive segments, and one admin surface beats N tools' N admin models for them. | H | G8, G10; well-attested operational pain; lower risk. |
| C-f | **P11–P15 (cross)** | The **regulated-enterprise & public-sector archetypes are Myelin's strongest fit** — so the *weight* given to C-jobs (and the sovereignty surfaces) is correct. | **L** | The whole C-audience prioritisation; **personas.md §7 explicitly calls this an unvalidated strategy hypothesis.** If the wedge is actually scale-ups (B-audience), the corpus over-invests in governance surfaces. Inherited directly from R-03 §9 uncertainty #2. |

### 1.D Cross-cutting (all personas) — the structural assumptions

| # | Assumption (HYPOTHESIS) | Conf. | What breaks if wrong |
|---|---|---|---|
| X-a | **The three-audience model is the right cut** (engineers / PM-delivery / corporate-governance) rather than, say, org-archetype (startup / scale-up / regulated). | M | The entire R-03 grouping and the dual-audience pairs §5. If the more salient cut is *archetype*, the dual-audience tension may be less central than the startup-vs-enterprise tension (see X-b). |
| X-b | **One architecture serves a 3-person startup and a 10,000-person enterprise** with governance that scales from invisible→fully-audited (personas.md §6). | M | Onboarding (R-20) + IA default-landing; if the startup is drowned by enterprise complexity (or the enterprise underserved by startup simplicity), the "world-scale day one" promise fractures the UX. |
| X-c | **Personas hold these jobs at all** — the *situations* in R-03's job stories actually occur for the recruited segment. | M | Any job whose situation is theoretical (R-03 §6.3 falsifier) is dead weight. The biggest unknown-unknown: a *top job we never wrote*. |

---

## 2. The persona-conflict matrix (whose needs collide, and the surface at risk)

These are pairs where **two personas' load-bearing needs are in genuine tension over the same
surface** — the design must *resolve* the conflict, not split into two products (the §2 trap).
Each names the **endangered surface**, the **R-03/R-04 artifact**, and the **resolution
direction** (the design rule that prevents a fork). **HOUSE STYLE** (our reading of the
tensions); each is a thing the deferred study must confirm is real and correctly resolved.

| ID | Persona pair | The collision | Endangered surface | Resolution direction (HOUSE STYLE) |
|---|---|---|---|---|
| **K1** | **P6 PM ↔ P1 engineer** *(the dual-audience tension — README §4 nominates this FIRST)* | PM wants **calm, spacious, narrative, outcome-framed** (Axis 1 calm / Axis 4 warm); engineer wants **dense, keyboard, terse, status-framed** (Axis 1 dense / Axis 2 palette) — **over the same issue model**. | **Views component** (§5.6) / the **D1** pair / issue surfaces (§7.3) | One component, role+density+vocabulary as **config not code** (R-16); each lens critiqued against its persona to prove **neither is a degraded compromise**. Sketch in **both** lenses (R-16 mandate). |
| **K2** | **P13 DPO / P12 security ↔ P1 engineer** *(the sovereignty-legibility bet — README nominates SECOND)* | Governance wants **always-on residency/visibility/attribution cues + auditability** near every artifact; engineers want **calm, uncluttered flow** with sovereignty *invisible by default*. | The **scope/residency cue** (§5.3), audit/attribution surfaces (§7.6), every artifact chip | **Axis 6 trade-off** (always-on cue ↔ on-demand console): sensible-invisible defaults for the startup/engineer; legible-on-demand depth for the DPO; per-artifact cue *present but quiet* (borders-over-shadow, status-not-colour-alone). R-19 owns it. |
| **K3** | **P5 OSS maintainer ↔ P12 security / P13 DPO** | P5 wants **public-by-default, open contribution, low-friction fork CI**; P12/P13 want **private-by-default, least-privilege, no secret/PII leak, strict residency**. | Issue/channel **visibility scope** (§7.3/§7.5), fork-CI permissions (§7.2), public/private separation | Default **must be per-tenant/archetype**, never global: public-default for the OSS cell, private-default for the regulated cell. The *same* visibility-chip mechanism, opposite defaults. (Maps A-e ↔ C-a/C-b.) |
| **K4** | **P7 EM ↔ P1–P5 engineers** | EM wants **delivery analytics / flow visibility** (M4); engineers fear **surveillance** and gaming. | Dashboards / delivery analytics (§7.3), the **D5** pair | Metrics framed as *team-unblocking insight*, drawn from **one event stream** (no per-audience re-derivation → no disagreement); transparency about what is measured; never individual-ranking by default. |
| **K5** | **P11 CTO/exec ↔ P1 engineer** | Exec wants **org-wide rollups & cost** (G1) — a *summarising/abstracting* lens; engineer wants **ground truth, no vanity metrics**. | Executive rollups / dashboards (§7.3), **D5** lens 3 | Rollups are **projections of the same event stream** the engineer trusts — so the exec number and the engineer number *cannot disagree* (the D5 anti-gaming rule). |
| **K6** | **P15 IT-admin ↔ P1 startup-founder-engineer** *(an X-b archetype collision)* | Admin wants **SSO/SCIM/residency/retention/agent-policy depth**; the startup founder wants **near-zero-friction, no enterprise config in their face**. | Onboarding / first-run (R-20), admin surfaces (§7.6), default-landing | **Progressive disclosure (P4):** enterprise depth one layer down, sensible-invisible defaults up top (personas.md §6 "configurable governance scales without forcing the startup through complexity"). |
| **K7** | **P9 designer ↔ P2 frontend engineer** | Designer authors in Figma and wants intent to *reach* the reference graph; engineer wants the design↔code link to be **live and trustworthy**, not a dead screenshot. | Design↔code reference chain (M8), backlinks (§7.4), external-design embed | Myelin **references** (not replaces) Figma cleanly — but the reference must be *live/permission-aware* (the §5.3 chip), not a snapshot. Depends on B-e holding. |

> **The two README-nominated conflicts (K1, K2) are the spine of the validation order in §4.**
> K1 is the central design problem made concrete at the persona level; K2 is the
> EU-sovereign positioning's load-bearing UX bet. Everything else either supports or extends
> these two.

---

## 3. Where the persona pressure-test exposes a *missing* assumption (carried to R-16/R-19)

Pressure-testing surfaced gaps R-03/R-04 did not flag explicitly — recorded so downstream items
inherit them:

- **The "satisfaction" half is the real unknown.** R-03's ODI plan (§6) rightly defers the
  ranking, but the pressure-test sharpens *why*: for engineers we assume **high pain** with the
  fragmented stack (low satisfaction) — but incumbents (GitHub, Linear, Slack) are *very good*;
  satisfaction may already be high, which would gut the wedge (assumptions A-a/A-b). **The
  riskiest number to measure is current satisfaction, not importance.** (HOUSE STYLE.)
- **Mock-agent vs real-agent trust is a persona assumption, not just a runtime caveat** (A-f,
  C-b). R-15 owns the trust-calibration plan; R-05 flags that the *persona's willingness to work
  with agents at all* (not just to trust a given output) is itself unvalidated.
- **The archetype cut may dominate the audience cut** (X-a/X-b). R-16 assumes the audience cut;
  if the deferred study shows archetype is more salient, R-16's "one component, many lenses"
  framing may need an *archetype* lens (startup-simple ↔ enterprise-deep) alongside the
  role lens. (HOUSE STYLE — flagged for R-16.)

---

## 4. The validation-priority register (which personas/assumptions to validate first)

**Method:** assumption-mapping (#4) — rank by **blast-radius × (1 − confidence)**: a wrong
assumption that kills a load-bearing thesis *and* has low evidence is tested first
(Strategyzer/Lean UX riskiest-assumption rule, cited §0). The ranking is **explicit and
justified**; it is a HYPOTHESIS-instantiation of a PROVEN prioritisation method.

| Rank | What to validate (assumption / persona) | Blast radius | Conf. | Why first (justification) |
|---|---|---|---|---|
| **1** | **K1 — P6-PM vs P1-engineer dual-audience tension** (B-a + B-b + A-a): does the *same-data* roadmap satisfy a PM, and does the *same component* not feel like a degraded compromise to either? | **Kills D1/D5 + the "one product" thesis** | L | **README §4 nominates this first.** It is the central design problem; if PMs keep a parallel reality, the platform's core justification fails. Low confidence + maximal blast radius → rank 1. |
| **2** | **K2 — P13-DPO / P12-security sovereignty-legibility bet** (C-a + C-b + C-f): does a DPO trust the consoles at a glance; does a CISO see agent governance as fear-allaying not fear-amplifying? | **Kills D9 + EU-sovereign positioning + agent-native adoption** | L | **README §4 nominates this second.** Sovereignty-as-UX has no playbook; P12/P13 are *gatekeepers* — they don't just dislike, they *block adoption*. Run jointly with R-19's regulated-buyer review (shared recruit). |
| **3** | **A-f / C-b — agents save time / agents are governable** (the agent-native bet at both ends): will engineers accept agent volume as net-positive, and will security govern it? | **Kills the agent-native differentiator** | L | The product's second pillar after unification. Caveat: mock ≠ real LLM (validate the *contract* willingness now; trust-calibration is R-15's deferred study). |
| **4** | **A-a / A-b — engineer speed-and-pain bet** (is the wedge flow actually painful enough, and is Myelin fast enough to win the switch)? | **Kills the engineer wedge** | M | The engineer audience is the highest-frequency adopter; if the incumbent is "good enough," the wedge flagship (F-ENG-1) has no wedge. The **switch test** (method #24) is the no-user proxy until users exist. |
| **5** | **C-f / X-a / X-b — segment-priority & audience-vs-archetype cut** (is regulated/public-sector really the strongest fit; is the audience cut right)? | **Mis-prioritises the whole corpus** | L | Strategy-level; cheaper to test via positioning interviews than full usability. Resolving it re-weights everything below. |
| **6** | **K3 / A-e — OSS public-default & EU-OSS adoption driver** | Mis-prioritises OSS/public surfaces | L | Real but narrower blast radius; gated by the segment-priority answer (rank 5). |
| **7** | **B-e / K7 — designer dual-homing (reference-not-replace Figma)** | Breaks design↔code chain (M8) | L | Open product question (personas.md §7); narrow blast radius; can follow the core. |
| **8** | **B-c / K4 — EM analytics as insight-not-surveillance** | Risks analytics rejection/gaming | M | Important but mitigable by framing/transparency; medium confidence. |
| **9** | **X-c — do the situations occur at all** (job-story reality check) | Removes dead-weight jobs / finds missed jobs | M | Bundled into every interview above (R-03 §6.2 contextual interviews) — not a separate study. |

**Ranking justification (summary):** ranks 1–3 are the three lowest-confidence,
highest-blast-radius bets and map exactly to Myelin's three pillars — **unification (K1),
sovereignty (K2), agent-native (A-f/C-b)**. Ranks 4–5 set whether the *engineer wedge* and the
*segment strategy* are sound. Ranks 6–9 are narrower or more mitigable. This ordering matches
both README §4's explicit nominations (K1 first, K2 second) and the assumption-mapping rule
(riskiest first).

**Feeds R-16:** ranks 1 (K1) and the audience-cut question (rank 5) are R-16's load-bearing
inputs — R-16 must critique each lens against the persona whose assumption is least confident
(P6 for the PM lens, P11 for the exec lens), and must carry the archetype-cut risk (§3).

---

## 5. `[DEFERRED-UNTIL-USERS]` — the real-persona replacement (the load-bearing risk)

**This is the single most important Phase-4 research item** (roadmap README §5.1 / §6). The
no-user substitute (this pressure-test + register) is **NOT validation** — it is a *map of what
to validate*. Presenting it as validated would be the exact honesty-rule failure VISION §3
forbids. The replacement is recorded here as a concrete, executable plan; **it is not done.**

`[DEFERRED-UNTIL-USERS]`

### 5.1 What to do
**Replace proto-personas P1–P15 with interview-derived real personas.** Recruit and interview
real practitioners per audience, then **re-author the persona set, the R-03 jobs, and this
register from evidence** — keeping the proto-personas only as the prior to compare against.

### 5.2 With whom (recruit — shared with R-03 §6.2 and R-19's regulated-buyer review to save fieldwork)
- **Engineers (A):** 8–12 spanning P1 backend, P2 frontend, P3 platform/SRE, P4 staff, P5 OSS
  maintainer — across **startup and scale-up** archetypes (to test X-a/X-b).
- **PM/delivery (B):** 8–12 spanning P6 PM, P7 EM, P8 PgM, P9 designer, P10 writer — **PMs and
  engineers recruited from the *same* orgs** so the K1 same-surface tension can be observed, not
  inferred.
- **Corporate/governance (C):** 6–10 spanning P11 CTO/VP, P12 security, P13 DPO, P14
  procurement, P15 IT-admin — **heavily weighted to regulated-enterprise & public-sector**
  (where K2/C-jobs are decisive; this *is* the R-19 regulated-buyer review).

### 5.3 Method (per the prioritised order in §4)
- **Riskiest-assumption interviews first** (ranks 1–3): semi-structured interviews +
  evidence-of-current-behaviour ("show me how you do this today") targeting K1, K2, agent-bet.
  For K1 specifically: put a *same-data-two-lenses* prototype in front of a PM **and** an
  engineer from the same org and observe whether each accepts their lens (this overlaps R-16's
  both-audience validation — run jointly).
- **The ODI importance × satisfaction survey** (R-03 §6) layered on top — interviews validate
  the *jobs are real and well-worded*; the survey ranks them. **Measure satisfaction with the
  incumbent stack especially carefully** (§3 — the riskiest number).
- **For K2/sovereignty:** a DPO/procurement *console review* (the R-19 substitute) — show the
  DSR console, residency cue, audit explorer; observe whether trust forms at a glance.

### 5.4 What would falsify each load-bearing assumption (per row)
A persona assumption is **falsified** when, in evidence:
- **K1 / B-a:** PMs, given the same-data roadmap, still maintain a separate
  narrative/aspirational tool → D1 is *not* a same-data pair; "serves both" fails.
- **K2 / C-a:** a DPO does **not** trust the DSR receipt/residency cue at a glance and reaches
  for a manual export to be sure → sovereignty-as-UX did not land.
- **C-b:** a CISO judges first-class agents as net-new attack surface no console allays → the
  agent-native thesis is blocked at the gate.
- **A-a/A-b:** engineers rate current-stack satisfaction *high* (incumbent good enough) → the
  wedge has no wedge; importance alone won't carry adoption.
- **A-f:** engineers experience agent-pending/HITL volume as a tax, not a help → calm-volume
  (R-15) is existential, not polish.
- **C-f / X-a/X-b:** the strongest-fit segment is **scale-ups**, not regulated/public-sector, or
  the **archetype** cut beats the **audience** cut in salience → the corpus's governance weight
  and R-16's role-lens framing must be re-balanced.
- **X-c:** a job's *situation* never occurs for the recruited segment → the job is theoretical;
  delete it. A high-importance/low-satisfaction job we never wrote → promote it.

### 5.5 What we will NOT claim until this runs
Until 5.1–5.4 execute with real users, **§1–§4 stand as an unvalidated, HYPOTHESIS-tagged map of
risks** — not validated personas, not a validated conflict set, not a validated ranking. The
ranking tells us *what to test first*; it does not tell us *what is true*.

---

## 6. Actionability toward the control artifacts & §9 gloss-risks

| Control artifact / risk | What R-05 equips | Where |
|---|---|---|
| **rubric D5** (dual-audience) | K1 + ranks 1 are the persona-level statement of the D5 bet; gives R-16 its least-confident lens to critique against. | §2 K1, §4, §3 |
| **rubric D9** (sovereignty) | K2 + rank 2 frame the sovereignty-legibility bet as a *persona conflict to resolve*, not an aesthetic. | §2 K2, §4 |
| **rubric D6** (agent) | A-f/C-b (rank 3) name the agent-native bet at both engineer and security ends. | §1.A/§1.C, §4 |
| **rubric D4** (one-product coherence) | The conflict matrix §2 is the persona-level "where coherence is hardest" list. | §2 |
| **sketch-funnel Axis 1/4 (density/tone)** | K1 sets the dense↔calm / terse↔warm tension the funnel must span over shared components. | §2 K1 |
| **sketch-funnel Axis 6 (sovereignty visibility)** | K2 articulates the always-on-cue ↔ on-demand-console trade R-19 owns. | §2 K2 |
| **R-16 (next consumer)** | Ranks 1 + 5 + the archetype-cut risk are R-16's load-bearing inputs. | §3, §4 |

**Completeness-critic (roadmap README §9) — which gloss-risks apply to R-05.** R-05 is a
*persona/assumption* deliverable, not a *states/screens* one, so most §9 unglamorous-state risks
are **consciously deferred to their owning items** (R-21 states, R-17 a11y, R-13 device). R-05's
relevant slice is the **process gloss-risk**: the funnel converging early on one instinct. R-05
counters it by making the **dual-audience (K1) and sovereignty (K2) tensions explicit
load-bearing conflicts** the sketch funnel must hold apart — directly supporting the
cull-for-spread rule. The §9 **edge-case/cross-surface flow** risks (partial-failure,
cross-cell, DSR-both-sides) are owned by R-04/R-19; R-05 ensures the *persona conflicts that make
those flows matter* (K2, K3) are named.

---

## 7. Self-check against R-05 acceptance criteria

| Criterion (prompt R-05 / ws-b) | Status | Evidence |
|---|---|---|
| **Every persona has named load-bearing assumptions + confidence + what-breaks-if-wrong** | ✅ Met | §1.A–§1.C cover P1–P15 (+ §1.D cross-cutting structural assumptions); each row has assumption, conf., breakage. |
| **Conflict matrix names real pairs + the endangered surface** | ✅ Met | §2 K1–K7: each names the persona pair, the collision, the endangered §7 surface + R-03/R-04 artifact, and a resolution direction. |
| **Validation-priority ranking explicit AND justified** | ✅ Met | §4 register ranks 1–9 by blast-radius × (1−confidence) with per-row justification + summary. |
| **Nominates K1 (P6-PM vs P1-engineer) and K2 (P13-DPO/P12-security sovereignty) FIRST** | ✅ Met | §4 ranks 1 (K1) and 2 (K2), matching README §4; justified. |
| **Deferred real-persona replacement recorded as a plan, NOT done** | ✅ Met | §5 `[DEFERRED-UNTIL-USERS]`: what/with-whom/method/per-row falsification + §5.5 "will not claim until run." |
| **Build ON R-03/R-04, don't duplicate** | ✅ Met | Pressure-tests the *assumptions under* R-03 jobs / R-04 flows; references them by id, re-derives nothing. |
| **Date the file; PROVEN/HOUSE-STYLE tags; cited methods; name uncertainties** | ✅ Met | Dated 2026-06-20; methods #3/#4 cited (Gothelf/Lean UX/Strategyzer/NN-g); HOUSE-STYLE vs HYPOTHESIS marked; §3 + below names uncertainties. |
| **Completeness-critic §9 gloss-risks addressed** | ✅ Met | §6: names the process gloss-risk R-05 owns; consciously defers state/a11y/device risks to owning items. |
| **Actionable toward rubric & sketch-funnel; feeds R-16** | ✅ Met | §6 maps to D4/D5/D6/D9, Axes 1/4/6; §3+§4 name R-16's inputs. |

**Honest partials / top uncertainties.**
1. **Every assumption, confidence, and rank is itself a HYPOTHESIS** — the register is a *map of
   risk*, not measured risk. The confidence ratings are our reasoned judgement and could be
   wrong about which assumptions are riskiest (the meta-risk).
2. **The riskiest single number is engineer current-satisfaction** (§3) — incumbents are
   excellent; if the fragmented stack is "good enough," the wedge premise weakens even if
   importance is high. This is under-weighted everywhere in the corpus and only §5's study
   resolves it.
3. **The audience-vs-archetype cut (X-a/X-b)** may be the deeper structuring question than any
   single persona; if so, R-16's role-lens framing needs an archetype lens too (flagged §3).
4. **K1 and K2 are low-confidence by construction** — they are the two bets with the least
   external evidence and the most blast radius; that is precisely why they rank 1 and 2, and why
   the deferred replacement (§5) is the load-bearing Phase-4 item.

---

*End of R-05 deliverable. Date: 2026-06-20. Methods #3 (proto-persona pressure-test) and #4
(assumption/risk mapping) PROVEN (cited); all specific assumptions, confidences, conflicts, and
the ranking are HYPOTHESIS-instantiation (no users). Real-persona replacement is
`[DEFERRED-UNTIL-USERS]` (§5), recorded as a plan, not done. Feeds R-16, rubric D4/D5/D6/D9,
sketch-funnel Axes 1/4/6.*
