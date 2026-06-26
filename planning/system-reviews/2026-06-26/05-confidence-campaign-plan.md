# The Confidence Campaign — Execution Plan

Date: 2026-06-26. Status: PLAN (authored this window; executed next window when weekly budget resets).

This document is the spec for getting Myelin from *shape-proven* to *genuinely trustworthy* — functionality that
is real, security that is real **and** well-conceived, and evidence that cannot be gamed. It is written to be
executed by an orchestrating agent running one prompt at a time, exactly like the P-001..P-521 ledger.

It supersedes nothing in M7; it sequences the work that finds and remediates the shortcuts M7 was the first to
surface, and folds the known M7 floors into one cost-ordered campaign.

---

## 0. The principle the whole campaign turns on

A shortcut is a divergence between the model and reality. The build optimized for "green gate + commit," and the
load-bearing failure mode is that **the same agent, in the same context, wrote both a stub and the test that
passes against it** — so the test inherits the stub's blind spot and proves the agent's idea of "done," not
reality. Green gates and green scorecards (including the band scorecards already in `testing/scorecards/`) are
therefore *not* evidence of confidence; they are exactly the artifact that can hide a shortcut.

The one rule that generalizes: **break the builder/verifier coupling.** Every detector must be independent of the
thing it inspects — a different agent, a different framing (outside-in/black-box), a real backend instead of a
mock, an **external oracle** instead of our own assertion. Wherever those coincide, a shortcut can hide.

**A second structural gap: the ledger has a conformance loop, not a learning loop.** Each prompt's gate asks "did
this pass?" — never "now that this exists, was it the right thing to build?" A frozen contract plus an incentive
to conform means an early-era shape gets *bolted onto* rather than reconsidered: the platform's multi-cell,
object-store, real-crypto, and real-agent layers are all retrofits over an earlier core, and each swap leaves a
seam that reads as "X, but actually X′ added later." Detailed planning did not prevent this — it *caused* it,
because the plan specified components and a build order, not smoothed interactions. Coherence is emergent from
interactions, so it needs a design-level feedback pass the ledger never ran. The bounds the plan imposed were not
too tight — they were *unidirectional*: an agent could conform-and-defer, but nothing rewarded reconsidering the
shape. Stage D supplies the missing direction.

Two classes of shortcut, handled differently:
- **Named floors** — carry a `Floor named:` comment; finite, enumerated (M7 audit + a grep). Tractable.
- **Silent shortcuts** — no comment; invisible to a search for comments. *The real risk.* They need discovery,
  not a checklist.

Confidence is **constructed and quantified, not declared.** The end-state acceptance test is a
**property-falsification map**: for every load-bearing claim the platform makes, write down what would make it
false and which *independent* gate catches that falsification. When every falsifier maps to an automated gate or
a recorded human blocker, confidence is earned.

---

## 1. Cost-ordered stages (cheap broad nets before the expensive comb)

Each stage shrinks the work the next, dearer stage must do. Run in order; do not pay for the line-by-line review
to rediscover things a grep or a system test would have caught for a fraction of the cost.

### Stage A — Known-gap triage + cheap mechanical census  *(cheapest; do first)*
- **Input:** `production-readiness-audit.md`, every `Floor named:` / `TODO` / `FIXME` / `todo!()` /
  `unimplemented!()` / `for now` / `in-memory` / `Structural*` / `oneshot` marker in the tree.
- **Method:** a mechanical sweep (grep + a cheap Haiku pass to classify each hit) → a single list of *known*
  gaps, split into **cheap/mechanical** (fixable in a small prompt) vs **deep** (already owned by an M7 prompt).
- **Fix the cheap known gaps now**, before the expensive review, so Stage C does not spend Opus tokens
  rediscovering flaws we already know about.
- **Output:** `known-gaps.md` (every marked shortcut, its disposition, its filling prompt or a new small one).

### Stage B — Whole-system use-case tests on real backends + external oracles  *(medium cost, high yield for "is it real?")*
- **Input:** the canonical use cases — the four E2E scenarios (E2E-1..E2E-4) + each subsystem's golden path.
- **Method:** drive the system end to end through production surfaces against the live stack, and assert against
  **external oracles that do not share our code's blind spot**:
  - Git → interop with the **real `git` client** (clone/push/fetch against our server), then `git fsck` the result.
  - Search → results equal a **brute-force reference scan** over the same corpus.
  - Knowledge/Markdown → `render(parse(md)) === md` round-trip.
  - CRDT/collab → **convergence against an independent CRDT library** or a model checker.
  - Durable workflow → **replay determinism** (same inputs → same effects across a kill).
  - Identity/authz → an unauthorized viewer asserts no-count / no-ranking / no-backlink leak, end to end.
- **Cost/benefit guardrail:** these are worth a lot per token, but **do not gold-plate.** One solid oracle test
  per subsystem beats ten variations. Stop when the golden path and its main adversarial case are covered.
- **Output:** a use-case test suite + a list of every realness divergence it surfaced (feeds Stage C / remediation).

### Stage C — Full-code adversarial scan, model-tiered, **scan-as-planner**  *(expensive; the fine-toothed comb; last)*
- **Input:** the whole tree (~410k LOC Rust, ~60 crates), minus what Stages A/B already resolved.
- **Method:** a fan-out where each agent adversarially reads its assigned crate/boundary **against the contracts
  and prompts it claims to satisfy**, looking for silent shortcuts — a body trivial relative to its contract, a
  mock-only test, an in-memory `*Store` in a production constructor, a `#[cfg(not(integration))]` path that is
  the only one exercised by default, a drill written to the stub's shape. Model tier per file by inferred
  risk×complexity (§2).
- **The scan is also a planner.** Its agents do not fix; they emit two artifacts:
  1. `shortcut-inventory.md` — every candidate, ranked by **blast radius** in the doctrine's order
     (*security > durability > money > privacy > features*), with `file:symbol`, the contract it should meet, the
     evidence it looks like a shortcut, and a fix sketch.
  2. **A chunked remediation prompt set** — the next ledger, each prompt sized to the ~400k–700k execution window
     (§3), dependency-ordered, reworking the M7 prompts where a finding changes their scope.
- **Output:** the inventory + the remediation ledger.

### Stage D — Design soundness **and shape-coherence** review of the load-bearing subsystems  *(real ≠ well-conceived; conformant ≠ coherent)*
Two questions, one independent review per security-critical / phased subsystem, by a reviewer empowered to say
"this whole approach is wrong" and to *change what gets built* — not a final rubber stamp:
- **Is it well-conceived?** Threat-model + architecture review of identity/authz (is tenant derived from a
  *verified* claim on every surface?), sandbox, KMS/crypto, tenancy/residency, GDPR erasure. Pairs with M7
  P-542/P-543 but is positioned early enough to inform remediation, not sign it off.
- **Is the frozen shape still right?** For every floor→follow-on seam — single-cell→multi-cell (`CrossCellPointer`
  over a single-cell core), fs-backed→object-store `BlobStore`, structural→real crypto, **mock→real agent
  runtime** — ask whether the evolved capability should be *native design* or is an adaptation layer bolted onto
  an earlier-era shape, and whether the retrofit left a coherence seam to smooth. This is the agile correction to
  the waterfall bound: re-open the contract question *once, after* the code exists to inform it — the
  design-level feedback the conformance loop never ran. A finding here becomes a reshape prompt in the
  remediation ledger, not just a note.
- **The mock-agent-runtime boundary is its own line item.** The entire agent fabric (M2..M6 — dispatch, HITL,
  plan-then-apply determinism, the surge drills, the E2E-2 "agent-native flagship") was validated against the
  scripted-deterministic `--use-mock` runtime; the real `LlmAgentRuntime` was deferred post-M5 (P-481 is a seam
  doc only). So every agent-governance property is proven against a *cooperative strawman*. The review must ask
  which of those properties survive a real, nondeterministic, occasionally-adversarial runtime, and treat any
  that do not as **unproven**. This is not on the M7 floor list and is arguably as load-bearing as the crypto
  floors — it is the corner the plan painted hardest, and the one the audit does not see.

### Stage E — Evidence integrity + the fail-closed gate  *(make the proofs un-gameable)*
- Production-graph absence scanners with **red fixtures** (a scanner with no red fixture is too easy to weaken),
  signed/attested scorecards (the current ones are generated but hand-editable), and the P-546-style fail-closed
  release gate that reads everything above and is red by default. This is largely M7 P-540/P-541/P-546,
  generalized to read the whole campaign's artifacts.

---

## 2. Model-tiering triage (for Stage C, and any fan-out)

A cheap first pass classifies every file by **inferred risk × complexity**, then assigns the model that reads it.
This keeps the expensive review expensive only where it must be.

- **Opus** — security-critical or high-complexity: `myelin-identity-service` (auth/token/crypto, the `Structural*`
  seams), `myelin-ci-sandbox` (escape boundary, the `launch()` stubs), `myelin-storage` (durability, KMS, RLS
  `set_config(..., false)`), `myelin-control-plane`/tenancy (multi-cell isolation), `myelin-gdpr-service`
  (erasure correctness), `myelin-agent-service` (agent authority / HITL / `EffectApi`).
- **Sonnet** — moderate risk/logic: `myelin-events`/substrate (bus, outbox, lints), `myelin-flow` (durable
  workflow), `myelin-git`/`myelin-knowledge` (CRDT, packs), `myelin-refs`/`myelin-search` (zero-leak projections).
- **Haiku** — low-risk / mechanical: glue crates, config, harness scaffolding, `myelin-issues`/`myelin-chat`
  feature surfaces with simple bodies, generated code, the mechanical marker census in Stage A.
- **Assignment rule:** when unsure, tier *up* one level for anything on a security/durability/privacy path; tier
  *down* for pure data-shuffling. The triage pass itself is Haiku-cheap (it only classifies, it does not review).

This map is a starting point; the Stage A census refines it with what each file actually contains.

---

## 3. Prompt-sizing discipline (binding for every prompt this campaign emits)

- A prompt is chunked so its **execution** — canon reads + code reads + implementation + verification — lands at
  roughly **400k–700k tokens of working context. Never above 700k. Below 400k is fine.**
- A prompt that would run past 700k is split into dependency-ordered follow-ons; a really large task becomes
  several prompts, each a coherent slice with its own gate.
- The number is a structuring tool, not a budget to spend up to. A prompt that fills the window forces a
  shortcut. Leave room for the agent to do the real thing **and** verify it adversarially.
- The Stage C scan emits prompts already honoring this, and **reworks the existing M7 prompts** (P-522..P-546) to
  honor it where a finding changes their scope — splitting any that now exceed 700k.
- Rough estimator: sum the canon sections it must read + the LOC it must touch + the tests/drills it must write +
  the gate runs. If that obviously blows past ~700k, split before authoring.

---

## 4. The scan-as-planner output contract (what Stage C must hand back)

Each Stage C agent returns, for its crate/boundary:
- files inspected; for each candidate shortcut: `file:symbol`, the contract/prompt it should satisfy, the
  concrete evidence it may be a shortcut, blast-radius severity, and a fix sketch;
- whether each candidate's existing test would pass on the *stub* (the coupling tell);
- proposed remediation prompt(s), each sized to §3, with dependencies and the gate that would prove the fix and
  **fail on the old floor**.
The orchestrator deduplicates and merges these into `shortcut-inventory.md` (ranked) + the remediation ledger.

---

## 5. What runs first when the budget resets

1. **Stage A** — the mechanical marker census (cheap) → `known-gaps.md`; fix the cheap known gaps in small prompts.
2. **Stage A→triage** — the Haiku classification pass that produces the §2 per-file model map.
3. **Stage B** — author the external-oracle use-case tests (cost/benefit-bounded), capture realness divergences.
4. **Stage C** — launch the model-tiered scan-as-planner over what remains; produce the inventory + remediation
   ledger (reworking M7 prompts to the §3 sizing).
5. **Stages D/E** — schedule the independent design review and stand up the evidence-integrity scanners + the
   fail-closed gate skeleton early (red by default), so every remediation prompt emits evidence in one shape.

Then execute the remediation ledger one prompt at a time, gate between each, exactly as P-001..P-521 — but now
with the builder/verifier coupling deliberately broken at every step.

---

## 6. The done-bar for the campaign

The platform is "truly confident" when:
- every load-bearing claim has a falsifier mapped to an **independent** gate (real backend / external oracle /
  adversarial drill / different-agent review), or a recorded human blocker where automation cannot prove it;
- no structural/mock impl remains in any production dependency path (scanner-proven, with red fixtures);
- the security-critical designs have passed an independent review empowered to reject the approach, **and every
  floor→follow-on seam — above all the mock→real agent runtime — has been re-examined for whether it should be
  native design rather than a retrofit, with the agent-governance properties re-proven against a real runtime;**
- the evidence itself is attested and un-gameable; and
- the fail-closed release gate reads it all and is green only on fresh, dated, attested artifacts.

Anything short of that is hope wearing a green checkmark — the precise thing this campaign exists to retire.
