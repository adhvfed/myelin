# Phase 5 — Refined Shared Systems Architecture (index & executive summary)

> Phase: `05-refined-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md)
> (single source of truth, never contradicted). Binding doctrine:
> [`external-insights/`](../../external-insights/) — esp.
> [`01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md),
> [`02-platform-substrate.md`](../../external-insights/02-platform-substrate.md),
> [`04-hard-problems.md`](../../external-insights/04-hard-problems.md). Spine:
> [`02-holistic-architecture/architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
> (ADR-01..ADR-20) + [`02b-doctrine-integration/integration-directives.md`](../02b-doctrine-integration/integration-directives.md).
> Date: 2026-06-19.

---

## 0. What Phase 5 is (the frame)

VISION §5 makes Phase 5 the **keystone of the planning process**: a reconciliation agent "reviews all
architectures as a whole, refines the shared systems, and **rewrites all of the `04` documents from scratch**
with the necessary adjustments. Also specifies a **testing strategy** for the system as a whole and in parts."

Phase 5 has four movements, each with its deliverables in this folder (and, for the subsystem rewrites, in
`../04-subsystem-architectures`):

1. **Reconcile the whole.** Fold the Phase-4 cross-subsystem change requests (CRs §1–11, CONFLICTS X-1..X-7,
   OPEN QUESTIONS OQ-A..OQ-L) into one set of ratified decisions. → [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md).
2. **Refine the shared layer.** Freeze the build-to contract surface and rewrite the 11 shared-system docs so
   every previously-open encoding is now concrete. → [`contract-index.md`](./contract-index.md) + the 11 docs.
3. **Rewrite the subsystems in place.** Re-author all five Phase-4 subsystem architectures against the
   reconciled layer, each with a `06-reconciliation-compliance.md` proving it implements the frozen contracts
   without drift. → [`../04-subsystem-architectures/<slug>/architecture/`](../04-subsystem-architectures).
4. **Specify the testing strategy.** System-wide and in-parts, with every gate resolved to a quantified
   threshold and a failure-injection drill. → [`testing-strategy/`](./testing-strategy/).

The discipline is the doctrine's: **no ADR is reversed** (none was requested); every decision is tagged
CONFIRM (the seam was right), SHARPEN (the contract stood, its encoding is now frozen), or NEW (a genuinely
new additive contract); every gate resolves to a number; a property is not real until a drill forces its
failure and observability watches it survive; `[OPEN — LEGAL]` items get a defensible engineering posture +
a flag for counsel/DPO, and the structural floor ships regardless.

---

## 1. The index

### 1.1 The two keystone documents (read these first)

| Document | What it is |
|---|---|
| [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) | The rationale: resolves the seven CONFLICTS (X-1..X-7) and twelve OPEN QUESTIONS (OQ-A..OQ-L), folds the eleven grouped change-request sections into the refined contracts, and ends with the per-system punch list + the honesty register (`[OPEN — LEGAL]` items, named floors, measured-not-predicted thresholds). |
| [`contract-index.md`](./contract-index.md) | The frozen build-to surface. The single consolidated map of **every cross-system contract** Phase 6/7/8 must implement or call — owner, consumers, definition site, and status vs Phase 3 (CONFIRMED / SHARPENED / NEW). **Supersedes** the Phase-3 contract index. |

### 1.2 The 11 refined shared-system docs

The shared layer, rewritten against the reconciled contracts:

| Doc | Subsystem |
|---|---|
| [`00-platform-substrate.md`](./00-platform-substrate.md) | Service shell, three-surface topology, lints, resilient-client, fail-static, shed order |
| [`event-bus.md`](./event-bus.md) | Envelope, outbox, consumer template, signals/automations/triggers, the firehose resume-cursor protocol |
| [`identity-and-access.md`](./identity-and-access.md) | `authenticate`/`check`/`list_objects` `SetExpr` push-down/`CaveatContext`, ReBAC fragments, pseudonym/run-token |
| [`reference-graph.md`](./reference-graph.md) | `ArtifactRef`, `resolve`/`project`, the unified `#sub` grammar + tombstone ladder, the Git↔CI `CheckStatus` seam |
| [`search-and-indexing.md`](./search-and-indexing.md) | `query`/`semantic`/`declare_indexable`, the ACL-filter conjoin, measured projection-feeder promotion |
| [`notifications.md`](./notifications.md) | The ONE inbox, `humanise` as the sole templating surface, escalation chain, delivery adapters |
| [`agent-fabric.md`](./agent-fabric.md) | `ToolSurface`/`EffectApi`/`ToolHands::exec`, the four uniform sandbox guarantees, explicit-first dispatch |
| [`durable-workflow.md`](./durable-workflow.md) | `DurableExecutor`, `SCHEDULE_AND_RUN_JOB`, per-effect `idem_key`, the timer wheel, multi-day HITL signal |
| [`gdpr-and-audit.md`](./gdpr-and-audit.md) | `PersonalDataHolder`, DSR fan-out, the ONE free-text/immutable erasure posture, audit hash-chain |
| [`storage.md`](./storage.md) | OLTP/`BlobStore`/KMS, per-subject DEK crypto-shred, reserve/settle, backup/restore-verify, the CI log tier |
| [`tenancy-and-control-plane.md`](./tenancy-and-control-plane.md) | `(tenant, region)` partition key, placement/discovery, residency attestation, the cross-cell PII-free pointer bridge |

The shared crates `myelin-content` and `myelin-query` get their refined (frozen, byte-identical) shapes in
contract-index §13.

### 1.3 The rewritten subsystem architectures (in `../04-subsystem-architectures`)

Per VISION §5, the five subsystem architectures were **rewritten in place** against the reconciled layer
(not re-homed under `05`). Each subsystem folder's `architecture/00-overview.md` opens with a "Changes vs the
Phase-4 first pass (reconciliation deltas absorbed)" section, and each carries a new
`06-reconciliation-compliance.md` that maps every frozen contract it touches to its implementation site.

| Subsystem | Architecture folder |
|---|---|
| Git hosting & code review | [`../04-subsystem-architectures/git-hosting/architecture/`](../04-subsystem-architectures/git-hosting/architecture/) |
| Continuous integration | [`../04-subsystem-architectures/continuous-integration/architecture/`](../04-subsystem-architectures/continuous-integration/architecture/) |
| Issue tracker | [`../04-subsystem-architectures/issue-tracker/architecture/`](../04-subsystem-architectures/issue-tracker/architecture/) |
| Knowledge platform | [`../04-subsystem-architectures/knowledge-platform/architecture/`](../04-subsystem-architectures/knowledge-platform/architecture/) |
| Chat | [`../04-subsystem-architectures/chat/architecture/`](../04-subsystem-architectures/chat/architecture/) |

Conformance of these rewrites to the frozen contracts is verified in [`consistency-pass.md`](./consistency-pass.md).

### 1.4 The testing strategy

[`testing-strategy/`](./testing-strategy/) — the system-wide-and-in-parts strategy (VISION §5):

| Doc | Scope |
|---|---|
| [`00-philosophy-levels-and-gates.md`](./testing-strategy/00-philosophy-levels-and-gates.md) | The keystone: the doctrine made concrete, the test pyramid for a Rust-on-Postgres workspace (incl. where mutation testing is mandatory), and the committed-gate/ratchet model |
| [`01-whole-system-e2e-and-drill-catalogue.md`](./testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) | Cross-subsystem chained-mutation E2E + the consolidated quantified failure-injection drill catalogue |
| [`02-parts-contracts-and-mock-agents.md`](./testing-strategy/02-parts-contracts-and-mock-agents.md) | Per-component suites, the consumer-driven contract/seam tests (one suite per glue contract), mock-agent determinism |
| [`03-gdpr-security-residency-and-ux-qa.md`](./testing-strategy/03-gdpr-security-residency-and-ux-qa.md) | GDPR/erasure, security (sandbox-escape, authz-leak, poisoned-pipeline), residency, and UX/design-QA gates |

---

## 2. Headline reconciliation outcomes

The decisions that most shape Phase 6 onward (full rationale in [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md);
the frozen shapes in [`contract-index.md`](./contract-index.md)):

- **The Git↔CI check seam is frozen (X-1, NEW contract 5.9).** One CI-owned `CheckStatus` fact keyed
  `(commit_oid, context)`, last-writer-wins by **monotonic `run_attempt`** (not wall-clock); emitted as
  `ci.check.updated` via the outbox; mirrored into a Git-owned projection table that drives the merge gate; the
  merge queue is a durable workflow waking on the rollup `ci.result` signal. An `untrusted_fork` success is
  **neutral for gating** until endorsed — the poisoned-pipeline-execution defence is structural.
- **`list_objects` push-down is frozen (OQ-E, contract 4.3) — the single most load-bearing inter-system
  contract.** A consumer-composable `SetExpr` set algebra lowered to a SQL predicate/JOIN over the consumer's
  own id column against a per-tenant authz reverse index: leak-free, no N+1, no post-filter, across all five
  subsystems' id columns. Field/transition ABAC moves to a `CaveatContext` at `check`-time, off the hot path.
- **The shared content + query crates are frozen byte-identical (X-2/X-3).** The `myelin-content` block/inline
  taxonomy (+ the ADF→content lossy import map) and the `myelin-query` field-type enum / `ViewSpec` / `QueryAst`
  / `order_key` LexoRank encoding are one definition; Chat/Issues consume strict subsets; the same `QueryAst`
  **is** the `EventMatcher` core (one grammar, four compile targets). No per-subsystem CEL/DSL.
- **One `#sub` sub-artifact grammar + one tombstone ladder (X-4, contract 5.7).** Git content-anchored line
  ranges, Knowledge block/heading/row anchors, and Chat message/thread anchors all share one self-describing
  grammar and one 4-step graceful-degradation ladder that never leaks and never dangles.
- **One sandbox, four uniform guarantees (X-6, ADR-20).** `ToolHands::exec` **is** the CI runner's
  `kind=agent` job; the real-kernel escape drill is the single hard go/no-go before any untrusted code runs;
  every tool inherits the cost gate, per-run-token attribution, HITL withhold, and the isolation floor by
  construction. The `requires_approval` defaults table is frozen.
- **`SCHEDULE_AND_RUN_JOB` + per-effect `idem_key` (OQ-F).** A long-parked-activity-completed-by-signal idiom
  (the workflow holds no runtime while a multi-hour job runs) and a per-effect approval-idempotency rule
  (`card_id` single / `card_id:<effect_idx>` multi/partial) — a double-click is one approval; a partial
  approval is well-defined.
- **The firehose resume-cursor protocol (OQ-J).** One `subscribe/resume/scope` protocol — reconnect loses
  zero ops, scope is always bounded (never `*`), `resync_required` falls back to a `*.snapshot` replay — used
  identically by Knowledge collab, Chat live, and CI logs.
- **ONE free-text/immutable-content erasure posture (X-7, NEW contract 10.9, `[OPEN — LEGAL]`).** A single
  platform posture (per-subject DEK crypto-shred + pseudonym-map shred + `restrict` suppression as the
  structural floor; a documented lawful-basis limit for residual third-party/immutable free-text PII),
  instantiated per subsystem **by reference**, not restated five times. The structural floor ships regardless;
  counsel/DPO ratify the residual basis in one statement.
- **Residency & multi-cell are frozen as a designed floor.** Repo-granular relocatable placement, the
  no-global-pool attestation extended to the CI runner/log/cache region, the outbound-mirror residency gate,
  and the cross-cell PII-free `CrossCellPointer` bridge (single-home-cell is complete v1; cross-cell is
  designed-not-built, resolution always cell-local).

**The two highest-fan-in contracts — `EventEnvelope` (2.1) and `list_objects` (4.3) — are now both frozen with
their concrete shapes**, closing the platform's highest drift risk.

---

## 3. Handoff to Phase 6 (roadmaps)

Phase 6 (`planning/06-roadmaps/<system>/`) produces one implementation roadmap per shared system and subsystem.
What Phase 5 hands it:

- **A frozen build-to surface.** Every roadmap implements or calls the contracts in
  [`contract-index.md`](./contract-index.md); changing one is a single whole-workspace PR (ADR-01), never a
  silent drift. The per-system punch list (recon §4) is the "what changed for me" each roadmap starts from.
- **The build-order law.** The testing strategy's order-by-non-negotiability binds the roadmaps: the
  failure-injection harness and the dependency-break/load-multiplier primitives are sequenced **early** (R-3);
  the silent-data-loss and sandbox-escape gates outrank every feature; **no later phase is done over a red
  earlier gate** (R-1/R-2).
- **A residual register, not a gap register.** No subsystem carries a contract gap. What it carries is named
  floors with named follow-ons (CAS→CRDT; node-backed→object-backed git; read-time→materialised-when-measured
  rollups; KB-comments→Chat-threading consolidation; single-cell→multi-cell), measured-not-predicted
  thresholds (projection-feeder promotion, `order_key` rebalance, column-store promotion), and the
  `[OPEN — LEGAL]` items below.
- **The `[OPEN — LEGAL]` track.** Carried to counsel/DPO, structural floor shipping regardless: the ONE
  free-text/immutable erasure posture (L-2); worklog/productivity special-category classification (OQ-H);
  build-data-as-LLM-training basis (OQ-H); Art. 17 reach into immutable git bytes; the audit-log retention
  carve-out (GD-5); the fail-static staleness-bound ratification (L-1).
- **The testing strategy as a live tool.** Phase 8 runs each prompt sequentially so agents can use the drill
  harness, the consumer-driven contract suites, and the committed gates as they build — and adapt when the plan
  meets reality (VISION §5.8).

---

## 4. Cross-references

- [`consistency-pass.md`](./consistency-pass.md) — the final verification that the rewritten subsystems
  implement the frozen contracts without drift.
- [`../03-shared-systems-architecture/contract-index.md`](../03-shared-systems-architecture/contract-index.md)
  — the superseded Phase-3 surface.
- [`../04-subsystem-architectures/cross-subsystem-change-requests.md`](../04-subsystem-architectures/cross-subsystem-change-requests.md)
  — the primary Phase-4 input the reconciliation consumed.
- [`../02-holistic-architecture/architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
  (ADR-01..ADR-20) + [`../02-holistic-architecture/design-language.md`](../02-holistic-architecture/design-language.md)
  (§8b design-QA gates) + [`../02b-doctrine-integration/integration-directives.md`](../02b-doctrine-integration/integration-directives.md).
