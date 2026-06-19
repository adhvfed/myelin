# Phase 7 — Prompt Ledger: Platform Substrate & Foundations (`myelin-substrate` + the glue crates)

> **Granularity note (Phase 7-A finer-granularity pass):** prompt count **17 → 38**. The first pass was high
> quality but bundled several independently-committable deliverables into one prompt (the failure-injection
> harness shipped three machines in one; the outbox shipped envelope + emit + relay in one; `serve` shipped
> the lifecycle + three ports + migrations + holder-registration in one; the twelve lints shipped as one;
> fail-static shipped the cache + shed-lane + bounded-everything + agent-caps in one). This pass splits each
> bundle into single-deliverable clean-context prompts — every milestone, contract, drill, and floor the first
> pass covered remains, now at finer granularity, with DEPENDS-ON re-threaded across the new local ids.
>
> Phase: `07-prompts/by-system`. The complete ordered set of implementation prompts that operationalize the
> **00-platform-substrate** shared system's entire Phase-6 roadmap into clean-context, independently-committable
> coding tasks. Each prompt follows the template in
> [`../00-ledger-overview.md`](../00-ledger-overview.md) §2 EXACTLY. Authored against the FROZEN sequence:
> master bands [`../../06-roadmaps/00-master-sequencing.md`](../../06-roadmaps/00-master-sequencing.md) (M0..M6 +
> the gate invariant), the substrate roadmap
> [`../../06-roadmaps/shared/00-platform-substrate.md`](../../06-roadmaps/shared/00-platform-substrate.md), the
> refined architecture [`../../05-refined-shared-systems-architecture/00-platform-substrate.md`](../../05-refined-shared-systems-architecture/00-platform-substrate.md)
> and the contracts [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
> §1 (owned) + §2/§3.5 (carried). Drills:
> [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> §4.2 (SUB-D1..SUB-D10). Doctrine: VISION §3/§7, EI-01 (§1 code-wins-over-docs + name-your-floors, §2
> order-by-non-negotiability, §3 prove-it-or-it-isn't-real, §5 the ratchet), EI-04 (§2 resume-cursor-first, §5
> untrusted-execution). Plain text identifiers throughout (no backticks-as-emphasis where a name stands alone).
> The global `P-<NNN>` ids are assigned by the Phase-7-B index; this file uses provisional ordinals (P-S01..)
> in the substrate's own band order — the index interleaves these with every other system's prompts.
> Date: 2026-06-19.
>
> **The shape of this system.** The substrate is the ROOT of the dependency DAG — it has no upstream (its only
> "dependency" is the failure-injection harness, which it also builds). Its core lands almost entirely in M0
> (the workspace, the eight glue crates, the harness, the outbox, the twelve lints, `serve(AppSpec)`, the
> resilience primitives, the scanner + thresholds). Later prompts complete-and-prove properties whose proof
> needs a system that does not exist in M0: fail-static (M1, against a real Identity hiccup), restore-verify
> cross-seam integrity (M1, with Storage), the firehose backpressure half (M2/M4), the surge family +
> migration-under-load (M5), and the dogfood loop (M6). The substrate's correctness floors (the outbox, the
> twelve lints, the harness) are NOT staged — they are absolute from M0; its named floors are scale/value/legal
> deferrals only (which blob backend, which budget number, which staleness value).

---

## Coverage map (this file → the substrate roadmap milestones)

| Roadmap milestone | Prompt ids (provisional) |
|---|---|
| SUB-M0 (= master M0; the core) | P-S01 (workspace + glue crates) · P-S02 (load generator) · P-S03 (dependency-break injector) · P-S04 (telemetry-assertion library + incident loop + harness self-test) · P-S05 (canonical envelope) · P-S06 (`OutboxTx::emit` causality) · P-S07 (outbox table + relay; SUB-D1/BUS-D4) · P-S08 (consumer template + dedup; SUB-D2) · P-S09 (upcaster registry) · P-S10 (four load-bearing lints) · P-S11 (eight remaining lints) · P-S12 (`serve` lifecycle + graceful drain) · P-S13 (three-surface topology + tenant-from-token; SUB-D7) · P-S14 (liveness ≠ readiness; SUB-D9) · P-S15 (holder auto-registration + migration runner) · P-S16 (resilient-client four primitives) · P-S17 (`Retry-After` honouring; SUB-D5) · P-S18 (fail-static mechanism) · P-S19 (shed lane + bounded-everything) · P-S20 (agent-load caps; SUB-D8) · P-S21 (contract-coverage scanner) · P-S22 (versioned thresholds file) · P-S23 (overlay/state primitives) · P-S24 (M0 exit-gate scorecard) |
| SUB-M1 (master M1) | P-S25 (fail-static proven vs Identity; SUB-D4) · P-S26 (restore-verify cross-seam half; SUB-D6) · P-S27 (exhaustive-holder confirmation) |
| SUB-M2 (master M2) | P-S28 (per-connection frame caps + slow-consumer drop) · P-S29 (scope-bounded selector + frame shed budgets; D-11 half) |
| SUB-M4 (master M4) | P-S30 (cross-language shim enforced) · P-S31 (firehose under connection-storm) |
| SUB-M5 (master M5) | P-S32 (SUB-D3 surge family) · P-S33 (tune per-surface shed budgets) · P-S34 (SUB-D10 migration-under-load) · P-S35 (restore-verify at cell scale) · P-S36 (per-target client tuning) |
| SUB-M6 (master M6) | P-S37 (lints + scanner + mutation gate as Myelin CI jobs) · P-S38 (incident-loop + truth-up pass) |

Total: 38 prompts. Bands: M0 ×24, M1 ×3, M2 ×2, M4 ×2, M5 ×5, M6 ×2. Every SUB-M0..SUB-M6 milestone and every
SUB-D1..SUB-D10 drill + the D-11 firehose half is greened by at least one prompt below.

---

### P-S01 — Stand up the Cargo workspace and the eight glue-crate skeletons

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (the workspace + glue-crate skeleton slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 4 + §1.1.
- **DEPENDS-ON.** none (this is a root prompt of M0; the whole DAG roots here).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../VISION.md` §4 (Rust default; the workspace `.gitignore` is pre-seeded for Cargo + cargo-mutants) and §3 (name-your-floors).
  - `../../external-insights/01-process-and-quality-doctrine.md` §5 (the ratchet — a gate must be committed) and §7 (keep the architecture coherent; abstract at the third copy).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §2 (the shared crates — responsibility + trait/type surface, the whole section), §2.9 (the dependency root, no cycles — the crate DAG root-last), §2.6 (`myelin-substrate` harness crate).
  - `../../05-refined-shared-systems-architecture/contract-index.md` §0 (the clusters) + §1 header (the substrate cluster) + the "Units (frozen)" block in the doc header.
  - `../00-ledger-overview.md` §6 (the repo/workspace conventions every prompt assumes — the eight glue crates as compile-time contract carriers).
- **DELIVERABLE (what to build + exactly where in the repo).** A single Cargo workspace at the repo root (`Cargo.toml` `[workspace]`). Create the eight glue crates as library crates with their public trait/type surface stubbed to the FROZEN signatures (compiling, `todo!()`-bodied where an impl lands in a later prompt): `myelin-tenancy`, `myelin-identity`, `myelin-events`, `myelin-refs`, `myelin-content`, `myelin-query`, `myelin-agent`, `myelin-gdpr`; plus the substrate crates `myelin-client` (the `ResilientClient` home) and `myelin-substrate` (the harness home). Wire the inter-crate dependency edges in the EXACT root-last order of architecture §2.9 (`myelin-tenancy` → `myelin-identity` → `myelin-events`/`-refs`/`-content`/`-query` → `myelin-agent`/`-gdpr` → `myelin-client` → `myelin-substrate`) — a dependency that would create a cycle must not compile. Each crate gets a top-of-file doc comment naming its owning architecture doc + contract-index cluster. Place the type/trait stubs only for cross-crate surfaces (architecture §2.1–§2.7): `EventEnvelope`, `OutboxTx`, `ArtifactRef`, `EventHandler`/`HandleOutcome` (in `myelin-events`); `Principal`/`PrincipalKind`/`AuthzClient`/`Consistency` (in `myelin-identity`); `Refs` (in `myelin-refs`); `AgentRuntime`/`ToolHands`/`ToolSurface` (in `myelin-agent`); `PersonalDataHolder`/`BlobStore` (in `myelin-gdpr`); `TenantId`/`Region` (in `myelin-tenancy`); `ResilientClient` (in `myelin-client`); `serve`/`AppSpec`/`FailStatic` (in `myelin-substrate`). **Floor named:** the bodies are stubs; each later prompt names which it fills. This prompt ships the skeleton only.
- **CONTRACTS TO IMPLEMENT.** The TYPE/TRAIT SHAPES (not bodies) of contract-index rows 1.1 (`serve` signature stub), 1.9 (`ResilientClient::call` signature), 1.10 (`FailStatic<T>` struct), 2.1 (`EventEnvelope`), 2.2 (`OutboxTx::emit`), 2.4 (`EventHandler`/`HandleOutcome`), 4.2/4.3/4.5 (`AuthzClient`), 5.1 (`ArtifactRef`), 8.1/8.3/8.4 (`AgentRuntime`/`ToolHands`/`ToolSurface`), 10.1/11.2 (`PersonalDataHolder`/`BlobStore`) — each to the frozen shape in architecture §2. Do not redesign a signature; if reality forces a divergence, write it down in the crate doc comment and escalate (EI-01 §1).
- **GATE / DRILLS (quantified; must be green to call this done).** `cargo build --workspace` and `cargo test --workspace` succeed (green artifact = a clean build log). The dependency DAG is acyclic by construction — assert it with a test that the crate graph has no cycle and `myelin-identity` depends on nothing above `myelin-tenancy` (architecture §2.9 — identity is a sink). No quantified runtime drill at this prompt (it ships skeleton, not behaviour); state this floor explicitly.
- **TESTS (required).** Unit: one compile-asserting test per glue crate that the public surface exists with the frozen field names/units (e.g. `EventEnvelope.occurred_at: Timestamp`, costs as integer minor-units). A `crate-graph-acyclic` test (the build-layer realisation of the `no-cross-sync-cycle` lint, which P-S10 ships as the real lint). No CDC pair yet (the contracts have no behaviour to verify here — named: CDC pairs land with each contract's impl prompt).
- **DEFINITION OF DONE.** The workspace and the ten crates exist and compile; the frozen trait/type shapes are present; the crate-graph-acyclic test passes; the stubbed bodies are named as a floor with their filling prompt; committed.
- **COMMIT.** Header `P-<NNN> M0: Cargo workspace + eight glue-crate skeletons`. Body: lists the ten crates, the acyclic-DAG assertion, and the named stub floor + filling prompts. Branch first if on default. End with the required `Co-Authored-By:` trailer.

---

### P-S02 — The 1×/10×/30× load generator with mixed principal kinds

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 0 — the failure-injection harness, the load-generator slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 1 (the 1×/10×/30× load generator).
- **DEPENDS-ON.** P-S01.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../VISION.md` §3 (prove-it / honesty about uncertainty).
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (prove-it-or-it-isn't-real — the failure-injection harness built EARLY: a load generator that multiplies traffic 1×/10×/30× and mixes principal types).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §7.6 (the per-surface storm profiles — CI-surge, collab op-stream, connection-storm, agent-mention-storm; OQ-K) and §7.2 (the five principal kinds the limiter reads).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.1 (the nine reusable families — the load generator is the spine of the surge family F6).
- **DELIVERABLE (what to build + exactly where in the repo).** A new crate `myelin-harness` (test-support, in the workspace). Build the **1×/10×/30× load generator**: a driver that issues traffic at a configurable multiplier with a configurable mix of the five principal kinds (human / agent / service / CI / external-MCP) and the named per-surface storm profiles (CI-surge, collab op-stream, connection-storm, agent-mention-storm — OQ-K, §7.6). The generator targets an abstract sink (an in-memory request handler in tests; later drills point it at a real `serve` instance). **Floor named:** the storm-profile parameters are v1 defaults; the tuned numbers are the M5 surge/connection-storm follow-on (P-S32/P-S33).
- **CONTRACTS TO IMPLEMENT.** None owned (this is harness machinery). It is the driver under every surge/storm drill in the whole ledger.
- **GATE / DRILLS (quantified; must be green to call this done).** No runtime survival drill yet (the assertion library lands in P-S04); the gate is the generator's own correctness: it hits the configured multiplier within ±tolerance and the configured principal mix. Green artifact = a test run showing the requested 1×/10×/30× rate and the five-kind mix realised.
- **TESTS (required).** Unit: the generator hits each multiplier (1×/10×/30×) within ±tolerance; the principal mix matches the requested ratios across the five kinds; each named storm profile selects the right surface shape. No CDC pair (not a cross-system contract).
- **DEFINITION OF DONE.** The `myelin-harness` crate exists and compiles; the load generator hits the multipliers + principal mix + storm profiles; the v1 storm-profile floor is named with its M5 follow-on; committed.
- **COMMIT.** Header `P-<NNN> M0: failure-injection load generator (1x/10x/30x, mixed principals)`. Body: the multiplier + five-kind mix + four storm profiles; the v1 storm-profile floor → M5. Co-Authored-By trailer.

---

### P-S03 — The scoped-reversible dependency-break injector

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 0 — the failure-injection harness, the dependency-break-injector slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 1 (the scoped-reversible dependency-break injector).
- **DEPENDS-ON.** P-S02.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (a scoped reversible way to break a dependency — sever one named dependency for one named scope without taking the rig down).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §11 intro (the failure-injection seam = a scoped reversible dependency break, T-3) + §11 (the drill table — the injector is what makes every D-row drillable).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 (the SUB drills the injector will drive — it severs Identity for SUB-D4, the broker for SUB-D2, a downstream for SUB-D5).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-harness`: the **scoped-reversible dependency-break injector** — `break_dependency(name, scope)` / `restore_dependency(name, scope)` that severs ONE named dependency (Identity, the broker, a downstream) for ONE named scope without taking the rig down, and restores it cleanly. The break is reversible (a broken dep restores to a fully working state), scoped (it does not affect other dependencies or other scopes), and idempotent (a double-break / double-restore is a no-op). This is the T-3 seam every later drill rides to force a failure.
- **CONTRACTS TO IMPLEMENT.** None owned (harness machinery). The injector is the failure-injection half of the unit-of-proof.
- **GATE / DRILLS (quantified; must be green to call this done).** No survival drill yet (the assertion library lands in P-S04). The gate is the injector's reversibility + scoping: a broken dependency restores to green, the break is scoped to its name + scope, a double-break/double-restore is a no-op. Green artifact = a test showing break → severed, restore → fully working again.
- **TESTS (required).** Unit: `break_dependency`/`restore_dependency` are reversible (a broken dep restores cleanly); a break is scoped to its named dependency + scope (an unrelated dependency stays up); double-break and double-restore are no-ops. No CDC pair.
- **DEFINITION OF DONE.** The injector exists and compiles; break/restore are reversible, scoped, and idempotent; committed.
- **COMMIT.** Header `P-<NNN> M0: scoped-reversible dependency-break injector`. Body: `break_dependency`/`restore_dependency`; the reversibility + scoping + idempotence guarantees. Co-Authored-By trailer.

---

### P-S04 — The telemetry-assertion library, the every-incident-adds-a-drill loop, and the harness self-test

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 0 — the failure-injection harness, the telemetry-assertion + incident-loop slice + the harness self-test) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 1 (the telemetry-assertion library + the every-incident-adds-a-drill loop) + the SUB-M0 exit harness self-test.
- **DEPENDS-ON.** P-S02, P-S03.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (assertions read from production telemetry; observability is part of the pass condition; a property is PROVEN only when a drill forces the failure and an assertion over the signals reads green) + §5 (loud, never silently swallowed — replace `... || true`).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §10.2 (the telemetry signal set — RED/USE per principal-kind, consumer-lag, outbox-depth, breaker-state, fail-static ratios, shed-counts, causal-depth, firehose frame-lag) and §11 intro (the green-artifact rule).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.8 (telemetry signal set — every drill asserts against this set; no signal = no provable drill).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-harness`: (a) the **telemetry-assertion library** — a typed reader over the contract-1.8 survival-signal set with `assert_signal(name, predicate)` (e.g. `outbox_depth == 0`, `cross_tenant_count == 0`, `fail_static_ratio`, `consumer_lag`, `breaker_state`, `shed_counts`, `causal_depth`, `firehose_frame_lag`); the library returns a typed green/red, NEVER a swallowed pass (no `... || true`); an inverted assertion is rejected (EI-01 §3). Here the harness reads an in-memory signal source so it is testable before `serve` exists (the producer side is wired into `serve` at P-S12/P-S13). (b) the **every-incident-adds-a-drill loop** (T-3): a `register_drill(scenario)` hook that a reproducing drill joins and re-runs forever. (c) the **harness self-test** (the SUB-M0 exit unit-of-proof): inject one fault via P-S03's `break_dependency`, drive one unit of load via P-S02's generator, read one telemetry assertion that reads green — the unit-of-proof drilling itself.
- **CONTRACTS TO IMPLEMENT.** Contract-index 1.8 (telemetry signal set — the harness is its first consumer; the producer side is wired into `serve` in P-S12/P-S13). The assertion library is the machinery under EVERY later drill in the whole ledger.
- **GATE / DRILLS (quantified; must be green to call this done).** The **harness self-test** (master §2 M0 exit; roadmap SUB-M0 exit): inject one fault, drive one unit of load, read one telemetry assertion that reads green. Quantified: the assertion library returns a typed green/red, never a swallowed pass. Green artifact = the self-test scenario emitting a dated PASS row.
- **TESTS (required).** Unit: `assert_signal` fails loudly on a red signal (a test that an inverted assertion is rejected, EI-01 §3); `register_drill` re-runs a registered scenario. The harness self-test scenario is itself a committed test (inject → load → assert green).
- **DEFINITION OF DONE.** The assertion library + the incident-loop hook + the harness self-test exist and compile; the self-test emits a dated green artifact; the assertion library is loud-never-swallowed; committed.
- **COMMIT.** Header `P-<NNN> M0: telemetry-assertion library + incident-loop + harness self-test`. Body: the signal-set reader, the `register_drill` hook, the harness self-test green artifact. Co-Authored-By trailer.

---

### P-S05 — The canonical `EventEnvelope` (the names/units anchor, X-5)

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 1 — the canonical envelope slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 2 (the `EventEnvelope` frozen as the names/units anchor, X-5).
- **DEPENDS-ON.** P-S01.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../VISION.md` §3 (name-your-floors; quality over plan-adherence).
  - `../../external-insights/01-process-and-quality-doctrine.md` §1 (code-wins-over-docs — if a field must diverge, write it down).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §2.1 (`myelin-events` — the `EventEnvelope` restated to the frozen anchor; the field list) and §2.10 (the canonical envelope field list + units — the X-5 names/units authority).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 2.1 (`EventEnvelope` — the canonical versioned envelope; the names/units anchor) + 2.7 (inline-PII events envelope-encrypted with `pii_key_ref`). Read `00-reconciliation-decisions.md` X-5 for the names/units rationale.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-events`: finalise the `EventEnvelope` struct to the frozen field list (architecture §2.1 + §2.10): `event_id` (ULID), `type_`, `schema_ver`, `tenant`, `region`, `actor`, `subject` (ArtifactRef), `aggregate`, `causation_id` (immediate parent), `correlation_id` (root), `caused_by`, `depth`, `contains_personal_data`, `data_role`, `visibility`, `pii_key_ref`, `occurred_at`, `recorded_at`, `payload`. Pin the frozen units: timestamps RFC-3339 UTC (`occurred_at`/`recorded_at`); costs integer minor-units; TTLs/staleness/timers seconds; `pii_key_ref = kms://<tenant>/<dek-epoch>/<class>`, `<class> ∈ {tenant, subject:<id>, blob}`. References-not-payloads: `payload` carries IDs/ArtifactRefs, never PII bodies. This envelope is frozen here as the anchor every later contract aligns to (X-5). **Floor named:** the envelope-encryption KMS hierarchy (the DEK epochs behind `pii_key_ref`) is Storage's M1 deliverable (11.3) — here only the field + its format ship.
- **CONTRACTS TO IMPLEMENT.** Owned: 2.1 (the envelope). The names/units anchor every later contract reconciles against.
- **GATE / DRILLS (quantified; must be green to call this done).** No runtime drill (this ships a type, not behaviour). The gate is the compile-assertion that the field list + units match the frozen anchor exactly. Green artifact = the field-shape test passing.
- **TESTS (required).** Unit: a compile-asserting test that every field name + type matches the §2.10 anchor (e.g. `occurred_at: Timestamp` RFC-3339, `depth: u32`, costs integer minor-units, `pii_key_ref` format). CDC: the provider-side envelope-shape contract test for 2.1 (the consumer side is the relay + consumers in P-S07/P-S08; the contract-coverage scanner, P-S21, fails the build if the row lacks a pair — mark the consumer half as landing in P-S07/P-S08).
- **DEFINITION OF DONE.** The `EventEnvelope` exists and compiles to the frozen field list + units; the shape test passes; the KMS-hierarchy floor is named with its Storage M1 follow-on; committed.
- **COMMIT.** Header `P-<NNN> M0: canonical EventEnvelope (the names/units anchor, X-5)`. Body: contract 2.1; the frozen field list + units; the KMS-hierarchy floor → Storage M1. Co-Authored-By trailer.

---

### P-S06 — `OutboxTx::emit(draft, cause)`: causality correct-by-construction, no `publish_now`

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 1 — the emit path slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 2 (`OutboxTx::emit` the ONLY sanctioned emit path, causality correct-by-construction).
- **DEPENDS-ON.** P-S05.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §2 (silent data loss outranks every feature — a shortcut that exists will be used and will lose data).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §2.1 (`OutboxTx::emit(draft, cause)` the ONLY emit path; causality correct-by-construction — root carries, parent = cause.event_id, depth = cause.depth + 1; there is NO `publish_now`).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 2.2 (`OutboxTx::emit(draft, cause)` — same tx; causality correct-by-construction; no `publish_now`).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-events`: implement `OutboxTx::emit(draft, cause: Option<&EventEnvelope>) -> Result<EventId>` deriving causality correct-by-construction — a root event carries its own `correlation_id`; a caused event sets `causation_id = cause.event_id`, `correlation_id = cause.correlation_id`, `depth = cause.depth + 1`. The emit inserts into the per-service `outbox` table IN THE SAME TRANSACTION as the state change (the table + relay land in P-S07; here the emit derives the row + provenance). There is intentionally NO `publish_now` / fire-and-forget path in the API surface. So a human or agent cannot typo their way into a loop (EI-02 §6).
- **CONTRACTS TO IMPLEMENT.** Owned: 2.2 (the emit path).
- **GATE / DRILLS (quantified; must be green to call this done).** No standalone survival drill (SUB-D1/BUS-D4 land in P-S07 once the relay exists). The gate is the causality-derivation correctness + the absence of any fire-and-forget symbol. Green artifact = the causality unit tests + the compile-fixture that the API has no `publish_now`.
- **TESTS (required).** Unit: `emit` derives causality correctly — a root carries its own correlation; a caused event sets `causation_id = parent.event_id`, `correlation_id = root`, `depth = parent.depth + 1`. A compile-fixture test that there is NO `publish_now` symbol (the API has no fire-and-forget path — the `no-raw-publish` lint, P-S10, enforces this externally). CDC: provider side of 2.2 (the consumer side rides the relay in P-S07).
- **DEFINITION OF DONE.** `OutboxTx::emit` exists and compiles to the frozen shape; causality is correct-by-construction; there is no `publish_now`; the unit + compile-fixture tests pass; committed.
- **COMMIT.** Header `P-<NNN> M0: OutboxTx::emit (causality correct-by-construction, no publish_now)`. Body: contract 2.2; the root/parent/depth derivation; the no-fire-and-forget compile fixture. Co-Authored-By trailer.

---

### P-S07 — The `outbox` table and the relay (SUB-D1, BUS-D4 — the silent-data-loss floor)

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 1 — the outbox table + relay slice; the silent-data-loss floor) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 2 (the `outbox` table + the relay).
- **DEPENDS-ON.** P-S06, P-S03, P-S04 (the relay drills ride the injector + assertion library).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §2 (silent data loss outranks every feature — stop-the-bleeding first) and §3 (prove-it: zero messages lost across a reconnect).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §3.3 (the outbox relay — claims unsent rows with `FOR UPDATE SKIP LOCKED` safe across replicas, stamps the stable ULID for broker-side dedup, publishes, marks sent, dead-letters after bounded retries; the relay is the ONLY component on the broker publish side) and §2.10 (the units the row carries).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 2.3 (`outbox` table — `(event_id UNIQUE, aggregate, seq, subject, envelope)`, `UNIQUE(aggregate, seq)` per-aggregate ordering, relay `FOR UPDATE SKIP LOCKED`).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 rows SUB-D1, BUS-D4.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-events`: (a) the `outbox` table migration — `(event_id UNIQUE, aggregate, seq, subject, envelope)` with `UNIQUE(aggregate, seq)` for per-aggregate ordering; (b) the **relay** — claims unsent rows with `FOR UPDATE SKIP LOCKED` (safe across replicas), stamps the stable ULID for broker-side dedup, publishes to the broker abstraction, marks sent, dead-letters after bounded retries. The broker is a trait (an in-process fake for tests; the real adapter is the Bus's M0 deliverable — name this boundary as the seam). Wire `OutboxTx::emit` (P-S06) to insert into this table in the same transaction. Export `outbox_depth` + dead-letter count into the telemetry signal set. **Floor named:** single-region event log on a general-purpose DB (roadmap §3 floor table) — the column-store seam is the post-M5 follow-on, added only when volume is measured.
- **CONTRACTS TO IMPLEMENT.** Owned: 2.3 (the outbox table + relay). Consumes 2.2 (the emit path) + 2.1 (the envelope).
- **GATE / DRILLS (quantified; must be green to call this done).** **SUB-D1** (kill service between commit and publish → exactly-once-in-effect, **0 ghost, 0 lost**; outbox-depth drains) — telemetry signals `outbox_depth → 0`, `ghost_count == 0`, `lost_count == 0`, CI. **BUS-D4** (crash producer between state-commit and publish → event delivered, never without state; outbox **emit-iff-committed**) — signal `emit_iff_committed == true`, CI. (SUB-D2 needs the consumer template P-S08; greened there.) Never weaken a threshold to pass; a red gate becomes a dated "claimed, not proven" thresholds-file row. **This is a PERMANENT gate (re-run on every emit-path change).**
- **TESTS (required).** Unit: the relay claims with `FOR UPDATE SKIP LOCKED` and is idempotent under a re-claim; an uncommitted state change produces no published event (emit-iff-committed); the stable ULID is stamped for dedup. CDC pair for 2.3 (provider = the relay; consumer = the broker fake). Drill scenarios for SUB-D1 + BUS-D4 (using the P-S03 injector to kill between commit and publish; the P-S04 assertion library reads the signals). Mutation floor ≥ 80% on the outbox/relay module (mandatory core — the emit path).
- **DEFINITION OF DONE.** The `outbox` table + the relay exist and compile to the frozen shape; SUB-D1 and BUS-D4 emit dated green artifacts; the CDC pair + unit tests + the mutation floor pass; the single-region floor + the permanent-gate marking are named; committed.
- **COMMIT.** Header `P-<NNN> M0: outbox table + relay (SUB-D1 / BUS-D4, the silent-data-loss floor)`. Body: contract 2.3; SUB-D1 (0 ghost/0 lost) + BUS-D4 (emit-iff-committed) with measured numbers; the permanent-gate marking; the single-region floor + post-M5 column-store follow-on. Co-Authored-By trailer.

---

### P-S08 — The idempotent event-consumer template and the dedup ledger (SUB-D2)

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 1 — the idempotent-consumer template slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 2 (the `EventHandler` template + the `consumer_dedup` ledger).
- **DEPENDS-ON.** P-S07.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §2 (silent data loss first) and §4 (chain mutations end-to-end; real sessions chain, that's where bugs live).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §5 (the event-consumer template + the seven encoded rules: idempotent on `event_id` via `consumer_dedup`; ack-after-enqueue; whitelist subjects never `*`; bind-durable-by-name; terminate poison; bounded prefetch; lag as a first-class metric) and §5.3 (causality through the consumer — `emit(draft, cause = Some(incoming))`).
  - `../../05-refined-shared-systems-architecture/contract-index.md` rows 2.4 (`EventHandler` template + `HandleOutcome`), 2.5 (`consumer_dedup` ledger `(consumer, event_id)` PK).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 row SUB-D2.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-events`: the `EventHandler` trait (`subjects() -> &'static [SubjectPattern]` whitelist — never `*`; `handle(ev) -> HandleOutcome`) + `HandleOutcome { Done | NonRetryable(Reason) | Retry(Backoff) }`; the consumer runtime that encodes the seven rules (idempotent on `event_id` via the `consumer_dedup` ledger `(consumer, event_id)` PK; ack-after-enqueue; reject a `*` subscription; bind-by-name; dead-letter poison; bounded prefetch; export consumer-lag as a metric); the `consumer_dedup` table migration. Wire the lag + dead-letter counts into the telemetry signal set (contract 1.8).
- **CONTRACTS TO IMPLEMENT.** Owned: 2.4, 2.5.
- **GATE / DRILLS (quantified; must be green to call this done).** **SUB-D2** (drop broker mid-stream → **0 lost** across reconnect by bind-by-name + dedup; a slow subject does NOT head-of-line-block others) — signals `consumer_lag` recovers, `lost_count == 0`, no HoL stall, CI. Re-confirm SUB-D1 end-to-end through a consumer (the dedup ledger absorbs the redelivery → 0 dup). **This is a PERMANENT gate (re-run on every emit-path change).**
- **TESTS (required).** Unit: a `*` subscription is rejected at registration; a redelivered `event_id` is a no-op (the ledger absorbs it); a poison message dead-letters and does not burn the redelivery budget; a slow subject does not block a fast one (bounded prefetch). CDC pair for 2.4/2.5. Drill scenario for SUB-D2 (the P-S03 injector drops the broker mid-stream). Tests CHAIN: emit → drop broker → reconnect → re-consume, asserting 0 lost + 0 dup over the sequence (EI-01 §4, the sequence property). Mutation floor ≥ 80% on the consumer-runtime module (mandatory core).
- **DEFINITION OF DONE.** The template + the dedup ledger exist and compile; SUB-D2 emits a dated green artifact and SUB-D1 re-confirms 0 dup through a consumer; CDC + chained tests + mutation floor pass; the permanent-gate marking is named; committed.
- **COMMIT.** Header `P-<NNN> M0: idempotent event-consumer template + dedup ledger (SUB-D2)`. Body: 2.4/2.5; SUB-D2 (0 lost / no HoL) with measured numbers; the permanent-gate marking. Co-Authored-By trailer.

---

### P-S09 — The schema-evolution upcaster registry (forward-only)

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 1 — the schema-evolution upcaster slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 2 (the `(type, from_ver) → to_ver` upcasters, forward-only).
- **DEPENDS-ON.** P-S08.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §2 (no data loss — a versioned event must always be readable forward).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §2.1 (`schema_ver` gates evolution; upcasters bridge versions at consume, forward-only).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 2.8 (schema-evolution upcasters — `(type, from_ver) → to_ver` pure fns at consume; forward-only).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-events`: the `(type, from_ver) → to_ver` **upcaster registry** — pure functions applied at consume that bridge an older `schema_ver` forward to the current one. Forward-only (no down-cast). The consumer runtime (P-S08) calls the registry before `handle` so a handler always sees the current shape. An unbridgeable version gap is a loud `NonRetryable`, never a silent drop.
- **CONTRACTS TO IMPLEMENT.** Owned: 2.8.
- **GATE / DRILLS (quantified; must be green to call this done).** No survival drill (this is a pure-function registry). The gate is the upcaster correctness: an event at `from_ver` is bridged to `to_ver` before `handle`; an unbridgeable gap is a loud `NonRetryable`. Green artifact = the upcaster unit tests passing.
- **TESTS (required).** Unit: an event at an older `schema_ver` is upcast to current before `handle` sees it; the upcasters are pure (no side effects); a missing upcaster for a version gap is a loud `NonRetryable`, not a silent pass. CDC pair for 2.8 (provider = an old-version emitter; consumer = the upcasting consumer).
- **DEFINITION OF DONE.** The upcaster registry exists and compiles; events bridge forward-only at consume; the CDC + unit tests pass; committed.
- **COMMIT.** Header `P-<NNN> M0: schema-evolution upcaster registry (forward-only)`. Body: 2.8; the `(type, from_ver) → to_ver` pure-fn registry; the loud-on-unbridgeable rule. Co-Authored-By trailer.

---

### P-S10 — The four load-bearing architecture lints (`tenant-predicate`, `no-raw-publish`, `no-host-exec`, `no-untagged-personal-data`), each with red + green fixtures

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 3 — the four most load-bearing lints) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 3 (the four most load-bearing — `tenant-predicate`, `no-raw-publish`, `no-host-exec`, `no-untagged-personal-data`).
- **DEPENDS-ON.** P-S01, P-S06, P-S08 (the lints scan the emit + tenant + consumer surfaces these shipped).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §5 (the ratchet — convert each discipline into a committed mechanical gate; an uncommitted gate is no gate; loud, never silently swallowed — replace `... || true`).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §2.11 (the lint table — the four most load-bearing: `tenant-predicate`, `no-raw-publish`, `no-host-exec`, `no-untagged-personal-data`).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.6 (the twelve lints; each ships with a red-fixture that proves it rejects + a green-fixture that proves it admits).
  - `../../06-roadmaps/00-master-sequencing.md` §1 Tier 3 (the committed ratchet).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-substrate` (or a dedicated `myelin-lints` crate inside it): the FOUR most load-bearing architecture lints as `cargo`-level architecture tests / clippy-style checks, wired into CI **loud-never-swallowed** (no `... || true`; a violation fails the build): `tenant-predicate` (every query-builder call carries a `TenantId` bound; a tenant-less query fails to compile — F2, the IDOR floor); `no-raw-publish` (no bus publish outside `OutboxTx::emit` — F5); `no-host-exec` (no host-execution path bypassing `ToolHands::exec` = the unified sandbox, X-6); `no-untagged-personal-data` (every PII-carrying field is `#[personal_data(...)]`-tagged; an untagged PII column fails to compile). For EACH of the four, ship a **red-fixture** (a code sample the lint MUST reject — the build fails on it) and a **green-fixture** (a sample it MUST admit). These four make whole bug-classes impossible to compile.
- **CONTRACTS TO IMPLEMENT.** Owned (the four-lint slice of 1.6).
- **GATE / DRILLS (quantified; must be green to call this done).** **All four lints green** with both fixtures: each red-fixture causes a build failure (4/4 reject), each green-fixture builds clean (4/4 admit). Green artifact = a CI run showing the four lint jobs green + the fixture matrix. Wired loud (a lint that silently passes is a defect).
- **TESTS (required).** The fixture matrix IS the test: a meta-test runs each red-fixture and asserts a compile/lint failure, each green-fixture and asserts success. A regression test that removing any of these four lints' wiring fails the meta-test. No CDC pair (lints are the gate UNDER every contract, not a cross-system contract).
- **DEFINITION OF DONE.** The four lints exist, are wired into CI loud-never-swallowed, and pass the 4×(red-reject + green-admit) fixture matrix; committed.
- **COMMIT.** Header `P-<NNN> M0: four load-bearing architecture lints + red/green fixtures`. Body: `tenant-predicate`/`no-raw-publish`/`no-host-exec`/`no-untagged-personal-data`; the 4/4-reject + 4/4-admit matrix. Co-Authored-By trailer.

---

### P-S11 — The remaining eight architecture lints, each with red + green fixtures

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 3 — the remaining eight lints completing the twelve / the ratchet) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 3 (the full twelve-lint table).
- **DEPENDS-ON.** P-S10 (the lint-harness machinery + fixture-matrix meta-test pattern), P-S01.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §5 (the ratchet — an uncommitted gate is no gate; loud, never swallowed).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §2.11 (the full lint table: `no-cross-db`, `forward-only-migration`, `no-cross-sync-cycle`, `residency-pin`, `control-plane-pii-free`, `search-requires-acl-filter`, `no-llm-in-platform`, `flow-determinism` — the rule text for each).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.6 (each lint ships a red + a green fixture).
  - `../../06-roadmaps/00-master-sequencing.md` §1 Tier 3 + the M0 exit gate (all twelve lints green with both fixtures).
- **DELIVERABLE (what to build + exactly where in the repo).** In the same lint crate as P-S10: the REMAINING EIGHT architecture lints, completing the twelve, each wired into CI loud-never-swallowed with a red + green fixture: `no-cross-db` (a service crate must not depend on another service's storage module); `forward-only-migration` (no rollback migration file; no blocking `ALTER` on a flagged-hot table — reads the hot-table declaration P-S15 ships); `no-cross-sync-cycle` (the sync call graph is acyclic; identity is a sink); `residency-pin` (every store/stream/index/cache declares a region; no global pool; outbound transfer gated); `control-plane-pii-free` (the control plane carries opaque ids only — never a name/email/body); `search-requires-acl-filter` (every search/list query conjoins the `list_objects` `Filter` before scoring — pre-filter, never post-filter); `no-llm-in-platform` (no LLM SDK / prompt / model name in platform code; the runtime is behind the `AgentRuntime` strategy seam); `flow-determinism` (a `myelin-flow` workflow body uses only the deterministic `WfCtx` surface). Some lints (`search-requires-acl-filter`, `flow-determinism`, `control-plane-pii-free`, `forward-only-migration`) target code that does not exist yet — ship the lint + its fixtures now so the gate is live before the consumer ships; **name this as each lint's floor** (it tightens as the targeted code lands).
- **CONTRACTS TO IMPLEMENT.** Owned (the remaining-eight slice of 1.6 — together with P-S10 this completes the ratchet).
- **GATE / DRILLS (quantified; must be green to call this done).** **All eight lints green** with both fixtures: 8/8 reject + 8/8 admit. Together with P-S10, the full **12/12 reject + 12/12 admit** matrix is now live. Green artifact = a CI run showing all twelve lint jobs green + the complete fixture matrix.
- **TESTS (required).** The fixture matrix meta-test (extended to twelve): each red-fixture asserts a compile/lint failure, each green-fixture asserts success. A regression test that removing any lint's wiring fails the meta-test (the ratchet cannot be silently un-wired). No CDC pair.
- **DEFINITION OF DONE.** The eight lints exist, are wired loud-never-swallowed, and pass their 8×(red-reject + green-admit) matrix; the full twelve-lint matrix is green; lints targeting not-yet-existing code are named as floors that tighten on their consumer; committed.
- **COMMIT.** Header `P-<NNN> M0: remaining eight architecture lints + red/green fixtures (completes the twelve)`. Body: the eight lints by name; the full 12/12-reject + 12/12-admit matrix; the floor lints + their tightening triggers. Co-Authored-By trailer.

---

### P-S12 — `serve(AppSpec)`: the boot → migrate → relay → consumers → drain lifecycle

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 6-precondition — the service-shell lifecycle) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 4 (the `serve` lifecycle half).
- **DEPENDS-ON.** P-S07, P-S08 (the relay + consumers `serve` wires), P-S04 (the telemetry producer side `serve` opens).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §7 (abstract at the third copy — the harness IS that abstraction; identical plumbing, visible logic).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §3.1 (the one call — `serve(AppSpec{ name, config, migrations, public, internal, consumers, holders, outbox })`; boot → migrate → relay → consumers → three ports → graceful drain), §3.2 (config env-first, validated at boot, fail fast), §3.3 (DB pool + outbox publisher; bounded pool, statement timeout, fast-fail on saturation; read-replica awareness), §3.5 (telemetry init + trace context), §3.6 (what the harness deliberately does NOT do).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.1 (`serve(AppSpec)`).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-substrate`: the `AppSpec` struct (`{ name, config, migrations, public, internal, consumers, holders, outbox }`) + `serve(AppSpec) -> Result<()>` owning the lifecycle: boot → run migrations (the runner lands in P-S15; here `serve` calls it) → start the outbox relay (P-S07) → start consumers (P-S08) → open the three ports (the topology lands in P-S13/P-S14; here `serve` calls the opener) → serve → **graceful drain** (stop intake, finish in-flight, ack-then-exit); non-zero on failed boot. Config env-first + validated at boot, fail fast (§3.2). The bounded DB pool (statement timeout, fast-fail on saturation; read-replica awareness, §3.3). Initialise the OpenTelemetry tracer/meter/logger and install the causality+tenant trace-context middleware (§3.5) — this is the producer side of the contract-1.8 signal set the harness (P-S04) reads. A hello-world test service that boots from `serve`, emits one event through the outbox, a consumer dedups it (the M0 "first runnable") — the three-port / tenant-from-token + readiness behaviours are proven in P-S13/P-S14.
- **CONTRACTS TO IMPLEMENT.** Owned: 1.1.
- **GATE / DRILLS (quantified; must be green to call this done).** No standalone catalogue drill at this prompt (SUB-D7 rides P-S13, SUB-D9 rides P-S14). The gate is the lifecycle + drain correctness + the hello-world boot. Green artifact = the hello-world boot test (boot → emit → consume → drain) passing and the telemetry signal set being produced.
- **TESTS (required).** Unit: graceful drain finishes in-flight before exit; a failed boot returns non-zero; config validation fails fast on a bad env. The hello-world service is an end-to-end boot test (boot → emit → consume → drain). CDC pair for 1.1 (provider = `serve`; consumer = the hello-world `main.rs`). Mutation floor ≥ 75% on the lifecycle module.
- **DEFINITION OF DONE.** `serve(AppSpec)` + the lifecycle + graceful drain exist and compile; the hello-world boot test + CDC + mutation floor pass; the three-port topology (P-S13/P-S14) and migration runner + holder registration (P-S15) dependencies are named; committed.
- **COMMIT.** Header `P-<NNN> M0: serve(AppSpec) lifecycle + graceful drain`. Body: 1.1; the boot→migrate→relay→consumers→ports→drain lifecycle; the hello-world boot artifact; the three-port + runner + holder follow-on prompts. Co-Authored-By trailer.

---

### P-S13 — The three-surface topology + tenant-from-token (SUB-D7, cross-tenant IDOR)

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 6-precondition — the three-surface topology + tenant-from-token) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 4 (the three ports + tenant-from-token).
- **DEPENDS-ON.** P-S12 (the ports open inside the `serve` lifecycle), P-S03, P-S04 (the SUB-D7 drill rides the injector + assertion library), P-S10 (the `tenant-predicate` lint).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §2 (a cross-tenant IDOR is a top-tier security bug).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §4 (the three-surface topology — public / internal / metrics-health; public↔internal is a security boundary), §4.1 (public surface — tenant from the verified token never the URL path; a mismatch is rejected + audited as an IDOR), §4.2 (internal RPC surface — re-authorize every call; not "internal = safe").
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.2 (three-surface topology + tenant-from-token).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 row SUB-D7 (cross-tenant IDOR).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-substrate`: the **three-surface topology** the `serve` lifecycle opens — (a) a **public surface** (gateway-fronted, identity+causality headers injected, **tenant from the verified token never the URL path** — a path-tenant ≠ token-tenant mismatch is rejected + audited as an IDOR); (b) an **internal RPC surface** (trust boundary; re-authorize every call — trusting the header for identity is fine, for authorization is not); (c) the **metrics-health surface** opener (exports the contract-1.8 signal set; the liveness ≠ readiness semantics land in P-S14). The public↔internal split is a security boundary, not a convenience.
- **CONTRACTS TO IMPLEMENT.** Owned: 1.2.
- **GATE / DRILLS (quantified; must be green to call this done).** **SUB-D7** (cross-tenant read via path-tenant ≠ token-tenant → **0**; the `tenant-predicate` lint catches a tenant-less query at compile time) — signals `misroute_count == 0` + lint green, CI.
- **TESTS (required).** Unit: a URL-path tenant ≠ token tenant is rejected + audited (the SUB-D7 mechanism); the internal surface re-authorizes every call (does not presume "internal = safe"); the three ports open with the standard middleware stack. CDC pair for 1.2. Drill scenario for SUB-D7 (path-tenant spoof; the P-S03 injector + P-S04 assertions). Mutation floor ≥ 75% on the tenant-from-token module.
- **DEFINITION OF DONE.** The three-surface topology + tenant-from-token exist and compile; SUB-D7 emits a dated green artifact; the CDC + mutation floor pass; committed.
- **COMMIT.** Header `P-<NNN> M0: three-surface topology + tenant-from-token (SUB-D7)`. Body: 1.2; SUB-D7 (misroute 0 + lint) with measured numbers. Co-Authored-By trailer.

---

### P-S14 — Liveness ≠ readiness on the metrics-health surface (SUB-D9)

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 6-precondition — liveness ≠ readiness) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 4 (liveness ≠ readiness).
- **DEPENDS-ON.** P-S13 (the metrics-health surface opener), P-S03, P-S04 (the SUB-D9 drill rides the injector + assertion library).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (observability is part of the pass condition).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §4.3 (liveness = not-wedged → restart on fail, must NOT check dependencies; readiness = can-serve-correct-traffic → a dead critical dependency reports not-ready and sheds, never healthy-but-failing; startup = not-ready-not-killed) and §8.3 (readiness handles a SUSTAINED outage; fail-static handles a TRANSIENT hiccup — the two compose).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.3 (liveness ≠ readiness).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 row SUB-D9 (liveness ≠ readiness).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-substrate`: the **liveness ≠ readiness** semantics on the metrics-health surface — liveness = "not wedged" (restart on fail; does NOT check dependencies); readiness = "can serve correct traffic now" (a dead critical dependency reports not-ready and stops taking traffic, never reports healthy-but-failing); startup = boot/migration incomplete → not-ready, not-killed. A severed critical dependency flips readiness and sheds; liveness stays up (no restart-storm). Names the composition with fail-static (P-S18): readiness handles the sustained outage; fail-static buys the transient hiccup.
- **CONTRACTS TO IMPLEMENT.** Owned: 1.3.
- **GATE / DRILLS (quantified; must be green to call this done).** **SUB-D9** (kill a critical dependency → instance reports not-ready + sheds; liveness does NOT restart-storm) — signals `readiness` flips, no liveness churn, CI.
- **TESTS (required).** Unit: readiness flips on a severed critical dep while liveness stays up; startup reports not-ready-not-killed; liveness does not check dependencies (a dead dep does not flip liveness). CDC pair for 1.3. Drill scenario for SUB-D9 (the P-S03 injector kills a critical dep; the P-S04 assertions read readiness + liveness churn). Mutation floor ≥ 75% on the readiness module.
- **DEFINITION OF DONE.** Liveness ≠ readiness exists and compiles; SUB-D9 emits a dated green artifact; the CDC + mutation floor pass; the fail-static composition is named; committed.
- **COMMIT.** Header `P-<NNN> M0: liveness != readiness (SUB-D9)`. Body: 1.3; SUB-D9 (readiness flips, no restart-storm) with measured numbers; the fail-static composition. Co-Authored-By trailer.

---

### P-S15 — `PersonalDataHolder` auto-registration + the forward-only migration runner

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 6-precondition — holder auto-registration + the migration runner) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 4 (holder auto-registration + the forward-only migration runner + hot-table declaration).
- **DEPENDS-ON.** P-S12 (`serve` calls the runner + holder registration), P-S11 (the `forward-only-migration` lint reads the hot-table declaration).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §2 (no data loss — forward-only, you can't un-delete data).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §3.4 (`PersonalDataHolder` auto-registration — every store the harness opens), §9 (forward-only online migrations runner; §9.1 expand→backfill→contract; §9.2 measure lock time against a restored copy; §9.4 hot-table declaration mechanism the `forward-only-migration` lint reads).
  - `../../05-refined-shared-systems-architecture/contract-index.md` rows 1.4 (`PersonalDataHolder` auto-registration mechanism), 1.5 (forward-only migrations + hot-table flags).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-substrate`: (a) **`holders: AppSpec::auto`** — auto-register every store the harness opens (OLTP schema, any blob prefix, any cache namespace, the search index if owned) as a `PersonalDataHolder`; the mechanism makes "we forgot a store" structurally impossible (the exhaustive H1–H18 list is GDPR's M1 deliverable, confirmed in P-S27). (b) the **forward-only migration runner** — runs embedded migrations at boot; expand→backfill→contract; no down migrations ("rollback" is a new forward migration); plus the **hot-table declaration mechanism** every subsystem declares in its `AppSpec`, which the `forward-only-migration` lint (P-S11) reads to forbid a blocking `ALTER` on a flagged-hot table. **Floor named:** the exhaustive holder list (H1–H18) is GDPR's M1 follow-on (P-S27); the hot-table flags are measured-not-predicted per subsystem (M1+); SUB-D10 (migration under load) proves at M5 (P-S34).
- **CONTRACTS TO IMPLEMENT.** Owned: 1.4 (mechanism), 1.5 (runner + declaration mechanism).
- **GATE / DRILLS (quantified; must be green to call this done).** No catalogue survival drill at this prompt (SUB-D10 under-load is M5, P-S34). The gate is: every store opened auto-registers as a holder; the runner applies forward-only migrations at boot; a down-migration file fails the build (the `forward-only-migration` lint); a blocking `ALTER` on a flagged-hot table fails the build. Green artifact = the holder-registration + migration-runner tests passing + the lint catching a hot-table blocking `ALTER`.
- **TESTS (required).** Unit: every store the harness opens auto-registers as a holder (no orphan store); the runner applies an expand→backfill→contract migration; a down-migration file is rejected; a blocking `ALTER` on a flagged-hot table is rejected by the lint. CDC pair for 1.4 + 1.5. Mutation floor ≥ 75% on the holder-registration + migration-runner modules.
- **DEFINITION OF DONE.** Holder auto-registration + the forward-only migration runner + the hot-table declaration mechanism exist and compile; the holder + runner + lint tests pass; the exhaustive-holder (P-S27), measured-hot-table (M1+), and SUB-D10 (P-S34) follow-ons are named; committed.
- **COMMIT.** Header `P-<NNN> M0: PersonalDataHolder auto-registration + forward-only migration runner`. Body: 1.4/1.5; the auto-registration mechanism; the runner + hot-table declaration; the holder-list (P-S27) + measured-hot-table + SUB-D10 (P-S34) follow-ons. Co-Authored-By trailer.

---

### P-S16 — The shared resilient inter-service client: timeout + breaker + bulkhead + jittered retry

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 6-precondition — the resilient-client four primitives) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 4 (`ResilientClient`).
- **DEPENDS-ON.** P-S01.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §5 (loud, never swallowed).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §6 (the shared resilient client — the four primitives, all mandatory + on-by-default: per-call timeout/deadline-propagation; circuit breaker closed→open→half-open never-retry-through-a-tripped-breaker; bounded-concurrency bulkhead fast-fails never queues unbounded; jittered retry idempotent-calls-only full-jitter), §6.3 (defaults — timeouts in ms; breaker as a failure ratio + min request count; bulkhead an integer cap; backoff base ms full jitter; per-target values are each consumer's call).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.9 (`ResilientClient::call(target, req, idem)` — timeout + breaker + bulkhead + jittered-retry-idempotent-only).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-client`: `ResilientClient::call<R>(target, req, idem: Idempotency) -> Result<R>` with the four mandatory on-by-default primitives — (1) per-call timeout (in ms; deadlines propagate); (2) circuit breaker (closed→open→half-open, never retry through a tripped breaker — the textbook retry-storm amplifier); (3) bounded-concurrency bulkhead per target (a semaphore per target; saturation fast-fails, never queues unboundedly); (4) jittered retry for `Idempotency::Idempotent` calls only, full jitter (Brooker 2015) — a `NonIdempotent` call is never retried. Defaults: timeouts ms; breaker a failure ratio over a rolling window + a min request count; bulkhead an integer concurrency cap; backoff base ms with full jitter. Export breaker-state + bulkhead-rejections into the contract-1.8 signal set. Ship one **default per-target value set** (the M0 floor). **Floor named:** default per-target values (M0) → per-target tuned values measured by the surge/latency drills (M5, P-S36). (`Retry-After` honouring + the SUB-D5 proof land in P-S17.)
- **CONTRACTS TO IMPLEMENT.** Owned: 1.9 (the four-primitive slice; `Retry-After` is P-S17).
- **GATE / DRILLS (quantified; must be green to call this done).** No catalogue survival drill at this prompt (SUB-D5 rides P-S17 once `Retry-After` is honoured). The gate is the four primitives' correctness. Green artifact = the primitive unit tests passing + breaker/bulkhead signals exported.
- **TESTS (required).** Unit: a tripped breaker rejects without calling through; a `NonIdempotent` call is never retried; a saturated bulkhead fast-fails rather than queueing; full-jitter backoff stays within the configured base. CDC: provider side of 1.9 (a fake downstream; the consumer-side `Retry-After` half lands in P-S17). Mutation floor ≥ 80% on the breaker + retry modules (mandatory core — a retry-storm amplifier is a cascade).
- **DEFINITION OF DONE.** `ResilientClient::call` + the four primitives exist and compile to the frozen shape; the primitive unit tests + mutation floor pass; the default-per-target floor is named with its M5 tuning follow-on (P-S36); committed.
- **COMMIT.** Header `P-<NNN> M0: resilient inter-service client (timeout + breaker + bulkhead + jittered retry)`. Body: 1.9 (four primitives); the per-target-value floor → M5 (P-S36). Co-Authored-By trailer.

---

### P-S17 — `Retry-After` honouring on the resilient client (SUB-D5, no retry-storm amplification)

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 6-precondition — `Retry-After` honouring + the SUB-D5 proof) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 4 (`Retry-After` honouring).
- **DEPENDS-ON.** P-S16, P-S03, P-S04 (the SUB-D5 drill rides the injector + assertion library).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (cheap drills in CI; prove no amplification) and §5 (loud, never swallowed).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §6.2 (`Retry-After` honouring — our clients MUST honour it as the floor of backoff; a hard requirement on the agent runtime + CLI too, so shedding cannot become a retry storm and the protected human lane holds).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.9 (honours `Retry-After`).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 row SUB-D5.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-client`: **`Retry-After` honouring** on `ResilientClient` — respect the `Retry-After` header as the floor of the client's backoff (a full-jitter backoff never goes below the `Retry-After` floor). This is a hard requirement on the agent runtime and the CLI too (they link this client), so shedding (§7) cannot become a retry storm. Export `Retry-After` issuance into the contract-1.8 signal set.
- **CONTRACTS TO IMPLEMENT.** Owned: 1.9 (the `Retry-After` slice — completes the contract row).
- **GATE / DRILLS (quantified; must be green to call this done).** **SUB-D5** (trip a downstream breaker under load → callers fail fast, NO retry through the tripped breaker, honour `Retry-After`, no amplification) — signals `breaker_state == open` while callers fail-fast, `retry_through_tripped == 0`, `retry_after_honoured == true`, CI.
- **TESTS (required).** Unit: full-jitter backoff respects the `Retry-After` floor (it never backs off below it); a tripped breaker + a `Retry-After` together produce fail-fast without amplification. CDC pair for 1.9 (provider = a fake downstream that trips + issues `Retry-After`; consumer = the client). Drill scenario for SUB-D5 (the P-S03 injector trips a downstream breaker under load; the P-S04 assertions read the signals). Mutation floor ≥ 80% on the `Retry-After`/backoff module.
- **DEFINITION OF DONE.** `Retry-After` honouring exists and compiles; SUB-D5 emits a dated green artifact; the CDC + mutation floor pass; committed.
- **COMMIT.** Header `P-<NNN> M0: Retry-After honouring (SUB-D5, no amplification)`. Body: 1.9 (`Retry-After`); SUB-D5 (fail-fast, no amplification, Retry-After honoured) with measured numbers. Co-Authored-By trailer.

---

### P-S18 — The fail-static mechanism (`FailStatic<T>`, bounded-staleness, never fail open)

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 6-precondition — the fail-static mechanism) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 4 (`FailStatic<T>`).
- **DEPENDS-ON.** P-S01, P-S22 (the value W is read from the thresholds file; if P-S22 is not yet merged, read a placeholder constant + name the dependency).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §2 (order by non-negotiability — a shared-dependency cascade is a platform-wide kill) and §3 (name your floors; record needs-human-verification honestly).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §8 (fail-static — distinguish fail-closed from fail-static; `FailStatic<T>{ fresh_ttl, static_max }`; `Answer<T> = Fresh | Static(degraded) | Closed`; stale-while-revalidate; never fail open), §8.2 (the staleness bound — `static_max ≤ revocation-SLA ≥ agent-token-TTL`; the VALUE W is `[OPEN — LEGAL]`, DPO-ratified, L-1).
  - `../../05-refined-shared-systems-architecture/contract-index.md` rows 1.10 (`FailStatic<T>`), 4.11 (the Id-usage fail-static bound; `[OPEN — LEGAL]` ratification).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-substrate`: `FailStatic<T>{ fresh_ttl: Seconds, static_max: Seconds }` with `get(key, refresh) -> Answer<T>` (`Fresh | Static(degraded) | Closed`): within `fresh_ttl` → fresh; between → serve stale + degraded marker + background refresh (stale-while-revalidate); past `static_max` → fail closed — **never fail open**. For authorization, the static answer is the coarse "actor still active / coarse grants" — never an escalation of access. The constraint `static_max ≤ revocation-SLA ≥ agent-token-TTL` is enforced in the constructor (a value violating it does not construct). The VALUE W is read from the thresholds file (P-S22) and flagged `[OPEN — LEGAL]`. Export the fresh/stale/closed ratios + staleness age into the contract-1.8 signal set. **Floor named:** the value W remains `[OPEN — LEGAL]` (DPO-ratified, L-1) — the mechanism + constraint ship regardless. Fail-static is PROVEN against a real Identity hiccup in M1 (P-S25); here only the mechanism is built + unit-drilled.
- **CONTRACTS TO IMPLEMENT.** Owned: 1.10 (mechanism + constraint). Carried: 4.11 (the Id-usage bound — the constraint; the value is DPO-ratified).
- **GATE / DRILLS (quantified; must be green to call this done).** No catalogue survival drill at this prompt (SUB-D4 against a real Identity is M1, P-S25 — named, not skipped). The gate is the mechanism correctness at the boundaries + the constructor constraint. Green artifact = the boundary unit tests passing.
- **TESTS (required).** Unit: `FailStatic` serves fresh/stale/closed at the `fresh_ttl` and `static_max` boundaries and never escalates access (never fails open); a constructor with `static_max > revocation-SLA` is rejected; a `static_max < agent-token-TTL` is rejected. CDC: provider side of 1.10 (the consumer-against-Identity side is P-S25). Mutation floor ≥ 80% on the fail-static module (mandatory core — a fail-open is catastrophic).
- **DEFINITION OF DONE.** `FailStatic<T>` + the constructor constraint exist and compile; the boundary unit tests + mutation floor pass; the SUB-D4 proof is named as deferred to M1 (P-S25); the value-W `[OPEN — LEGAL]` floor is named; committed.
- **COMMIT.** Header `P-<NNN> M0: fail-static mechanism (FailStatic<T>, never fail open)`. Body: 1.10/4.11; the fresh/stale/closed boundaries + the constructor constraint; the value-W `[OPEN — LEGAL]` floor; the SUB-D4-deferred-to-M1 (P-S25) note. Co-Authored-By trailer.

---

### P-S19 — The protected-human-lane shed order and bounded-everything

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 6-precondition — the shed lane + bounded-everything) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 4 (the shed lane).
- **DEPENDS-ON.** P-S13 (the shed lane runs at the public surface), P-S17 (`429 + Retry-After` honoured by clients).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §2 (order by non-negotiability; per-tenant blast radius).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §7.1 (bounded everything — every queue and pool is bounded; an unbounded queue = unbounded latency = indistinguishable from down), §7.2 (the principal-aware limiter + protected human lane — shed order: speculative → batch/CI → agent → human-last; `429 + Retry-After`; per-tenant so one tenant's surge doesn't shed another's humans), §7.3 (why this order — promise strength), §7.6 (the per-surface shed-budget v1 floor table — CI-surge / collab op-stream / connection-storm / agent-mention-storm).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.11 (protected-human-lane shed order + per-surface shed budgets).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-substrate`: (a) the **principal-aware shed lane** at the public surface — read `Principal.kind` + run-class from the injected headers, reserve the protected human lane, apply the shed order (speculative → batch/CI → agent → human-last) with `429 + Retry-After`, **per-tenant** (one tenant's surge does not shed another's humans). (b) **Bounded everything**: every queue/pool fast-fails (sheds) rather than growing latency unboundedly — consumer prefetch, the DB pool, the bulkhead per target, per-tenant in-flight work, the HTTP intake queue (Little's Law: an unbounded queue is indistinguishable from down). (c) the §7.6 per-surface shed-budget **v1 floor table** (named floors). Export shed-counts per lane + per-surface budget into the contract-1.8 signal set. **Floors named:** the shed-budget numbers (M0 floor table → tuned numbers M5, P-S33). (The agent-load caps + SUB-D8 land in P-S20.)
- **CONTRACTS TO IMPLEMENT.** Owned: 1.11 (shed order + v1 budget floor table).
- **GATE / DRILLS (quantified; must be green to call this done).** No catalogue survival drill at this prompt (the SUB-D8 loop-guard rides P-S20; the SUB-D3 surge is M5, P-S32). The gate is: the shed order sheds in the right priority, the human lane is shed last and per-tenant, every queue is bounded and fast-fails. Green artifact = the shed-order + bounded-everything unit tests passing + the shed-count signals exported.
- **TESTS (required).** Unit: the shed order sheds speculative → batch/CI → agent → human-last; the human lane is shed last and is per-tenant (one tenant's surge does not shed another's humans); every bounded queue fast-fails rather than growing latency unboundedly. CDC pair for 1.11. Mutation floor ≥ 80% on the shed-order module (mandatory core).
- **DEFINITION OF DONE.** The shed lane + bounded-everything + the v1 budget floor table exist and compile; the shed-order + bounded unit tests + mutation floor pass; the shed-budget floor is named with its M5 follow-on (P-S33); the SUB-D8 dependency on P-S20 is named; committed.
- **COMMIT.** Header `P-<NNN> M0: protected-human-lane shed order + bounded-everything`. Body: 1.11; the shed order + per-tenant human lane + bounded queues; the shed-budget v1 floor → M5 (P-S33). Co-Authored-By trailer.

---

### P-S20 — Agent-generated-load caps + the causal-loop guard (SUB-D8)

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (Tier 6-precondition — the agent-load caps + the loop-guard machinery) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 4 (the bounded dispatch pool + depth ceiling + tripwire).
- **DEPENDS-ON.** P-S19 (the shed lane + bounded-everything), P-S06 (the depth ceiling reads `EventEnvelope.depth`), P-S03, P-S04 (the SUB-D8 drill rides the injector + assertion library).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §2 (a shared-dependency cascade / an unbounded agent fan-out is a platform-wide kill).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §7.4 (agent-generated load — bounded dispatch pool drops over-cap never forks; causal-depth ceiling; shared-root-within-a-window tripwire; reserve/settle cost gate), §7.5 (bounded predicate evaluation — a step/time ceiling per predicate so a crafted matcher cannot DoS the trigger engine) and §5.3 (causality through the consumer — the depth/tripwire read `EventEnvelope` fields, so no convention/typo defeats them).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.11 (the shed order — the agent lane) + 1.8 (causal-depth tripwire firings + dispatch-pool drops survival signal).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 row SUB-D8.
- **DELIVERABLE (what to build + exactly where in the repo).** In crate `myelin-substrate`: the agent-load caps — (a) the **bounded dispatch pool** (drops over-cap, never forks); (b) the **causal-depth ceiling** (reads `EventEnvelope.depth`; ceilings 12/16 from the thresholds file, P-S22); (c) the **shared-causal-root-within-a-window tripwire** (reads `correlation_id`); (d) the **bounded predicate-evaluation guard** (§7.5 — a step/time ceiling per predicate so a crafted matcher cannot DoS the trigger engine). Export the causal-depth histogram + tripwire firings + dispatch-pool drops into the contract-1.8 signal set. **Floor named:** the full agent-loop proof re-runs in M2 with the agent fabric; the substrate ships + drills the machinery here.
- **CONTRACTS TO IMPLEMENT.** Owned (the agent-load slice of 1.11; the loop-guard machinery under AG-6). Consumes 1.8 (the causal-depth signals).
- **GATE / DRILLS (quantified; must be green to call this done).** **SUB-D8** (adversarial agent→agent loop → the depth ceiling (12/16) + the shared-root tripwire + the bounded pool halt it) — signals `causal_depth` histogram bounded, `tripwire_fired`, `dispatch_pool_drops` (over-cap dropped, never forked), CI. (The full agent-loop proof re-runs in M2 with the agent fabric.)
- **TESTS (required).** Unit: the dispatch pool drops over-cap rather than forking; the depth ceiling halts a constructed loop at 12/16; the shared-root tripwire fires within its window; the predicate-evaluation guard rejects a crafted over-cost matcher. CDC pair for the agent-load slice of 1.11. Drill scenario for SUB-D8 (the P-S03 injector drives an agent→agent loop; the P-S04 assertions read the signals). Mutation floor ≥ 80% on the dispatch-pool + loop-guard modules (mandatory core).
- **DEFINITION OF DONE.** The bounded dispatch pool + depth ceiling + tripwire + predicate guard exist and compile; SUB-D8 emits a dated green artifact; the CDC + mutation floor pass; the M2 full-agent-loop re-run is named; committed.
- **COMMIT.** Header `P-<NNN> M0: agent-load caps + causal-loop guard (SUB-D8)`. Body: the bounded dispatch pool + depth ceiling + tripwire + predicate guard; SUB-D8 (depth ceiling + tripwire + bounded pool) with measured numbers; the M2 agent-fabric re-run. Co-Authored-By trailer.

---

### P-S21 — The contract-coverage scanner (the meta-gate)

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (the committed-gate machinery — the contract-coverage scanner) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 5 (the contract-coverage scanner).
- **DEPENDS-ON.** P-S07, P-S08, P-S17 (the scanner reads the contract surfaces these shipped).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §5 (the ratchet — an uncommitted gate is no gate; make violations loud; a contract violation dropped silently is a multi-day misdiagnosis).
  - `../../05-refined-shared-systems-architecture/contract-index.md` §1 + §2 (the contract rows the scanner enforces have a provider + consumer CDC pair).
  - `../00-ledger-overview.md` §6 (the contract-coverage scanner fails the workspace if any contract-index row lacks provider + consumer CDC).
- **DELIVERABLE (what to build + exactly where in the repo).** The **contract-coverage scanner** (a CI tool in `myelin-substrate` or a workspace `xtask`): it reads the contract-index rows and FAILS the workspace if any row lacks BOTH a provider-side and a consumer-side CDC test — loud, never swallowed. A row not yet implemented is marked explicitly with its landing prompt; a row that CLAIMS coverage without a pair is a build failure. This is the meta-gate every later prompt's DEFINITION OF DONE leans on.
- **CONTRACTS TO IMPLEMENT.** No new contract-index row is owned; the scanner enforces all of §1–§13.
- **GATE / DRILLS (quantified; must be green to call this done).** The **contract-coverage scanner passes** on the (still-small) contract set shipped so far (every row with a provider has a CDC pair, or is explicitly marked not-yet-implemented with its landing prompt) — green artifact = a scanner run with 0 uncovered rows that claim coverage.
- **TESTS (required).** Unit: the scanner FAILS loudly on a deliberately-uncovered row (a red-fixture for the scanner itself — it must reject); a row marked not-yet-implemented with its landing prompt passes. No CDC pair (the scanner is a meta-gate, not a cross-system contract).
- **DEFINITION OF DONE.** The scanner exists and is wired into CI loud-never-swallowed; it passes with 0 falsely-claimed coverage; the scanner red-fixture rejects; committed.
- **COMMIT.** Header `P-<NNN> M0: contract-coverage scanner (the meta-gate)`. Body: the scanner (fails on uncovered rows); the scanner red-fixture. Co-Authored-By trailer.

---

### P-S22 — The versioned thresholds file (every Q32 default-to-beat)

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (the committed-gate machinery — the versioned thresholds file) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 5 (the versioned thresholds file).
- **DEPENDS-ON.** P-S04 (the telemetry signals the thresholds key on).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (the thresholds file; a red gate becomes a "claimed, not proven" row, never edited green).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §10.2 (the telemetry signal set the thresholds key on) + §8.2 (the fail-static value W `[OPEN — LEGAL]` constraint).
  - `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 5 (the Q32 defaults: N=5min revocation, 30× surge, W=fail-static, RPO ≤ 5min, RTO ≤ 1h/tenant ≤ 4h/cell, depth ceilings 12/16, per-surface shed budgets v1) + §5 (the green-artifact / thresholds-file discipline) + §6 (the honesty register).
- **DELIVERABLE (what to build + exactly where in the repo).** The **versioned thresholds file** (one file, e.g. `thresholds.toml` at the workspace root): every Q32 default-to-beat as a named, dated row — revocation N = 5 min; surge multiplier = 30×; fail-static W = `[OPEN — LEGAL]` placeholder with the `≤ revocation-SLA ≥ agent-token-TTL` constraint; RPO ≤ 5 min; RTO ≤ 1h/tenant ≤ 4h/cell; causal-depth ceilings 12/16; per-surface shed budgets v1 floors. A drill reads its threshold from here (no hardcoded magic number in a drill); a red gate becomes a "claimed, not proven" scorecard row IN THIS FILE — never edited green. A missing threshold is a loud error, not a default.
- **CONTRACTS TO IMPLEMENT.** No new contract-index row owned; this is the THRESHOLDS SOURCE every drill reads (it backs 1.8/1.10/1.11/4.11/11.5 numbers).
- **GATE / DRILLS (quantified; must be green to call this done).** The thresholds file parses and every M0 drill that has shipped reads its threshold from it (no hardcoded magic number in a drill). Green artifact = the thresholds-file round-trip test + a sample drill reading its threshold from the file.
- **TESTS (required).** Unit: the thresholds file round-trips (parse → serialize → parse); a missing threshold is a loud error, not a silent default; the `[OPEN — LEGAL]` W carries its constraint. No CDC pair (the file is a config source, not a cross-system contract).
- **DEFINITION OF DONE.** The thresholds file exists, parses, holds every Q32 default-to-beat as a dated row, and is the source every M0 drill reads; the round-trip + missing-threshold tests pass; committed.
- **COMMIT.** Header `P-<NNN> M0: versioned thresholds file (the Q32 defaults-to-beat)`. Body: the Q32 defaults (N/surge/W/RPO/RTO/depth ceilings/shed budgets); the claimed-not-proven discipline. Co-Authored-By trailer.

---

### P-S23 — The shared overlay/state primitives (the design-system bug-class floor)

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (the committed-gate machinery — the shared overlay/state primitives) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 5 (the shared overlay/state primitives built BEFORE any feature consumes them).
- **DEPENDS-ON.** P-S01.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §4 (the off-screen-picker / clipped-dialog / unreachable-control bug-classes — foreclose them at the design-system layer) and §7 (abstract at the third copy — applied pre-emptively because the doctrine names these exact recurring bugs).
  - `../../05-refined-shared-systems-architecture/testing-strategy/README.md` §5 (the design-system layer floor — the overlay/state primitives).
  - `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 5 (the shared overlay/state primitives built once so no feature hand-rolls them).
- **DELIVERABLE (what to build + exactly where in the repo).** The **shared overlay/state primitives** (the design-system layer floor): the popover/dialog/picker/focus-trap primitives that foreclose the off-screen-picker, clipped-dialog, and focus-leak bug-classes — built once so no feature hand-rolls them (EI-01 §7 abstract-at-the-third-copy, applied pre-emptively). If the frontend stack is not yet chosen, ship the primitive CONTRACT (the API + the bug-class invariants the primitives must hold) and **name the implementation as a floor tied to the first frontend-bearing subsystem (M3+)**.
- **CONTRACTS TO IMPLEMENT.** No contract-index row owned; this is the design-system bug-class floor every frontend feature stands on.
- **GATE / DRILLS (quantified; must be green to call this done).** The overlay-primitive invariant tests pass (a picker never opens off-screen; a dialog is never clipped; focus is trapped) OR, if the impl is floored, the invariant CONTRACT is committed + the floor named. Green artifact = the invariant tests (or the committed invariant contract) passing.
- **TESTS (required).** Unit: the overlay primitives hold their invariants (a picker never opens off-screen; a dialog is never clipped; focus is trapped) OR a contract test asserts the invariant set exists. No CDC pair.
- **DEFINITION OF DONE.** The overlay primitives (or their floored contract) exist; the invariant tests pass (or the invariant contract is committed); any floored impl is named with its M3+ follow-on; committed.
- **COMMIT.** Header `P-<NNN> M0: shared overlay/state primitives (the bug-class floor)`. Body: the popover/dialog/picker/focus-trap primitives or their floored contract + the M3+ follow-on. Co-Authored-By trailer.

---

### P-S24 — Green the M0 exit gate: SUB-D1/D2/BUS-D4/D5/D7/D8/D9 + all twelve lints + the harness self-test

- **BAND.** M0.
- **ROADMAP MILESTONE.** SUB-M0 (the M0 exit gate — the consolidated band-boundary proof) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 exit gate + `../../06-roadmaps/00-master-sequencing.md` §2 M0→M1 row.
- **DEPENDS-ON.** P-S04, P-S07, P-S08, P-S10, P-S11, P-S13, P-S14, P-S17, P-S20, P-S21, P-S22.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §2 (the gate invariant — no later band done over a red earlier gate) and §3 (PROVEN not CLAIMED; observability is part of the pass condition; never weaken a threshold to make a check pass).
  - `../../06-roadmaps/00-master-sequencing.md` §2 M0 exit gate + §4 (the M0→M1 boundary row: SUB-D1/SUB-D2/BUS-D4 + all twelve lints + the harness self-test).
  - `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 exit gate (the full list incl. SUB-D5/D7/D8/D9) + §5 (the green-artifact rule).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 rows SUB-D1, SUB-D2, BUS-D4, SUB-D5, SUB-D7, SUB-D8, SUB-D9.
- **DELIVERABLE (what to build + exactly where in the repo).** A consolidated **M0 exit-gate scorecard** (a CI workflow + a committed scorecard artifact under the testing tree, e.g. `testing/scorecards/sub-m0.md`) that runs and records, each as a DATED green artifact reading the telemetry signal set (contract 1.8): SUB-D1 (0 ghost/0 lost), SUB-D2 (0 lost/no HoL), BUS-D4 (emit-iff-committed), SUB-D5 (fail-fast/Retry-After), SUB-D7 (misroute 0 + lint), SUB-D8 (depth ceiling + tripwire), SUB-D9 (readiness flips/no restart-storm), all twelve lints green with both fixtures, the contract-coverage scanner pass, and the harness self-test. This prompt does NOT re-implement the drills (they live with their feature prompts P-S07..P-S20); it WIRES them into one band-boundary gate, asserts each emits a dated green artifact, and records any red as a "claimed, not proven" thresholds-file row (never edited green). It is the build-layer realisation of the master M0→M1 gate invariant.
- **CONTRACTS TO IMPLEMENT.** None new — the gate-aggregation prompt. It MAKES THE M0 BAND BOUNDARY a single green/red signal (the permanent gates SUB-D1/D2 + BUS-D4 re-run on every emit-path change from here on).
- **GATE / DRILLS (quantified; must be green to call this done).** ALL of: SUB-D1, SUB-D2, BUS-D4, SUB-D5, SUB-D7, SUB-D8, SUB-D9 green (each with its quantified threshold + telemetry signal as above); 12/12 lints green with both fixtures; the contract-coverage scanner passes; the harness self-test passes. Green artifact = the dated `sub-m0` scorecard with every row PROVEN. A single red row blocks M1 (the gate invariant) — record it honestly, do not soften it.
- **TESTS (required).** The scorecard workflow itself is a committed CI gate. A meta-test: removing any drill from the scorecard or flipping any threshold green-without-proof fails the gate (the ratchet cannot be gamed). No CDC pair.
- **DEFINITION OF DONE.** The M0 exit-gate scorecard exists, is wired into CI, and every row emits a dated green artifact (or is honestly recorded as claimed-not-proven, blocking M1); the permanent gates SUB-D1/D2/BUS-D4 are marked re-run-forever; committed.
- **COMMIT.** Header `P-<NNN> M0: M0 exit-gate scorecard (SUB-D1/D2/BUS-D4/D5/D7/D8/D9 + 12 lints + harness self-test)`. Body: each drill greened with its measured number; the twelve lints; the scanner pass; the permanent-gate marking. Co-Authored-By trailer.

---

### P-S25 — Prove fail-static against a real Identity hiccup (SUB-D4)

- **BAND.** M1.
- **ROADMAP MILESTONE.** SUB-M1 (fail-static proven) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M1 (the fail-static-proven slice).
- **DEPENDS-ON.** P-S18 (the `FailStatic` mechanism), P-S24 (M0 green), and the Identity authz-client prompts that ship `authenticate`/`check`/`list_objects`/zookie (Identity's M1 ledger — the substrate wires `FailStatic` into Identity's read path; this prompt MUST start after Identity's authz client is mergeable).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (prove-it — a property is not real until a drill forces the failure and observability watches the system survive) and §2 (a shared-dependency cascade kills the platform — fail-static, not fail-closed, is the availability default).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §8 (fail-static — full), §8.2 (the staleness bound `static_max ≤ revocation-SLA ≥ agent-token-TTL`; the zookie bypass — a security-sensitive read passes the zookie so it BYPASSES the fail-static cache), §8.3 (interaction with readiness — fail-static handles a TRANSIENT hiccup; readiness handles a SUSTAINED outage).
  - `../../05-refined-shared-systems-architecture/contract-index.md` rows 1.10 (`FailStatic<T>`), 4.10 (`Consistency`/zookie — zookie-stamped reads bypass the cache; the authz reverse index honours the zookie revision watermark), 4.11 (the Id-usage bound; `[OPEN — LEGAL]` value W).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 row SUB-D4 (Id-hiccup → already-authenticated survives within W; revoked denied when window closes).
- **DELIVERABLE (what to build + exactly where in the repo).** In `myelin-substrate` (and the thin authz-client wiring in `myelin-identity`'s consumer surface): wire the M0 `FailStatic<T>` mechanism into the Identity authz read path so that on a transient Identity hiccup, already-authenticated traffic is served the coarse "actor still active / coarse grants" answer within `static_max` (never an escalation of access — never fail open). Enforce the **zookie bypass**: a security-sensitive read carrying a zookie BYPASSES the fail-static cache (contract 4.10) and forces a fresh read past the watermark. Read the value W from the thresholds file (`[OPEN — LEGAL]` placeholder; the mechanism + constraint ship regardless). Export the fresh/stale/closed ratios into the telemetry signal set. **Floor named:** the value W remains `[OPEN — LEGAL]` (DPO-ratified) — the mechanism is proven here regardless of the final number.
- **CONTRACTS TO IMPLEMENT.** Consumed: 4.10 (the zookie bypass + watermark — Identity owns the index; the substrate wires the bypass), 4.11 (the bound). Owned (proven here): 1.10 against a real dependency.
- **GATE / DRILLS (quantified; must be green to call this done).** **SUB-D4** (inject a transient Identity hiccup → already-authenticated traffic survives on the coarse cache within W; a revoked actor is denied once the window closes; an agent token expires inside the window; a zookie-stamped read bypasses the cache) — signals: fail-static fresh/stale/closed ratios read green, staleness never exceeds `static_max ≤ revocation-SLA`, `revoked_after_window_denied == true`, CI. This contributes to the master M1→M2 boundary (mirror of Identity's ID-D2). Never weaken W to pass — if the chosen W is unproven, record it claimed-not-proven.
- **TESTS (required).** Unit: within W a hiccup serves stale + degraded (never open); past W it fails closed; a zookie read bypasses the cache; a revoked actor is denied once the window closes. CDC pair for 1.10 against the Identity authz client (provider = Identity hiccuping; consumer = the substrate cache). Drill scenario for SUB-D4 (the P-S03 injector injects an Identity hiccup; the P-S04 assertions read the survival signals). Tests CHAIN: authenticate → hiccup → serve-stale → revoke → window-closes → deny (the sequence property, EI-01 §4). Mutation floor ≥ 80% on the fail-static-wiring module.
- **DEFINITION OF DONE.** Fail-static is wired into the Identity read path with the zookie bypass; SUB-D4 emits a dated green artifact; the value-W `[OPEN — LEGAL]` floor is named; CDC + chained + mutation tests pass; committed.
- **COMMIT.** Header `P-<NNN> M1: fail-static proven against a real Identity hiccup (SUB-D4)`. Body: 1.10/4.10/4.11; SUB-D4 (survives within W, revoked denied at window close) with measured numbers; the value-W `[OPEN — LEGAL]` follow-on. Co-Authored-By trailer.

---

### P-S26 — The restore-verify cross-seam half (SUB-D6, the silent-data-loss floor, with Storage)

- **BAND.** M1.
- **ROADMAP MILESTONE.** SUB-M1 (the restore-verify cross-seam half) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M1 (the restore-verify slice).
- **DEPENDS-ON.** P-S03, P-S04 (the injector/assertion machinery), P-S24 (M0 green), and the Storage M1 prompts that ship the backup/restore + WAL/PITR + the blob/index/offset seams (Storage's M1 ledger — the substrate supplies the injection/assertion half; this prompt MUST start after Storage's backup/restore is mergeable).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §2 (silent data loss outranks every feature — the restore-verify gate is a CI job, not an aspiration) and §3 (RPO/RTO are quantified thresholds; a target you cannot measure is not a gate).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §11 row D-6 (restore + cross-seam integrity — rebuild from backups; OLTP rows ↔ blob ↔ search index ↔ event-log offsets restore to one consistent point, no row → missing blob) + §9.2 (measure lock time against a restored copy — ties to the restore machinery).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 11.5 (backup/restore/cross-seam integrity — Storage owns the restore-verify CI job; the substrate owns the failure-injection + telemetry half). Read `00-reconciliation-decisions.md` for STOR-4 / ADR-18.
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 row SUB-D6 (= STOR-D1/STOR-D2: rebuild from backups → no loss; one consistent cross-seam point; RPO ≤ 5 min, RTO ≤ 1h/tenant ≤ 4h/cell).
- **DELIVERABLE (what to build + exactly where in the repo).** In `myelin-harness` + `myelin-substrate`: the substrate's half of the restore-verify gate — the **failure-injection + telemetry-assertion machinery** that drives SUB-D6: trigger a rebuild-from-backups via Storage's restore path, then assert (via the telemetry-assertion library) that the rebuild lands at ONE consistent cross-seam point — OLTP rows ↔ blob ↔ search index ↔ event-log offsets (no row pointing at a missing blob), 0 loss, and that RPO ≤ 5 min / RTO ≤ 1h-per-tenant / ≤ 4h-per-cell (thresholds read from the thresholds file). Storage owns the WAL+PITR restore and the `restore-verify` CI job; the substrate supplies the drill machinery that makes it PROVABLE. (The exhaustive-holder confirmation against the real H1–H18 set is split to P-S27.)
- **CONTRACTS TO IMPLEMENT.** Carried (substrate half): 11.5 (the injection/assertion half — Storage owns the gate).
- **GATE / DRILLS (quantified; must be green to call this done).** **SUB-D6 / STOR-D1 / STOR-D2** (rebuild from backups → **0 loss**; OLTP↔blob↔index↔offsets ONE consistent point; **RPO ≤ 5 min, RTO ≤ 1h/tenant ≤ 4h/cell**) — telemetry signal `restore-verify-pass`, SCHED. **This is the silent-data-loss floor and a PERMANENT gate (re-run on every store-touching change); M2 does NOT start over a red STOR-D1.** Never weaken RPO/RTO to pass — a red row is a dated claimed-not-proven scorecard entry.
- **TESTS (required).** The restore-verify drill scenario (the substrate's injection/assertion half driving Storage's restore). Unit: the cross-seam consistency assertion catches a deliberately-injected row→missing-blob mismatch (the assertion must reject an inconsistent rebuild). CDC pair for 11.5 (provider = Storage restore; consumer = the substrate assertion). This is a SCHED drill (expensive) — wired into the scheduled gate, with a cheaper CI smoke variant at small scale.
- **DEFINITION OF DONE.** The restore-verify injection/assertion half exists; SUB-D6/STOR-D1/STOR-D2 emit dated green artifacts at single-tenant scale (cell-scale re-confirm is the M5 follow-on, P-S35); the permanent-gate marking + the M5 cell-scale follow-on are named; committed.
- **COMMIT.** Header `P-<NNN> M1: restore-verify cross-seam half (SUB-D6, the silent-data-loss floor)`. Body: 11.5 (substrate half); SUB-D6/STOR-D1/STOR-D2 (0 loss, one cross-seam point, RPO/RTO) with measured numbers; the permanent-gate marking + the M5 cell-scale follow-on (P-S35). Co-Authored-By trailer.

---

### P-S27 — The exhaustive `PersonalDataHolder` (H1–H18) confirmation

- **BAND.** M1.
- **ROADMAP MILESTONE.** SUB-M1 (the exhaustive-holder confirmation) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M1 (the exhaustive-holder slice; contract 1.4 follow-on).
- **DEPENDS-ON.** P-S15 (the M0 holder auto-registration mechanism), P-S11 (the `no-untagged-personal-data` lint), and the GDPR M1 holder-list prompt + the Identity/Storage M1 store prompts (the real H1–H18 holders come online here).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §2 (silent data loss / a forgotten store is a GDPR + data-loss hole) and §5 (the lint goes red — loud, never swallowed).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §3.4 (`PersonalDataHolder` auto-registration — the M1 exhaustive H1–H18 list; the mechanism makes "we forgot a store" structurally impossible).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.4 (holder auto-registration; the exhaustive list is GDPR's M1 deliverable) + 10.1 (the `PersonalDataHolder` trait + the H1–H18 list).
- **DELIVERABLE (what to build + exactly where in the repo).** In `myelin-substrate` + `myelin-harness`: **confirm** the M0 `PersonalDataHolder` auto-registration mechanism (P-S15) is exercised against the REAL H1–H18 holder set as Identity/Storage/GDPR stores come online — every store the harness opens is in the H1–H18 set (no orphan store, no store outside the list). The `no-untagged-personal-data` lint goes red on any untagged PII field (the GA-D5 mirror). GDPR owns the exhaustive H1–H18 list itself; the substrate confirms the mechanism catches every one and that no store escapes registration.
- **CONTRACTS TO IMPLEMENT.** Consumed/confirmed: 1.4 (the exhaustive-holder mechanism against the real H1–H18 set). Carried: 10.1 (the trait — GDPR owns it; the substrate confirms registration completeness).
- **GATE / DRILLS (quantified; must be green to call this done).** A **holder-completeness assertion**: every store the harness opens is in the H1–H18 set (no orphan store); the `no-untagged-personal-data` lint is green across the real schema set. Green artifact = the holder-completeness test passing + the lint green on the real H1–H18 holders.
- **TESTS (required).** A holder-completeness test: every store the harness opens is in the H1–H18 set (no orphan store); a deliberately-orphaned store (a store opened without registration) fails the test. The `no-untagged-personal-data` lint is run across the real schema set (a deliberately-untagged PII field fails the build). CDC: confirm 1.4 against the real holders (provider = each store; consumer = the DSR fan-out). No new mutation floor (this is a confirmation prompt).
- **DEFINITION OF DONE.** The exhaustive-holder mechanism is confirmed against the real H1–H18 set; no orphan store exists; the `no-untagged-personal-data` lint is green on the real schema; committed.
- **COMMIT.** Header `P-<NNN> M1: exhaustive PersonalDataHolder (H1-H18) confirmation`. Body: 1.4 confirmation against the real holder set; the holder-completeness assertion; the `no-untagged-personal-data` lint green. Co-Authored-By trailer.

---

### P-S28 — The firehose per-connection in-flight frame caps + slow-consumer drop to `resync_required`

- **BAND.** M2.
- **ROADMAP MILESTONE.** SUB-M2 (the firehose backpressure half — the bounded-frame-caps + slow-consumer-drop slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M2.
- **DEPENDS-ON.** P-S19 (bounded-everything + the shed order), P-S26 (M1 green), and the Bus M2 prompt that ships the firehose resume-cursor protocol (contract 3.5 — the Bus owns `subscribe`/`resume`/`scope` + the zero-loss-replay half; this prompt MUST start after the Bus firehose protocol is mergeable).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/04-hard-problems.md` §2 (build the durable resume-cursor transport FIRST — a dropped connection must lose nothing).
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (zero messages lost across a reconnect is a quantified gate).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §7.7 (the firehose resume-cursor seam — per-connection in-flight frame caps; a slow consumer dropped to `resync_required` never buffered unboundedly) + §7.1 (bounded-everything generalised to streaming) + §10.2 last row (firehose per-(stream,scope) frame-lag + `resync_required` count survival signal).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 3.5 (the firehose transport + resume-cursor protocol — `subscribe`/`resume`; `resync_required` → `*.snapshot` fallback).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §11 row D-11 (firehose reconnect-loses-zero-ops — the substrate owns the bounded-queue half).
- **DELIVERABLE (what to build + exactly where in the repo).** In `myelin-substrate` (the firehose backpressure layer that rides the Bus's 3.5 protocol): (a) **per-connection in-flight frame caps** — a subscription's frame buffer is bounded; over-cap sheds in the firehose's own bounded queue; (b) **slow-consumer drop to `resync_required`** — a slow consumer falls back to a full `*.snapshot` replay (the cold-rebuild path, NAMED not silent) rather than the transport growing memory. Export the per-(stream,scope) frame-lag + `resync_required` count into the telemetry signal set. The Bus owns the zero-loss-replay half (3.5); the substrate owns the bounded-and-sheds half. (The scope-bounded selector + per-surface frame shed budgets are split to P-S29.)
- **CONTRACTS TO IMPLEMENT.** Carried (substrate half): 3.5 (the per-connection caps + slow-consumer→`resync_required` slice).
- **GATE / DRILLS (quantified; must be green to call this done).** Under a hot-stream drill: per-(stream,scope) frame-lag bounded; a slow consumer is dropped to `resync_required` (not buffered unboundedly, memory stays bounded) — signals `firehose_frame_lag` bounded, `resync_required_count` accurate, memory bounded, CI. This is part of the substrate's half of the D-11 reconnect-loses-zero-ops drill (the Bus owns the zero-loss-replay assertion). Re-confirmed under real connection-storm load in M4 (P-S31).
- **TESTS (required).** Unit: an over-cap subscription sheds rather than growing memory; a slow consumer is dropped to `resync_required` (memory stays bounded). CDC pair for the substrate half of 3.5 (provider = the Bus firehose; consumer = the substrate bounded layer). Drill scenario for the D-11 substrate half (the P-S03 injector drops a firehose subscription on a hot stream). Mutation floor ≥ 75% on the frame-buffer module.
- **DEFINITION OF DONE.** The per-connection frame caps + slow-consumer drop exist and compile; the firehose frame-lag + `resync_required` survival signals read green under a hot-stream drill; CDC + mutation tests pass; the M4 connection-storm re-confirm (P-S31) is named; committed.
- **COMMIT.** Header `P-<NNN> M2: firehose per-connection frame caps + slow-consumer drop to resync_required`. Body: 3.5 (this slice); the bounded/shed firehose drill greened with measured frame-lag + `resync_required`; the M4 follow-on (P-S31). Co-Authored-By trailer.

---

### P-S29 — The firehose scope-bounded selector + the per-surface frame shed budgets (D-11 substrate half complete)

- **BAND.** M2.
- **ROADMAP MILESTONE.** SUB-M2 (the firehose backpressure half — the scope-bounded-selector + frame-shed-budget slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M2.
- **DEPENDS-ON.** P-S28 (the per-connection caps + slow-consumer drop), P-S19 (the per-surface shed budgets).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (zero ops lost; named-not-silent fallbacks).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §7.7 (scope as a bounded selector never `*` — a 50k-row board paginates its scope; the per-surface shed budgets apply to frames — presence/speculative frames shed before message delivery, agents shed before humans) + §7.6 (the per-surface budgets).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 3.5 (scope a bounded selector never `*`; board:/doc:/channel:).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §11 row D-11 (the firehose bounded-queue half).
- **DELIVERABLE (what to build + exactly where in the repo).** In `myelin-substrate`: (a) **scope as a bounded selector, never `*`** — a 50k-row board paginates its scope (visible window + margin); the firehose delivers only that slice's frames; a `*` scope is rejected (bounded selector only, board:/doc:/channel:). (b) the **per-surface shed budgets** (§7.6) applied to frames — presence/speculative frames shed before message delivery; agents shed before humans. Together with P-S28 this completes the substrate's half of D-11 (bounded-and-sheds-never-unbounded-memory).
- **CONTRACTS TO IMPLEMENT.** Carried (substrate half): 3.5 (the scope-bounded-selector + frame-shed-budget slice — completes the substrate's half of the firehose contract).
- **GATE / DRILLS (quantified; must be green to call this done).** Under a hot-stream drill: a `*` scope is rejected; presence/speculative frames shed before message frames; the per-surface frame budgets hold — signals `shed-counts/lane` (frame budgets), `firehose_frame_lag` bounded, CI. This completes the substrate's half of the D-11 reconnect-loses-zero-ops drill. Re-confirmed under real connection-storm load in M4 (P-S31).
- **TESTS (required).** Unit: a `*` scope is rejected (bounded selector only); a 50k-row board delivers only its paginated slice's frames; presence frames shed before message frames. CDC pair for the scope-bounded-selector slice of 3.5. Drill scenario completing the D-11 substrate half (frame-budget shedding on a hot stream). Mutation floor ≥ 75% on the scope-selector + frame-shed-budget module.
- **DEFINITION OF DONE.** The scope-bounded selector + the per-surface frame shed budgets exist and compile; the D-11 substrate half is now complete and proven; CDC + mutation tests pass; the M4 connection-storm re-confirm (P-S31) is named; committed.
- **COMMIT.** Header `P-<NNN> M2: firehose scope-bounded selector + per-surface frame shed budgets (D-11 substrate half complete)`. Body: 3.5 (this slice); the scope-bounded + frame-budget drill greened; the M4 follow-on (P-S31). Co-Authored-By trailer.

---

### P-S30 — Enforce the cross-language harness shim (if Chat diverges)

- **BAND.** M4.
- **ROADMAP MILESTONE.** SUB-M4 (the cross-language shim enforced slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M4.
- **DEPENDS-ON.** P-S29 (the firehose half live), and the Chat M4 prompts (the connection tier — if Chat diverges to a non-Rust tier, TE-21). This prompt MUST start after Chat's connection tier is mergeable.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §5 (an uncommitted gate is no gate — the shim cannot be quietly dropped at a language boundary) and §7 (reconcile cross-component contracts at the plan layer; a non-negotiable dropped at a boundary calcifies).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §3.7 (the cross-language harness shim — the frozen divergence contract; the SEVEN non-negotiables a non-Rust subsystem must satisfy: three-surface topology, liveness ≠ readiness, no fire-and-forget emit, `PersonalDataHolder` registration, the resilient-client behaviour + `Retry-After`, the principal-aware shed order, forward-only online migrations).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.7 (the cross-language harness shim — the contract a non-Rust subsystem must satisfy; Chat connection tier likely, TE-21).
- **DELIVERABLE (what to build + exactly where in the repo).** In `myelin-harness`: **enforce the cross-language harness shim** IF AND ONLY IF Chat diverges to a non-Rust connection tier — a conformance test suite that asserts the divergent tier provides all SEVEN non-negotiables to the SAME guarantee as the Rust harness: (1) three-surface topology, (2) liveness ≠ readiness, (3) no fire-and-forget emit (the outbox pattern in the divergent language too), (4) `PersonalDataHolder` registration, (5) the resilient-client behaviour + `Retry-After` honouring, (6) the principal-aware shed order, (7) forward-only online migrations. A no-op (a recorded N/A) if Chat stays Rust — but the shim CANNOT be quietly dropped; an N/A is recorded LOUDLY (silent skip is the failure, EI-01 §4). **Floor named:** the shim is `specified` from M0 (P-S12..P-S19/§3.7) → `enforced` here iff Chat diverges (roadmap §3 floor table).
- **CONTRACTS TO IMPLEMENT.** Carried/enforced: 1.7 (the cross-language shim — enforced here iff Chat diverged).
- **GATE / DRILLS (quantified; must be green to call this done).** **The cross-language shim's seven non-negotiables green** in the divergent tier (only if Chat diverged) — signal `shim-conformance` green ×7, CI. If Chat stays Rust, the gate is a loudly-recorded N/A (a committed, dated N/A row), not a silent skip. Contributes to the master M4→M5 boundary.
- **TESTS (required).** The shim conformance suite (seven non-negotiables, each a committed test; an N/A recorded loudly if Chat stays Rust). CDC pair for 1.7 (if enforced). Mutation floor ≥ 75% on any new shim-enforcement code.
- **DEFINITION OF DONE.** The shim is enforced (seven non-negotiables green) or recorded N/A loudly; the specified→enforced floor is named; tests pass; committed.
- **COMMIT.** Header `P-<NNN> M4: cross-language harness shim enforced (or N/A recorded loudly)`. Body: 1.7 (seven non-negotiables or N/A); the specified→enforced floor. Co-Authored-By trailer.

---

### P-S31 — The firehose backpressure half under connection-storm

- **BAND.** M4.
- **ROADMAP MILESTONE.** SUB-M4 (the firehose under connection-storm slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M4.
- **DEPENDS-ON.** P-S28, P-S29 (the firehose backpressure half), P-S02 (the connection-storm + collab-op-stream storm profiles), and the Chat M4 prompts (the connection tier) + the Knowledge M3 collab prompts (the hot-doc op-stream). This prompt MUST start after Chat's connection tier and KN's collab stream are mergeable.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (the bounded/shed half holds under real load, not just unit scale).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §7.6 (the connection-storm + collab op-stream budgets) + §7.7 (the firehose backpressure role).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 3.5 (the firehose under hot-stream load).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2/§4.3 rows for CHAT-D1/CHAT-D13/CHAT-D14 (the firehose path: resume 0 lost/0 dup; co-commit; idempotent send) — the substrate asserts its survival signals hold under these.
- **DELIVERABLE (what to build + exactly where in the repo).** In `myelin-substrate` + `myelin-harness`: **re-confirm the firehose backpressure half (P-S28 + P-S29) under connection-storm** — drive Chat's connection-storm budget and KN's hot-doc collab op-stream budget (§7.6 floors), using the P-S02 connection-storm + collab-op-stream storm profiles. The substrate asserts its survival signals (`shed-counts/lane`, `firehose_frame_lag` bounded, `resync_required_count`) hold under the real load; Chat owns the end-to-end resume-0-lost/0-dup + co-commit + idempotent-send drill.
- **CONTRACTS TO IMPLEMENT.** Carried: 3.5 (the substrate's bounded/shed half re-confirmed under connection-storm).
- **GATE / DRILLS (quantified; must be green to call this done).** **The firehose under connection-storm**: the bounded/shed half holds under Chat's connection-storm + KN's hot-doc budgets — signals `shed-counts/lane`, `firehose_frame_lag` bounded, contributing to CHAT-D1/CHAT-D13/CHAT-D14 (the substrate asserts its survival signals; Chat owns the end-to-end drill), CI. Contributes to the master M4→M5 boundary.
- **TESTS (required).** The connection-storm drill scenario (the P-S02 generator drives the connection-storm + collab-op-stream profiles; the P-S04 assertions read the survival signals). CDC pair for 3.5 under storm load. Mutation floor ≥ 75% on any new storm-handling code.
- **DEFINITION OF DONE.** The firehose half holds under connection-storm (survival signals green, contributing to CHAT-D1/D13/D14); tests pass; committed.
- **COMMIT.** Header `P-<NNN> M4: firehose backpressure half under connection-storm`. Body: 3.5 connection-storm; the survival signals greened (contributing to CHAT-D1/D13/D14). Co-Authored-By trailer.

---

### P-S32 — World-scale: the 30× surge family (SUB-D3)

- **BAND.** M5.
- **ROADMAP MILESTONE.** SUB-M5 (the surge family — the SUB-D3 slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M5.
- **DEPENDS-ON.** P-S19 (the shed lane), P-S20 (the agent-load caps), P-S31 (M4 green). The F6 surge family runs across all owners; the substrate is one owner — this prompt MUST start after M4 (all five subsystems on the substrate).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (the load generator multiplies traffic 1×/10×/30× and mixes principal types; the human lane within budget is the quantified pass) and §2 (per-tenant blast radius — one tenant's surge unaffects another).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §7.2 (the shed order + the protected human lane) + §11 row D-3.
  - `../../05-refined-shared-systems-architecture/contract-index.md` rows 1.11 (the shed order) + 1.8 (the surge survival signals — shed-counts/lane, per-tenant RED).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 row SUB-D3 (30× agent surge one tenant → human lane holds, agent sheds, others unaffected).
- **DELIVERABLE (what to build + exactly where in the repo).** Drive the **30× surge family** (SUB-D3, part of the F6 family across all owners) using the P-S02 load generator at 30× on one tenant with the agent/CI mix: assert the **protected human lane holds** (within its latency budget), the **agent lane sheds** (`429 + Retry-After`, and the clients honour it — §6.2), and **cross-tenant impact is 0** (another tenant's humans are unaffected). This proves the shed order + the human lane at world scale. (Tuning the §7.6 budget NUMBERS to measured values is split to P-S33.)
- **CONTRACTS TO IMPLEMENT.** Owned (proven at scale): 1.11 (the shed order). Consumed: 1.8 (the surge survival signals).
- **GATE / DRILLS (quantified; must be green to call this done).** **SUB-D3** (30× agent surge one tenant → human lane within budget, agent sheds with `429 + Retry-After`, cross-tenant impact **0**) — signals `shed-counts/lane`, per-tenant RED (the surged tenant's agents shed, its humans hold, another tenant's RED unchanged), SCHED. Contributes to the master M5→M6 boundary (the F6 surge family). Never relax the human-lane budget to pass — record a missed budget as claimed-not-proven.
- **TESTS (required).** The SUB-D3 drill scenario (the P-S02 generator at 30×, agent/CI mix, one tenant). Assert the three properties (human lane holds, agent sheds, cross-tenant 0) read from the telemetry signal set. SCHED frequency; a cheaper 10× CI smoke variant.
- **DEFINITION OF DONE.** SUB-D3 emits a dated green artifact (human lane holds, agent sheds, cross-tenant 0); the budget-tuning follow-on (P-S33) is named; committed.
- **COMMIT.** Header `P-<NNN> M5: 30x surge family (SUB-D3)`. Body: 1.11 proven at scale; SUB-D3 (human lane holds, agent sheds, cross-tenant 0) with measured numbers; the budget-tuning follow-on (P-S33). Co-Authored-By trailer.

---

### P-S33 — Tune the per-surface shed budgets to measured numbers

- **BAND.** M5.
- **ROADMAP MILESTONE.** SUB-M5 (the per-surface shed-budget tuning slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M5.
- **DEPENDS-ON.** P-S32 (the SUB-D3 surge results), P-S31 (the connection-storm results), P-S22 (the thresholds file the tuned numbers are written into), P-S19 (the v1 budget floor table).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (measured-not-predicted — the v1 floors become measured numbers; never edited green without the drill).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §7.6 (the per-surface budget v1 floor table — now TUNED by the drills).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.11 (the per-surface shed budgets — the v1 floors become measured numbers).
- **DELIVERABLE (what to build + exactly where in the repo).** **Tune the §7.6 per-surface shed-budget numbers** against the measured SUB-D3 (P-S32) + connection-storm (P-S31) drill results — the v1 floor table becomes measured numbers, written into the thresholds file (P-S22) as a dated update (the named floor is now a measured value; never edited green without the drill). The floor DISCIPLINE (bounded + reserved human lane + shed order) is the unchanged contract; only the NUMBERS are tuned.
- **CONTRACTS TO IMPLEMENT.** Owned (tuned): 1.11 (the shed budgets, now measured).
- **GATE / DRILLS (quantified; must be green to call this done).** The per-surface shed-budget numbers are written into the thresholds file as dated measured values, each backed by a SUB-D3 / connection-storm drill result. Green artifact = the dated thresholds-file update + a regression test that re-running SUB-D3 with the tuned numbers still holds the human lane.
- **TESTS (required).** A regression test that a budget tuned BELOW the human-lane floor fails the gate (you cannot tune the human lane into starvation). A regression test that the tuned numbers, re-driven through SUB-D3, still hold the three properties. The thresholds-file update round-trips.
- **DEFINITION OF DONE.** The per-surface shed-budget numbers are tuned to measured values in the thresholds file; the human-lane-starvation regression holds; the v1-floor→measured follow-on is closed; committed.
- **COMMIT.** Header `P-<NNN> M5: tuned per-surface shed budgets`. Body: 1.11 tuned; the measured budget values written into the thresholds file; the human-lane-starvation regression. Co-Authored-By trailer.

---

### P-S34 — World-scale: online-migration-under-load (SUB-D10)

- **BAND.** M5.
- **ROADMAP MILESTONE.** SUB-M5 (the online-migration-under-load slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M5.
- **DEPENDS-ON.** P-S15 (the migration runner), P-S26 (the restore-verify machinery the migration runs against), P-S32 (M5 surge, same band — start after the surge proof).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (expensive drills run scheduled; the quantified thresholds — lock-wait p99, 0 errored writes, 0 downtime).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §9 (forward-only online migrations; §9.1 expand→backfill→contract; §9.2 measure lock time against a restored production-scale copy — the lock-time-against-a-restore rule ties the migration runner to the restore machinery) + §11 row D-10.
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.5 (forward-only migrations under load).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 row SUB-D10 (expand→backfill→contract on a restored prod-scale copy under load → no blocking lock beyond budget; 0 downtime).
- **DELIVERABLE (what to build + exactly where in the repo).** In `myelin-substrate` + `myelin-harness`: **online-migration-under-load** (SUB-D10): run an expand→backfill→contract migration on a **restored production-scale copy under load** (using the P-S26 restore machinery + the P-S02 load generator); assert no blocking lock beyond budget (lock-wait p99 from the thresholds file) and 0 errored writes / 0 downtime. This ties the migration runner (1.5) to the restore-verify machinery (the §9.2 lock-time-against-a-restore rule).
- **CONTRACTS TO IMPLEMENT.** Owned/proven: 1.5 (under load).
- **GATE / DRILLS (quantified; must be green to call this done).** **SUB-D10** (expand→backfill→contract on a restored prod-scale copy under load → lock-wait p99 within budget, **0 errored writes, 0 downtime**) — SCHED. Contributes to the master M5→M6 boundary. Never weaken the lock-wait budget to pass — a red row is a dated claimed-not-proven entry.
- **TESTS (required).** The SUB-D10 drill scenario (migration on a restored copy under load; the P-S02 generator drives the load; the P-S04 assertions read lock-wait p99 + errored-write + downtime signals). SCHED frequency; a cheaper CI smoke variant where feasible.
- **DEFINITION OF DONE.** SUB-D10 emits a dated green artifact (lock-wait p99 within budget, 0 errored writes, 0 downtime); committed.
- **COMMIT.** Header `P-<NNN> M5: online-migration-under-load (SUB-D10)`. Body: 1.5 under load; SUB-D10 (lock-wait p99, 0 downtime) with measured numbers. Co-Authored-By trailer.

---

### P-S35 — World-scale: restore-verify re-confirmed at cell scale (SUB-D6 / STOR-D2)

- **BAND.** M5.
- **ROADMAP MILESTONE.** SUB-M5 (the restore-verify-at-cell-scale slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M5.
- **DEPENDS-ON.** P-S26 (the restore-verify half — the cell-scale follow-on named there), P-S34 (M5 migration, same band).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (expensive drills run scheduled; RPO/RTO are quantified thresholds).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §11 row D-6 (restore + cross-seam integrity at scale) + §9.2 (the lock-time-against-a-restore tie).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 11.5 (restore-verify at cell scale).
  - `../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 rows SUB-D6 / STOR-D2 (restore-verify at cell scale; RPO ≤ 5 min, RTO ≤ 1h/tenant ≤ 4h/cell under world-scale load).
- **DELIVERABLE (what to build + exactly where in the repo).** In `myelin-substrate` + `myelin-harness`: **restore-verify re-confirmed at cell scale** (SUB-D6 / STOR-D2) — re-drive the P-S26 restore-verify machinery against a cell-scale restored copy under world-scale load (the P-S02 generator), asserting RPO ≤ 5 min / RTO ≤ 1h-per-tenant / ≤ 4h-per-cell hold (thresholds from the thresholds file). This closes the cell-scale follow-on named in P-S26.
- **CONTRACTS TO IMPLEMENT.** Owned/proven: 11.5 (at cell scale).
- **GATE / DRILLS (quantified; must be green to call this done).** **SUB-D6 / STOR-D2 at cell scale** (RPO/RTO held under world-scale load) — telemetry signal `restore-verify-pass at scale`, SCHED. **Permanent gate (re-run on every store-touching change).** Contributes to the master M5→M6 boundary. Never weaken RPO/RTO to pass.
- **TESTS (required).** The cell-scale restore-verify drill (SUB-D6/STOR-D2 at scale; the P-S02 generator drives the load; the P-S04 assertions read the cross-seam + RPO/RTO signals). SCHED frequency; a cheaper CI smoke variant.
- **DEFINITION OF DONE.** SUB-D6/STOR-D2 at cell scale emit a dated green artifact (RPO/RTO held under world-scale load); the cell-scale floor named in P-S26 is closed; the permanent-gate marking is named; committed.
- **COMMIT.** Header `P-<NNN> M5: restore-verify at cell scale (SUB-D6 / STOR-D2)`. Body: 11.5 at cell scale; SUB-D6/STOR-D2 (RPO/RTO) with measured numbers; the permanent-gate marking. Co-Authored-By trailer.

---

### P-S36 — Tune the resilient-client per-target values to measured numbers

- **BAND.** M5.
- **ROADMAP MILESTONE.** SUB-M5 (the resilient-client per-target tuning slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M5.
- **DEPENDS-ON.** P-S16 (the resilient client + its default per-target floor), P-S17 (`Retry-After`), P-S32 (the surge/latency drill results the tuning measures against), P-S22 (the thresholds file the tuned numbers are written into).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §3 (measured-not-predicted — the auth hot path tighter than a batch indexer, measured by the surge/latency drills).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §6.3 (the resilient-client per-target values — the auth hot path tighter than a batch indexer, measured by the surge/latency drills).
  - `../../05-refined-shared-systems-architecture/contract-index.md` row 1.9 (resilient-client per-target tuning).
- **DELIVERABLE (what to build + exactly where in the repo).** In `myelin-client` + the thresholds file: **tune the resilient-client per-target values** (1.9) — the auth hot path gets a tighter timeout than a batch indexer — measured by the surge/latency drills (P-S32, the F6 family), written into the thresholds file (the M0 default-per-target floor, P-S16, becomes measured values; the follow-on named in P-S16 is closed here). The shape + on-by-default posture are unchanged; only the per-target NUMBERS are tuned.
- **CONTRACTS TO IMPLEMENT.** Owned (per-target tuned): 1.9.
- **GATE / DRILLS (quantified; must be green to call this done).** The per-target values are written into the thresholds file as dated measured numbers, each backed by a surge/latency drill result (e.g. the auth hot path's timeout measured tighter than a batch indexer's). Green artifact = the dated thresholds-file update + a regression test that a per-target value tuned looser than the measured latency budget fails the gate.
- **TESTS (required).** A regression test that a per-target value tuned looser than the measured latency budget fails the gate. The thresholds-file update round-trips. The auth-hot-path-tighter-than-batch-indexer relation holds.
- **DEFINITION OF DONE.** The resilient-client per-target values are tuned to measured numbers in the thresholds file; the looser-than-budget regression holds; the M0 default-per-target floor (P-S16) is closed; committed.
- **COMMIT.** Header `P-<NNN> M5: resilient-client per-target tuning`. Body: 1.9 per-target tuned; the measured per-target values; the looser-than-budget regression. Co-Authored-By trailer.

---

### P-S37 — Dogfood: run the lints, the contract-coverage scanner, and the mutation gate as Myelin CI jobs

- **BAND.** M6.
- **ROADMAP MILESTONE.** SUB-M6 (the dogfood loop — the ratchet-as-CI-jobs slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M6.
- **DEPENDS-ON.** P-S10, P-S11 (the lints), P-S21 (the scanner), P-S07/P-S08/P-S16/P-S18/P-S19/P-S20 (the mandatory-core mutation floors), and the CI subsystem M4 prompts + the M6 self-hosting prompts (the Myelin CI graph the substrate jobs run on). This prompt MUST start after the self-hosting CI graph is mergeable.
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §5 (the ratchet runs on the builders' own work).
  - `../../05-refined-shared-systems-architecture/00-platform-substrate.md` §2.11 (the twelve lints) + §10 (the telemetry baseline — the dogfood loop reads these on Myelin's own commits).
  - `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M6 (the lints + scanner + mandatory-core mutation gate run as Myelin CI jobs on every Myelin commit; the harness drives the surge/restore/migration drills as part of the self-hosting CI graph).
  - `../00-ledger-overview.md` §6 (the cargo-mutants mutation gate runs as a Myelin CI job on every Myelin commit).
- **DELIVERABLE (what to build + exactly where in the repo).** Wire the substrate's ratchet into the self-hosting Myelin CI graph (a Myelin CI pipeline definition): the twelve architecture lints (P-S10/P-S11) + the contract-coverage scanner (P-S21) + the mandatory-core cargo-mutants mutation gate run as **Myelin CI jobs on every Myelin commit** (the dogfood loop is live). Make the harness drive the substrate's surge/restore/migration drills (SUB-D3/D6/D10) as part of the self-hosting CI graph. (The incident-loop + the truth-up pass are split to P-S38.)
- **CONTRACTS TO IMPLEMENT.** None new — this wires the M0 ratchet (1.6, the scanner, the mutation gate) to run on Myelin's own commits.
- **GATE / DRILLS (quantified; must be green to call this done).** **The Myelin self-hosting CI graph is green** on the platform's own commits (the twelve lints + the scanner + the mutation gate run there). Green artifact = a Myelin CI run on a Myelin commit showing the substrate ratchet green. Contributes to the master M6 done-bar.
- **TESTS (required).** The self-hosting CI pipeline IS the test: a Myelin commit triggers the lints + scanner + mutation gate; a deliberately-violating commit is rejected (the ratchet rejects on Myelin's own work). No CDC pair.
- **DEFINITION OF DONE.** The lints + scanner + mutation gate run as Myelin CI jobs on every Myelin commit and reject a violating commit; the harness drives SUB-D3/D6/D10 on the self-hosting graph; committed.
- **COMMIT.** Header `P-<NNN> M6: dogfood the substrate ratchet (lints + scanner + mutation gate on Myelin's own commits)`. Body: the self-hosting CI ratchet green; the harness driving SUB-D3/D6/D10. Co-Authored-By trailer.

---

### P-S38 — The every-incident-adds-a-drill loop on Myelin's tracker + the truth-up pass

- **BAND.** M6.
- **ROADMAP MILESTONE.** SUB-M6 (the incident-loop + truth-up slice) — `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M6.
- **DEPENDS-ON.** P-S04 (the `register_drill` incident-loop hook), P-S37 (the self-hosting CI graph live), and the M6 self-hosting prompts (Myelin's own issue tracker).
- **CANON DOCS (read these first, in full, before writing any code).**
  - `../../external-insights/01-process-and-quality-doctrine.md` §1 (the code wins over the docs; the truth-up pass re-syncs docs to what the code does; date every status note) and §5 (every-incident-adds-a-drill).
  - `../../06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M6 (the every-incident-adds-a-drill loop files a Myelin issue + a reproducing drill).
  - `../../06-roadmaps/00-master-sequencing.md` §2 M6 + §4 M6-done row (the truth-up pass confirms 0 red earlier gates).
- **DELIVERABLE (what to build + exactly where in the repo).** (a) Wire the **every-incident-adds-a-drill loop** (the P-S04 `register_drill` hook) to file a Myelin issue + a reproducing drill for any substrate incident (the T-3 loop, now on Myelin's own tracker — a reproducing drill joins the catalogue and re-runs forever). (b) Run a **truth-up pass** (EI-01 §1): confirm every substrate PROVEN row rests on a dated green artifact (not a doc claim); where a doc and the code disagree, fix the doc; the gate invariant holds end-to-end (no earlier substrate gate is red).
- **CONTRACTS TO IMPLEMENT.** None new — this closes the dogfood loop (the incident loop + the honesty pass).
- **GATE / DRILLS (quantified; must be green to call this done).** **No earlier substrate gate is red** (the truth-up pass confirms every substrate PROVEN row rests on a dated green artifact — the gate invariant holds end-to-end). Green artifact = a dated truth-up scorecard with 0 red earlier rows + an incident-loop run that files a Myelin issue + a reproducing drill on a simulated incident. Contributes to the master M6 done-bar.
- **TESTS (required).** The incident-loop produces a Myelin issue + a reproducing drill on a simulated incident (the loop is live). The truth-up pass is a committed scorecard (every substrate PROVEN row → its dated green artifact; 0 red earlier rows). No CDC pair.
- **DEFINITION OF DONE.** The incident-loop files a Myelin issue + drill on a simulated incident; the truth-up pass confirms 0 red earlier substrate gates; committed.
- **COMMIT.** Header `P-<NNN> M6: incident-loop on Myelin's tracker + truth-up pass`. Body: the incident-loop wired; the truth-up scorecard (0 red earlier gates). Co-Authored-By trailer.

---

## Digest

**38 prompts** (first pass: 17), covering every SUB-M0..SUB-M6 milestone and every SUB-D1..SUB-D10 drill + the
D-11 firehose substrate half, with no gap. The split exposes each bundled sub-deliverable as its own
clean-context, independently-committable prompt; coverage is preserved (every milestone/contract/drill/floor
the first pass covered remains, now at finer granularity), and DEPENDS-ON is re-threaded across the new local
ids.

- **M0 (24 prompts, P-S01..P-S24):** the workspace + eight glue crates (P-S01); the harness, now three prompts —
  the 1×/10×/30× load generator (P-S02), the scoped-reversible dependency-break injector (P-S03), the
  telemetry-assertion library + incident-loop + harness self-test (P-S04); the outbox tier, now three prompts —
  the canonical envelope (P-S05), `OutboxTx::emit` causality (P-S06), the outbox table + relay with SUB-D1/BUS-D4
  (P-S07); the consumer tier, now two prompts — the idempotent consumer template + dedup with SUB-D2 (P-S08), the
  upcaster registry (P-S09); the lints, now two prompts — the four load-bearing (P-S10), the remaining eight
  (P-S11); the service shell, now four prompts — the `serve` lifecycle + drain (P-S12), the three-surface
  topology + tenant-from-token with SUB-D7 (P-S13), liveness ≠ readiness with SUB-D9 (P-S14), holder
  auto-registration + the migration runner (P-S15); the resilient client, now two prompts — the four primitives
  (P-S16), `Retry-After` with SUB-D5 (P-S17); the resilience tier, now three prompts — the fail-static mechanism
  (P-S18), the shed lane + bounded-everything (P-S19), the agent-load caps with SUB-D8 (P-S20); the committed-gate
  machinery, now three prompts — the contract-coverage scanner (P-S21), the thresholds file (P-S22), the overlay
  primitives (P-S23); the consolidated M0 exit-gate scorecard (P-S24).
- **M1 (3 prompts, P-S25..P-S27):** fail-static proven vs a real Identity hiccup, SUB-D4 (P-S25); the
  restore-verify cross-seam half, SUB-D6/STOR-D1/STOR-D2, the silent-data-loss floor + a permanent gate (P-S26);
  the exhaustive H1–H18 holder confirmation (P-S27).
- **M2 (2 prompts, P-S28..P-S29):** the firehose per-connection frame caps + slow-consumer drop (P-S28); the
  scope-bounded selector + per-surface frame shed budgets, completing the D-11 substrate half (P-S29).
- **M4 (2 prompts, P-S30..P-S31):** the cross-language shim enforced (if Chat diverges) (P-S30); the firehose
  backpressure half under connection-storm, contributing to CHAT-D1/D13/D14 (P-S31).
- **M5 (5 prompts, P-S32..P-S36):** the 30× surge family, SUB-D3 (P-S32); the tuned per-surface shed budgets
  (P-S33); online-migration-under-load, SUB-D10 (P-S34); restore-verify at cell scale, SUB-D6/STOR-D2 (P-S35); the
  resilient-client per-target tuning (P-S36).
- **M6 (2 prompts, P-S37..P-S38):** the lints + scanner + mutation gate as Myelin CI jobs (P-S37); the
  incident-loop on Myelin's tracker + the truth-up pass (P-S38).

**The two substrate-owned permanent gates** (re-run forever, never "done"): the outbox 0-loss/0-ghost floor
(SUB-D1/SUB-D2/BUS-D4, P-S07/P-S08, re-run on every emit-path change) and the restore-verify gate
(SUB-D6/STOR-D1/STOR-D2, P-S26 + P-S35 at cell scale, re-run on every store-touching change). **Named floors**
(each with its follow-on prompt): fs-backed BlobStore → object-store (M5, Storage-owned); per-surface
shed-budget v1 table → tuned numbers (P-S19 → P-S33); resilient-client default per-target values → tuned values
(P-S16 → P-S36); single-region event log → column-store seam (P-S07, post-M5, when volume is measured);
fail-static value W → DPO-ratified (P-S18/P-S25, parallel/legal); cross-language shim specified → enforced
(P-S12..P-S19 → P-S30); the EventEnvelope KMS hierarchy (P-S05 → Storage M1 11.3); the exhaustive holder list
(P-S15 → P-S27); the overlay-primitive impl (P-S23 → first frontend-bearing subsystem, M3+); the not-yet-existing
lint targets (P-S11 → their consumer-code landing). The substrate's correctness floors (the outbox, the twelve
lints, the harness) are NOT staged — absolute from M0.
