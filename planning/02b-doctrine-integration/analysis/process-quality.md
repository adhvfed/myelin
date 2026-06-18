# Doctrine Integration Analysis — Process & Quality Doctrine

> **Phase 2b — Doctrine Integration.** Source doc:
> [`external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (read in full). Doctrine framing:
> [`external-insights/README.md`](../../../external-insights/README.md) — each item is a
> **DEFAULT we follow unless we write down why not**, the same canonical status as
> [`VISION.md`](../../../VISION.md).
>
> **What this doc is.** A per-principle classification of the process/quality doctrine against
> the existing Myelin planning, with a concrete integration ACTION and a single binding target
> for every non-CONFIRMS item. Classifications: **CONFIRMS** (already committed — validation,
> kept brief), **SHARPENS** (we have it weaker; the insight tightens it), **RESOLVES-OPEN**
> (answers a Phase-1 open question with a default-to-beat), **CONFLICTS** (disagrees with a
> committed decision — flagged honestly), **NEW** (net-new).
>
> **The headline.** This doc is *process/quality doctrine*, not architecture. Our committed
> spine (VISION + the 14-ADR register) is overwhelmingly *what/where* decisions; this doctrine
> is *how to build it without it rotting*. The two barely overlap, so almost nothing here
> CONFLICTS and little is a pure CONFIRMS. Instead, **most of this doctrine binds FORWARD** to
> phases that have **no document yet**: Phase 5 (testing strategy), Phase 6 (roadmap
> sequencing), and Phase 8 (execution discipline). The integration job is therefore mostly
> *routing* — planting each principle as a named, non-negotiable input to the phase where it
> becomes real — plus a small number of back-patches to VISION and Phase 2 where the doctrine
> deserves canonical/structural status now (so it can't be skipped later).

---

## 0. Where this doc's principles live vs. the Myelin spine

| Doctrine principle | Our spine today | Binds primarily to |
|---|---|---|
| 1. Code wins over docs | VISION §3 "Quality over plan-adherence" (weaker) | VISION + Phase 8 |
| 2. Order by non-negotiability | Implicit in risk-severity (H/M/L) + ADR-15; never stated as a sequencing law | Phase 6 (roadmap) + Phase 8 |
| 3. Prove it or it isn't real | `cargo-mutants` signal (VISION §4); no quantified gates anywhere | Phase 5 (testing) + Phase 8 |
| 4. Actually try it (drive the real UI) | VISION §3 design-before-build (adjacent, not the same) | Phase 5 (testing) + Phase 8 |
| 5. The ratchet — committed mechanical gates | ADR-01 "lint/architecture-test obligation"; `cargo-mutants` | Phase 5 + Phase 6 + Phase 8 |
| 6. Investigate before you build | Nowhere | Phase 8 (execution discipline) |
| 7. Keep architecture coherent (abstract-at-third, contract reconciliation) | ADR-01/05/06/13 "share-the-X-not-the-Y" boundaries; TE-1 glue-rot | Phase 8 + Phase 4 + Phase 5 |
| 8. Human sign-off is the bottleneck | VISION design-before-build; ADR-08 HITL (for *runtime agents*, not the *build process*) | VISION + Phase 7 + Phase 8 |
| README: "name your floors" (honesty rule) | VISION §3 "honesty about uncertainty" (weaker — about *uncertainty*, not *shipped-floor honesty*) | VISION + Phase 5 + Phase 8 |
| README: specificity contract / settled-vs-open | ADR altitude vocabulary (DECIDED vs OPEN→P3/P4) — strong match | CONFIRMS |

---

## 1. Per-principle classification

### Principle 1 — The code wins over the docs

| Sub-insight | Class | Where it binds | Action |
|---|---|---|---|
| Treat running code as source of truth; docs are intent re-verified against code | **SHARPENS** of VISION §3 "Quality over plan-adherence" | **VISION** (canonical) + **Phase 8** | VISION §3 says "the plan is a tool… choose quality and write down why" — it makes the plan subordinate but never states the *operational* rule "**when a doc and the code disagree, the code wins; fix the doc, then proceed.**" Back-patch VISION §3 with that one sentence so it is canonical for every execution agent. |
| Scheduled **truth-up passes** that re-sync docs to code | **NEW** | **Phase 8** (execution discipline) + **Phase 6** (schedule them) | No planning artifact schedules doc/code reconciliation. Phase 6 roadmaps must budget periodic truth-up passes; Phase 8's between-agents orchestration step (VISION §5.8 already gives the orchestrator a "decide whether intermediate work is needed" hook) is the natural place to run them. |
| **Date your status/capability docs; a claim that outlives its verification actively misleads the next agent** | **NEW** | **Phase 8** + **Phase 7** (prompts) | This is the killer detail given our model: Phase 7 produces one *sequential* prompt chain and Phase 8 runs it one agent at a time, each agent reading prior agents' capability notes. A stale "X is a stub" note is exactly the cross-agent hazard the doctrine warns of. **Default-to-beat:** every capability/status note an execution agent writes carries a date + the commit/verification it was true at; the prompt template (Phase 7) mandates this field. |

### Principle 2 — Order work by non-negotiability, not by size

| Sub-insight | Class | Where it binds | Action |
|---|---|---|---|
| Sequence by "what kills you first," not by layer/convenience | **SHARPENS** | **Phase 6** (roadmap sequencing) | We *have* the raw material — the Phase-1 register's severity (H/M/L) and §6 "top cross-cutting risks," and ADR-15's resolver-phase tags — but **no committed rule that the roadmap is ordered by non-negotiability.** Make it the explicit Phase-6 sequencing law. |
| **Stop-the-bleeding first: silent data loss and RCE outrank every feature** | **RESOLVES-OPEN** for **TE-28** (CI untrusted-code isolation), **GD-1/3/4** (erasure/durability), **TE-9** (bus loss) — gives them sequencing priority | **Phase 6** | Default-to-beat handed to Phase 6: the durability/no-data-loss floor (event bus delivery, outbox, crypto-shred correctness) and the CI sandbox-escape floor (TE-28, the §6 #10 dedicated security track) are sequenced **before** any feature surface on top of them. This operationalises the severity ratings that today are just labels. |
| Then keystones → breadth → polish/scale | **CONFIRMS** (matches VISION §6 "breadth over depth where depth isn't yet due" + the phased plan) | — | Validation only. |
| **Gate invariant: no later phase claimed done while an earlier phase's gate is red — enforced, not aspirational** | **NEW** | **Phase 5** (defines the gates) + **Phase 8** (enforces) | We have phase *sequencing* (VISION §5) but no *gate invariant* tying "done" to an earlier gate being green. Phase 5 must define per-phase/per-system gates; Phase 8 enforces the invariant mechanically (ties into Principle 5's ratchet). |

### Principle 3 — Prove it or it isn't real

| Sub-insight | Class | Where it binds | Action |
|---|---|---|---|
| **A property does not exist until a test forces the failure and observability watches the system survive it** | **NEW** (core) | **Phase 5** (testing strategy) | This is the single most important forward-binding principle in the doc and Phase 5 has **no document yet**. Make it Phase 5's organising thesis: every non-negotiable property gets a failure-injection drill + an observability assertion, or it is not claimed. |
| Gates resolve to **quantified thresholds** (RPO/RTO, "zero sandbox escapes," "zero messages lost across a reconnect," "disabled user has zero access paths within N minutes") | **RESOLVES-OPEN** — converts the still-qualitative risks into measurable gates: **GD-14** (backup-window RPO), **TE-9/SC-4** (bus zero-loss), **TE-28/AG-4** ("zero sandbox escapes"/loop-runaway), **TE-2/SC-1** (permission-leak = "zero cross-tenant read"), **SC-13** ("disabled user → zero access in N min") | **Phase 5** | Default-to-beat: Phase 5 attaches a *quantified threshold* to each top-cross-cutting risk in the Phase-1 register §6. Hand Phase 5 a starter table mapping each H-severity open question to a measurable gate. |
| **Never weaken a threshold or invert an assertion to make a check pass; a red gate is information; record honest "needs human verification"** | **NEW** | **Phase 5** + **Phase 8** | Execution discipline: an agent under "make it green" pressure must not soften a gate. Bind into the Phase-7 prompt template and Phase-8 discipline; it is the testing-side twin of "name your floors." |
| **Observability is part of the pass condition** — survive a drill but emit no signal = failed the drill | **NEW** | **Phase 5** + **Phase 3** (it implies an observability/telemetry substrate the shared systems must expose) | Phase 5 makes "the system emitted the survival signal" a pass criterion. This also lands a *requirement on Phase 3*: the shared systems (bus, authz, GDPR/audit) must expose the telemetry the drills read — currently unspecified. Flag for Phase 3 as a cross-cutting observability contract. |
| Build the **failure-injection harness early** (load gen 1×/10×/30×, mixed principal types, scoped-reversible dependency break, assertions from prod telemetry; cheap drills in CI, expensive scheduled; every incident adds a drill) | **NEW** + partially **RESOLVES-OPEN** **AG-4/AG-5** ("wants adversarial design + load testing"), **SC-2/SC-4** (world-scale load) | **Phase 5** + **Phase 6** (budget it early) + **Phase 4 (CI)** (the harness *is* a CI capability) | Phase 5 specifies the harness; Phase 6 sequences it **early** (it is itself a "keystone" by Principle 2). The "mix principal types" detail directly serves the agent-load and loop-runaway validation that AG-4/AG-5 explicitly deferred to "testing (P5)." Note the harness reuses the CI sandbox substrate (TE-31). |

### Principle 4 — Actually try it (exercise the real thing before claiming it)

| Sub-insight | Class | Where it binds | Action |
|---|---|---|---|
| **Drive the real UI in a browser before claiming it works**; the "switch test" — done only when someone could move to it without hitting a wall the old tool didn't have | **NEW** | **Phase 5** (E2E strategy) + **Phase 8** (frontend execution discipline) | VISION §3 mandates *design-before-build* (sketches first) but says nothing about *driving-after-build*. These are complementary, not the same. Add the "drive the real UI / switch test" as the frontend acceptance bar in Phase 5's testing strategy and Phase 8's frontend-task discipline (VISION §5.8 already routes frontend tasks through sketches; add "and is exercised in a browser before claimed done"). The "switch test" is also a crisp acceptance phrasing for the MVP wedge (PR-1/PR-10). |
| Integration tests use fresh DB + single handler + render-once; **real sessions chain mutations and update state mid-flight — write E2E tests that chain operations** | **NEW** | **Phase 5** | Concrete, non-obvious testing-design instruction. Bind as a Phase-5 default: every subsystem's test plan includes *chained-mutation* E2E flows, not just isolated handler tests. Especially load-bearing for collaborative editing (TE-15/16), issue transitions, and agent plan-then-apply loops (ADR-08) where state mutates mid-flight. |
| **Untested is acceptable if you name it untested** (record yes/no/partial per piece of work); silent skipping is the failure | **SHARPENS** of VISION §3 "honesty about uncertainty" + the README "name your floors" rule | **VISION** + **Phase 5** + **Phase 8** | VISION §3 covers honesty about *uncertainty/assumptions*; it does not require a *per-work-item tested: yes/no/partial* record. Sharpen by adding the explicit "name your floors / record whether it was exercised" discipline (see the README honesty-rule row below) to VISION §3 and the Phase-7 prompt template. |

### Principle 5 — The ratchet (turn discipline into committed, loud gates)

| Sub-insight | Class | Where it binds | Action |
|---|---|---|---|
| Convert each assumed discipline into a **committed mechanical gate** (CI jobs, pre-commit hooks, custom scanners from the fingerprint of a recurring failure) | **SHARPENS** of ADR-01 (which already names a "lint/architecture-test obligation" for no-cross-subsystem-DB, but only for that one rule and "[Deferred mechanism → P3/P6]") | **Phase 6** (roadmap: build the gates) + **Phase 5** (which gates) + **Phase 8** (write-the-check-when-a-bug-recurs) | Generalise ADR-01's single lint into a platform-wide *ratchet practice*: every quality invariant becomes a committed check. Phase 6 budgets the gate-building; Phase 8 carries the "when the same bug recurs a few times, commit the check that makes it impossible" habit. |
| **An uncommitted gate is no gate** (a config on disk never wired into CI lets drift accumulate) | **NEW** | **Phase 6** + **Phase 8** | Sharp acceptance criterion: a gate counts only if it runs in CI. Bind into the definition-of-done for any roadmap item that "adds a check." Note `.gitignore` already seeds `cargo-mutants` (VISION §4) — the ratchet rule says: *wire it into CI*, don't just leave it available. |
| Make violations **loud, never silently swallowed** — replace `... \|\| true` and silent filters with explicit noisy failures | **NEW** | **Phase 8** (execution discipline) + **Phase 5** | Concrete anti-pattern ban. Add to Phase-8 discipline and as a candidate custom-scanner (a lint for `\|\| true` / swallowed errors) — itself an instance of the ratchet. Especially relevant to idempotent bus consumers (ADR-04): a swallowed consumer error is exactly the "multi-day misdiagnosis." |

### Principle 6 — Investigate before you build

| Sub-insight | Class | Where it binds | Action |
|---|---|---|---|
| **Test the hypothesis before fixing** (introspect DB, replay events, reproduce symptom); the obvious cause is frequently wrong | **NEW** | **Phase 8** (execution discipline) | Pure execution discipline; nothing in planning covers debugging method. Bind to Phase 8. Note Myelin's **event log makes "replay the events" a first-class debugging tool** (ADR-04) — call that out as the platform's investigation affordance. |
| Follow the chain to **root cause** (surface → API → data/architecture); "looks right but doesn't fire" = dig, not patch | **NEW** | **Phase 8** | Same. |
| **Triage** — not every signal warrants a work item, but every fix needs a confirmed cause | **NEW** | **Phase 8** | Same; pairs with Principle 8 (spend the scarce human/decision budget well). |

### Principle 7 — Keep the architecture coherent as it grows

| Sub-insight | Class | Where it binds | Action |
|---|---|---|---|
| **Abstract at the third copy** (hoist a pattern into one primitive on its third hand-roll; earlier is premature, later is load-bearing duplication) | **SHARPENS** of ADR-01/05/06/13's "share-the-X-not-the-Y" boundaries — gives a concrete *trigger rule* for when reuse boundaries get drawn during build | **Phase 8** (execution) + **Phase 4** (subsystem design) | Our ADRs *drew* the big shared boundaries (content AST, db/views, query AST, glue triad) but give no rule for *emergent* duplication discovered during build. "Abstract at the third copy" is that rule. Bind to Phase 8 (and Phase 4 for per-subsystem patterns). |
| **Spawn a cleanup pass the moment a workaround threatens to go load-bearing** (trigger: "would building more on top of this gap make it harder to fix later?") | **NEW** | **Phase 8** (between-agents orchestration) | VISION §5.8 already lets the orchestrator launch "intermediate agents" between execution steps — this principle is the *trigger criterion* for doing so. Wire it into Phase 8's between-agents decision: a load-bearing workaround is a stop-and-repair signal (echoed by the doc's closing "if features keep getting harder to add, repair the foundation"). |
| **Reconcile cross-component contracts at the plan layer, before either side ships — agree on field names AND units up front; a unit mismatch calcifies and is brutal to unwind** | **SHARPENS** of ADR-13 (the glue contracts) + ADR-04 (envelope fields) + **RESOLVES-OPEN** for **TE-1** (glue-rot) with a concrete pre-ship discipline | **Phase 3** (shared-system contracts) + **Phase 5** (reconciliation) + **Phase 8** | ADR-13 makes the contracts binding but doesn't mandate *pre-ship reconciliation of names+units*. This is precisely Phase 5's job (VISION §5.5: "a reconciliation agent reviews all architectures as a whole"). Strengthen Phase 5's mandate to include explicit field-name-and-unit reconciliation across every cross-component contract (event envelope fields, `ArtifactRef` shape, authz `list-objects` results, budget/quota units in ADR-08). The "100× scale difference" example maps directly to e.g. CI metering units (TE-32), budget/quota units (AG-5), SLA timer units (SC-11). |

### Principle 8 — The human sign-off is the bottleneck — design around it

| Sub-insight | Class | Where it binds | Action |
|---|---|---|---|
| When most building is autonomous, **human approval, not agent capacity, is the scarce resource — spend it well** | **NEW** (about the *build process*; distinct from ADR-08 HITL which is about *runtime agents*) | **VISION §5** + **Phase 7/8** | Important framing for *our own* phased-agent build (VISION §5 runs agents to do the work). Today VISION §5.8 has the orchestrator decide between agents but never frames human sign-off as the *bottleneck to optimise*. Add this as a build-process principle. **Do not conflate** with ADR-08's runtime HITL gates — flag the distinction explicitly so Phase 7/8 don't merge them. |
| Surfaces with **security/abuse/cost/irreversible-scope** implications are *decision-shaped*: produce a sketch and **pause for human sign-off**; tag each next step "just build it" vs "needs a decision first" | **SHARPENS** of VISION §3 (design-before-build) + ADR-08 (decision-shaped runtime actions) | **Phase 7** (prompt design) + **Phase 8** | VISION §3 mandates sketch-before-frontend; this generalises "pause for sign-off" to *any* security/cost/irreversible surface, not just frontends. Bind into Phase 7's prompt-chunking: each prompt is tagged "autonomous" vs "needs-a-decision-first," concentrating human review on the decision-shaped ones. This is the build-time analogue of ADR-08's runtime "suggest-by-default; human-confirm consequential." |
| **Don't churn a document while a human is reading it for sign-off** | **NEW** | **Phase 8** | Small but concrete orchestration rule. |
| Let genuinely-safe autonomous work proceed ungated; reserve the human for calls only a human should make | **CONFIRMS** the spirit of VISION §5.8 (orchestrator judgement) | — | Validation; the tagging action above operationalises it. |

### README cross-cutting rules (doctrine-wide)

| Rule | Class | Where it binds | Action |
|---|---|---|---|
| **"Name your floors"** — shipping a floor is fine; a floor *masquerading as done* is the failure. If a thing is partial/untested/deferred, say so in writing and name the follow-on. Untested-but-named is acceptable; silent skipping is the failure mode | **SHARPENS** of VISION §3 "honesty about uncertainty" — VISION covers honesty about *what you're unsure of*; this adds honesty about *shipped completeness* | **VISION** (canonical) + **Phase 5** + **Phase 8** | This is the doctrine's stated keystone ("the one that keeps the whole effort honest") and deserves canonical status. Back-patch VISION §3 with an explicit "name your floors" clause; bind the per-work-item "tested: yes/no/partial + named follow-on" record into the Phase-7 prompt template and Phase-8 discipline. Pairs with Principle 4's "untested is acceptable if named." |
| **Specificity contract** — settled areas (durability, tenancy, identity, sandboxing, event backbone) are named directly; genuinely-open areas (collab editor, UX details, per-subsystem storage) leave the design to you | **CONFIRMS** — our ADR vocabulary already does exactly this: DECIDED for settled (ADR-01 monorepo, ADR-03 ReBAC, ADR-11 tenancy, ADR-12 GDPR), OPEN→P4 for genuinely-open (TE-15 CRDT/OT, TE-21 chat tier, TE-17 flexible-field execution) | — | Validation. Our DECIDED / DECIDED(directional) / OPEN→PN ladder *is* the specificity contract. Worth noting it confirms we drew the settled/open line in the same places the doctrine would. |

---

## 2. Genuine conflicts

**None.** This doctrine is process/quality discipline and the committed spine is architecture;
they are orthogonal by design. The closest tension is *terminological, not substantive*:
Principle 8's "human sign-off bottleneck" and ADR-08's "HITL gates" both say "pause for a human,"
but one governs **our phased-agent build process** and the other governs **runtime agents acting
on tenant data**. The only risk is a downstream agent *merging* them. Recommended resolution:
the integration action for Principle 8 explicitly names the distinction so Phase 7/8 keep the
two HITL notions separate (build-time review gates vs. runtime Art. 22 / AI-Act gates).

A second, milder note: VISION §3 "Quality over plan-adherence" and Principle 1 "code wins over
docs" are *compatible* (both subordinate the plan to reality), so this is a SHARPENS, not a
conflict — but an execution agent could read "quality over plan-adherence" as licence to ignore
docs entirely. The back-patch (add the precise "code wins; fix the doc, then proceed" rule)
removes that ambiguity.

---

## 3. Forward-binding summary — what each downstream phase inherits

Because Phases 5/6/8 have no document yet, this doc is effectively a **specification of inputs**
for them:

- **VISION (back-patch §3):** add (a) "code wins over docs — when they disagree, fix the doc
  then proceed"; (b) the **"name your floors"** clause (shipped-completeness honesty +
  tested: yes/no/partial); (c) the build-time "human sign-off is the scarce resource" framing.
  These earn canonical status because they are *honesty/quality invariants*, the same class as
  the existing §3 non-negotiables.
- **Phase 2 (no new ADR needed):** the doctrine *confirms* the spine; the one structural item
  worth a small Phase-2 note is generalising ADR-01's single "architecture-test lint" into the
  **ratchet practice** (Principle 5) — but the mechanism was already deferred to P3/P6, so this
  is a pointer, not a new ADR.
- **Phase 3 (shared systems):** inherits **one new cross-cutting contract** — an *observability/
  telemetry surface* every shared system exposes (bus, authz, GDPR/audit, durable-workflow), so
  Phase-5 drills can read survival signals (Principle 3). Plus the pre-ship field-name-and-unit
  reconciliation discipline on the glue contracts (Principle 7).
- **Phase 5 (testing strategy) — the biggest inheritor:** organising thesis = "prove it or it
  isn't real"; quantified gates per top-risk (the §6 register becomes a gate table with RPO/RTO/
  zero-X thresholds); the failure-injection harness spec (load 1×/10×/30×, mixed principals,
  reversible dependency breaks, telemetry assertions); chained-mutation E2E tests; drive-the-
  real-UI / switch test; observability-as-pass-condition; "never weaken a gate."
- **Phase 6 (roadmap sequencing):** the sequencing law = "order by non-negotiability" (stop-the-
  bleeding: data-loss + RCE/sandbox-escape floors first), the gate invariant (no later phase
  done over a red earlier gate), build the failure-harness and the ratchet gates *early*, budget
  truth-up passes.
- **Phase 7 (prompts):** the prompt template carries the discipline forward — each chunk tagged
  "autonomous" vs "needs-a-decision-first"; each work item records tested: yes/no/partial + named
  follow-on; status notes are dated.
- **Phase 8 (execution) — the second-biggest inheritor:** code-wins + dated status notes;
  investigate-before-you-build (use event-replay); abstract-at-the-third-copy; spawn-a-cleanup-
  pass when a workaround goes load-bearing (the between-agents trigger); loud-not-swallowed
  failures; don't-churn-a-doc-under-review; reserve the human for decision-shaped calls.

---

## 4. Prioritised top deltas (the 5–8 that matter most)

1. **"Prove it or it isn't real" + quantified gates → Phase 5 organising thesis.** Phase 5 has
   no doc; this principle defines its spine. Convert the Phase-1 register §6 top-risks into a
   gate table with measurable thresholds (RPO/RTO, "zero sandbox escapes" TE-28, "zero messages
   lost across reconnect" TE-9, "zero cross-tenant read" TE-2/SC-1, "disabled user → zero access
   in N min" SC-13). **Highest leverage; resolves several H-severity opens into measurables.**

2. **"Order by non-negotiability" → Phase 6 sequencing law (stop-the-bleeding first).** Make the
   data-loss and CI-sandbox-escape floors sequence *before* feature surfaces, plus the **gate
   invariant** (no later phase done over a red earlier gate). Turns today's H/M/L *labels* into a
   committed *ordering*.

3. **The failure-injection harness, built early → Phase 5 spec + Phase 6 early-sequence + Phase
   4 (CI).** Directly discharges the AG-4/AG-5 "wants adversarial design + load testing" debt
   (agents waking agents, agent-generated load) that the ADRs explicitly punted to testing. Its
   "mix principal types / 1×–30× load" shape is the only way to validate loop-runaway and
   agent-load governance before production trust. Reuses the CI sandbox (TE-31).

4. **"Name your floors" + "code wins over docs" → VISION §3 back-patch (canonical).** The
   doctrine's stated keystone honesty rule and the operational doc/code-precedence rule deserve
   the same canonical status as the existing §3 non-negotiables, because our *entire build is
   sequential agents reading each other's notes* — stale/over-claimed status is our sharpest
   self-deception risk. Cheap to write now, expensive as drift later.

5. **Dated status/capability notes + truth-up passes → Phase 7 prompt template + Phase 8.**
   Specific to Myelin's one-agent-at-a-time execution model (VISION §5.8): an undated "X is a
   stub" note actively misleads the next agent. Mandate a date + verified-at-commit field on
   every status note, and budget periodic doc/code truth-up passes.

6. **Drive-the-real-UI / "switch test" + chained-mutation E2E → Phase 5 + Phase 8.** Complements
   (does not duplicate) VISION's design-before-build: design-first *and* drive-after-build. The
   "switch test" doubles as a crisp MVP-wedge acceptance bar (PR-1/PR-10). Chained-mutation E2E
   tests target exactly where collab-editing (TE-15), issue transitions, and agent plan-then-
   apply (ADR-08) bugs live.

7. **The ratchet — committed, loud, mechanical gates → Phase 6 + Phase 8.** Generalise ADR-01's
   lone architecture-test into a platform practice: an uncommitted gate is no gate; wire
   `cargo-mutants` (already seeded) into CI; ban silently-swallowed failures (`|| true`); write
   the scanner when a bug-class recurs. The compounding-payoff signal (features getting *harder*
   to add ⇒ repair the substrate) is the trigger to spend a cleanup pass.

8. **Pre-ship contract reconciliation (names AND units) → Phase 5 reconciliation mandate.**
   Strengthen Phase 5's existing "review all architectures as a whole" to *explicitly* reconcile
   field names and units across every glue contract (envelope fields ADR-13, budget/quota units
   AG-5, CI metering TE-32, SLA timer units SC-11) before either side ships — the concrete
   anti-rot discipline behind TE-1.

---

## 5. Cross-references

- [`external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
  — the source doctrine.
- [`VISION.md`](../../../VISION.md) §3 (non-negotiables; back-patch target), §5 (phased build).
- [`planning/02-holistic-architecture/architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md)
  — ADR-01 (ratchet seed), ADR-04 (bus loss gates), ADR-08 (runtime HITL vs build-time
  sign-off distinction), ADR-13 (glue-contract reconciliation).
- [`planning/01-research/open-questions-and-risks.md`](../../01-research/open-questions-and-risks.md)
  — §6 top cross-cutting risks (→ Phase-5 gate table), AG-4/AG-5 (→ failure harness),
  TE-28/GD-1/GD-4 (→ stop-the-bleeding sequencing).
- **Binds forward to:** Phase 5 (testing strategy — biggest inheritor), Phase 6 (roadmap
  sequencing), Phase 7 (prompt template), Phase 8 (execution discipline).
