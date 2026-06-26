//! # `myelin-harness` — the failure-injection / drill harness (test-support)
//!
//! **Owning doctrine:** `external-insights/01-process-and-quality-doctrine.md` §3
//! ("prove-it-or-it-isn't-real" — build the failure-injection harness EARLY: a load
//! generator that multiplies traffic 1×/10×/30× and mixes principal types, a scoped
//! reversible dependency-break, assertions read from production telemetry).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md` §7.2 (the
//! five principal kinds the limiter reads + the protected human lane), §7.6 (the
//! per-surface storm profiles — CI-surge / collab op-stream / connection-storm /
//! agent-mention-storm, OQ-K), §11 (the failure-injection seam).
//!
//! **Testing-strategy doc:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! §3.1 (the load generator) + §4.1 family **F6** (30× surge + protected human lane — the
//! load generator is the spine of every F6 surge drill / the SUB-D3 surge family).
//!
//! **Contracts owned:** none. The harness is test-support machinery — the driver UNDER
//! every surge/storm drill in the whole ledger, not a cross-system contract. (Per the
//! P-S02 prompt: "None owned — this is harness machinery.")
//!
//! ## DAG position
//! This crate is **NOT** a node in the production crate DAG (architecture §2.9). It is
//! test-support that sits ABOVE `myelin-substrate` as a leaf consumer (like a service's
//! `main.rs`): it depends on `myelin-identity` (for [`myelin_identity::PrincipalKind`])
//! and `myelin-tenancy` (for [`myelin_tenancy::TenantId`]), and **nothing depends on it**.
//! The substrate's `crate-graph-acyclic` test continues to model the ten production crates
//! only — adding `myelin-harness` to a production crate's `[dependencies]` would pull
//! test-support into the substrate DAG and is forbidden.
//!
//! ## What P-S02 ships
//! The **1×/10×/30× load generator** ([`load_generator`]): a driver that issues traffic at
//! a configurable [`Multiplier`] with a configurable [`PrincipalMix`] across the five
//! [`LoadPrincipalKind`]s (human / agent / service / CI / external-MCP) and the four named
//! [`StormProfile`]s. The generator targets an abstract [`Sink`] (an in-memory request
//! handler in tests; later drills point it at a real `serve` instance).
//!
//! ## What P-S03 ships (this prompt)
//! The **scoped-reversible dependency-break injector** ([`dependency_break`]): the T-3 seam
//! every later drill rides to force ONE [`Dependency`] to fail for ONE [`Scope`] without
//! taking the rig down. [`DependencyBreaker::break_dependency`] /
//! [`DependencyBreaker::restore_dependency`] are **reversible** (a broken dep restores to a
//! fully working state), **scoped** (a break touches only its named dependency + scope —
//! never an unrelated dependency or another tenant/cell), and **idempotent** (a
//! double-break / double-restore is a no-op, observably via [`BreakOutcome::NoChange`]). It
//! is the failure-injection half of the unit-of-proof: the load generator (P-S02) drives
//! traffic, this injector severs a dependency, and the telemetry-assertion library (P-S04)
//! reads that the system survived.
//!
//! ## What P-S04 ships (this prompt)
//! The **telemetry-assertion library** ([`telemetry`]) — a typed reader over the contract-1.8
//! survival-signal set ([`telemetry::SignalName`], architecture §10.2) with
//! [`telemetry::SignalSource::assert_signal`] returning a typed
//! [`telemetry::Assertion`] (green/red/rejected) that is **never** a swallowed pass: a red
//! cannot be `|| true`-ed away (`#[must_use]` + the loud
//! [`telemetry::Assertion::expect_green`]), and an inverted/vacuous assertion is **rejected**,
//! not green (EI-01 §3). The **every-incident-adds-a-drill loop** ([`drills::DrillRegistry`])
//! — [`drills::DrillRegistry::register_drill`] joins a reproducing [`drills::DrillScenario`]
//! that [`drills::DrillRegistry::run_all`] re-runs forever (EI-01 §3/§5). The **harness
//! self-test** (the SUB-M0 exit unit-of-proof) — inject one fault (P-S03), drive one unit of
//! load (P-S02), read one telemetry assertion that reads green (P-S04) — committed as a test
//! emitting a dated PASS row (`drills.rs` `harness_self_test_*`).
//!
//! ## What P-S26 ships (this prompt → global P-056)
//! The **restore-verify cross-seam half** ([`restore`]) — the substrate's half of the
//! SUB-D6 / STOR-D1 / STOR-D2 silent-data-loss floor (architecture §11 D-6; contract 11.5).
//! Storage owns the WAL+PITR rebuild and the CI-wired `restore-verify` job (its M1 follow-ons
//! P-059/P-060/P-061); this prompt ships the **failure-injection + telemetry-assertion machinery**
//! that makes the gate PROVABLE: [`restore::RestoredSnapshot::verify_cross_seam`] — the cross-seam
//! consistency assertion over the four seams (OLTP rows ↔ blob ↔ search index ↔ event-log offsets)
//! that REJECTS an inconsistent rebuild (a row → missing blob, an orphan index doc, a past-offset
//! row), and [`restore::RestoreOutcome`] — the MEASURED RPO/RTO carried onto the
//! [`telemetry::SignalSource`] via the three new restore signals
//! ([`telemetry::SignalName::RestoreCrossSeamMismatch`] / `RestoreRpoSecs` / `RestoreRtoSecs`), so
//! the drill asserts 0 loss + RPO ≤ 5 min + RTO ≤ 1 h/tenant ≤ 4 h/cell (thresholds read from the
//! thresholds file, never hardcoded). The drill scenario + the CDC pair (provider = Storage
//! restore; consumer = this assertion) + the SCHED/smoke variants live in
//! `myelin-substrate/tests/drill_sub_d6_restore_verify.rs`. **SUB-D6 / STOR-D1 / STOR-D2 are a
//! PERMANENT gate** (re-run on every store-touching change; M2 does NOT start over a red STOR-D1);
//! the cell-scale re-confirm is the M5 follow-on **P-S35**.
//!
//! ## Floors named (deferred + filling prompt)
//! - **The telemetry PRODUCER side lands at P-S12/P-S13.** This prompt ships the *consumer*
//!   side: an in-memory [`telemetry::SignalSource`] the rig populates in tests. A real service
//!   exporting the §10.2 set on its metrics-health port (OpenTelemetry, architecture §3.5/§10)
//!   lands inside the `serve` lifecycle at **P-S12/P-S13** and populates the SAME
//!   [`telemetry::SignalName`]s — the assertion surface does not change.
//! - **The self-test asserts at the M0 floor.** It models the SUB-D2-shaped "zero events lost
//!   across a broker outage" property against the injector + in-memory signals (no relay
//!   exists yet). The real assertion-backed drills the registry hosts are re-pointed at their
//!   live fault-points at the owning prompts: SUB-D1/D2 at **P-S07/P-S08** (the relay +
//!   consumer), SUB-D4 at **P-S25** (fail-static), SUB-D5 at **P-S17** (downstream breaker),
//!   SUB-D7 at **P-S13** (cross-tenant IDOR). The inject → load → assert SHAPE is frozen here.
//! - **Storm-profile parameters are v1 defaults.** The tuned per-surface shed-budget
//!   numbers are the M5 surge / connection-storm follow-on (**P-S32 / P-S33**;
//!   architecture §7.6 names them as floors tuned by the drills, not claimed-final). Here
//!   each profile carries a v1 default shape so the generator selects the right surface
//!   behaviour; the *numbers* tighten at M5.
//! - **No runtime survival drill yet.** The telemetry-assertion library (the thing a drill
//!   asserts against) lands in **P-S04**; this prompt's gate is the injector's OWN
//!   reversibility + scoping + idempotence, and the generator's OWN correctness. The
//!   assertion-backed survival drills the injector drives are wired at the prompts that own
//!   each fault-point: SUB-D2 at **P-S07/P-S08** (sever the broker), SUB-D4 at **P-S25**
//!   (hard-down Identity / fail-static), SUB-D5 at **P-S16/P-S17** (trip a downstream).
//! - **Consult seam, not a real process-killer.** The injector models a break as shared
//!   queryable state ([`DependencyBreaker::is_broken`]); the real fault-points that consult
//!   it (the relay's publish, `AuthzClient::check`, a downstream RPC) do not exist until
//!   **P-S07 / P-S12 / P-S13 / P-S16** — each wires its fault-point to this consult then.
//! - **Abstract sink only.** The generator drives an in-memory [`Sink`]; pointing it at a
//!   real three-port `serve` instance lands once `serve` exists (**P-S12 / P-S13**).
//!
//! ## What P-S30 ships (this prompt → global P-319)
//! The **cross-language harness shim conformance suite** ([`cross_language_shim`]) — the
//! enforcement mechanism for contract 1.7 / architecture §3.7 (the frozen divergence
//! contract). [`Nonnegotiable`] is the exhaustive, frozen seven-element set a non-Rust
//! subsystem's shim must satisfy; [`DivergentTierProbe`] is the seam the divergent tier
//! implements; [`ShimConformance::check`] runs all seven and is green iff all seven pass
//! (`shim-conformance` ×7); [`ShimEnforcement`] is the LOUD discharge record — either
//! `Enforced` (a divergent tier passed) or `RecordedNa` (the subsystem stayed Rust — a NO-OP
//! recorded loudly with a dated reason). There is no third variant and no path from a failing
//! suite to an `Enforced` record (EI-01 §5: a non-negotiable cannot be quietly dropped at a
//! language boundary; §4: an N/A is recorded, never silently skipped). **Today this is the
//! N/A path:** Chat's TE-21 pin is Rust (the BEAM hatch is written-but-closed,
//! `myelin_chat::glue`), so there is no cross-language boundary; the loudly-recorded dated N/A
//! row + the CDC pair for 1.7 live in `tests/shim_conformance_p_s30.rs` (a dev-dependency on
//! `myelin-chat`, keeping the production harness dep set tiny). When Chat diverges (CHAT-P26)
//! the suite binds: the BEAM tier's probe implements [`DivergentTierProbe`] and the tier
//! cannot ship unless all seven clauses are green.
//!
//! ## What P-S38 ships (this prompt → global P-510, SUB-M6 — the substrate dogfood done-bar)
//! The **substrate dogfood** ([`dogfood`]) — the last substrate prompt: (a) the
//! **every-incident-adds-a-drill loop on Myelin's OWN tracker** ([`dogfood::SubstrateIncidentLoop`])
//! — a simulated substrate incident files a Myelin issue ref AND registers a reproducing
//! [`drills::DrillScenario`] into the substrate's real [`drills::DrillRegistry`] via the P-S04
//! `register_drill` hook; the loop is *live* (the repro re-runs forever and reads green), and an
//! incident missing either leg is a LOUD gap. (b) the **substrate truth-up pass**
//! ([`dogfood::SubstrateTruthUpPass`]) — enumerates every substrate PROVEN row
//! ([`dogfood::proven_substrate_rows`], SUB-D1..D11 + BUS-D4/D7 + the twelve lints + the
//! contract-coverage scanner + the harness self-test + the M5 world-scale/tuning legs) and asserts
//! each rests on a DATED green artifact; a claimed-not-proven row is a LOUD
//! [`dogfood::SubstrateTruthUpVerdict::Red`] (code-wins-over-docs, EI-01 §1). The gate invariant
//! holds end-to-end (no earlier substrate gate is red). The ONE legitimate remaining floor — the
//! world-scale 30× FLEET-hardware load drill — is named in the rendered scorecard, never claimed.

pub mod cross_language_shim;
pub mod dependency_break;
pub mod dogfood;
pub mod drills;
pub mod load_generator;
pub mod make_it_real;
pub mod restore;
pub mod scorecard;
pub mod self_hosting_ci;
pub mod telemetry;

pub use cross_language_shim::{
    DivergentTierProbe, Nonnegotiable, ShimConformance, ShimEnforcement,
};
pub use dependency_break::{BreakOutcome, Dependency, DependencyBreaker, Scope};
pub use dogfood::{
    outbox_relay_stall_repro, proven_substrate_rows, ProvenSubstrateRow, SubstrateIncident,
    SubstrateIncidentLoop, SubstrateTruthUpPass, SubstrateTruthUpRed, SubstrateTruthUpVerdict,
};
pub use drills::{DrillContext, DrillRegistry, DrillResult, DrillScenario};
pub use load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, Request, RunClass,
    Sink, StormProfile, Surface,
};
pub use restore::{
    BlobAddr, CrossSeamMismatch, CrossSeamReport, IndexDoc, Offset, OltpRow, RestoreOutcome,
    RestoredSnapshot, RestoredSnapshotBuilder, RtoGrain,
};
pub use scorecard::{Band, GateRow, RowResult, RowVerdict, Scorecard};
pub use self_hosting_ci::{
    run_graph, run_job_via_cargo, self_hosting_jobs, JobKind, JobResult, JobRunner, JobTool,
    SelfHostJob, SelfHostingRun,
};
pub use telemetry::{
    AssertedSignal, Assertion, Label, Predicate, RejectReason, SignalName, SignalSource,
};
