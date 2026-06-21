//! # `myelin-agent-service` — the Agent-Fabric data model (run / tool_def / proposed_effect /
//! hitl_gate / trace), `(tenant, region)`-first + RLS + the tenant-predicate lint (AG-P2 / P-131)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/agent-fabric.md`
//! §4 (the data model — the five tables, all `(tenant, region)`-first, RLS-enforced, no
//! cross-tenant query path, residency-pinned, per-tenant envelope-encrypted, `PersonalDataHolder`;
//! the exact field lists §4.1..§4.5). Carried forward from Phase-3 §4.
//!
//! **Contract-index:** rows 12.1 (the Tenancy `(tenant, region)` partition key from the verified
//! token), 11.3/11.4 (per-subject DEK envelope encryption), 1.5 (forward-only online migrations),
//! 1.6 (the `tenant-predicate` + `forward-only-migration` lints — the loud, committed ratchet
//! gates). Implemented to the frozen shapes.
//!
//! **VISION §3** (GDPR-safe & EU-sovereign by construction: residency + RLS are *architectural*,
//! not a runtime check that can be forgotten). **EI-01 §5** (the committed ratchet — the
//! tenant-predicate + forward-only-migration lints are loud gates), **§3** (a property does not
//! exist until a test forces it — the cross-tenant-read denial test, `tests/integration_rls.rs`).
//!
//! ## This crate is the IMPLEMENTATION crate, distinct from the glue crate `myelin-agent`
//! `myelin-agent` (AG-P1 → P-130) is the **glue crate** — the frozen six-trait contract surface +
//! the `ToolDef` / `EffectKind` / `EffectResult` value types, NO engine logic. **This** crate is the
//! Fabric's **service / implementation** crate (architecture §4 names them distinct). At AG-P2 it
//! ships the **data model**: the five forward-only `(tenant, region)`-first RLS migrations + the
//! schema row tag-carriers. The runtime that drives these tables lands later: the SKELETON runtime
//! (AG-P4 → P-216), `MockAgentRuntime` (AG-P5 → P-217), the plan-then-apply `EffectApi` pipeline
//! (AG-P6 → P-218).
//!
//! ## The five tables (architecture §4.1..§4.5) — all `(tenant, region)`-first, RLS-enforced
//! - **`run`** (§4.1) — the unit of agent execution, a durable-workflow instance (ADR-09). A run may
//!   pause for *days* on a HITL gate holding no thread.
//! - **`tool_def`** (§4.2) — the one permissioned registry. The `requires_approval` COLUMN exists
//!   here; its per-subsystem **seed defaults** land in AG-P8 (→ P-220), not here.
//! - **`proposed_effect`** (§4.3) — the plan-then-apply audit row: every proposed effect recorded
//!   whether applied, gated, or denied.
//! - **`hitl_gate`** (§4.4) — the approval state, a durable-workflow wait surfaced as a chat card.
//! - **`trace`** (§4.5) — the content-addressed execution-trace pointer (`run.trace_ref` is its
//!   `ArtifactRef`). The trace is a `PersonalDataHolder`; the holder body lands with Knowledge in M3
//!   (AG-P19 → P-268). Here the column + the residency pin exist.
//!
//! ## The `(tenant, region)`-first + RLS construction (the IDOR floor — storage §1.1)
//! Every one of the five tables leads with `(tenant_id, region)` and is made RLS-ready by the
//! `myelin_make_tenant_scoped(table)` convention the dev/prod Postgres init installs
//! (`scripts/pg-init/00-rls-conventions.sql`): `ENABLE` + `FORCE ROW LEVEL SECURITY` + the standard
//! `(tenant_id, region)` isolation policy keyed on `current_setting('myelin.tenant_id')` /
//! `current_setting('myelin.region')`. The app role is `NOSUPERUSER NOBYPASSRLS`, so a session set
//! to tenant A reads **only** tenant A's rows — **0 cross-tenant rows readable**, enforced in
//! Postgres, not just app code. The migrations run through the storage forward-only **online**
//! runner ([`myelin_storage::migration::OnlineMigrationRunner`]) so they are forward-only by
//! construction (a `DROP` / a blocking `ALTER` on a hot table / a contract-before-backfill is
//! refused). See [`migrations`].
//!
//! ## The lints this crate is bound by (contract 1.6 — PERMANENT ratchet gates)
//! - **`tenant-predicate`** — every query against a tenant-owned table threads the `(tenant,
//!   region)` predicate; a tenant-less query is a cross-tenant IDOR and is rejected. The agent-shaped
//!   red+green fixtures live in `crates/myelin-lints/tests/fixtures/tenant_predicate.agent.*` and are
//!   exercised by `crates/myelin-lints/tests/agent_lints.rs`.
//! - **`forward-only-migration`** — no rollback/down migration; no in-place rewrite; no blocking
//!   `ALTER` on a hot table; the online expand→backfill→contract shape only. Agent-shaped red+green
//!   fixtures live alongside the above (`forward_only_migration.agent.*`).
//!
//! ## Floors named (state cross-references; VISION §3)
//! - **The `PersonalDataHolder` REGISTRATION seam landed in AG-P3 (→ P-132) — [`holder`].** The five
//!   tables carry their `#[personal_data(...)]` classification tags (here, AG-P2); the holder
//!   *registration* through the substrate `HolderRegistry` (so the harness auto-registers the
//!   Fabric's H11 OLTP + H17 trace holders on boot) ships in [`holder`] (AG-P3). The holder BODIES
//!   are the named floor below.
//! - **The `PersonalDataHolder` BODIES (locate / export / erase) land in AG-P23 (→ P-1371-band).**
//!   The schema is complete and the crypto-shred lever (per-subject DEK) exists by tag here; the full
//!   DSR fan-out across all Fabric holders (run table, trace, agent memory) is the M5 follow-on
//!   (drill AG-D10 — erasure reaches the trace + memory).
//! - **The trace HOLDER body lands with Knowledge (AG-P19 → P-268, KN-D11/KN-D12).** Here the
//!   `trace` table + the `run.trace_ref` `ArtifactRef` column + the residency pin exist; the
//!   content-addressed write of the trace document into Knowledge is that follow-on.
//! - **The concrete DDL execution against a live Postgres connection** is the storage driver's
//!   (P-S12); here the [`migrations::runner`] *validates ordering + admits the online shape* and the
//!   `integration` test proves the RLS policy denies a cross-tenant read against the LIVE dev stack.
//!   The validation logic does not change shape when the driver lands.

pub mod app;
pub mod card_text;
pub mod cost_gate;
pub mod defaults;
pub mod dry_run;
pub mod effect_api;
pub mod exec;
pub mod hitl;
pub mod hitl_batch;
pub mod holder;
pub mod identity;
pub mod long_park;
pub mod loop_guards;
pub mod migrations;
pub mod mock;
pub mod schema;
pub mod skeleton;
pub mod tool_scope;

// Re-export the PersonalDataHolder registration seam (AG-P3 → P-132) at the crate root — the shape
// the harness `serve` (AG-P4) + the DSR fan-out (AG-P23) + the 10.1 CDC consume (mirrors
// myelin-refs-service / myelin-search re-exporting their holder seam).
pub use holder::{
    agent_store_classifier, register_agent_holders, AgentHolderRegistration, AgentOltpHolder,
    AgentTraceHolder, AGENT_OLTP_STORE, AGENT_TRACE_STORE,
};

// The SKELETON runtime (AG-P4 → P-216): the gateway → identity → dispatch → reserve → trace path at
// zero cost. The brain (no model, no tools) + the platform-owned `Agent::handle` durable-workflow
// loop body + the per-run identity (mint/revoke/anti-leak) + the contract-1.8 telemetry signals.
pub use skeleton::{
    ChildEnv, RunOutcomeKind, RunSubstrate, RunTokenRevoker, SkeletonAgent, SkeletonAgentRuntime,
    SkeletonError, SkeletonTelemetry, AGENT_RUN_TRACED_EVENT, SKELETON_STEP_UNIT,
};

// The reserve/settle cost gate as the runaway self-limiter (AG-P14 → P-227, M2-B, AG-D11): the
// agent-fabric CONSUMER that fronts every run through the Storage AgentRunGate/CostLedger (11.7) and
// proves the gate as the runaway self-limiter end-to-end — a MockAgentRuntime brain looping
// run-after-run against ONE draining wallet, reserve refuses past exhaustion (the loop stops at the
// wallet, never by a kill), the in-flight run is NEVER interrupted (0 interrupt), and the books
// balance (reserved == settled; a Mock bills 0 → the reservation refunds). NO FLOOR in the gate
// mechanism — real per-model-call cost metering arrives with LlmAgentRuntime (AG-P25, post-M5); the
// Mock metering ZERO is correct (the limiter is brain-independent).
pub use cost_gate::{
    runaway_brain, AgentFabricCostSignal, RunawaySelfLimiter, RunawayStep,
};

// Per-run identity COMPLETED (AG-P13 → P-225): mint at dispatch (token life == run life), scrub the
// shared platform token in the child env, revoke idempotently on teardown even on crash, and RE-MINT
// on resume after a multi-day HITL pause / long SCHEDULE_AND_RUN_JOB with the SAME caveats and the
// REMAINING run life (the §5.7 C6 clamp: TTL == min(W, remaining run life) — a long pause never
// widens the attribution window beyond the run's deadline). The AttributionWindow is the AG-D8
// re-mint-leg proof (0 unattributed window across the pause). Completes AG-P4's simple form +
// reconciles with myelin-flow's WfCtx re-mint (the engine-side lease). NO FLOOR — per-run identity is
// complete; the anti-leak scrub is RE-ASSERTED inside ToolHands::exec (AG-P15 → P-226).
pub use identity::{AttributionWindow, RunIdentity, FAIL_STATIC_W_SECS};

// The MockAgentRuntime (AG-P5 → P-217): the deterministic scripted brain on the real `--use-mock`
// code path. The scripted queue + the stateless step, the platform-owned history reconstruction
// (build_conversation), the AG-D9 step-determinism replay lever (golden + cargo-mutants), and the
// real `--use-mock` runtime flag (select_runtime behind the frozen &dyn AgentRuntime seam).
pub use mock::{
    build_conversation, model_turns_taken, replay, replay_bounded, select_runtime, HistoryEntry,
    MockAgentRuntime, MockScript, ReplayRecord, RuntimeFlag, TraceHistory, MOCK_MAX_STEPS,
};

// The plan-then-apply EffectApi pipeline (AG-P6 → P-218): the eight-step fail-closed body — SCHEMA
// → CAPABILITY → DELEGATION → TENANT → BUDGET → HITL-GATE → APPLY → METER. The owned 8.2 body + the
// consumer seams (4.2 check / 4.5 delegation / tenant / 11.7 budget / subsystem public-endpoint
// apply) + the AG-D2 denial signals (0 privileged fallback by construction).
pub use effect_api::{
    decode_proposed, encode_proposed, validate_schema, ApplyError, CapabilityCheck, DelegationLookup,
    EffectApiBridge, EffectBudget, EffectCost, PipelineSignals, PipelineStep, PlanThenApply,
    PlanVerdict, PlannedEffect, SubsystemApply, TenantGuard,
};

// ToolHands::exec on the unified sandbox (AG-P15 → P-226, M2-C, contract 8.4 — the Fabric half): the
// real `ToolHands::exec` body realised as the dispatch of CI's `kind=agent` JobSpec onto the ONE
// unified sandbox (`myelin-ci-sandbox`, CI-P1 → P-129), with NO host-exec bypass (the `no-host-exec`
// lint, 1.6, green over the crate). The ROUTING SPLIT (the safety boundary, §5.0/X-6 #3) is encoded
// in the TYPE: only `compute` builds a `SandboxJob` (`route_of`), so a `mutate`/`external` effect can
// NEVER reach exec (0 mutate-via-exec). The FOUR uniform guarantees are wired by construction (every
// dispatch inherits them — NO subsystem re-implements any): #1 reserve at dispatch (11.7, AG-P14), #2
// per-run attenuated token + the anti-leak platform-token scrub into the child env (4.7, AG-P13), #3
// HITL withhold (mutation via EffectApi, structural), #4 the FULL hardening profile fed into the
// kind=agent spec (digest-pinned fail-closed on un-digested tags, default-deny egress, pids.max +
// zero swap, read-only root + tmpfs, secrets as in-boundary refs, whole-guest kill on teardown).
// FLOORS: the ZERO-escapes real-kernel GATE proving guarantee #4 is AG-P17 (→ P-229) / CI-P5 (→
// P-239); the Firecracker backend is CI-P2 (→ P-237); SCHEDULE_AND_RUN_JOB long-park is AG-P16 (→
// P-228); the real LlmAgentRuntime against this runner is post-M5 (AG-P25).
pub use exec::{
    compute_tool_def, route_of, ExecError, RoutingError, SandboxJob, SandboxToolHands, ToolRoute,
    PLATFORM_TOKEN_ENV,
};

// The FROZEN §6.3 requires_approval defaults seed (AG-P8 → P-220): the per-subsystem default lookup
// + the cross-subsystem "governed where it lands" rule + the seed seam + the VISION §3
// no-silent-loosening guard (a frozen yes→no loosening without a written deviation is rejected).
pub use defaults::{
    assert_no_silent_loosening, default_for_tool, requires_approval_default,
    requires_approval_for_landing, seed_requires_approval, LooseningViolation, WrittenDeviation,
};

// The run --dry-run lever (AG-P8 → P-220, contract 8.7): the side-effect-free planner (steps 1..6,
// 0 apply + 0 meter) + the AG-D9 proposed-effect-sequence determinism (two runs byte-identical) +
// the frozen DryRun bridge.
pub use dry_run::{
    dry_run_plan, proposed_effect_sequence, DryRunBridge, DryRunEntry, DryRunPlanner,
};

// The HITL withhold → surface → resume loop + the `hitl_gate` state machine (AG-P9 → P-221): the
// agent-fabric side the durable wait (9.4) drives — the `hitl_gate` state machine
// (Waiting → Approved/Rejected/Expired), the card projection (action + risk + LIVE cost estimate),
// the approver-set derivation (4.4 `list_subjects`), and the resume that threads the approved tool
// into the run's `approved` set so `EffectApi::apply`'s step 6 passes. The 0-mutation-pre-approval
// guarantee is structural (the loop opens the gate but never applies). Floors: per-effect resume
// idempotency AG-P10 (→ P-222); humanise card text AG-P11 (→ P-223); auto-dispatch L-3 AG-P20.
pub use hitl::{
    derive_approver_set, gate_id_of, live_cost_estimate, run_hitl_loop, surface_card, ApprovedTools,
    ApproverSet, Halted, HitlCard, HitlGate, HitlGateState, HitlOutcome, HitlWait, InvalidTransition,
    RiskSummary, WaitDecision,
};

// The AG-P11 (→ P-223) card-text path (C9/OQ-L): the HITL card text + every agent-authored message
// route through Notif `humanise` (contract 7.3, the ONE templating surface) — NEVER raw strings. The
// `risk_summary` + an `AgentMessage` are `(template_key, args)` pairs humanised per-viewer
// (permission-/erasure-safe); the same card renders differently for two viewers with different
// permissions. `assert_no_raw_agent_surface` is the 0-raw-string-surfaces GATE (a raw agent string is
// REJECTED). The agent-fabric templates register into the SAME Notif `TemplateStore` — no second
// engine, no frontend string map. NO FLOOR — humanise is the sole templating surface.
pub use card_text::{
    assert_no_raw_agent_surface, humanise_agent_message, humanise_card, humanise_risk_summary,
    register_agent_templates, AgentMessage, RawAgentString, RenderedCard,
    AGENT_PLATFORM_DEFAULT_TEMPLATES,
};

// Per-effect HITL idempotency (C4/OQ-F; AG-P10 → P-222): the AG-D5 EXACTLY-ONCE leg — a batch /
// partial-approval card gating N effects keys each effect's resume signal per-effect (`card_id` single,
// `card_id:effect_idx` multi), so a PARTIAL approval (approve 0+2, decline 1) sends three
// independently-idempotent decisions (each → exactly one apply) and a DOUBLE-CLICK on approve-all
// re-sends the same keys → no double-apply. The `ApplyLedger` is the exactly-once binding
// (apply-counter == approved-effect count, never more). Reconciles with the durable-engine half
// (`myelin-flow::approval`, P-206) by producing the SAME per-effect key (a parity CDC, not a second
// engine). No floor — the M2-B HITL family (AG-D5) is now complete.
pub use hitl_batch::{
    per_effect_idem_key, run_batch_hitl_loop, ApplyLedger, BatchApprovalCard, BatchGatedEffect,
    BatchHitlWait, BatchOutcome, DecisionScript, EffectOutcome,
};

// The FIVE structural loop guards, re-enforced at the Fabric tier (AG-P12 → P-224, AG-D7): the
// self-guard + reference gate (owned here, keyed on the frozen 13.1 inline ref nodes) + the apply-time
// idempotent-tool ledger on (run, effect_id) (owned here), composed with the RE-USED engine three
// (causal-depth ceiling default 12 + shared-root tripwire + bounded dispatch pool — myelin_flow::
// CausalGuard, P-214, NOT re-implemented). Every refusal is Drop/Park — there is NO Fork variant (the
// 0-unbounded-fork invariant is in the TYPE). Loop prevention is structural, not a convention: a human
// or agent can NEVER typo into a loop (raw text never re-triggers; only a structured artifact_ref node
// does). Floors named: the agent-lane shed budget (the in-flight cap NUMBER) is AG-P22; per-run
// identity (mint/scrub/revoke/re-mint, the principal the self-guard reads) is AG-P13 (→ P-225).
pub use loop_guards::{
    AgentLoopGuards, GuardRefusal, GuardVerdict, IdempotentToolLedger, ReferenceGate, SelfGuard,
    AGENT_CEILING, AGENT_DISPATCH_POOL_CAP, AGENT_SHARED_ROOT_CAP,
};

// The SCHEDULE_AND_RUN_JOB long-park idiom CONSUMED by the Agent-Fabric (AG-P16 → P-228, M2-C,
// §5.6): a long `kind=agent` `ToolHands::exec` job (a compute that takes minutes-to-hours) dispatches
// via the engine's `WfCtx::schedule_and_run_job` (9.2) and the run PARKS holding NO runtime;
// completion arrives HOURS later as a durable `job.done` signal (9.4) idempotent on the deterministic
// dispatch idem_token (a doubly-delivered job.done wakes the run EXACTLY ONCE). On wake the per-run
// token is RE-MINTED through the engine's wait-resume leg (4.7 / §5.7 C6, AG-P13 — token life ==
// activity life). The Fabric CONSUMES the idiom (the `AgentJobDispatcher` JobRunner async-dispatches
// the SAME hardened SandboxJob the in-line exec builds — the routing split + four-guarantee profile
// are identical; only completed-by-signal instead of in-line); it does NOT reinvent durable waits.
// The metered form fronts the dispatch with the reserve/settle bookend (11.7 — no balance → never
// dispatched). NO FLOOR — the idiom is consumed from the durable-workflow engine; the real microVM
// backend (CI-P2 → P-237) + the ZERO-escapes real-kernel GATE (AG-P17 → P-229) are RECORDED in
// `crate::exec`, not owned here.
pub use long_park::{
    dispatch_long_compute, dispatch_long_compute_metered, AgentJobDispatcher, LongComputeProfile,
    LongParkOutcome,
};

// The delegation-scoped tool-list (AG-P7 → P-219): the `list_objects` SetExpr push-down (the no-N+1
// pre-filter the brain's `Conversation.tools` is built from, §2.1) + the apply-time re-check (the
// scoping is an OPTIMISATION; `EffectApi` is the GUARANTEE, fail-closed). The single-query subset
// builder (4.3 consumed, 4.10 zookie honoured, 8.1 resolve) lowered to ONE predicate over the
// Fabric's own `tool_def.id`; the live-PG push-down SQL behind the `integration` feature.
pub use tool_scope::{
    apply_scope_to_conversation, assert_apply_rechecks_revoked, build_scoped_tool_list,
    lower_list_objects, lower_set_expr, scoped_tool_ids_sql, tool_def_id, ScopedToolList,
    ToolCatalogueIds, ToolListObjects, ToolScopePredicate, TOOL_DEF_OBJECT_TYPE, TOOL_ID_COLUMN,
    TOOL_USE_PERMISSION,
};

// The agent-service `serve(AppSpec)` shell (AG-P4 → P-216): the three ports, liveness != readiness,
// holder auto-registration, and the dispatch consumer bound by name with a subjects() whitelist.
pub use app::{
    agent_app_spec, agent_dispatch_consumer_reg, boot_agent, run_agent, SkeletonDispatchConsumer,
    AGENT_DISPATCH_SUBJECT_PREFIX, SERVICE_NAME,
};
