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
//!     all fourteen CI Control-Plane tables (`ci_run`, `ci_job`, `check_attempt`, `job_queue` +
//!     its three claim indexes, `fair_deficit`, `runner`, `log_segment`, `log_anchor`, `artifact`,
//!     `cache_entry`, `environment`, `deployment`, `secret_binding`, `cost_event`), each
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
pub mod ci_pipeline;
pub mod ci_result_signal;
pub mod deployment;
pub mod events;
pub mod fairness;
pub mod fleet;
pub mod holder;
pub mod live_tail;
pub mod log_pipeline;
pub mod metering;
pub mod migrations;
pub mod rebac_fragment;
pub mod schedule_and_run_job;
pub mod scheduler;
pub mod schema;
pub mod secret_broker;
pub mod supply_chain;
pub mod surfacing;
pub mod surfacing_index;
pub mod surfacing_tools;

// CI-P15 (P-358): the `ci.pipeline` DURABLE WORKFLOW BODY + the X-1 producer side. The deterministic
// Rust body registered under `CI_PIPELINE_WF_TYPE` at serve (guarded by the flow-determinism lint):
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
    meter_resource_seconds, metered_units_for, reserve_settle_parity_drill, CiMeter, CostEventRow,
    CostKind, FlatBpsMarkup, MarkupPolicy, Meter, MeteredResource, ReserveSettleParitySignal,
};

pub use holder::{
    ci_store_classifier, register_ci_holders, CiHolder, CiHolderRegistration, CiStoreClass,
    RestrictionFlag, CI_OLTP_STORE, CI_RESIDUAL_POSTURE_REF,
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
    SchedulerState, CANCEL_SUPERSEDED_QUERY, CLAIM_QUERY, REAP_QUERY,
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

pub use migrations::{
    ci_controlplane_hot_tables, ci_controlplane_migrations, make_tenant_scoped_ddl, ARTIFACT_TABLE,
    CACHE_ENTRY_TABLE, CHECK_ATTEMPT_TABLE, CI_JOB_TABLE, CI_RUN_TABLE, COST_EVENT_TABLE,
    CREATE_ARTIFACT_DDL, CREATE_CACHE_ENTRY_DDL, CREATE_CHECK_ATTEMPT_DDL, CREATE_CI_JOB_DDL,
    CREATE_CI_RUN_DDL, CREATE_COST_EVENT_DDL, CREATE_DEPLOYMENT_DDL, CREATE_ENVIRONMENT_DDL,
    CREATE_FAIR_DEFICIT_DDL, CREATE_JOB_QUEUE_DDL, CREATE_JOB_QUEUE_INDEXES_DDL,
    CREATE_LOG_ANCHOR_DDL, CREATE_LOG_SEGMENT_DDL, CREATE_RUNNER_DDL, CREATE_SECRET_BINDING_DDL,
    DEPLOYMENT_TABLE, ENVIRONMENT_TABLE, FAIR_DEFICIT_TABLE, JOB_QUEUE_TABLE, JQ_CLAIMABLE_INDEX,
    JQ_IDEM_INDEX, JQ_SERIALIZE_INDEX, LOG_ANCHOR_TABLE, LOG_SEGMENT_TABLE, RUNNER_TABLE,
    SECRET_BINDING_TABLE,
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
/// migrations create all fourteen control-plane tables `(tenant, region)`-first + RLS-on; the four
/// hot tables are declared; `broker` / `authz` / `runner_pool` are declared critical. No consumers
/// are registered here — the scheduler/check-emitter behaviour is the per-table follow-ons (named in
/// [`migrations`]); the dedup consumer is the Trigger & Dispatch shell.
pub fn controlplane_app_spec(config: Config) -> AppSpec {
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
        outbox: OutboxSpec::default(),
        critical: controlplane_critical(),
    }
}

/// **Boot the CI Control Plane service to the pre-serve [`ServeHandle`]** (the harness's [`boot`] of
/// [`controlplane_app_spec`]). Separated from [`run_controlplane`] so a test/drill can boot, assert
/// the three ports opened + the migrations ran + the holders registered, drive ticks, and drive the
/// drain deterministically.
pub fn boot_controlplane(config: Config) -> Result<ServeHandle, ServeError> {
    boot(controlplane_app_spec(config))
}

/// **The CI Control Plane service entry — the one `serve(AppSpec)` call (contract 1.1).** The
/// `ci-controlplane` binary (`src/main.rs`) does nothing but hand [`controlplane_app_spec`] to this.
/// A failed boot / incomplete drain returns non-zero (§3.1) — loud, never a silent success.
pub fn run_controlplane(config: Config) -> Result<(), ServeError> {
    serve(controlplane_app_spec(config))
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
        let handle = boot_controlplane(Config::default())
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
        let handle = boot_controlplane(Config::default()).expect("boot");
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
        let handle2 = boot_controlplane(Config::default()).expect("boot");
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
            run_controlplane(Config::default()),
            Ok(()),
            "the CI Control Plane shell boots → … → drains cleanly"
        );
    }

    /// **A failed boot returns non-zero (§3.1).** A config that fails boot-time validation aborts
    /// boot loudly — the shell never starts half-booted.
    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_controlplane(Config("BAD_POOL".into()));
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(
            r.unwrap_err().0.contains("fail-fast"),
            "the error names the §3.2 fail-fast validation"
        );
    }

    /// **The shell's AppSpec carries the complete data model + the four hot tables + the critical
    /// deps, and NO consumers (the behaviour floor).** Pins the shell's surface so a later edit that
    /// smuggles in a consumer without reconciliation, or drops a table / a hot-table flag, is loud.
    #[test]
    fn the_shell_carries_the_complete_data_model_and_no_consumers() {
        let spec = controlplane_app_spec(Config::default());
        assert_eq!(
            spec.migrations.0.len(),
            14,
            "all 14 control-plane tables are in the forward-only migration set"
        );
        assert!(
            spec.consumers.is_empty(),
            "no consumers at the shell (the scheduler is not a bus consumer; dedup is the dispatch shell)"
        );
        // the four hot tables are declared.
        for t in [
            JOB_QUEUE_TABLE,
            LOG_SEGMENT_TABLE,
            COST_EVENT_TABLE,
            CHECK_ATTEMPT_TABLE,
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
