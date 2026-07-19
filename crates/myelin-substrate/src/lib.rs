//! # `myelin-substrate` — the bootstrap harness (`serve(AppSpec)`) + fail-static primitives
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.6 (`myelin-substrate` — the bootstrap harness crate), §3 (`serve(AppSpec)`),
//! §8 (fail-static primitives), §2.9 (the dependency root — root-last, no cycles).
//!
//! **Contract-index cluster:** 1 — Bootstrap & service shell
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` rows 1.1
//! `serve(AppSpec)`, 1.10 `FailStatic<T>`).
//!
//! ## What crosses the crate boundary here (the frozen surface)
//! - `serve(AppSpec)` (1.1) — boot → migrate → outbox relay → consumers → three ports →
//!   graceful drain; non-zero on failed boot. The one call a service's `main.rs` makes.
//! - `AppSpec{ name, config, migrations, public, internal, consumers, holders, outbox }`
//!   (1.1, architecture §3.1) — the spec the harness consumes.
//! - `FailStatic<T>` (1.10) — bounded-staleness cache; `static_max ≤ revocation SLA` and
//!   `≥ agent-token TTL`; `get` returns `Fresh | Static(degraded) | Closed`.
//!
//! ## DAG root (§2.9): this crate is root-LAST
//! `myelin-substrate` depends on ALL the glue crates and NO glue crate depends on it.
//! The [`crate_graph`] module encodes the §2.9 DAG declaratively and the
//! `crate-graph-acyclic` test asserts (a) the graph has no cycle and (b)
//! `myelin-identity` is a sink whose only dependency is `myelin-tenancy` (identity
//! depends on nothing above tenancy). This is the build-layer realisation of the
//! `no-cross-sync-cycle` lint; P-S10 ships the real source-scanning lint.
//!
//! ## Status (P-S12 → P-010, 2026-06-19) — the `serve(AppSpec)` lifecycle is IMPLEMENTED
//! The boot → migrate → outbox relay → consumers → three ports → graceful drain lifecycle
//! (contract 1.1) is **implemented** in [`serve`]: `serve(AppSpec)` boots (validates config
//! fail-fast §3.2, opens the bounded OLTP pool §3.3), runs the forward-only migrations at boot
//! (rejecting a destructive `DROP`), auto-registers every opened store as a `PersonalDataHolder`
//! (§3.4), starts the outbox relay (P-S07) + the idempotent consumers (P-S08), opens the three
//! surfaces, exports the producer side of the contract-1.8 telemetry signal set (§3.5,
//! `outbox_depth`/`dead_letter_count`/`consumer_lag`), serves, then graceful-drains (stop intake,
//! finish in-flight, ack-then-exit; a clean drain leaves `outbox_depth == 0`). The hello-world
//! boot test (boot → emit → consume → drain) + the CDC 1.1 pair are the dated green artifact.
//!
//! ## Status (P-S13 → P-030, 2026-06-19) — the three-surface topology + tenant-from-token is DONE
//! The **tenant-from-token** mechanism (1.2, SUB-D7) is **implemented** in [`topology`]: the
//! lifecycle-opened [`topology::PublicSurface`] derives the operating tenant from the verified
//! token's `Principal`, NEVER from the URL path — a path-tenant ≠ token-tenant mismatch is
//! REJECTED ([`topology::PublicReject::CrossTenantIdor`]) and AUDITED ([`topology::AuditSink`],
//! PII-free) as a cross-tenant IDOR, and `misroute_count` stays 0. The internal RPC surface
//! ([`topology::InternalSurface`]) re-authorizes every call through the [`topology::Authorizer`]
//! seam (identity trusted, authorization re-run — "internal = safe" is not presumed). The SUB-D7
//! drill (`tests/drill_sub_d7_idor.rs`) + the CDC 1.2 pair (`tests/cdc_1_2_topology.rs`) are the
//! dated green artifact: 60 spoofs rejected + audited, 0 served (`CrossTenantCount == 0`).
//!
//! ## Status (P-S14 → P-031, 2026-06-19) — liveness ≠ readiness on the metrics-health surface DONE
//! The **liveness ≠ readiness** semantics (1.3, SUB-D9) are **implemented** in [`metrics_health`]:
//! the lifecycle-opened [`metrics_health::MetricsHealthSurface`] exposes two INDEPENDENT probes —
//! [`metrics_health::MetricsHealthSurface::liveness`] ("not wedged"; reads ONLY the process's own
//! [`metrics_health::LivenessState`], structurally incapable of checking a dependency) and
//! [`metrics_health::MetricsHealthSurface::readiness`] ("can serve correct traffic now"; a dead
//! **critical** dependency → [`metrics_health::Readiness::NotReady`] + shed; startup =
//! boot/migration incomplete → not-ready-not-killed). A severed critical dependency flips
//! readiness and sheds while liveness stays `Up` (no restart-storm). `serve` opens it in the
//! `Booting` state and [`metrics_health::MetricsHealthSurface::mark_started`]s it at the end of a
//! successful boot. The SUB-D9 drill (`tests/drill_sub_d9_liveness_readiness.rs`) + the CDC 1.3
//! pair (`tests/cdc_1_3_liveness_readiness.rs`) are the dated green artifact: a dead critical dep
//! → `readiness` gauge `1 → 0`, `liveness_restart_count == 0` (no churn). The composition with
//! **fail-static** (§8.3) is named: readiness handles the *sustained* outage, fail-static (P-S18)
//! buys the *transient* hiccup.
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The real `/livez` + `/readyz` HTTP handlers + the OTLP readiness/liveness gauge export**
//!   on the real metrics-health listener land with the real transport wiring (P-S13/P-S14+). The
//!   semantics (liveness ignores deps; readiness sheds on a dead critical dep; startup is
//!   not-ready-not-killed) are COMPLETE now; the live `DependencyHealth` probe is fed by the
//!   resilient client's breaker state (§6, P-S16) in production — here a [`metrics_health::HealthTable`]
//!   fixture + the harness dependency-break injector drive it.
//! - The real **gateway transport + mTLS/signed-internal-credential wire format + the durable
//!   tamper-evident audit sink** for the IDOR records → the gateway/listener wiring (P-S14+) and
//!   GDPR `P-GA-19`/`P-062` (the audit *consumer* reads the same PII-free
//!   [`topology::IdorAuditRecord`] shape). The substrate-side security property (tenant-from-token,
//!   re-authorize-every-call, every IDOR audited) is complete now; the wire transport is named.
//! - The [`topology::Authorizer`] body (the depth-bounded Zanzibar `check`/`list_objects`) is
//!   Identity M1 (`P-ID-09`/`P-ID-11`). Here the trait is the re-authorize-every-call SEAM.
//! ## Status (P-S15 → P-032, 2026-06-19) — holder auto-registration + the forward-only runner DONE
//! The **`PersonalDataHolder` auto-registration mechanism** (1.4) is **implemented** in
//! [`holders`]: every store the harness opens — OLTP / blob / cache / search index
//! ([`holders::StoreKind`]) — is registered through the one door, [`holders::HolderRegistry::open`]
//! (opening IS registering, so "we forgot a store" is structurally impossible, §3.4 / GD-3). The
//! **forward-only online migration runner** (1.5) is **implemented** in [`migrations`]:
//! [`migrations::MigrationRunner`] applies the embedded DDL in order at boot and REFUSES a
//! destructive (`DROP`) migration AND a blocking `ALTER` on a declared-**hot** table (§9.1/§9.4),
//! carrying the expand→backfill→contract [`migrations::MigrationPhase`] on each migration. The
//! **hot-table declaration mechanism** ([`migrations::HotTables`], the `AppSpec::hot_tables` field)
//! is the §9.4 frozen contract both the runner (at boot) and the `forward-only-migration` lint
//! (P-S11, at source-scan) read. The holder-registration + runner + lint tests are the dated green
//! artifact.
//!
//! ## Status (P-S19 → P-035, 2026-06-19) — the protected-human-lane shed order + bounded-everything
//! The **principal-aware shed lane** + **bounded everything** + the **§7.6 per-surface shed-budget
//! v1 floor table** (contract 1.11) are **implemented** in [`shed`]:
//! - **(a) the shed lane** — [`shed::ShedLane`] reads the run-class ([`shed::RunClass::derive`],
//!   derived from the verified `Principal.kind` + the injected run-class header — a header may only
//!   *down-class*, the human lane is structurally unspoofable) and applies the shed order
//!   `speculative → batch/CI → agent → human-last` with `429 + Retry-After` ([`shed::ShedDecision`]),
//!   **per-tenant** (one tenant's surge fills only its own budget — it can never shed another
//!   tenant's human; EI-02 §1 / EI-01 §2 blast-radius). The human lane is shed LAST and only in true
//!   saturation.
//! - **(b) bounded everything** — [`shed::BoundedQueue`] is the one bounded-queue/pool primitive
//!   (consumer prefetch / DB pool / bulkhead per target / per-tenant in-flight / HTTP intake): it
//!   **fast-fails (sheds)** when full rather than growing latency unboundedly (§7.1, Little's Law).
//! - **(c) the §7.6 v1 floor table** — [`shed::ShedBudgetTable::v1_floor`] (named floors).
//!
//! The shed-count-per-lane + per-surface signals are exported (the contract-1.8 producer slice,
//! [`shed::ShedLane::shed_count`] / [`shed::BoundedQueue::shed_count`]). The shed-order + bounded
//! unit tests (`shed::tests`) + the CDC 1.11 pair (`tests/cdc_1_11_shed_order.rs`) are the dated
//! green artifact. **Floors named:** the shed-budget NUMBERS are the M0 v1 floor → tuned by the
//! surge/latency drills in **M5 (P-S33)**; the **agent-load caps + the SUB-D8 causal-loop guard**
//! (bounded dispatch pool / causal-depth ceiling / shared-root tripwire / bounded predicate guard)
//! land in **P-S20 (P-036)** — this module ships only the agent *lane* of the shed order.
//!
//! ## Status (P-S20 → P-036, 2026-06-19) — agent-load caps + the causal-loop guard (SUB-D8) DONE
//! The **agent-generated-load caps + the causal-loop guard** (contract 1.11 agent slice / the AG-6
//! loop-guard machinery) are **implemented** in [`agent_load`]:
//! - **(a) the bounded dispatch pool** — [`agent_load::DispatchPool`] takes a fixed number of permits
//!   and **drops over-cap reactions ([`agent_load::DispatchAdmission::Dropped`]), never forks** a new
//!   worker (§7.4). The `dispatch_pool_drops` signal is exported.
//! - **(b) the causal-depth ceiling** — [`agent_load::DepthCeiling`] reads [`myelin_events::EventEnvelope::depth`]
//!   (§5.3, stamped correct-by-construction by `OutboxTx::emit`, P-S06): a reaction at/over the **hard
//!   ceiling (`16`)** is **halted** ([`agent_load::DepthVerdict::Halt`]); at/over the **soft ceiling
//!   (`12`)** it is admitted-but-flagged so a deepening loop is visible early. The ceilings are the v1
//!   floor read from the thresholds file (P-S22 / P-038).
//! - **(c) the shared-causal-root-within-a-window tripwire** — [`agent_load::SharedRootTripwire`]
//!   reads [`myelin_events::EventEnvelope::correlation_id`] (the causal root, §5.3): too many reactions
//!   off ONE root within the sliding window → **fire** ([`agent_load::TripwireVerdict::Fired`]) +
//!   quarantine the root (the *wide-fan-out* guard a per-chain depth ceiling would miss). `tripwire_fired`
//!   is exported.
//! - **(d) the bounded predicate-evaluation guard** — [`agent_load::PredicateGuard`] caps the static
//!   step count + the runtime evaluation time per predicate (§7.5), **rejecting a crafted over-cost
//!   matcher** ([`agent_load::PredicateVerdict::OverBudget`]) before it can DoS the trigger engine.
//!
//! The four caps are wired into one consult, [`agent_load::AgentLoadGuard::admit`] (the call the Bus
//! reactive/dispatch tier, EB-23/P-143, makes per delivered reaction): a halt at any cap returns
//! WITHOUT taking a permit, so a stopped loop leaks no resources. The contract-1.8 producer slice
//! (`causal_depth` histogram + `tripwire_fired` + `dispatch_pool_drops`) is exported via
//! [`agent_load::AgentLoadGuard::signals`], mapping onto the harness's `SignalName::{CausalDepthFirings,
//! DispatchPoolDrops}` set the SUB-D8 drill reads. The unit tests (`agent_load::tests`) + the CDC 1.11
//! agent-slice pair (`tests/cdc_1_11_agent_load.rs`) + the SUB-D8 drill
//! (`tests/drill_sub_d8_causal_loop.rs`, an adversarial agent→agent loop driven by the P-S03 injector,
//! asserted by the P-S04 telemetry library) are the dated green artifact. **Floors named:** the ceiling
//! NUMBERS (`12`/`16`) + the tripwire window/threshold are the v1 floor → the versioned thresholds file
//! (**P-S22 / P-038**); the **full agent-loop proof re-runs in M2** with the agent fabric (**AG-P12 /
//! P-224**, AG-D7; **P-FLOW-18 / P-214**); the **reserve/settle cost gate** (§7.4's third cap) is the
//! durable-wallet body in **Storage M1 (P-ST-16) + Agent (AG-P14 / P-227)** — named here, not built.
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - The env-first `Config::from_env()` parse of the real `DATABASE_URL`/broker/KMS/region knobs
//!   plus the concrete `tokio-postgres`/`sqlx` connection behind [`myelin_storage::OltpPool`] land
//!   with the driver; the bounded-pool + fast-fail semantics are complete now.
//! - The **exhaustive H1–H18 holder confirmation** against the REAL Identity/Storage/GDPR holder
//!   set is **P-S27**; here the MECHANISM auto-registers every opened store. The blob / cache /
//!   search-index holders' concrete `PersonalDataHolder` DSR bodies land with their backends
//!   (Storage M1 blob, Search M2); the OLTP holder's DSR bodies are the GDPR M1 floor (P-ST-01).
//! - **SUB-D10 (online migration under load)** — expand→backfill→contract on a restored
//!   production-scale copy under load, with no blocking lock beyond budget, plus the lock-time
//!   measurement against a restore (§9.2) — proves at **M5 (P-S34)**. The runner, the phase model,
//!   the hot-table declaration, and the destructive/blocking refusals are complete + testable at
//!   boot scale now.
//! - The per-subsystem hot-table FLAGS are **measured-not-predicted** (M1+); each high-write
//!   subsystem declares its set in its `AppSpec` as it lands (the §9.4 seed set is named).
//! - The real **OpenTelemetry meter/tracer/logger + the OTLP export + the causality+tenant
//!   trace-context middleware** (§3.5) and the **`SIGTERM`/`SIGINT` → drain** OS trigger →
//!   **P-S13/P-S14**. Here the producer is a typed in-process meter ([`serve::Telemetry`])
//!   exporting the SAME contract-1.8 `SignalName`s the harness reads, and the drain is the
//!   deterministic [`serve::ServeHandle::signal_drain`] trigger.
//! - `FailStatic<T>::get` (1.10) → **P-S18** (the mechanism) and **P-S25** (proven vs a real
//!   Identity hiccup, SUB-D4). The `static_max` VALUE is `[OPEN — LEGAL]` (DPO ratifies, L-1) —
//!   the mechanism + the `≤ revocation-SLA ≥ agent-token-TTL` constraint ship regardless; the
//!   number is a named legal floor.
//! - The failure-injection harness (load generator / dependency-break injector /
//!   telemetry-assertion library) is **P-S02–P-S04** in a separate `myelin-harness` crate. The
//!   twelve architecture lints are **P-S10/P-S11**.
//! - **Mutation floor (cargo-mutants ≥ 75% on the lifecycle module, [`serve`]).** cargo-mutants
//!   is the M6 dogfood CI gate (**P-S37**); it is not run in this prompt's environment. The
//!   lifecycle is covered by unit + CDC tests that chain boot → emit → consume → drain
//!   end-to-end (a sequence property, EI-01 §4); the mutation run is named as the M6 gate.
//!
//! ## Status (P-S23 → P-041, 2026-06-19) — the shared overlay/state primitives DONE (contract)
//! The **shared overlay/state primitives** (the design-system bug-class floor, SUB-M0 item 5) are
//! shipped in [`overlay`] as the **stack-agnostic CONTRACT + invariants**, because Myelin has no
//! chosen frontend stack at M0 (the P-S23 prompt's explicit "if the frontend stack is not yet
//! chosen, ship the primitive CONTRACT" branch). The three EI-01 §4 recurring bug-classes are each
//! foreclosed as a TESTED structural guarantee, not prose: (1) **off-screen-picker** →
//! [`overlay::place_overlay`] returns a rect ALWAYS contained by the viewport (flip-then-clamp);
//! (2) **clipped-dialog** (+ the "control unreachable on a phone" sibling) →
//! [`overlay::center_dialog`] shrinks-to-fit rather than clipping + [`overlay::reachable_within`];
//! (3) **focus-leak** → [`overlay::FocusTrap`] is a closed cyclic ring focus can never escape. The
//! invariant unit tests (`overlay::tests`, incl. the exhaustive on-screen sweep + the mixed-key
//! focus-never-leaks sequence) are the dated green artifact. **Floor named:** the **rendering
//! binding** (mapping these rects + this focus order onto the chosen frontend toolkit's real
//! layout/portal/focus APIs) lands with the **first frontend-bearing subsystem (M3+)** — the
//! design-system pass (**GIT-P7 / P-233** and the Knowledge/Issues design-system passes). Every
//! frontend feature from M3 on builds on THESE primitives (EI-01 §7: abstract once, here).
//!
//! ## Status (P-S28 → P-135, M2) — firehose per-connection frame caps + slow-consumer drop DONE
//! The substrate's **bounded-and-sheds half** of the firehose resume-cursor seam (contract 3.5,
//! §7.7) is **implemented** in [`firehose`] — it rides the Bus's protocol (`subscribe`/`resume`,
//! the zero-loss-replay half, P-141/EB-21) without depending on its impl:
//! - **(a) per-connection in-flight frame caps** — [`firehose::FrameBuffer`] is a per-subscription
//!   bounded frame queue built on the one [`shed::BoundedQueue`] primitive (§7.1 generalised to
//!   streaming): a frame offered over-cap **sheds in the firehose's own bounded queue**
//!   ([`firehose::PushOutcome::Shed`]) — buffered frames NEVER exceed the cap (Little's Law).
//! - **(b) slow-consumer drop to `resync_required`** — once a buffer's per-`(stream,scope)` frame
//!   lag crosses the slow-consumer ceiling, the connection is **dropped to `resync_required`**
//!   ([`firehose::PushOutcome::ResyncRequired`]): the buffer is RELEASED (memory → bounded, lag → 0)
//!   and the consumer falls back to a full `*.snapshot` replay (the cold-rebuild path, NAMED not
//!   silent). The drop is counted exactly once per connection.
//!
//! The per-`(stream,scope)` frame-lag + `resync_required` count are exported into the contract-1.8
//! telemetry set ([`firehose::FirehoseSignals`], mapping onto the harness
//! `SignalName::{FirehoseFrameLag, ResyncRequiredCount}`). The unit tests (`firehose::tests`) + the
//! CDC 3.5-substrate-half pair (`tests/cdc_3_5_firehose_backpressure.rs`) + the SUB-D11 hot-stream
//! drill (`tests/drill_sub_d11_firehose_slow_consumer.rs`, the P-S03 injector drops a firehose
//! subscription on a hot stream, asserted by the P-S04 telemetry library) are the dated green
//! artifact. **Floors named:** the Bus-side **zero-loss-replay half** (`subscribe`/`resume`/
//! `resync_required → *.snapshot`) is **P-141 (EB-21)** — the full D-11 reconnect-loses-zero-ops
//! drill needs BOTH halves; the **scope-bounded selector (reject `*`) + the per-surface frame shed
//! budgets** are **P-S29 (P-136)**; the **M4 connection-storm re-confirm** of this half is **P-S31
//! (P-326)**.
//!
//! ## Status (P-S29 → P-136, M2) — firehose scope-bounded selector + per-surface frame shed budgets DONE
//! The substrate's half of D-11 is now **COMPLETE** (P-S28 the cap + slow-consumer drop; P-S29 the
//! scope-bounded selector + frame shed budgets). Shipped in [`firehose_selector`] — it WRAPS the P-S28
//! [`firehose::FrameBuffer`] (coherence, EI-01 §7 — no re-defined buffer), adding the two seams P-S28
//! named:
//! - **(a) scope as a bounded selector, never `*`** — [`firehose_selector::BoundedSelector::parse`]
//!   admits ONLY `board:`/`doc:`/`channel:<id>` and **REJECTS `*`** (and `board:*`, empty, un-prefixed,
//!   unknown-kind) with a typed [`firehose_selector::SelectorError`] — an unbounded subscription is
//!   unrepresentable. A 50k-row board subscribes to a [`firehose_selector::ScopeWindow`] (visible rows +
//!   margin); a frame on an off-screen row is [`firehose_selector::FrameOutcome::OutOfWindow`] and never
//!   enters the buffer (memory bounded by the WINDOW, not the board — the §7.7 "never `*`" guarantee).
//! - **(b) the per-surface shed budgets applied to FRAMES** — [`firehose_selector::FrameShedBudget`]
//!   gives each [`firehose::FrameClass`] a fraction of the buffer (presence ≤ agent ≤ human, the §7.6
//!   order): **presence/speculative frames shed BEFORE message delivery; agents shed BEFORE humans**.
//!   The human/message frames are shed LAST (only the per-connection cap, in true saturation, sheds them).
//!
//! [`firehose_selector::FrameSelector`] composes the window + the frame budget over the P-S28 buffer
//! into the connection tier's one call per inbound frame ([`firehose_selector::FrameSelector::offer`]).
//! The per-class frame-shed count is exported as the §10.2 `ShedCount`-by-lane signal (labelled by the
//! frame class). The unit tests (`firehose_selector::tests`) + the CDC 3.5 scope-selector pair
//! (`tests/cdc_3_5_firehose_scope_selector.rs`) + the SUB-D11 frame-budget drill completion
//! (`tests/drill_sub_d11_firehose_frame_budgets.rs`) are the dated green artifact. **Floors named:** the
//! Bus-side **zero-loss-replay half** (`subscribe`/`resume`/`resync → *.snapshot`) is **P-141 (EB-21)**
//! — the full D-11 reconnect-loses-zero-ops proof needs BOTH halves; the per-class frame-budget
//! FRACTIONS are the M2 v1 floor → tuned by the connection-storm drill in **M5 (P-S33)**; the **M4
//! connection-storm re-confirm** of this half is **P-S31 (P-326)**.

use serde::{Deserialize, Serialize};

pub mod agent_load;
pub mod crate_graph;
pub mod fail_static;
pub mod fail_static_authz;
pub mod firehose;
pub mod firehose_selector;
pub mod holder_catalog;
pub mod holder_registered;
pub mod holders;
pub mod metrics_health;
pub mod migrations;
pub mod overlay;
pub mod serve;
pub mod shed;
pub mod thresholds;
pub mod topology;

pub use agent_load::{
    count_by_root, AgentLoadGuard, AgentLoadSignals, BudgetBreach, DepthCeiling, DepthVerdict,
    DispatchAdmission, DispatchPool, GuardOutcome, PredicateGuard, PredicateVerdict,
    SharedRootTripwire, TripwireVerdict,
};
pub use fail_static::{
    Answer, Clock, FailStatic, FailStaticError, FailStaticSignals, StalenessBound, SystemClock,
    TestClock,
};
pub use fail_static_authz::{
    encode_authz_key, AuthzDecision, AuthzServed, CoarseAuthz, FailStaticAuthz, AUTHZ_FRESH_TTL_SECS,
};
pub use firehose::{
    FirehoseScope, FirehoseSignals, Frame, FrameBuffer, FrameClass, FrameLagSample, PushOutcome,
};
pub use firehose_selector::{
    BoundedSelector, FrameBudgetVerdict, FrameOutcome, FrameSelector, FrameShedBudget, ScopeWindow,
    SelectorError, SelectorKind, WindowVerdict,
};
pub use holder_catalog::{
    assert_holder_completeness, classify_store, holder_completeness, Holder, OrphanStore,
    StoreClassifier, StoreHolder,
};
pub use holder_registered::{
    assert_all_holders_registered, holder_registered, DeclaredStore, HolderViolation, StoreManifest,
};
pub use holders::{HolderRegistration, HolderRegistry, StoreKind};
pub use metrics_health::{
    CriticalDependencies, CriticalDependency, DependencyHealth, HealthTable, Liveness,
    LivenessState, MetricsHealthSurface, Readiness, ReadinessReport, Startup,
};
pub use migrations::{
    is_blocking_alter, is_destructive, HotTables, Migration, MigrationPhase, MigrationRunner,
    Migrations,
};
pub use overlay::{
    center_dialog, place_overlay, reachable_within, FocusId, FocusMove, FocusTrap, Placement, Px,
    Rect, Side,
};
pub use serve::{
    boot, serve, serve_until_shutdown, AppSpec, ConsumerReg, HoldersSpec, InternalRpc, OutboxSpec,
    PortOpener, PublicRoutes, ServeHandle, Surface, Telemetry,
};
pub use shed::{
    BoundedQueue, RunClass, RunClassHeader, ShedBudgetError, ShedBudgetTable, ShedDecision,
    ShedLane, Surface as ShedSurface, SurfaceBudget,
};
pub use thresholds::{
    CellSizing, ClaimedNotProven, DepthCeilings, DsrDeadline, FailStaticThreshold, FlexDb,
    Revocation, RpoRto, ShedBudgetRow, Surge, ThresholdError, Thresholds, THRESHOLDS_FILENAME,
};
pub use topology::{
    AllowPrincipal, AuditSink, Authorizer, DenyAll, IdorAuditRecord, InjectedIdentity,
    InternalReject, InternalSurface, PublicReject, PublicSurface,
};

/// Seconds (frozen unit, architecture §2.10) — the fail-static window bounds.
pub type Seconds = u64;

/// The validated, env-first service config (architecture §3.2; contract 1.1). Opaque
/// string-backed on this floor; the env-first `Config::from_env()` parse of the real
/// `DATABASE_URL`/broker/KMS/region knobs lands with the driver (**P-S15**). `serve`
/// validates it at boot (fail fast, §3.2) — see [`serve::boot`]. A config of `"BAD_POOL"`
/// models the boot-time validation-failure path the §3.2 fail-fast test exercises.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config(pub String);

/// The error type for the boot/serve lifecycle (a failed boot / failed migrate / incomplete
/// drain). A loud, typed value — a failed boot returns non-zero (architecture §3.1), never a
/// silent success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeError(pub String);

impl core::fmt::Display for ServeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ServeError {}

// The fail-static mechanism (`FailStatic<T>`, `Answer<T>`, the §8.2 constructor constraint, the
// contract-1.8 signals) lives in [`fail_static`] (P-S18). It was a `todo!()`-bodied stub here in
// the P-S01 skeleton; P-S18 implements the real bounded-staleness mechanism in its own module so
// the mandatory-core mutation floor has a clean target, and re-exports it (see the `pub use
// fail_static::{...}` above). The types are PROVEN against a real Identity hiccup in P-S25 (SUB-D4).

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-asserting test: the `serve(AppSpec)` + `AppSpec{...}` field shape is frozen
    /// (contract 1.1, architecture §3.1) — the eight fields `name, config, migrations,
    /// public, internal, consumers, holders, outbox`. We construct an `AppSpec` (proving the
    /// field names) and take a fn pointer to `serve` (proving its signature). The lifecycle
    /// behaviour itself is exercised by the `serve::tests` + the CDC 1.1 integration test.
    #[test]
    fn serve_and_appspec_shape_is_frozen() {
        let spec = AppSpec {
            name: "hello",
            config: Config::default(),
            migrations: Migrations::default(),
            hot_tables: HotTables::none(),
            public: PublicRoutes::default(),
            internal: InternalRpc::default(),
            consumers: vec![],
            holders: HoldersSpec::Auto,
            stores: StoreManifest::new(),
            outbox: OutboxSpec::default(),
            critical: CriticalDependencies::default(),
        };
        assert_eq!(spec.name, "hello");
        assert_eq!(spec.holders, HoldersSpec::Auto);
        let _f: fn(AppSpec) -> Result<(), ServeError> = serve;
    }

    /// Compile-asserting test: the `FailStatic<T>` re-export shape + the `Answer<T>` ladder are
    /// frozen (contract 1.10) — `fresh_ttl()`/`static_max()` in SECONDS, the constrained constructor
    /// enforces the §8.2 bound. The full boundary mechanism is drilled in [`fail_static::tests`];
    /// this asserts the re-export surface is reachable from the crate root.
    #[test]
    fn fail_static_shape_and_units_are_frozen() {
        // seconds (the frozen unit); the value is the engineering seed — the real bound is
        // DPO-ratified (L-1), not a default set here. The constructor enforces the §8.2 bound.
        let bound = StalenessBound {
            revocation_sla_secs: 300,
            agent_token_ttl_secs: 60,
        };
        let fs: FailStatic<&str, u8> = FailStatic::try_new(30, 300, bound).expect("valid bound");
        assert_eq!(fs.fresh_ttl(), 30u64);
        assert_eq!(fs.static_max(), 300u64);
        assert!(fs.static_max() >= fs.fresh_ttl());
        // the answer ladder exists with all three rungs (never fail-open).
        let a: Answer<u8> = Answer::Static(1);
        assert!(a.is_static());
        let _closed: Answer<u8> = Answer::Closed;
    }
}
