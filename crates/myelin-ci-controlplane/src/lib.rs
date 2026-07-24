//! # myelin-ci-controlplane — the CI Control Plane service shell (CI-P6 → P-349, M4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/00-overview.md` §4 (the
//! five logical services — this is service #2, the CI Control Plane: scheduler + reaper + fleet
//! autoscaler + log-pipeline coordinator + secret broker + supply-chain verifier + the check
//! emitter; each a `serve(AppSpec)` shell, its own Postgres, no cross-DB) + §5 (cell topology — no
//! global pool, residency by construction); `01-tech-and-data-model.md` §3 (the complete
//! control-plane schema this shell migrates) + §4 (the encryption/residency/GDPR posture).
//! **Contracts:** `contract-index.md` rows 1.1 (`serve(AppSpec)` — the service shell, NOT a
//! hand-rolled `main`), 1.2/1.3 (the three ports + liveness ≠ readiness), 1.5 (the forward-only
//! migrations + hot-table flags), 11.1 (OLTP), 12.1 (the `(tenant, region)` partition key), 10.2
//! (the `#[personal_data(...)]` tags), 4.8 (pseudonym subjects — `triggered_by`/`approved_by`).
//!
//! ## What CI-P6 ships here — the bootable SHELL + the COMPLETE data model, NOT the behaviour
//! [`controlplane_app_spec`] assembles the CI Control Plane [`AppSpec`] the harness's ONE call
//! drives (boot → migrate → outbox relay → consumers → three ports → graceful drain, liveness ≠
//! readiness). The CI Control Plane is an `AppSpec`, not a hand-rolled lifecycle — the EXACT analog
//! of the Search / Refs / Identity service shells. The shell:
//!   - declares the **three ports** (public / internal / metrics-health) via the harness (1.2/1.3) —
//!     liveness must not check deps; readiness gates on the DB pool + the declared critical deps
//!     (arch 00 §4: DB + broker + authz + at-least-one-healthy-runner-pool);
//!   - runs the **complete forward-only data-model migrations** ([`migrations::ci_controlplane_migrations`]):
//!     all eighteen CI Control-Plane tables (`ci_run`, immutable `ci_drive_manifest`, `ci_job`,
//!     `check_attempt`, `job_queue` +
//!     its three claim indexes, `fair_deficit`, `runner`, `log_segment`, `log_anchor`, `artifact`,
//!     `cache_entry`, `environment`, `deployment`, `secret_binding`, `cost_event`, immutable
//!     `ci_job_accounting`), each
//!     `(tenant_id, region)`-first + RLS-on (contract 11.1/12.1/1.5);
//!   - declares the **four hot tables** ([`migrations::ci_controlplane_hot_tables`]) — `job_queue`,
//!     `log_segment`, `cost_event`, `check_attempt` (arch 01 §3 "Hot-table flags declared");
//!   - declares its critical downstreams (`broker`, `authz`, `runner_pool`; the OLTP store is
//!     implicitly critical) for the readiness probe (§4.3, SUB-D9 — readiness red until DB + broker
//!     + authz + at-least-one-healthy-runner-pool reachable, arch 00 §4);
//!   - carries the `#[personal_data(...)]`-tagged row mirrors ([`schema`]) so the
//!     `no-untagged-personal-data` lint is GREEN on the CI schema (contract 10.2 / 4.8).
//!
//! ## Floors named (the per-table-behaviour follow-ons — see [`migrations`])
//! The CI-P6 shell shipped the table SHAPES + the bootable shell. The per-table behaviour lands in
//! its own prompt: the scheduler pull-lease claim over `job_queue` + concurrency + affinity + the
//! dead-runner reaper is now SHIPPED in [`scheduler`] (CI-P12 / P-355); DRR fair-share over
//! `fair_deficit` (CI-P13 — the claim ORDERs on `fair_deficit.deficit DESC`, the advance/replenish is
//! CI-P13), the fleet autoscaler over `runner` (CI-P14), the `check_attempt` counter + the
//! `ci.check.updated` producer (CI-P18), the log index (CI-P20), trust-scoped artifacts/caches
//! (CI-P22), reserve/settle metering into `cost_event` (CI-P17), the deploy/secret broker (CI-P24).
//! No consumers are registered at the shell (the Trigger & Dispatch dedup consumer is the OTHER
//! shell, [`myelin-ci-dispatch`]; the scheduler is not a bus consumer).
//!
//! ## DB-free by default; the live-stack proof behind `integration`
//! `cargo build --workspace` / `cargo test --workspace` stay DB-free (the shell boots over the
//! substrate's in-process floor pool; the migrations are `&str` DDL the runner admits without a DB).
//! The REAL forward-only apply against the dev-stack Postgres (RLS isolation + the claim indexes) is
//! `tests/integration_ci_p6_controlplane_schema.rs` behind the `integration` cargo feature.

pub mod artifact_cache;
pub mod check_emitter;
/// Claim-time durable authority verification in front of the raw Identity credential minter.
pub mod ci_claim_token_issuer;
/// Canonical, insert-only replay authority for a durable CI workflow. The manifest binds the exact
/// DAG, per-context check attempts, workflow code identity, and server-granted launch templates;
/// secret values and minted token JTIs are structurally absent.
pub mod ci_drive_manifest;
/// Real Identity PASETO/S7 adapter at claim-time mint and final pre-launch authorization.
pub mod ci_identity_adapter;
/// Server-owned mapping from the authored `linux-small-v1` request to fixed default-deny runtime
/// terms plus explicit durable budget/token authority handles.
pub mod ci_launch_authority;
/// Exact manifest job identity, sandbox translation, and explicit token authority.
pub mod ci_manifest_job_runner;
/// Production manifest resolver and DAG-native durable `ci.pipeline` body.
pub mod ci_manifest_pipeline;
pub mod ci_pipeline;
pub mod ci_result_signal;
pub mod ci_runner_composition;
pub mod ci_runner_host;
pub mod ci_runtime_composition;
/// The subsystem-owned total grammar for the durable `myelin ci` read surface.
pub mod cli;
pub use ci_claim_token_issuer::{CiJobCredentialMinter, LockedManifestCiJobTokenIssuer};
pub use ci_drive_manifest::{
    ci_check_context_v1, CiDriveManifestError, CiDriveManifestStore, CiDriveManifestV1,
    CiJobLaunchGrantV1, CiLaunchAuthorityV1, CiManifestLaneV1, CiManifestLimitsV1,
    CiManifestSchedulingV1, CiManifestTrustTierV1, CiManifestWorkspaceV1, CiMergeWaiterV1,
    GrantedCiJobV1, CI_DRIVE_MANIFEST_DIGEST_V1_DOMAIN, CI_DRIVE_MANIFEST_SCHEMA_V1,
    MAX_CI_DRIVE_MANIFEST_BYTES,
};
pub use ci_identity_adapter::{
    ci_job_authorization_context, IdentityCiJobCredentialMinter, IdentityCiJobLaunchAuthorizer,
    CI_JOB_PRINCIPAL_ID, CI_JOB_REQUIRED_CAPABILITIES,
};
pub use ci_launch_authority::{
    CiJobBudgetReservationProvider, CiJobRuntimeAuthorityRequest, LinuxSmallV1LaunchAuthority,
    ManifestBoundCiJobTokenAuthority, PgTierPCiJobBudgetReservation, TierPOperationalCiJobPricer,
    LINUX_SMALL_V1_POLICY_REVISION, LINUX_SMALL_V1_RUNNER_LABELS,
    TIER_P_OPERATIONAL_ACTIVE_RESERVATION_CEILING,
};
pub use ci_manifest_job_runner::{
    register_durable_ci_manifest_pipeline, CiJobTokenIssueError, CiJobTokenIssuer,
    CiJobTokenRequest, CiManifestDurableJobRunner,
};
pub use ci_manifest_pipeline::{
    decode_resolved_ci_manifest, drive_resolved_ci_manifest_pipeline,
    register_ci_manifest_pipeline, run_ci_manifest_pipeline, CiManifestInputResolver,
    CiManifestPipelineOutcome,
};
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use ci_runner_composition::{
    ci_runner_cancellation_coordinator, CiRunnerCancellationCoordinator,
};
pub use ci_runner_composition::{
    ci_runner_hooks, ci_runner_identity_authorities, CiRunnerIdentityAuthorities,
    CiRunnerIdentityCompositionError,
};
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use ci_runtime_composition::ci_production_runtime_factory_test_support;
pub use ci_runtime_composition::{
    ci_manifest_pipeline_definition, ci_production_runtime_factory, CiProductionRuntimeFactory,
    CiProductionWorkflowPoller, CiRuntimeCompositionError, CiWorkflowFanoutBatch,
    CI_FLOW_OUTBOX_SCHEMA_VERSION, CI_FLOW_WORKER_LEASE_TTL_SECS, CI_MANIFEST_PIPELINE_VERSION,
    MAX_CI_WORKFLOW_DRIVES_PER_SCOPE, MAX_CI_WORKFLOW_SCOPES_PER_PASS,
};
/// CT-004d.2 chunk 4 — the durable `ci_run` writer ([`ci_run_store::CiRunStore`]): the CI
/// run-of-record. The `ci-dispatch.trigger` consumer's reserve bundle must persist a durable `ci_run`
/// row (`state = queued`), but the production reserve store only staged a NOTE — the row was written
/// durably ONLY in the CT-004b integration test's `CoCommitReserveStore`. This is the production writer
/// that test proved: it co-commits the run-of-record ROW on the consumer's co-commit `HandlerTx`
/// connection (the SAME tx as the dedup mark — atomic), while the co-emitted events stay absorb-mode
/// through the outbox (the honest #7 H1 split). Constructed at the composition root by
/// [`ci_run_store_factory`]; the `ci_run` table it writes is created by the shared
/// [`ci_durable_migrations`] both CI mains apply at boot. Manifest-backed registration now lives in
/// [`register_durable_ci_manifest_pipeline`]; production activation remains gated on composing the
/// exact-tenant worker/poller, accounted reporter routing, and CI-run finalization around the
/// now-real policy, Identity, and scoped reservation authorities, then closing the complete
/// crash/recovery matrix.
pub mod ci_run_region;
pub mod ci_run_starter_poller;
pub mod ci_run_store;
pub mod ci_run_supersession;
pub mod ci_scheduler_db;
/// CT-004a (CI backend reconcile-and-harden — the FOUNDATION chunk): the REAL durable `cost_event`
/// projection store ([`cost_store::CiCostEventStore`]). Turns the previously model-only CI metering
/// path (SQL constants + the in-memory [`metering::CostEventRow`] model, no production-callable store)
/// into a genuine durable store that executes the BYTE-IDENTICAL [`metering::INSERT_COST_EVENT_QUERY`]
/// / [`metering::SELECT_COST_EVENTS_FOR_RUN_QUERY`] against the OLTP pool. It owns ONLY the CI
/// `cost_event` PROJECTION (run/job-attributed, meter-dimensioned reporting rows); the reserve/settle
/// MONEY-truth stays in `myelin_storage::reserve_settle::CostLedger` (delegated via
/// [`metering::CiMeter`]/[`myelin_flow::BudgetGate`]) — see the module docs for the split. Constructed
/// at the composition root by [`ci_cost_event_store`]; the live consumer that drives a real settle
/// through it (co-committing the run-state transition) is CT-004d.
pub mod cost_store;
pub mod crypto_shred_erase;
pub mod deployment;
/// The CI dogfood done-bar (CI-P35 / P-509, M6): the CI **switch test** ([`dogfood::CiSwitchTest`] —
/// the Git OQ-12 / CI switch test, driven against the real `myelin ci` run/log/deploy views vs the
/// GitHub Actions anchor, measured latency, EI-01 §4), the CI **truth-up pass**
/// ([`dogfood::CiTruthUpPass`] — every PROVEN CI row rests on a dated green artifact, EI-01 §1), and
/// the self-hosted **every-incident-adds-a-drill** loop ([`dogfood::IncidentDrillLoop`]). The
/// build/test/lint/mutation pipeline running AS a Myelin `ci.pipeline` is wired in the self-hosting CI
/// graph (`myelin-harness/src/self_hosting_ci.rs`). This is the platform done-bar (no follow-on).
pub mod dogfood;
/// CI's slice of the whole-system E2E-2 agent-native FLAGSHIP (CI-P34 / P-494, M5): CI-fail → triage
/// agent → issue → chat → fix-PR. Drives CI's side of the joint flagship end-to-end — the structured
/// `ci.run.failed` triage hook (which stage/step/test/log-excerpt ref), the AG-D4-gated runner the
/// triage agent's compute runs on (`kind=Agent`, 8.4 / X-6), the fix-PR check seam (5.9), the
/// `ci.result` merge wake EXACTLY ONCE (9.4 → merge-count == 1), and the balanced reserve/settle
/// (11.7) — over the UNCHANGED producer seams, emitting its named green ([`e2e_wedge::E2eArtifact`]).
/// The Agent-Fabric leg is AG-P24/P-480; the durable park/resume spine is `myelin-flow`'s P-477.
///
/// **MR-009b W6b2 — `#[cfg(any(test, feature = "test-support"))]`:** this whole in-process E2E-2 slice
/// module constructs the now-`test-support`-gated in-memory `CostLedger::new`; nothing in the production
/// graph references it, and its only caller (the `drill_ci_p34_*` test) reaches it via the
/// `myelin-ci-controlplane/test-support` self dev-dependency.
#[cfg(any(test, feature = "test-support"))]
pub mod e2e_flagship;
/// CI's slices of the whole-system E2E wedge (CI-P33 / P-493, M5): E2E-1 (the PR context pane —
/// CI's check rows resolve per-viewer, 0 leak, `#step-<n>` anchor) + E2E-3 (spec-to-ship
/// traceability — HITL-gated deploy ships, cold-reindex == live, audit tamper detected). Each leg
/// drives the whole flow end-to-end (chaining mutations mid-flight, EI-01 §4) over the UNCHANGED
/// production-hardened CI engine and emits its named green artifact ([`e2e_wedge::E2eArtifact`]).
/// E2E-2 is CI-P34; E2E-4 (DSAR) is covered for CI by CI-P32's CI-D3. No new contract; no weakened
/// gate.
// MR-009b W3b.5: the E2E wedge drill runners construct the `test-support`-gated in-memory
// OutboxStore double — a drill harness, never production serving code. Gated with it; the
// `drill_ci_p33_*` test reaches it via the `myelin-ci-controlplane/test-support` self dev-dep.
#[cfg(any(test, feature = "test-support"))]
pub mod e2e_wedge;
pub mod events;
pub mod fairness;
pub mod fleet;
pub mod job_accounting_store;
/// CT-004c.1 (CI backend reconcile-and-harden — the DURABILITY-PLUMBING half of making the CI
/// scheduler live): the REAL durable `job_queue` store ([`job_queue_store::CiJobQueueStore`]) + the
/// dead-runner reaper loop ([`job_queue_store::JobQueueReaper`]). Turns the previously two-form
/// scheduler (the `&str` SQL constants + the DB-free [`scheduler::SchedulerState`] model) into a
/// genuine pool-backed store that runs the BYTE-IDENTICAL claim/reap/cancel-superseded SQL under a
/// tenant-/region-scoped transaction (the FORCE-RLS `job_queue` table). It is the durable equivalent
/// of BOTH the controlplane `SchedulerState` and the sandbox `JobLeaseStore` — the ONE store both
/// will claim from in CT-004c.2 (which binds the runner + starts the pipeline body on the executor;
/// this chunk leases a row and launches NOTHING). Constructed at the composition root by
/// [`ci_job_queue_store`]; the reaper is spawned by the service `main` onto the serve runtime.
pub mod job_queue_store;
/// CT-004d.1 — the durable `JobSpec` store ([`job_spec_store::CiJobSpecStore`]) + the dispatch→durable
/// co-persist. The missing half of the dispatch→durable→resolve bridge: CT-004c.1 landed the durable
/// `job_queue` (scheduling metadata only) and CT-004c.2 wired the runner to claim from it, but the
/// production spec-resolver was a fail-closed no-op ([`spec_store_unavailable_resolver`]). This store is
/// the real backing that resolver reads — it persists a dispatched stage's [`myelin_ci_sandbox::JobSpec`]
/// as one `spec jsonb` column keyed `(tenant, job_id)`, co-committed with the `job_queue` row in one
/// tenant-scoped tx on the shared `idem_token`, feeding the claim-gating `trust_tier` FROM the spec
/// (never widened). Constructed at the composition root by [`ci_job_spec_store`]; the real resolver is
/// [`runner_bind::durable_spec_resolver`]. The manifest-native body is registered separately through
/// [`register_durable_ci_manifest_pipeline`]; it preserves the manifest job UUID and exact sandbox /
/// scheduling terms. The live settle bookend and production token-authority composition remain
/// activation floors.
pub mod job_spec_store;
/// Versioned, tenant-bound loader for the immutable CI execution plan stored in the definition
/// snapshot CAS object. This module prepares and validates a plan; it deliberately does not start
/// jobs or flatten the dependency DAG into a sequential workflow.
pub mod run_plan;
pub use run_plan::{
    decode_resolved_run_plan, derive_concrete_job_name, load_launch_run_plan_v2,
    load_resolved_run_plan, CiExecutionProfileV1, CiExecutionRequestV1, PreparedRunPlan,
    PreparedRunPlanV2, RedispatchReason, ResolvedJobV1, ResolvedJobV2, ResolvedRunPlanV1,
    ResolvedRunPlanV2, RunPlanError, VersionedResolvedRunPlan, EXECUTION_REQUEST_SCHEMA_V1,
    LAUNCH_REQUEST_DIGEST_V1_DOMAIN, RUN_PLAN_SCHEMA_V1, RUN_PLAN_SCHEMA_V2,
};
/// Exact `(tenant, region)` PostgreSQL starter that co-commits the canonical `ci_job` DAG ledger and
/// pre-minted workflow start with the durable CI run's `queued -> running` transition.
pub mod pg_pipeline_starter;
pub use pg_pipeline_starter::{
    ci_job_id_v1, ci_job_id_v2, decode_ci_claimed_input, CiLaunchAuthorityError,
    CiLaunchAuthorityMaterializer, CiWorkflowDefinitionPin, ClaimedCiInput, ClaimedCiInputError,
    PgCiPipelineStarter, PgCiRunStarterFactory, PgCiStarterError, StartQueuedOutcome,
    CI_INITIAL_CHECK_EVENT_V1_DOMAIN, CI_JOB_ID_V1_DOMAIN, CI_JOB_ID_V2_DOMAIN,
};
/// Durable CI dispatch and claim-bound completion components. [`ci_pipeline_driver::DurableJobRunner`]
/// dispatches each stage into
/// the DURABLE `job_queue` + `ci_job_spec` via [`job_spec_store::CiJobSpecStore::co_persist_dispatch`]
/// (the run's `trust_tier`/`region` forwarded UNCHANGED — the security invariant);
/// [`ci_pipeline_driver::CiPipelineReporter`] atomically consumes the exact live claim and buffers a
/// typed verdict for [`myelin_flow::PgFlowWorker`]. The former in-memory driver exists only under the
/// `test-support` feature and is not part of the default production build.
pub mod ci_pipeline_driver;
pub mod ci_pipeline_reporter_router;
pub mod floor_followons;
pub mod holder;
/// CT-004c.1: the REGION-scoped, CROSS-TENANT half of the durable scheduler — the raw `CLAIM_QUERY` /
/// `REAP_QUERY` executions (a hosted runner claims across ALL tenants in its region; the DRR fairness
/// spans tenants). Isolated here so it is a NAMED, LOUD `tenant-predicate` exclusion (the
/// `placement_durable.rs` control-plane-routing posture) while the per-tenant
/// enqueue/cancel/complete/heartbeat queries in [`job_queue_store`] stay FULLY linted. Not part of
/// the public surface — [`job_queue_store::CiJobQueueStore::claim`] / `reap` delegate to it.
pub mod job_queue_region;
pub mod live_tail;
pub mod log_pipeline;
pub mod log_sink;
pub mod log_sink_durable;
pub mod metering;
pub mod migrations;
pub mod permanent_gates;
pub mod rebac_fragment;
pub mod residency_drill;
/// CT-004c.2 — the runner exec binding: the durable `job_queue` store adapted to the sandbox
/// [`myelin_ci_sandbox::LeaseStore`] port ([`runner_bind::DurableLeaseAdapter`]) + the bounded runner
/// loop the service `main` spawns ([`runner_bind::CiRunnerLoop`]). Binds
/// [`RunnerAgent`](myelin_ci_sandbox::RunnerAgent) to CT-004c.1's [`job_queue_store::CiJobQueueStore`]
/// and executes the leased job in a real gVisor (`runsc`) guest — the tier/region claim predicate
/// forwarded UNCHANGED (the adversarial-verifier surface).
pub mod runner_bind;
pub mod schedule_and_run_job;
pub mod scheduler;
pub mod schema;
pub mod secret_broker;
pub mod supply_chain;
pub mod surfacing;
pub mod surfacing_index;
pub mod surfacing_store;
pub mod surfacing_tools;
pub mod surge;

// CI-P15 (P-358): the legacy sequential compatibility body + X-1 producer fixtures. Fresh V2
// manifest-backed runs use `ci_manifest_pipeline`, whose deterministic body preserves the DAG and
// exact per-context attempts. This older body remains for its pinned definitions and drills:
// the protected-env / manual gates (9.4), the runner stages over the FROZEN `SCHEDULE_AND_RUN_JOB`
// long-park substrate (9.4/11.7/9.3), and CI's X-1 producer emits — the per-context terminal
// `ci.check.updated` facts + `ci.run.failed`/`ci.run.succeeded` + the `ci.result` rollup signal
// (contract 5.9). The `SCHEDULE_AND_RUN_JOB` handshake into the live scheduler/runner is CI-P16; the
// reserve/settle metering into `cost_event` is CI-P17; the `check_attempt` monotonic counter + the
// outbox producer plumbing is CI-P18; the end-to-end merge-queue seam GATE (GIT-D10/CI-D8) is now
// CLOSED by CI-P19 (P-362) — the `ci.result` rollup SIGNAL ([`ci_result_signal::CiResultSignal`]).
pub use ci_pipeline::{CheckFacts, PipelineRun, PipelineStage, RunVerdict, CI_PIPELINE_WF_TYPE};

// CI-P18 (P-361): the X-1 `check_attempt` monotonic counter + the `ci.check.updated` PRODUCER (the
// check-fact half of contract 5.9). The `check_attempt` counter (arch 01 §3.2 — CI's SOURCE of
// `run_attempt`, monotonic, never wall-clock) + the FROZEN 5.9 `CheckStatus` assembly (the
// byte-identical shape Git's consumer decodes off the OPAQUE `ci.check.updated` payload): the
// `summary` is a HumanisedRef (7.3, never a raw string), `cost_settled` flips ONLY on settle (X-1),
// `trust_tier` is STAMPED from provenance (CI never endorses a fork). The `ci.pipeline` body
// ([`ci_pipeline`]) now assembles its terminal facts THROUGH this module (ONE producer shape, no
// divergence). The `ci.result` rollup end-to-end + the GIT-D10/CI-D8 seam GATE is CI-P19 (P-362).
pub use check_emitter::{
    assemble_check_status, check_status_payload, details_ref, summary_for, CheckAttemptCounter,
    CheckEmitContext, CheckProvider, CheckState, CostPosture, TrustTier, BUMP_CHECK_ATTEMPT_SQL,
};

// CI-P19 (P-362): the X-1 `ci.result` ROLLUP SIGNAL that wakes Git's merge queue (the seam-closer
// half of contract 5.9). CI's `ci.pipeline` body emits the per-context `ci.check.updated` BUS EVENTS
// + the `ci.result` BUS EVENT; the merge-queue durable workflow waits on the durable `ci.result`
// SIGNAL (`wait_for_signal("ci.result", idem_key=<merge_attempt_id>)`, contract 9.4). This module
// turns CI's rollup into THAT signal: derive the FROZEN 5.9 verdict (REUSES `rollup_ci_result`),
// encode it references-not-payloads (`encode_ci_result`), and deliver it idempotently on the
// `idem_token` — a doubly-delivered rollup is ONE buffered `wf_signal` row → the merge queue wakes
// EXACTLY ONCE (0 double-merge). CI emits the fact; Git gates; CI never merges. The end-to-end
// GIT-D10/CI-D8 seam GATE (CI's real `run_ci_pipeline_body` → this signal → Git's `run_merge_attempt`)
// is `tests/drills_ci_p19_seam_gate.rs`. This CLOSES the X-1 seam — no new floor.
pub use ci_result_signal::{CiResultSignal, RollupDelivery};

// CI-P23 (P-366, M4, drill CI-D4): the SUPPLY-CHAIN TRUST verifier (arch 05 HP-4). The control-plane
// service (arch 00 §4 names it) enforces the THREE HP-4 controls fail-closed:
// digest-pin-or-fail-closed AT RUN (CI-P11 enforces it at plan; this re-asserts the FROZEN
// `ImageRef::digest_pinned` rule before USE — 0 un-pinned executions), sign + verify-before-use
// (sigstore Fulcio keyless bound to the 4.7 OIDC `BuildIdentity` + CI's sigstore Rekor transparency
// log — the SAME RFC 6962 BLAKE3 Merkle structure GDPR/Audit's tamper-evident log builds, contract
// 10.6 — verified BEFORE use; 0 unsigned-component runs), and SLSA L1–L2 provenance + SBOM
// (CycloneDX/SPDX) for produced artifacts. Every refusal builds the audit-critical
// `ci.supply_chain.verification_failed` draft via the outbox (contract 2.2). FLOORS: SLSA L3+
// (hermetic/two-party) is demand-triggered (CI-M5); the component-registry PRODUCT is
// commercial-flagged; the live Fulcio CA / EU-hosted Rekor witness round-trip is a deploy concern
// (the verify-before-use LOGIC + the fail-closed gates are real + tested here).
pub use supply_chain::{
    BuildIdentity, KeylessSignature, RekorLog, Sbom, SbomFormat, SlsaProvenance,
    SupplyChainVerifier, VerificationFailure,
};

// CI-P24 (P-367, M4, drill CI-D7): the IN-BOUNDARY SECRET BROKER (arch 02 §7.3). The `JobSpec` carries
// secret NAMES (`SecretRef`); the broker resolves them AFTER the sandbox is up, scoped to EXACTLY this
// job's references, via the shared secret capability (placed under Id/GDPR). The two structural
// defences: (1) a fork-tier run short-circuits to ZERO secrets BEFORE any authz check (the
// `!is_untrusted_fork` arm by construction — CI-D7's "0 fork secret reads"); (2) a trusted run resolves
// ONLY its referenced names AND only those its subject can `read` via the DIRECT NARROW
// `secret#direct_reader` grant (CI-1, contract 4.9). OIDC short-lived audience-scoped federated
// credentials (4.7) over static keys; a fork is refused those too. MUTATION-SCORE FLOOR: the broker is
// mandatory-core (security-load-bearing) — `cargo-mutants` ≥ 90% viable mutants caught. FLOOR named:
// none new (the broker composes the FROZEN `SecretRef`/`TrustTier`/`IdentityService::check` surfaces).
pub use secret_broker::{
    OidcCredential, ResolvedSecret, SecretBroker, SecretCapability, SecretOutcome,
    SecretResolution, WithholdReason, SECRET_READ_PERMISSION,
};

// CI-P24 (P-367, M4): DEPLOYMENTS & the protected-env HITL gate (arch 03 §1.2). The deploy state
// machine + the gate composition over the FROZEN `myelin-flow` HITL substrate: the per-effect `idem_key`
// (OQ-F) makes a DOUBLE-CLICK ONE apply + a DECLINED deploy WITHHELD (returns Denied, 0 mutation, AG-8);
// the approver set resolves via `list_subjects(environment, approve)` (4.4); the `ci.deployment.*` event
// drafts ride the OUTBOX (the only emit path); rollback is first-class (`ci.deployment.rolled_back` —
// reversibility). FLOOR named: none new (composes frozen myelin-flow signals + the frozen
// `requires_approval` defaults X-6 + the frozen `ci.deployment.*` tokens).
pub use deployment::{
    deploy_outcome_of, deploy_requires_approval, deployment_approval_required_draft,
    deployment_approved_draft, deployment_failed_draft, deployment_rejected_draft,
    deployment_requested_draft, deployment_rolled_back_draft, deployment_started_draft,
    deployment_succeeded_draft, resolve_approvers, DeployGate, DeployGateOutcome, DeployState,
    ENVIRONMENT_APPROVE_PERMISSION,
};

// CI-P35 (P-509, M6): the CI dogfood done-bar — the switch test + the truth-up pass + the self-hosted
// every-incident-adds-a-drill loop (the Git OQ-12 / CI switch test; continuous-integration §3 CI-M6).
pub use dogfood::{
    proven_ci_rows, switch_capability_matrix, CiIncident, CiSwitchTest, CiTruthUpPass,
    CiTruthUpRed, CiTruthUpVerdict, IncidentDrillLoop, ProvenCiRow, SwitchCapability,
    SwitchVerdict,
};

// CI-P16 (P-359): the `SCHEDULE_AND_RUN_JOB` dispatch handshake into CI's `job_queue` + the
// effectively-once invariant (CI-D1). The concrete `JobRunner` that BINDS the FROZEN engine dispatch
// seam (9.2/9.4) onto the scheduler's `job_queue` — minting the deterministic `idem_token` (engine),
// idempotent enqueue on `jq_idem` (a reaper re-queue + a control-plane re-dispatch = ONE row), and the
// runner's terminal `job.done` delivery (idempotent on `idem_token`, the `wf_signal` PK = one wake).
// The reserve/settle metering bookends into `cost_event` are CI-P17 (P-360); the live runner lease +
// in-sandbox execution is GATED by AG-D4.
pub use schedule_and_run_job::{complete_job, JobScheduleTerms, SchedulerJobRunner};

// CI-P17 (P-360): reserve/settle = the ONE metering path + the `cost_event` ledger + parity CI ↔ agent
// (CI-D5). The CI METER on the FROZEN reserve/settle path: the resource-second taxonomy
// ([`metering::Meter`]) is the wholesale meter (directly comparable to an agent `compute` call, X-6);
// the CI `cost_event` schema row ([`metering::CostEventRow`]) carries wholesale + markup as SEPARATE
// integer-minor-units columns with `kind ∈ {ci, agent}` (UNIFY / X-6); [`metering::CiMeter`] wraps the
// engine [`myelin_flow::BudgetGate`] (contract 9.5/11.7 — CI builds NO second ledger) so a CI dispatch
// `reserve_budget()`s (refuse-to-start on exhaustion, never interrupt in flight) + `settle_budget()`s
// on `job.done`. FLOOR named: the resource-second → credit/price MARKUP mapping is Commercial's (arch
// 06 R-2 — the `MarkupPolicy` seam carries it; CI owns only the meter + the wholesale column).
pub use metering::{
    meter_resource_seconds, metered_units_for, CiMeter, CostEventRow, CostKind, FlatBpsMarkup,
    MarkupPolicy, Meter, MeteredResource, ReserveSettleParitySignal, INSERT_COST_EVENT_QUERY,
    SELECT_COST_EVENTS_FOR_RUN_QUERY,
};
// MR-009b W6b2 — the CI-D5 parity DRILL is `test-support`-gated (it builds the in-memory
// `BudgetGate::new`); its re-export follows the same gate so the default build does not name a
// non-existent item.
#[cfg(any(test, feature = "test-support"))]
pub use metering::reserve_settle_parity_drill;

// CT-004a (CI backend reconcile-and-harden, FOUNDATION): the REAL durable `cost_event` projection
// store + its deterministic idempotency-key derivation + the typed fail-loud error. The metering
// path is no longer model-only — `CiCostEventStore::settle_in_tx` co-commits the CI projection rows a
// settle produces, and `cost_events_for_run` reads them back (wholesale ≠ markup intact). The
// storage-`CostLedger`-vs-CI-projection split + the CT-004d live-wiring follow-on are in the module docs.
pub use cost_store::{cost_id_for, verify_ci_cost_event_shape, CiCostEventStore, CiCostStoreError};
pub use job_accounting_store::{
    CiJobAccountingError, CiJobAccountingRecord, CiJobAccountingStore, CiJobAccountingWrite,
    INSERT_CI_JOB_ACCOUNTING_QUERY, SELECT_CI_JOB_ACCOUNTING_QUERY,
};

pub use holder::{
    ci_store_classifier, register_ci_holders, CiHolder, CiHolderRegistration, CiStoreClass,
    RestrictionFlag, CI_OLTP_STORE, CI_RESIDUAL_POSTURE_REF, ERASED_OUTCOME_NONE_REMAIN,
};

// CI-P32 (P-492): the CI `PersonalDataHolder` ERASE crypto-shred fan-out — erasure-reaches-every-holder
// (CI-D3). Fills CI-P9's `erase` stub: `erase(subject)` crypto-shreds the subject's PII across all five
// CI store classes (run-state/logs/artifacts/caches/deployments) by destroying the per-subject DEK (where
// isolable) + the per-tenant DEK fallback through Storage's frozen `KmsEngine` (11.4 — no second crypto),
// pseudonym-shreds the `triggered_by`/`approved_by` identity edges (the row survives for audit), emits
// `ci.*.erased` tombstones so every unfurl degrades (§OQ-D, 0 dangling leak), and re-verifies 0 recoverable
// PII incl. backups (§7.5 — the backup snapshot excludes a shredded DEK). The residual third-party free-text
// PII is the ONE platform posture, by reference (10.9 / X-7), never restated CI-local. FLOOR: the live-stack
// CI-D3 fan-out over Postgres/RustFS/Valkey is the integration follow-on (`floor_followons`). NO cycle:
// `myelin-storage`/`myelin-gdpr`/`myelin-events` have no edge back to `myelin-ci-controlplane`.
pub use crypto_shred_erase::{
    drive_ci_d3_erasure_reaches_every_holder, subject_dek_ref, tenant_dek_ref, CiD3Report,
    CiEraseFanOut, CiEraseReceipt, CiErasedTombstone, CiSealedRow, CiShredError,
    CiSubjectFootprint, CI_ERASED_VERB, ERASED_PSEUDONYM,
};

// CI-P20 (P-363): logs over the firehose + the sealed T3 (job, step, byte-range) log tier +
// `ci.log.available` pointers. The log-pipeline coordinator — `ship_line` redacts secrets (in-flight
// masking, defence-in-depth), publishes the frame to the firehose (the LIVE TAIL; CI is the heaviest
// firehose producer, contract 3.5 — never the durable bus), seals segments into T2 content-addressed
// blobs + the `(job, step, byte-range)` index (`log_segment`/`log_anchor`, contract 11.8), and emits
// COALESCED `ci.log.available` POINTER events via the outbox (contract 2.2 — never per-line). The
// residency-pin lint is green on every log write (logs near the runner region, contract 1.6). FLOORS
// named: the time-series/wide-column log tier is CI-P29 (P-489); the resume-cursor live-tail VIEWER +
// the `details_ref` jump-to-failure RESOLUTION is CI-P21 (P-364); the per-subject DEK for isolable
// inline log PII is CI-P22 (P-365). NO cycle: `myelin-events` (firehose) / `myelin-storage`
// (BlobStore) have no edge back to `myelin-ci-controlplane` (the same acyclic leaf edges CI carries).
pub use log_pipeline::{
    AnchorStatus, CoalesceBudget, CrossRegionLogWrite, LogAnchorRow, LogAvailablePointer, LogCoord,
    LogPipeline, LogSegmentRow, LogWritePin, SealThreshold, SecretRedactor, CI_LOG_STREAM,
    INSERT_LOG_SEGMENT_QUERY, REDACTION_MARKER, UPSERT_LOG_ANCHOR_QUERY,
};

// CT-004f (CI-P20): the cycle-safe binding of the runner's `FirehoseSink` seam to the real
// `LogPipeline` — `LogPipelineSink` (per-`(tenant, run, job)` pipeline, boundary-redaction-honest)
// + the `LogPersist` durable-write seam the sealed index/pointers flush through (sub-step 4 fills it).
pub use log_sink::{
    FlushedJobLogs, LogPersist, LogPipelineSink, PRODUCTION_LOG_SEGMENT_MAX_BYTES, SINGLE_STEP_ID,
};
// CT-004f sub-step 4a: the DURABLE LogPersist — the live `log_segment`/`log_anchor` writer (frozen
// bind-param SQL, one tenant-scoped FORCE-RLS tx per finished job).
pub use log_sink_durable::DurableLogPersist;

// CI-P21 (P-364): the resume-cursor live-tail VIEWER + the `details_ref` jump-to-failure resolution
// (CI-D11). The `LiveTail` viewer composes the frozen firehose `subscribe`/`resume` on the bounded
// `run:<id>` scope (0 lost lines on reconnect; `resync_required` → a range-read of the sealed
// segments); the `DetailsRefResolver` resolves `CheckStatus.details_ref = …/ci/run/<id>#step-<n>`
// through `log_anchor` → `log_segment` → the byte range (0 dangling step anchors).
pub use live_tail::{
    parse_step_ref, read_range_from_archive, DetailsRefError, DetailsRefResolver, LiveTail,
    ParsedStepRef, ResumeOutcome, SegmentIndex, SegmentRange, StepByteRange,
};

pub use events::{
    ci_event_tokens, is_durable, register_ci_taxonomy, register_ci_tokens, validate_ci_type_token,
    validate_ci_type_tokens, CiTypeTokenError, CI_DURABLE_TOKENS, CI_FIREHOSE_TOKENS,
    CI_SUBSYSTEM_TOKEN, CI_TYPE_TOKENS,
};

use myelin_substrate::{
    boot, serve, AppSpec, Config, CriticalDependencies, InternalRpc, OutboxSpec, PublicRoutes,
    ServeError, ServeHandle,
};

// CI-P25 (P-368, M4): CROSS-FABRIC SURFACING — the read+ref half (arch 03 §5.1/§7.1/§7.2). The
// leak-free `list_objects` SetExpr push-down over `ci_run.run_id` (the OQ-E JOIN against the
// per-tenant `authz_visible` reverse index — ONE query, NO N+1, NO post-filter; the
// `search-requires-acl-filter` lint conjoins the Filter before scoring), the `ArtifactRef`/`#sub`
// mints (`run`/`deployment`/`pipeline`/`runner`/`artifact` + the ci-owned `step-<n>`/`check-<context>`/
// `L<a>-L<b>` subs — the `#step-<n>` mint is byte-identical to `check_emitter::details_ref`, one
// source of truth), and `project(ref, viewer)` (the ONLY cross-DB read of a CI artifact — permission
// FIRST, deny ⇒ a content-free Tombstone, 0 title leak). Backs the chat run unfurl / PR context pane /
// knowledge embed / inbox humanisation / search snippet. FLOOR named: `declare_indexable` + humanise +
// `replay(*.snapshot)` + the agent `ToolDef` registrations are CI-P26. NO cycle: `myelin-refs` /
// `myelin-identity` have no edge back to `myelin-ci-controlplane` (the same acyclic leaf edges CI
// carries; the SetExpr lowering SHAPE is restated because a producer LEAF cannot depend on the
// Identity SERVICE crate, §2.9).
pub use surfacing::{
    ci_artifact_ref, ci_deployment_ref, ci_pipeline_ref, ci_run_id_colref, ci_run_ref,
    ci_runner_ref, commit_check_ref, compose_run_list_query, lower_over_run_id,
    run_search_pre_filter, run_step_line_ref, run_step_ref, ArtifactStore, AuthzJoin,
    AuthzVisibleIndex, BoundParam, CiArtifactType, CiSearchPreFilter, ComposedRunListQuery,
    DeploymentMeta, LoweredFilter, PipelineMeta, ProjectError, Projected, Projection, Projector,
    RenderHint, RunMeta, SubAnchor, Tombstone, TombstoneReason, AUTHZ_VISIBLE_TABLE, CI_SUBSYSTEM,
    RUN_LIST_PERMISSION, VIEW,
};

// CI-P33 (P-493): CI's slices of the whole-system E2E wedge — the two named green artifacts the
// master M5 exit gate cites (E2E-1 the PR context pane, E2E-3 spec-to-ship traceability). Each leg
// runs the whole flow end-to-end over the UNCHANGED engine; both must be `is_green()`.
// MR-009b W3b.5: gated with the harness module above.
#[cfg(any(test, feature = "test-support"))]
pub use e2e_wedge::{
    run_ci_e2e_slices, run_e2e1_pr_context_pane, run_e2e3_spec_to_ship_lineage, E2eArtifact,
    E2E_SCENARIOS,
};

// CI-P26 (P-369): the cross-fabric surfacing INDEX + REPLAY half — `declare_indexable` (the `ci/run`
// IndexSpec, 6.3), the replay re-export + no-cross-db rebuild gate (2.6), and the humanise re-export
// (7.3). `CI_SUBSYSTEM` is already re-exported from `surfacing` (one token) so it is referenced via
// the module path here, not re-exported again.
pub use surfacing_index::{
    ci_run_index_spec, ci_summary, register_ci_run_index_spec, register_ci_summary_templates,
    run_doc_is_indexable, summary_template_key, CheckVerdict, CiReindexSource, CiReplayKind,
    CiSummary, CI_RUN_ACL_OBJECT_TYPE, CI_RUN_TYPE, CI_SUMMARY_TEMPLATES,
};
// CI-P26 (P-369): the `ToolDef` registrations half (8.1) — the complete CI agent-tool set with the
// FROZEN X-6 `requires_approval` defaults. `CI_SUBSYSTEM` / `CI_TOOL_VERSION` are referenced via the
// `surfacing_tools` module path (the surfacing module already owns the `CI_SUBSYSTEM` re-export).
pub use surfacing_tools::{
    ci_effect_kind, ci_required_caps, ci_requires_approval_default, ci_side_effecting, ci_tool_def,
    ci_tool_defs, register_ci_tools, CI_TOOL_NAMES,
};

pub use scheduler::{
    lane_token, state_token, ClaimRequest, Claimed, EnqueueOutcome, JobState, Lane, QueuedJob,
    SchedulerState, AUTHORIZE_JOB_LAUNCH_QUERY, CANCEL_SUPERSEDED_QUERY, CLAIM_QUERY,
    COMPLETE_JOB_QUERY, HEARTBEAT_QUERY, INSERT_JOB_QUEUE_QUERY, REAP_QUERY,
};

// CT-004c.1 (CI backend reconcile-and-harden — durability plumbing): the REAL durable `job_queue`
// store + the dead-runner reaper loop. The scheduler is no longer model-only — `CiJobQueueStore`
// runs the claim/reap/cancel-superseded/enqueue/complete/heartbeat SQL against the OLTP pool under
// the tenant-/region-scoped RLS transaction the `job_queue` table requires. `JobQueueReaper` is the
// periodic lease-driven driver the service `main` spawns onto the serve runtime. The runner-binds-to-
// this-store + starts-the-body handoff is CT-004c.2 (adversarially verified — the trust-tier claim
// predicate + the sandbox exec path).
pub use job_queue_region::CiRegionQueueStore;
pub use job_queue_store::{
    CiJobLaunchClaim, CiJobQueueStore, DurableEnqueue, JobQueueReaper, JobQueueStoreError,
    LeasedJob, LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY,
};

// CT-004d.1: the durable `JobSpec` store + the dispatch→durable co-persist + the fail-closed
// resolve. `CiJobSpecStore::co_persist_dispatch` writes the `job_queue` row AND the digest-pinned
// non-launchable template (as one `spec jsonb`) in ONE tenant-scoped tx on the shared `idem_token`,
// feeding the claim-gating `trust_tier` FROM the template (a mismatch is refused fail-closed).
// `get_launch_template` resolves a leased row before claim-generation token mint (corrupt/missing →
// fail-closed, never a default). `MAX_JOB_TIMEOUT_SECS`
// is the wall-clock ceiling below the runner's lease TTL (the CT-004c.2 double-run fix).
pub use job_spec_store::{
    CiJobSpecStore, CiJobSpecStoreError, ClaimedDispatchIdentity, DispatchOutcome,
    DurableCiJobLaunchTemplate, INSERT_JOB_SPEC_QUERY, MAX_JOB_TIMEOUT_SECS,
    NON_TERMINAL_NULL_STAGE_JOBS_QUERY, SELECT_JOB_SPEC_IDENTITY_QUERY, SELECT_JOB_SPEC_QUERY,
};

// CT-004d.2 chunk 4: the durable `ci_run` writer (the CI run-of-record). `CiRunStore::co_commit_insert`
// writes the `ci_run` row on the consumer's co-commit `HandlerTx` connection (atomic with the dedup
// mark); `insert_ci_run`/`get_ci_run` are the pool-based tenant-scoped write/read (the run-view /
// check-emitter resolve path). A conflict on `(tenant_id, run_id)` is accepted only after a separate
// locking query verifies every immutable field; divergent/invisible conflicts fail typed and loud.
// The co-emitted events stay absorb-mode through the outbox (the honest #7 H1 split).
pub use ci_run_region::{
    CiActiveRunCursor, CiActiveRunPage, CiActiveRunRoute, CiRegionRunDiscovery,
    DISCOVER_ACTIVE_CI_RUNS_QUERY, DISCOVER_QUEUED_CI_RUN_TENANT_QUERY, MAX_ACTIVE_CI_RUN_PAGE,
};
pub use ci_run_starter_poller::{
    CiRunStarterBatch, CiRunStarterPollerError, PgCiRunStarterPoller, MAX_CI_RUN_START_BATCH,
};
pub use ci_run_store::{
    CiRunFinalization, CiRunFinalizationJob, CiRunFinalizationOutcome, CiRunFinalizationWrite,
    CiRunFinalizer, CiRunInsert, CiRunRecord, CiRunStore, CiRunStoreError, CiRunTerminalState,
    DurableCiRunFinalizer, FINALIZE_CI_RUN_QUERY, INSERT_CI_RUN_QUERY,
    LOCK_CI_RUN_FOR_FINALIZE_QUERY, LOCK_CI_RUN_FOR_TOKEN_MINT_QUERY,
    SELECT_CI_RUN_ACCOUNTING_QUERY, SELECT_CI_RUN_QUERY, VERIFY_CI_RUN_REPLAY_QUERY,
};
pub use ci_run_supersession::{CiRunSupersessionError, PgCiRunSupersession};
pub use ci_runner_host::{
    wait_for_ci_runner_host_drain_timeout, wait_for_ci_runner_host_failure, CiRunnerHost,
    CiRunnerHostConfig, CiRunnerHostFailure, CiRunnerHostHandle, CI_RUNNER_HOST_DRAIN_TIMEOUT,
    CI_RUNNER_HOST_POLL_INTERVAL,
};
pub use ci_scheduler_db::{
    CiSchedulerDbConfig, CiSchedulerDbError, CiSchedulerDbProvider, CI_SCHEDULER_DATABASE_URL_ENV,
};

// CT-004c.2: the runner exec binding — the durable-store lease adapter + the bounded runner loop the
// service `main` spawns (WIRES `RunnerAgent` to `CiJobQueueStore` + a real gVisor backend). CT-004d.1
// adds the REAL spec resolver (`durable_spec_resolver`) + the lease-TTL floor (`CI_RUNNER_LEASE_TTL_SECS`).
#[cfg(any(test, feature = "test-support"))]
pub use runner_bind::durable_spec_resolver_test_support;
pub use runner_bind::{
    durable_spec_resolver, spec_store_unavailable_resolver, CiRunnerLoop, CiRunnerLoopExit,
    DurableLeaseAdapter, JobSpecResolver, CI_RUNNER_LEASE_TTL_SECS,
};

// CT-004d.2 CULMINATION (chunks 2/3/5): the CI pipeline driver — `DurableJobRunner` (chunk 5) dispatches
// each stage into the DURABLE `job_queue`+`ci_job_spec` (trust_tier/region forwarded UNCHANGED);
// `CiPipelineDriver` (chunks 2+3) registers+drives `run_ci_pipeline_body` over a shared `FlowExecutor`
// and starts the parked run under the pre-minted `wf_run_id`; `CiPipelineReporter` bridges the runner's
// `passed` marker to the stage-verdict codec + wakes the parked run. `fixed_command_spec_builder` is the
// digest-pinned compute-spec seam the integration test injects (a real `runsc` guest); the real
// pinned-snapshot→JobSpec resolver is the named `unresolved_stage_spec_builder` follow-on.
#[cfg(any(test, feature = "test-support"))]
pub use ci_pipeline_driver::{fixed_command_spec_builder, CiPipelineDriver, StartRunError};
pub use ci_pipeline_driver::{
    unresolved_stage_spec_builder, CiJobAccountingPricer, CiJobPricingError, CiPipelineReporter,
    ClaimRefusal, DurableCiJobAccounting, DurableJobRunner, PricedCiJobUsage, StageSpecBuilder,
    TIER_P_OPERATIONAL_PRICING_REVISION,
};
pub use ci_pipeline_reporter_router::{
    CiPipelineReporterFactory, CiPipelineReporterFactoryError, CiPipelineReporterRouter,
    CiPipelineReporterRouterError,
};

// CI-P14 (P-357): the EU fleet autoscaler — the FleetProvider impl + autoscale-on-queue-depth +
// per-residency-zone pools (no global pool) + the fleet events. The residency-pin runner-write
// boundary (1.6), the `residency_verify` report (12.4), the two EU adapters, and the autoscaler that
// sizes the per-(region, label-class) pool to the scheduler's queue depth.
pub use fleet::{
    AutoscalePolicy, Autoscaler, BareMetalPxeAdapter, CrossRegionRunnerWrite, EuFleetProvider,
    FleetAdapter, FleetError, FleetEvent, FleetPools, FleetResidencyReport, GenericEuIaasAdapter,
    PoolKey, RunnerWritePin, ScalePlan, COUNT_RUNNERS_BY_POOL_QUERY, DELETE_RUNNER_QUERY,
    INSERT_RUNNER_QUERY,
};

// CI-P13 (P-356): the scheduler fairness slice — DRR fair-share over `fair_key` + the lane shed
// order + per-tenant backpressure (the slice CI-P12 named as its floor). The deficit advance/
// replenish (plan-weighted) + the bounded run-queue cap + the lane shed order, with the live
// `fair_deficit`/`job_queue` SQL the integration test proves against Postgres.
pub use fairness::{
    shed_order, Backpressure, FairShare, PlanTier, ADVANCE_DEFICIT_QUERY, BASE_QUANTUM,
    DEFAULT_TENANT_IN_FLIGHT_CAP, DEFICIT_CEILING, IN_FLIGHT_COUNT_QUERY, REPLENISH_DEFICIT_QUERY,
};

pub use migrations::CI_RUN_SURFACE_INDEX_READINESS;
pub use migrations::{
    ci_controlplane_hot_tables, ci_controlplane_migrations, ci_durable_hot_tables,
    ci_durable_migrations, make_tenant_scoped_ddl, ALTER_CI_JOB_ACCOUNTING_ADD_SKIPPED_DDL,
    ALTER_CI_JOB_SPEC_ADD_STAGE_DDL, ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL,
    ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL, ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL,
    ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL, ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL,
    ALTER_JOB_QUEUE_ADD_COMPLETION_DDL, ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL, ARTIFACT_TABLE,
    CACHE_ENTRY_TABLE, CHECK_ATTEMPT_TABLE, CI_COST_EVENT_TABLE, CI_DRIVE_MANIFEST_TABLE,
    CI_DURABLE_WRITER_IDS, CI_JOB_ACCOUNTING_SKIPPED_MIGRATION_ID, CI_JOB_ACCOUNTING_TABLE,
    CI_JOB_QUEUE_CLAIM_AUTHORITY_MIGRATION_ID, CI_JOB_QUEUE_CLAIM_TIME_MIGRATION_ID,
    CI_JOB_QUEUE_COMPLETION_MIGRATION_ID, CI_JOB_QUEUE_RETRY_ATTEMPTS_MIGRATION_ID,
    CI_JOB_RUN_LEDGER_INDEX, CI_JOB_RUN_LEDGER_INDEX_MIGRATION_ID,
    CI_JOB_RUN_LEDGER_VALIDATION_MIGRATION_ID, CI_JOB_SPEC_STAGE_MIGRATION_ID, CI_JOB_SPEC_TABLE,
    CI_JOB_TABLE, CI_REGION_SCHEDULER_RLS_MIGRATION_ID, CI_RUN_CAUSAL_PROVENANCE_MIGRATION_ID,
    CI_RUN_CHECK_ATTEMPT_TABLE, CI_RUN_CONCURRENCY_GROUP_MIGRATION_ID,
    CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID, CI_RUN_QUEUED_REGION_INDEX,
    CI_RUN_QUEUED_REGION_INDEX_MIGRATION_ID, CI_RUN_SURFACE_REPO_CREATED_INDEX,
    CI_RUN_SURFACE_REPO_CREATED_INDEX_MIGRATION_ID, CI_RUN_TABLE,
    CI_SCHEDULER_CI_RUN_DISCOVERY_MIGRATION_ID, CI_SCHEDULER_CI_WORKFLOW_DISCOVERY_MIGRATION_ID,
    CI_SCHEDULER_CLAIM_NONCE_GRANT_MIGRATION_ID, CI_SCHEDULER_CLAIM_TIME_GRANT_MIGRATION_ID,
    CI_SCHEDULER_LEASE_EPOCH_GRANT_MIGRATION_ID, CI_WORKFLOW_ACTIVE_REGION_INDEX_MIGRATION_ID,
    CREATE_ARTIFACT_DDL, CREATE_CACHE_ENTRY_DDL, CREATE_CHECK_ATTEMPT_DDL,
    CREATE_CI_COST_EVENT_DDL, CREATE_CI_DRIVE_MANIFEST_DDL, CREATE_CI_JOB_ACCOUNTING_DDL,
    CREATE_CI_JOB_DDL, CREATE_CI_JOB_RUN_LEDGER_INDEX_DDL, CREATE_CI_JOB_SPEC_DDL,
    CREATE_CI_REGION_SCHEDULER_RLS_DDL, CREATE_CI_RUN_CHECK_ATTEMPT_DDL, CREATE_CI_RUN_DDL,
    CREATE_CI_RUN_QUEUED_REGION_INDEX_DDL, CREATE_CI_RUN_SURFACE_REPO_CREATED_INDEX_DDL,
    CREATE_DEPLOYMENT_DDL, CREATE_ENVIRONMENT_DDL, CREATE_FAIR_DEFICIT_DDL, CREATE_JOB_QUEUE_DDL,
    CREATE_JOB_QUEUE_INDEXES_DDL, CREATE_LOG_ANCHOR_DDL, CREATE_LOG_SEGMENT_DDL, CREATE_RUNNER_DDL,
    CREATE_SECRET_BINDING_DDL, DEPLOYMENT_TABLE, ENVIRONMENT_TABLE, FAIR_DEFICIT_TABLE,
    GRANT_SCHEDULER_CI_RUN_DISCOVERY_DDL, GRANT_SCHEDULER_CLAIM_NONCE_DDL,
    GRANT_SCHEDULER_CLAIM_TIME_DDL, GRANT_SCHEDULER_LEASE_EPOCH_DDL, JOB_QUEUE_TABLE,
    JQ_CLAIMABLE_INDEX, JQ_IDEM_INDEX, JQ_SERIALIZE_INDEX, LOG_ANCHOR_TABLE, LOG_SEGMENT_TABLE,
    RUNNER_TABLE, SECRET_BINDING_TABLE, VALIDATE_CI_JOB_RUN_LEDGER_INDEX_DDL,
};

pub use permanent_gates::{
    ci_restore_verify_stores, m4_boundary_permanent_gates, run_ci_restore_verify_or_fail,
    PermanentGate, PermanentGateKind,
};

pub use floor_followons::{
    all_floor_followons, FloorFollowOn, TriggerStatus, DEFERRED_BY_REFERENCE_FLOORS,
    MEASURED_TRIGGER_FLOORS,
};

// CI-P30 (P-490): the 30× CI surge family (CI-D2) — the interactive lane holds, the batch/CI lane sheds,
// the tuned DRR/shed-budget numbers + the pre-warm buffer sizing + the measured per-`fair_key`
// starvation signal (CI-P29's hierarchical-scheduler promotion gate). WIRES the existing shed lane
// (Surface::CiDispatch) + the DRR fair-share (fairness) + the dead-runner reaper (scheduler) — no
// parallel second implementation (EI-01 §7).
pub use surge::{
    drive_ci_d2_surge, CiDispatchShed, CiSurgeControls, CiSurgeGate, CiSurgeReport,
    StarvationHistogram, CI_SURGE_MULTIPLIER,
};

// CI-P31 (P-491): world-scale hardening at CELL scale — the CI-R3 residency-at-scale drill (in-region
// runner only; logs/artifacts/caches never leave region; residency_verify attests; residency-pin lint
// green) + the CI-D10 self-hosted-runner trust-boundary drill (a compromised self-hosted runner is
// bounded by its scoped token to its own tenant's SelfHosted jobs; 0 cross-tenant job/secret reads;
// attestation failure → cannot claim). The cell-scale DRILL layer over CI-P14/CI-P22/CI-P4 (no fork).
pub use residency_drill::{
    drive_ci_d10_self_hosted_boundary, drive_ci_r3_residency, CellJob, CiD10Report, CiR3Report,
    CiStoreResidency,
};

/// The deployable service name (the `AppSpec::name` + the telemetry/trace service identifier). The
/// `ci-controlplane` binary (`src/main.rs`) and the `AppSpec` both read this.
pub const SERVICE_NAME: &str = "ci-controlplane";

/// The critical-dependency set the metrics-health readiness probe reads (§4.3, SUB-D9 / arch 00 §4).
/// The OLTP store is implicitly critical (the harness adds it). The CI Control Plane declares:
/// - `broker` — the durable bus the check emitter / log-pipeline coordinator publishes to (an
///   outbox→bus producer cannot serve correct traffic without it);
/// - `authz` — Identity's `check`/`list_objects` the trust-tier evaluation + the surfacing
///   push-down depend on (a dead authz means CI cannot make correct trust/visibility decisions);
/// - `runner_pool` — at-least-one-healthy-runner-pool (arch 00 §4: a control plane with NO runner
///   pool cannot dispatch any job, so it reports not-ready + sheds rather than queuing into a void).
///
/// A dead critical dependency reports not-ready + sheds while liveness stays Up (no restart storm).
fn controlplane_critical() -> CriticalDependencies {
    CriticalDependencies::new(["broker", "authz", "runner_pool"])
}

/// **Assemble the CI Control Plane service [`AppSpec`] (contract 1.1; the service shell).** The
/// harness owns the lifecycle around it (boot → migrate → relay → consumers → three ports →
/// graceful drain, liveness ≠ readiness). The CI Control Plane is an `AppSpec` + handlers, NOT a
/// hand-rolled `main`.
///
/// `config` is the validated, env-first config (§3.2; `Config::from_env()` lands with the driver,
/// P-S15 — the shell boots over the validated default today). The complete forward-only data-model
/// migrations create all eighteen control-plane tables `(tenant, region)`-first + RLS-on; the five
/// hot tables are declared; `broker` / `authz` / `runner_pool` are declared critical. No consumers
/// are registered here — the scheduler/check-emitter behaviour is the per-table follow-ons (named in
/// [`migrations`]); the dedup consumer is the Trigger & Dispatch shell.
///
/// **The outbox is INJECTED (MR-009b W3b.6 — the W3b.4 debt discharged):** the production
/// `main.rs` constructs `OutboxStore::durable(PgOutboxBacking)` over the MR-022
/// `SubstrateProvider` pool (foundation migrations applied, FAIL LOUD on missing durable config);
/// a test/drill passes the `test-support`-gated in-memory `OutboxStore::new()` double. This
/// builder constructs NO store of its own — the issues/flow W3b.4 injection pattern.
pub fn controlplane_app_spec(config: Config, outbox: myelin_events::OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: ci_controlplane_migrations(),
        hot_tables: ci_controlplane_hot_tables(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        // No consumers at the shell — the scheduler is not a bus consumer; the dedup consumer is the
        // Trigger & Dispatch shell. The check emitter is an outbox PRODUCER (CI-P18), not registered
        // as a consumer here.
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        // The implicit OLTP store (the harness adds it) is the only store the control plane owns at
        // the shell — every control-plane table lives in the one Postgres; the blob/cache/log-tier
        // stores are declared by their behaviour bands (CI-P20/CI-P22). Auto-registered as holders.
        stores: myelin_substrate::StoreManifest::new(),
        // Producer-only: the separately elected cell relay publishes the shared durable outbox.
        // This process must not claim rows belonging to another subsystem.
        outbox: OutboxSpec::external_relay(outbox),
        critical: controlplane_critical(),
    }
}

/// **Boot the CI Control Plane service to the pre-serve [`ServeHandle`]** (the harness's [`boot`] of
/// [`controlplane_app_spec`]). Separated from [`run_controlplane`] so a test/drill can boot, assert
/// the three ports opened + the migrations ran + the holders registered, drive ticks, and drive the
/// drain deterministically.
pub fn boot_controlplane(
    config: Config,
    outbox: myelin_events::OutboxStore,
) -> Result<ServeHandle, ServeError> {
    boot(controlplane_app_spec(config, outbox))
}

/// **The deterministic CI Control Plane lifecycle entry.** This synchronous one-tick wrapper is
/// retained for shell tests and drills; the production binary uses
/// [`run_controlplane_until_shutdown`] so it remains live until an OS shutdown signal arrives.
/// A failed boot / incomplete drain returns non-zero (§3.1) — loud, never a silent success.
pub fn run_controlplane(
    config: Config,
    outbox: myelin_events::OutboxStore,
) -> Result<(), ServeError> {
    serve(controlplane_app_spec(config, outbox))
}

/// Run the production control plane until the supplied OS-shutdown future resolves, then let the
/// shared harness stop intake and drain in-flight work. The synchronous [`run_controlplane`] remains
/// the deterministic one-tick shell-test entry.
pub async fn run_controlplane_until_shutdown<F>(
    config: Config,
    outbox: myelin_events::OutboxStore,
    shutdown: F,
) -> Result<(), ServeError>
where
    F: std::future::Future<Output = ()>,
{
    myelin_substrate::serve_until_shutdown(controlplane_app_spec(config, outbox), shutdown).await
}

/// **Construct the durable CI `ci_cost_event` projection store at the composition root (CT-004a).**
/// The service `main` builds this from the MR-022 `SubstrateProvider` pool (`provider.db_pool().clone()`)
/// after the migrations have run, so the metering path has a real, production-callable
/// [`CiCostEventStore`] — not the prior model-only SQL-constants-plus-in-memory-model. It is a thin,
/// explicit composition seam (over [`CiCostEventStore::with_pg`]) so the wiring point is named + the
/// CT-004d follow-on is documented in ONE place.
///
/// **DORMANT at the shell (CT-004a scope).** No consumer drives this yet: constructing it only wraps
/// the pool (no query runs at boot), so it is safe to build in the composition root even though the
/// live settle path is not attached. **CT-004d** attaches it to the `SCHEDULE_AND_RUN_JOB` dispatch
/// settle bookend (a real `job.done` co-commits its run-state transition + `settle_in_tx` here).
///
/// **CT-004m — the `cost_event` table-name collision is RESOLVED.** The platform runs ONE shared
/// `myelin` Postgres for every service (docs/dev-stack.md — NOT "each service its own DB"). Storage's
/// money-ledger `cost_event` (migration `0050`; `(tenant, region, run_id text, ord, unit, wholesale,
/// markup)`) and CI's metering projection formerly BOTH named `cost_event` → `CREATE TABLE IF NOT
/// EXISTS cost_event` no-op'd whichever applied second, silently. CT-004m RENAMED CI's table to
/// `ci_cost_event` ([`crate::migrations::CI_COST_EVENT_TABLE`] / [`crate::INSERT_COST_EVENT_QUERY`]),
/// so the two DISTINCT-shaped tables coexist in the shared DB. The rename was safe in place: CI was
/// fully dormant (no `ci_*` migration had ever been applied to any real DB). This store now targets
/// `ci_cost_event`; the tables it needs are created by [`crate::ci_durable_migrations`] (applied by
/// BOTH CI service mains at boot). **CT-004d** remains the follow-on that DRIVES a live settle through
/// this store (attaching it to the dispatch settle bookend with a tenant-scoped tx for the FORCE-RLS
/// `ci_cost_event` table) — the SCHEMA it writes to is now sound.
pub fn ci_cost_event_store(pool: sqlx::PgPool, region: myelin_tenancy::Region) -> CiCostEventStore {
    CiCostEventStore::with_pg(pool, region)
}

/// Construct the immutable per-job accounting receipt store at the control-plane composition root.
/// The production terminal reporter requires this store explicitly; there is no unaccounted
/// production constructor or default pricing adapter.
pub fn ci_job_accounting_store(
    pool: sqlx::PgPool,
    region: myelin_tenancy::Region,
) -> CiJobAccountingStore {
    CiJobAccountingStore::with_pg(pool, region)
}

/// **Construct the durable CI `job_queue` store at the composition root (CT-004c.1).** The service
/// `main` builds this from the MR-022 `SubstrateProvider` pool (`provider.db_pool().clone()`) after
/// the migrations have run, so the scheduler has a real, production-callable [`CiJobQueueStore`] — not
/// the prior two-form (SQL-constants + in-memory `SchedulerState` model). A thin composition seam
/// (over [`CiJobQueueStore::with_pg`]) so the wiring point is named in ONE place. The `job_queue` +
/// `fair_deficit` tables it needs are created by the full [`ci_controlplane_migrations`] the
/// ci-controlplane `serve(AppSpec)` applies at boot (job_queue is the control plane's hot claim
/// surface — single-owner, so it stays in the full control-plane migration set, NOT the shared
/// `ci_durable_migrations` writer subset that exists for tables BOTH CI mains write; ci-dispatch does
/// not touch `job_queue`).
///
/// **CT-004c.2** binds the `RunnerAgent` to this store (long-poll [`CiJobQueueStore::claim`] → lease a
/// row → hand it to the AG-D4-gated sandbox executor → heartbeat/complete) and starts the pipeline
/// body — the surfaces the adversarial verifier must cover (the trust-tier claim predicate + the
/// sandbox exec path). CT-004c.1 leases a row and launches NOTHING.
pub fn ci_job_queue_store(pool: sqlx::PgPool) -> CiJobQueueStore {
    CiJobQueueStore::with_pg(pool)
}

/// Test-only raw-pool seam for legacy scheduler algorithm integration fixtures. Production must
/// construct this capability only through [`CiSchedulerDbProvider::region_queue_store`].
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn ci_region_queue_store_test_support(pool: sqlx::PgPool) -> CiRegionQueueStore {
    CiRegionQueueStore::with_pg(pool)
}

/// Test-only raw-pool seam for starter-poller integration fixtures. Production must construct this
/// capability only through [`CiSchedulerDbProvider::region_run_discovery`].
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn ci_region_run_discovery_test_support(pool: sqlx::PgPool) -> CiRegionRunDiscovery {
    CiRegionRunDiscovery::with_pg(pool)
}

/// **Construct the durable CI `ci_job_spec` store at the composition root (CT-004d.1).** The service
/// `main` builds this from the MR-022 `SubstrateProvider` pool after the migrations have run, so the
/// dispatch has a real, production-callable [`CiJobSpecStore`] to co-persist a stage's [`JobSpec`] into
/// and the runner's real resolver ([`durable_spec_resolver`]) has a store to read back from. A thin
/// composition seam (over [`CiJobSpecStore::with_pg`]) so the wiring point is named in ONE place. The
/// `ci_job_spec` table it needs is created by the full [`ci_controlplane_migrations`] the
/// ci-controlplane `serve(AppSpec)` applies at boot — a single-owner control-plane table (the
/// control-plane dispatch writes it, the control-plane runner reads it), so it stays in the full set,
/// NOT the shared `ci_durable_migrations` writer subset (the SAME posture as `job_queue`; ci-dispatch
/// does not touch `ci_job_spec` in CT-004d.1 — the dispatch consumer is CT-004d.2).
pub fn ci_job_spec_store(pool: sqlx::PgPool) -> CiJobSpecStore {
    CiJobSpecStore::with_pg(pool)
}

/// **Construct the durable CI `ci_run` store at the composition root (CT-004d.2 chunk 4).** The service
/// `main` builds this from the MR-022 `SubstrateProvider` pool after the migrations have run, so the
/// `ci-dispatch.trigger` consumer's reserve path has a real, production-callable [`CiRunStore`] to
/// co-commit the run-of-record ROW into (on the consumer's co-commit `HandlerTx` connection — atomic
/// with the dedup mark) and the run-view / check-emitter read path has a store to resolve `ci_run` from.
/// A thin composition seam (over [`CiRunStore::with_pg`]) so the wiring point is named in ONE place. The
/// `ci_run` table it writes is created by the SHARED [`ci_durable_migrations`] BOTH CI mains apply at
/// boot (`ci_0001_ci_run` — the writer subset, not the full control-plane set), so the writer table
/// exists regardless of which CI service booted first.
pub fn ci_run_store_factory(pool: sqlx::PgPool) -> CiRunStore {
    CiRunStore::with_pg(pool)
}

/// **Construct the per-tenant ci_run starter factory at the composition root (CT-004 — the ci_run-poll
/// STARTER floor).** The service `main` builds this from the MR-022 `SubstrateProvider` pool (after the
/// migrations have run) + the cell region + the blob CAS, so the runner-lane autonomy wire has a real,
/// production-callable [`PgCiRunStarterFactory`] that mints an exact-cell [`PgCiPipelineStarter`] for a
/// discovered run's authoritative tenant. A thin composition seam (over [`PgCiRunStarterFactory::new`])
/// so the wiring point is named in ONE place. The durable aggregate and full
/// [`ci_controlplane_migrations`] create the `ci_run` / `ci_job` tables; Flow's exact shared
/// migrations create `workflow_run` / `wf_definition`, and production applies those before the CI
/// recovery follow-ons.
///
/// **Explicitly activation-gated.** `main` composes this behind the same `MYELIN_CI_RUNNER=1` seam as
/// the runner lane. Unset / `0` leaves the factory unconstructed and no minted starter is driven.
/// Constructing it wraps the pool + blob client only — no query runs — so construction alone cannot
/// start a queued run. Production construction binds the real fixed,
/// default-deny `linux-small-v1` policy and the PostgreSQL Tier-P operational reservation source.
/// Its outstanding-reservation ceiling is deliberately distinct from the scheduler's measured
/// leased/running cap: it admits one largest valid run for an otherwise-idle tenant while bounding
/// queued durable authority. This is a Tier-P safety/metering seam, not a Commercial wallet or
/// billing integration. The token-authority reference is already
/// content-bound to the complete immutable request, but is not a bearer or mint.
/// [`PgCiRunStarterPoller`] is attached to coordinated lifecycle shutdown with the deployed
/// definition pin and the exact manifest-to-sandbox runner/token authority and terminal
/// settlement path.
pub fn ci_run_starter_factory(
    pool: sqlx::PgPool,
    region: myelin_tenancy::Region,
    blobs: std::sync::Arc<dyn myelin_storage::BlobStore + Send + Sync>,
    rt: tokio::runtime::Handle,
    supersession_ledger: myelin_storage::DurableCostLedger,
) -> Result<PgCiRunStarterFactory, CiLaunchAuthorityError> {
    let reservations = ci_launch_authority::PgTierPCiJobBudgetReservation::new(
        pool.clone(),
        region.0.clone(),
        ci_launch_authority::TIER_P_OPERATIONAL_ACTIVE_RESERVATION_CEILING,
    )?;
    Ok(PgCiRunStarterFactory::new_with_authority_and_supersession(
        pool,
        rt,
        std::sync::Arc::new(myelin_events::MonotonicMinter::new()),
        region,
        blobs,
        std::sync::Arc::new(ci_launch_authority::LinuxSmallV1LaunchAuthority::new(
            std::sync::Arc::new(reservations),
        )),
        supersession_ledger,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{Liveness, Surface};

    /// **THE CI Control Plane shell boot test (contract 1.1/1.2/1.3): boots from `serve(AppSpec)`
    /// with three ports + liveness ≠ readiness; the complete forward-only data model applies.** This
    /// is the prompt's GATE: the shell compiles + boots from `serve(AppSpec)` with the three-surface
    /// split and liveness ≠ readiness, and the forward-only migrations create every CI table.
    #[test]
    fn controlplane_boots_from_serve_appspec_with_three_ports() {
        let handle = boot_controlplane(Config::default(), myelin_events::OutboxStore::new())
            .expect("the CI Control Plane shell boots from serve(AppSpec)");
        assert_eq!(handle.name(), SERVICE_NAME, "the deployable service name");

        // (1.2) the three ports opened in the lifecycle (public / internal / metrics-health).
        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports opened (contract 1.2)"
        );

        // (1.3) liveness ≠ readiness: after a successful boot the startup gate is Complete, so
        // readiness is governed by the critical-dependency health (not the same signal as liveness).
        let mh = handle.metrics_health();
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness = not-wedged (never checks a dependency)"
        );
        assert!(
            mh.readiness().is_ready(),
            "readiness = can-serve-now (all critical deps healthy at boot) — distinct from liveness"
        );
    }

    /// **A dead critical dependency (`runner_pool`) flips readiness to not-ready WITHOUT flipping
    /// liveness (liveness ≠ readiness, contract 1.3 / SUB-D9 / arch 00 §4).** The CI Control Plane
    /// cannot dispatch a job with NO healthy runner pool, so it reports not-ready + sheds — but it
    /// stays live (no restart storm). This proves the readiness probe gates on the runner pool (arch
    /// 00 §4: readiness = DB + broker + authz + at-least-one-healthy-runner-pool).
    #[test]
    fn dead_runner_pool_flips_readiness_not_liveness() {
        let handle =
            boot_controlplane(Config::default(), myelin_events::OutboxStore::new()).expect("boot");
        let mh = handle.metrics_health();
        assert!(
            mh.readiness().is_ready(),
            "ready while the runner pool is healthy"
        );

        // Mark the declared-critical `runner_pool` dependency down.
        handle.health_probe().mark_down("runner_pool");

        assert!(
            !mh.readiness().is_ready(),
            "no healthy runner pool → not-ready + shed (arch 00 §4)"
        );
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness stays UP (not-ready is NOT not-alive — no restart storm)"
        );

        // The other two declared critical deps (broker, authz) also gate readiness.
        let handle2 =
            boot_controlplane(Config::default(), myelin_events::OutboxStore::new()).expect("boot");
        handle2.health_probe().mark_down("authz");
        assert!(
            !handle2.metrics_health().readiness().is_ready(),
            "a dead authz also flips readiness (the trust/visibility decision dependency)"
        );
    }

    /// **The CI Control Plane shell runs the whole lifecycle end-to-end and drains cleanly (contract
    /// 1.1).** `run_controlplane` boots → migrates (creates every CI table) → … → graceful-drains →
    /// returns Ok. The CDC consumer side of 1.1 (a service `main` that just calls the one entry).
    #[test]
    fn run_controlplane_runs_lifecycle_and_returns_ok() {
        assert_eq!(
            run_controlplane(Config::default(), myelin_events::OutboxStore::new()),
            Ok(()),
            "the CI Control Plane shell boots → … → drains cleanly"
        );
    }

    #[tokio::test]
    async fn production_controlplane_waits_for_shutdown_then_drains() {
        assert_eq!(
            run_controlplane_until_shutdown(
                Config::default(),
                myelin_events::OutboxStore::new(),
                async {},
            )
            .await,
            Ok(())
        );
    }

    /// **A failed boot returns non-zero (§3.1).** A config that fails boot-time validation aborts
    /// boot loudly — the shell never starts half-booted.
    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_controlplane(Config("BAD_POOL".into()), myelin_events::OutboxStore::new());
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(
            r.unwrap_err().0.contains("fail-fast"),
            "the error names the §3.2 fail-fast validation"
        );
    }

    /// **The shell's AppSpec carries the complete data model + the five hot tables + the critical
    /// deps, and NO consumers (the behaviour floor).** Pins the shell's surface so a later edit that
    /// smuggles in a consumer without reconciliation, or drops a table / a hot-table flag, is loud.
    #[test]
    fn the_shell_carries_the_complete_data_model_and_no_consumers() {
        let spec = controlplane_app_spec(Config::default(), myelin_events::OutboxStore::new());
        assert_eq!(
            spec.migrations.0.len(),
            43,
            "all 18 tables, three ci_run forward ALTERs, 8 concurrent indexes, the ledger validator, 4 job_queue ALTERs, the ci_job_spec-stage and accounting-skipped ALTERs, scheduler RLS boundary, 3 claim-column grants, and 3 ci_run/workflow discovery grants are present"
        );
        assert!(
            spec.consumers.is_empty(),
            "no consumers at the shell (the scheduler is not a bus consumer; dedup is the dispatch shell)"
        );
        // the five hot tables are declared.
        for t in [
            JOB_QUEUE_TABLE,
            LOG_SEGMENT_TABLE,
            CI_COST_EVENT_TABLE,
            CHECK_ATTEMPT_TABLE,
            CI_RUN_CHECK_ATTEMPT_TABLE,
        ] {
            assert!(spec.hot_tables.is_hot(t), "`{t}` is declared hot");
        }
        // the three critical downstreams are declared (beyond the implicit OLTP store).
        let deps: Vec<&str> = spec.critical.deps().iter().map(|d| d.0.as_str()).collect();
        assert!(deps.contains(&"broker"), "broker is critical");
        assert!(deps.contains(&"authz"), "authz is critical");
        assert!(deps.contains(&"runner_pool"), "runner_pool is critical");
    }
}
