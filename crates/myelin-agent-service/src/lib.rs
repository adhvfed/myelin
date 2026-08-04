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
pub mod chat_tools;
pub mod ci_tools;
pub mod cost_gate;
pub mod defaults;
pub mod dispatch;
pub mod dispatch_surge;
/// The platform's own agents run on its own commits/issues/chat (AG-P26 → P-517, M6): the dogfood
/// loop — Myelin's own triage agent runs on the self-hosting CI graph with a BALANCED reserve/settle
/// ledger + a content-addressed trace per run (contract 1.8), the truth-up pass over every PROVEN
/// Fabric row (AG-D1..AG-D11 + E2E-2), and the every-incident-adds-a-drill loop. The MOCK-runtime
/// floor (the real LlmAgentRuntime swap is AG-P25) is named in [`dogfood::DOGFOOD_RUNTIME_FLOOR`].
///
/// **MR-009b W6b2 — `#[cfg(any(test, feature = "test-support"))]`:** this whole in-process dogfood
/// drill module (the AG-P26 done-bar) constructs the now-`test-support`-gated in-memory
/// `CostLedger::new` in its triage/fabric runners; nothing in the production graph references it, and
/// its callers (`tests/ag_p26_dogfood_drill.rs`) reach it via the `myelin-agent-service/test-support`
/// self dev-dependency.
#[cfg(any(test, feature = "test-support"))]
pub mod dogfood;
pub mod dry_run;
/// The Fabric's FULL `PersonalDataHolder` BODIES + the AG-D10 erasure fan-out (AG-P23 → P-479): the
/// per-subject DEK crypto-shred, pseudonym attribution fallback, the erasure ledger + post-restore
/// re-erasure. Fills the AG-P3 [`holder`] floor with real locate/export/erase.
pub mod dsr;
pub mod effect_api;
pub mod escape_gate;
pub mod exec;
pub mod git_tools;
pub mod hitl;
pub mod hitl_batch;
pub mod holder;
pub mod identity;
pub mod issues_agents;
pub mod issues_tools;
pub mod knowledge_tools;
pub mod long_park;
pub mod loop_guards;
pub mod metering;
pub mod migrations;
pub mod mock;
pub mod schema;
pub mod skeleton;
pub mod tool_exec;
pub mod tool_scope;
pub mod trace_seam;

// Re-export the PersonalDataHolder registration seam (AG-P3 → P-132) at the crate root — the shape
// the harness `serve` (AG-P4) + the DSR fan-out (AG-P23) + the 10.1 CDC consume (mirrors
// myelin-refs-service / myelin-search re-exporting their holder seam).
pub use holder::{
    agent_store_classifier, register_agent_holders, AgentHolderRegistration, AgentOltpHolder,
    AgentTraceHolder, AGENT_OLTP_STORE, AGENT_TRACE_STORE,
};

// The Fabric's FULL DSR holder BODIES + the AG-D10 erasure fan-out (AG-P23 → P-479): the real
// locate/export/erase over the per-subject DEK crypto-shred + pseudonym attribution fallback, the
// erasure ledger + post-restore re-erasure.
pub use dsr::{
    subject_dek_ref, AgentFabricHolder, AgentFabricStore, FabricEraseReceipt, FabricErasureLedger,
    FabricLocateReport, FabricReErasureReceipt, FreeTextRow, RunAttribution,
};

// The SKELETON runtime (AG-P4 → P-216): the gateway → identity → dispatch → reserve → trace path at
// zero cost. The brain (no model, no tools) + the platform-owned `Agent::handle` durable-workflow
// loop body + the per-run identity (mint/revoke/anti-leak) + the contract-1.8 telemetry signals.
pub use skeleton::{
    ChildEnv, RunOutcomeKind, RunSubstrate, RunTokenRevoker, RunWallet, SkeletonAgent,
    SkeletonAgentRuntime, SkeletonError, SkeletonTelemetry, SpendCapStage, AGENT_RUN_TRACED_EVENT,
    DEFAULT_MAX_TURNS, SKELETON_STEP_UNIT, WALLET_MIN_BALANCE_FLOOR,
};

// v1 TOKEN METERING (this slice): the PURE pricing half — raw token counts → a micro-dollar charge
// (`wholesale` + `markup`). A NEW, non-disruptive layer LAYERED ON the untouched reserve/settle gate;
// the wallet DEBIT that consumes a price is the driving loop's job (`SkeletonAgent::handle_run`, over
// the `RunWallet` seam). Vendor-neutral rates (`LUNA_RATES` today; Anthropic slots in as a sibling).
pub use metering::{price, ModelRates, PriceError, Priced, LUNA_RATES};

// The bounded driving loop's `ToolExecutor` seam (the loop half of 8.5): turn one VALIDATED
// `ToolCall` into a `ToolResult`. This slice ships ONLY the seam + the test doubles; the three real
// per-route impls (Read→subsystem read, Compute→SandboxToolHands::exec, Mutate/External→
// EffectApi::apply) are named-but-unwired follow-ons (route_of decides which). NO metering here —
// the per-call cost meter + spend cap is a decision-gated follow-on; the loop's runaway guard is the
// max-turns bound (DEFAULT_MAX_TURNS).
pub use tool_exec::{ToolExecError, ToolExecutor};
// The in-process test doubles (test-support gated, mirroring the crate's other mocks — the `tests/`
// integration targets reach them via the self `test-support` dev-dependency; NEVER in the prod DAG).
#[cfg(any(test, feature = "test-support"))]
pub use tool_exec::{MockToolExecutor, MockToolSurface};

// The M6 dogfood loop (AG-P26 → P-517): the platform's own triage agent runs on the self-hosting CI
// graph (a real Myelin CI failure → explicit-first dispatch → a costed triage run) with a BALANCED
// reserve/settle ledger + a content-addressed trace per run (contract 1.8), the Fabric truth-up pass
// over every PROVEN row (AG-D1..AG-D11 + E2E-2), and the every-incident-adds-a-drill loop. NO new
// engine — drives the already-shipped Fabric surface over the Myelin self-tenant (EI-01 §7). FLOOR:
// the dogfood agents run on the MOCK runtime (DOGFOOD_RUNTIME_FLOOR); the real LlmAgentRuntime swap is
// the named post-M5 follow-on AG-P25.
// MR-009b W6b2 — the whole `dogfood` in-process drill module is `test-support`-gated (its runners build
// the in-memory `CostLedger::new`); the entire re-export follows the same gate.
#[cfg(any(test, feature = "test-support"))]
pub use dogfood::{
    proven_fabric_rows, run_fabric_over_myelins_own_work, run_fabric_truth_up_scorecard,
    run_myelin_triage_on_ci_failure, FabricDogfoodArtifact, FabricIncident,
    FabricIncidentDrillTicket, FabricIncidentIssueDraft, FabricRowStatus, FabricScorecardEntry,
    FabricTruthUpPass, FabricTruthUpRed, FabricTruthUpScorecard, FabricTruthUpVerdict,
    ProvenFabricRow, TriageFace, DOGFOOD_RUNTIME_FLOOR, MYELIN_SELF_REGION, MYELIN_SELF_TENANT,
};

// The reserve/settle cost gate as the runaway self-limiter (AG-P14 → P-227, M2-B, AG-D11): the
// agent-fabric CONSUMER that fronts every run through the Storage AgentRunGate/CostLedger (11.7) and
// proves the gate as the runaway self-limiter end-to-end — a MockAgentRuntime brain looping
// run-after-run against ONE draining wallet, reserve refuses past exhaustion (the loop stops at the
// wallet, never by a kill), the in-flight run is NEVER interrupted (0 interrupt), and the books
// balance (reserved == settled; a Mock bills 0 → the reservation refunds). NO FLOOR in the gate
// mechanism — real per-model-call cost metering arrives with LlmAgentRuntime (AG-P25, post-M5); the
// Mock metering ZERO is correct (the limiter is brain-independent).
pub use cost_gate::{runaway_brain, AgentFabricCostSignal, RunawaySelfLimiter, RunawayStep};

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
    decode_proposed, effect_gate_key, effect_gate_key_str, encode_proposed, validate_call,
    validate_schema, validate_tool_arguments, ApplyError, CapabilityCheck, DelegationLookup,
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

// The AG-D4 / CI-T1 hard escape GATE consumed on the Fabric exec dispatch path (AG-P17 → P-229,
// M2-C, contract 8.4 — the drill-as-gate). CI owns the runner + the real-kernel escape drill (CI-P5
// → P-239, proven on a real Firecracker microVM); this is the FABRIC half that CONSUMES the green
// `EscapeAttestation` (myelin-ci-sandbox, NOT re-implemented / NOT forked) and turns it into a
// fail-closed gate: the `AgentExecGate` can ONLY be obtained from a GREEN attestation for the
// production backend (ZERO escapes, matching kernel/rootfs/corpus identity) — so a `SandboxToolHands`
// / `AgentJobDispatcher` cannot exist without one (no green attestation ⇒ no untrusted compute; the
// fail-closed property is in the TYPE, never a hardcoded `true`). FLOORS (AG-P17): there is NO floor
// on AG-D4 — ZERO escapes is both floor and full answer; it is a PERMANENT GATE re-run on every
// backend/image/kernel change. The M4 re-confirm on the prod CI image is AG-P21 (→ P-348); the real
// LlmAgentRuntime against this hardened runner is post-M5 (AG-P25); continuous fuzzing + CVE corpus +
// pre-GA pentest remain ongoing residuals.
pub use escape_gate::{AgentExecGate, GateRefusal, ProductionBackendId};

// The FROZEN §6.3 requires_approval defaults seed (AG-P8 → P-220): the per-subsystem default lookup
// + the cross-subsystem "governed where it lands" rule + the seed seam + the VISION §3
// no-silent-loosening guard (a frozen yes→no loosening without a written deviation is rejected).
pub use defaults::{
    assert_no_silent_loosening, default_for_tool, requires_approval_default,
    requires_approval_for_landing, seed_requires_approval, LooseningViolation, WrittenDeviation,
};

// The per-producer GIT ToolDefs (AG-P18 → P-267, M3): git.merge (the consequential gate, gated by
// the frozen §6.3 default — withholds at EffectApi step 6 → Gated until the HITL resume) + open_pr
// (reversible, NOT gated — applies directly). Registered into the ONE ToolSurface (8.1 / §6.1) with
// their required_caps sourced from the FROZEN Git ReBAC fragment (4.9: pull_request.merge / repo.push)
// and their requires_approval SEEDED from the frozen defaults (AG-P8), guarded by the VISION §3
// no-silent-loosening ratchet. NO new engine — a ToolDef is a row in the existing registry; the
// routing/gating/HITL are the existing plan-then-apply pipeline. The KNOWLEDGE producer ToolDefs +
// the agent-trace holder seam (KN-D11/KN-D12) are AG-P19 (→ P-268) and reuse THIS registration pattern.
// GIT-P28 → P-289 (M3-G6) extends the Git producer surface with the AGENT AUTHOR/REVIEWER tools:
// git.comment + git.submit_review + git.suggest_change + git.resolve_thread (all reversible Mutate →
// EffectApi, requires_approval = no — the ONLY consequential git gate stays git.merge). Agents are
// FIRST-CLASS, legible (myelin_git::agent_author::Authorship — never disguised as human, ADR-08 /
// AI-Act), bounded (every effect rides the eight-step pipeline) authors/reviewers governed by the
// SAME pull_request.review cap (4.9) a human reviewer is. NO new engine — registration data.
pub use git_tools::{
    git_author_tool_defs, git_comment_tool_def, git_history_rewrite_tool_def,
    git_merge_required_caps, git_merge_tool_def, git_resolve_thread_tool_def,
    git_scip_index_tool_def, git_submit_review_tool_def, git_suggest_change_tool_def,
    git_tool_defs, open_pr_required_caps, open_pr_tool_def, register_git_tools, GIT_MERGE_TOOL,
    GIT_SUBSYSTEM, GIT_TOOL_VERSION, OPEN_PR_TOOL,
};

// The per-producer KNOWLEDGE ToolDefs (AG-P19 → P-268, M3): publish + edit_confidential (the
// consequential gates — gated by the frozen §6.3 default, withhold at EffectApi step 6 → Gated until
// the HITL resume) + draft + comment (reversible, NOT gated — apply directly). Registered into the
// ONE ToolSurface (8.1 / §6.1) with their required_caps sourced from the FROZEN KN ReBAC carrier
// (4.9: page.publish / page.edit / page.draft / page.comment from myelin_content::rebac_fragment) and
// their requires_approval SEEDED from the frozen defaults (AG-P8), guarded by the VISION §3
// no-silent-loosening ratchet. NO new engine — a ToolDef is a row in the existing registry; the
// routing/gating/HITL are the existing plan-then-apply pipeline (the same registration pattern as the
// Git producer tools, AG-P18 — the compounding-payoff reuse).
pub use knowledge_tools::{
    comment_required_caps, comment_tool_def, draft_required_caps, draft_tool_def,
    edit_confidential_required_caps, edit_confidential_tool_def, knowledge_tool_defs,
    publish_required_caps, publish_tool_def, register_knowledge_tools, COMMENT_TOOL, DRAFT_TOOL,
    EDIT_CONFIDENTIAL_TOOL, KNOWLEDGE_SUBSYSTEM, KNOWLEDGE_TOOL_VERSION, PUBLISH_TOOL,
};

// The per-CONSUMER Issues ToolDefs (AG-P20 → P-347, M4): forecast / triage / sla_draft (advisory, NOT
// gated — suggest-by-default) + transition (the SLA-bound, approver-edged transition — the gated §6.3
// FLOOR + the field/transition ABAC caveat at EffectApi check-time, §5.2 step 2). Registered into the
// ONE ToolSurface (8.1) with required_caps sourced from the FROZEN Issues ReBAC fragment (4.9:
// issue.transition / issue_transition.perform_transition) and requires_approval SEEDED from the frozen
// defaults (AG-P8), guarded by the VISION §3 no-silent-loosening ratchet. NO new engine — the
// transition-ABAC caveat is carried by crate::effect_api::PlannedEffect into the existing pipeline.
pub use issues_tools::{
    advisory_required_caps, forecast_tool_def, issues_tool_defs, register_issues_tools,
    sla_draft_tool_def, transition_caveat, transition_required_caps, transition_tool_def,
    triage_tool_def, FORECAST_TOOL, ISSUES_SUBSYSTEM, ISSUES_TOOL_VERSION, SLA_DRAFT_TOOL,
    TRANSITION_TOOL, TRIAGE_TOOL,
};

// The FULL Issues ToolDef catalogue + the MOCK forecast/triage agents (ISS-P23 → P-390, M4-I6):
// EXTENDS AG-P20's four agent-facing Issues tools with the human/CLI CRUD tools (create/update/
// comment/link/estimate/reorder/assign/close — arch 03 §8) so the FULL arch-§8 catalogue registers
// into the ONE ToolSurface (8.1; UI=CLI=agent parity, no privileged back-channel). Every tool is a
// Mutate routed through the existing plan-then-apply pipeline (8.2, no carve-out); exactly two are
// gated by the frozen §6.3 default — close + the SLA-bound transition (the consequential split,
// conservative floor + ABAC caveat refinement). The MOCK forecast agent (compute-only, linear
// remaining÷velocity off OLAP — the named R-5 floor; Monte-Carlo is ISS-P32) + the MOCK triage agent
// (the S9 suggestion strip via run --dry-run, 8.7 — proposed, NOT applied) run on the SAME
// --use-mock MockAgentRuntime path (8.3 — the named R-10 floor; the real LlmAgentRuntime is the
// post-M5 AG-P25 swap, never a rewrite). AG-D9 (identical replay/effect-sequence) is greened here;
// AG-D5 (the governed-transition HITL withhold) is the drills_iss_p23 test. NO new engine.
// NOTE: `comment_required_caps` / `comment_tool_def` / `COMMENT_TOOL` are NOT re-exported here (the
// Knowledge surface above already exports same-named items); reach the Issues comment tool via
// `issues_agents::comment_tool_def`. Every other Issues catalogue/agent surface is re-exported.
pub use issues_agents::{
    assign_required_caps, assign_tool_def, close_tool_def, create_required_caps, create_tool_def,
    estimate_tool_def, full_issues_tool_defs, link_tool_def, mock_forecast_agent,
    mock_triage_agent, register_full_issues_tools, reorder_tool_def, replay_forecast_agent,
    triage_effect_for, triage_suggestion_strip, update_required_caps, update_tool_def,
    ForecastInput, ForecastOutput, LinearForecast, ASSIGN_TOOL, CLOSE_TOOL, CREATE_TOOL,
    ESTIMATE_TOOL, LINK_TOOL, REORDER_TOOL, UPDATE_TOOL,
};

// The per-CONSUMER Chat ToolDefs (AG-P20 → P-347, M4): post_message / react (reversible, NOT gated) +
// the cross-subsystem "governed where it LANDS" rule (§6.3 last row) — any EffectApi tool a Chat run
// invokes against another subsystem inherits THAT subsystem's frozen default (a chat-invoked git.merge
// is GATED — Git's default). required_caps from the FROZEN Chat ReBAC fragment (4.9: channel.post);
// requires_approval SEEDED from the frozen defaults (AG-P8). Explicit-first dispatch is crate::dispatch
// (a mention NOTIFIES, does not auto-spawn). NO new governance model — landing_requires_approval reuses
// the AG-P8 requires_approval_for_landing helper.
pub use chat_tools::{
    chat_tool_defs, landing_requires_approval, post_message_tool_def, post_required_caps,
    react_tool_def, register_chat_tools, CHAT_SUBSYSTEM, CHAT_TOOL_VERSION, POST_MESSAGE_TOOL,
    REACT_TOOL,
};

// The per-CONSUMER CI ToolDefs (AG-P20 → P-347, M4): deploy / approve_deploy / write_secret (the
// privileged gates — gated by the frozen §6.3 default, withhold at EffectApi step 6 → Gated until the
// HITL resume) + run_pipeline non-prod (cheap/reversible/metered, NOT gated). required_caps from the
// canonical CI ReBAC fragment (4.9: environment.deploy / ci_project.administer / run.trigger from
// myelin_identity_service::ci_fragment); requires_approval SEEDED from the frozen defaults (AG-P8),
// guarded by the VISION §3 no-silent-loosening ratchet. NO new engine — a ToolDef is a row in the
// existing registry. FLOOR: the AG-D4 / CI-T1 re-confirm on the PRODUCTION CI runner image is AG-P21
// (→ P-348), the M4 hard gate — the CI deploy tools run on that prod image; that re-confirm is the
// SEPARATE next prompt, not this one.
pub use ci_tools::{
    approve_deploy_tool_def, ci_tool_defs, deploy_required_caps, deploy_tool_def,
    register_ci_tools, run_pipeline_required_caps, run_pipeline_tool_def,
    write_secret_required_caps, write_secret_tool_def, APPROVE_DEPLOY_TOOL, CI_SUBSYSTEM,
    CI_TOOL_VERSION, DEPLOY_TOOL, RUN_PIPELINE_TOOL, WRITE_SECRET_TOOL,
};

// Explicit-first dispatch wiring (AG-P20 → P-347, M4, contract 8.6 / §3.4 / CHAT-1): the TYPED
// classifier the dispatch tier consults to decide notify-vs-dispatch. A casual @agent mention
// (DispatchTrigger::Mention) ALWAYS resolves Notify (0 auto-spawn — the dispatch-counter stays 0, the
// CHAT-D17 gate); only an explicit "run an agent here" trigger (ExplicitRun) / a structured artifact-
// ref re-trigger (StructuredRef) resolves Dispatch — and EVEN the explicit run passes the reserve gate
// (the costed run is still crate::skeleton::SkeletonAgent::handle_run). The 0-auto-spawn property is in
// the TYPE (no arm maps a Mention to Dispatch) and COUNTED (DispatchCounter, EI-01 §3). FLOOR: implicit
// auto-dispatch on a casual mention remains [OPEN → LEGAL] (L-3, counsel-gated — GDPR Art. 22 / EU
// AI-Act human-oversight); the auto-spawn path is NOT wired until counsel ratifies the basis.
pub use dispatch::{classify, DispatchCounter, DispatchDecision, DispatchTrigger};

// The 30× agent-dispatch surge family (AG-P22 → P-478, M5, AG-D6 / contract 1.11/1.9/11.7): the
// protected-human-lane shed gate at the agent-DISPATCH front door. WIRES the substrate's
// shed::ShedLane over Surface::AgentMention (budget read from the thresholds file) in front of the
// storage AgentRunGate reserve gate — the two structural defences (concurrency front + wallet front)
// compose. The agent lane sheds with 429 + Retry-After (the runtime HONOURS it — no retry storm), the
// human lane is protected (humans never queue behind agent runs), reserve refuses the over-budget runs
// without interrupting in-flight, and cross-tenant impact is 0. The agent-lane shed budget moves from
// the M2 placeholder floor to the MEASURED cap (thresholds.toml AgentMention 96/24, AG-D6 2026-06-25).
pub use dispatch_surge::{
    run_agent_dispatch_surge, AgentDispatchShed, AgentDispatchSurgeGate, AgentDispatchSurgeReport,
    DispatchFrontError, RetryAfterHonouringRuntime, RuntimeReaction,
    AGENT_DISPATCH_SURGE_MULTIPLIER, AGENT_LANE_SHED_BUDGET_IS_MEASURED,
};

// The agent-trace HOLDER seam (AG-P19 → P-268, M3, AG-7 / contract 8.8): the execution trace is a
// CONTENT-ADDRESSED Knowledge document reusing the frozen myelin-content 13.1 block model;
// run.trace_ref resolves to its blake3:<hex> content address; the holder registers as the erasable
// H17 PersonalDataHolder (crate::holder::AgentTraceHolder), DISTINCT from the audit log. The Fabric
// owns the SEAM (build the 13.1 document + content-address it + resolve run.trace_ref); Knowledge owns
// the live holder body (the content-addressed write/erase against the real store). FLOOR named in
// STATELESS_EXCEPT_TRACE_FLOOR: v1 agents are stateless across runs EXCEPT for this trace document;
// long-term memory / RAG is post-M5 (AG-P25); the full DSR fan-out over the trace is AG-P23 (→ P-479).
pub use trace_seam::{
    is_content_addressed_kn_document, trace_ref_of, TraceDocument, STATELESS_EXCEPT_TRACE_FLOOR,
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
    derive_approver_set, gate_id_of, live_cost_estimate, persist_gate_decision, persist_gate_open,
    run_hitl_loop, surface_card, ApprovedTools, ApproverSet, Halted, HitlCard, HitlGate,
    HitlGateState, HitlOutcome, HitlWait, InvalidTransition, RiskSummary, WaitDecision,
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
