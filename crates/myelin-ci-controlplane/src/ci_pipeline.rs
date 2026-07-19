//! # `ci_pipeline` — the `ci.pipeline` DURABLE WORKFLOW BODY + the X-1 producer side (CI-P15 → P-358, M4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §3.1 (the hybrid boundary + the `ci_pipeline` pseudocode — "the pipeline IS a durable workflow"),
//! §3.2 (the activity boundary is the JOB, not the step), §3.4 (definition snapshot vs workflow
//! versioning), §4 (the Git↔CI check seam, PRODUCED — CI's X-1 producer side).
//! **Contracts:** 9.1 (the `ci.pipeline` workflow registration), 9.2 (the `WfCtx` body), 9.3 (the SLA
//! timer wheel), 9.4 (the protected-env / manual gate as `wait_for_signal("approval:<stage>")`),
//! 11.7 (reserve/settle per stage), 5.9 (the `CheckStatus` / `ci.result` PRODUCER — CI is the
//! producer/owner), 1.6 (the `flow-determinism` lint, OBEYED on the body).
//!
//! ## What CI-P15 ships — THE `ci.pipeline` BODY + ITS DETERMINISM (CI-D9)
//!
//! The substrate this body sits on — the durable executor, `WfCtx`, the `SCHEDULE_AND_RUN_JOB`
//! long-park idiom, the per-stage reserve/settle, the SLA timer wheel — is the FROZEN `myelin-flow`
//! engine (P-FLOW-22 / P-345; reconciled in place, NOT re-built here). This module is **CI's pipeline
//! body**: the deterministic Rust function registered under [`myelin_flow::CI_PIPELINE_WF_TYPE`] at
//! `serve`, guarded by the `flow-determinism` lint. It is the X-1 PRODUCER half the engine substrate
//! NAMED as CI's M4 deliverable (the floor the substrate recorded: "CI's real pipeline definitions +
//! the `CheckStatus`/`ci.result` PRODUCER are CI's M4 deliverable, contract 5.9").
//!
//! The body, EXACTLY as the arch 02 §3.1 pseudocode:
//!
//! 1. **The protected-env / manual GATE (9.4).** A stage may be a `gate` — the body
//!    `ctx.wait_for_signal("approval:<stage>", window)` (may wait DAYS, holding no runtime). A DENY or
//!    a TIMEOUT emits `ci.deployment.rejected` + fails the run fast (0 wasted spend on a rejected
//!    deploy).
//! 2. **The stages (the FROZEN `SCHEDULE_AND_RUN_JOB` long-park, 9.4 / 11.7).** The body runs the
//!    snapshot's ordered stages through the engine's [`myelin_flow::WfCtx::run_ci_pipeline`] — each
//!    stage is one `kind=ci` dispatch + park (reserve at dispatch, settle on `job.done`, an SLA timer
//!    bounding a vanished runner). Stages gate sequentially, fail-fast.
//! 3. **On any failure** — `emit_check(CheckStatus{failure})` PER context (X-1, §4) +
//!    `ctx.emit(ci.run.failed, structured_failure)` (the agent-native triage hook) +
//!    `signal_merge_queue(ci.result{failure})` (the rollup signal that wakes Git's merge queue, X-1).
//! 4. **On success** — `emit_check(CheckStatus{success})` PER context + `ctx.emit(ci.run.succeeded)`
//!    + `signal_merge_queue(ci.result{success})`.
//!
//! **Determinism (the CI-D9 property).** The body reads NO clock/RNG/IO outside `WfCtx`: every X-1
//! emit rides `ctx.emit` (journaled co-commit), every gate/stage outcome flows through a journaled
//! `wait_for_signal` / `SCHEDULE_AND_RUN_JOB` activity, and the per-context check facts are a PURE
//! function of the snapshot's context set + the journaled stage verdict. A REPLAY is BIT-IDENTICAL
//! (the same emits land at the same command positions) and ONLY a journaled `job.done` (the stage
//! verdict) feeds the body. The `flow-determinism` lint guards this file (the `// @workflow-body`
//! marker scopes the scan); the substrate's CI-D9 / CI-D1 drills
//! (`myelin-flow/tests/drills_ci_pipeline.rs`) prove the bit-identical replay + effectively-once
//! crash-recovery on the engine the body composes.
//!
//! ## references-not-payloads (the X-1 facts)
//!
//! Every `ci.check.updated` fact + the `ci.result` rollup is the FROZEN small, PII-free struct CI
//! carries through `myelin_events::check_seam::{check_updated_draft, ci_result_draft, rollup_ci_result}`
//! — `run` / `details_ref` are `ArtifactRef`s (the producing run + the jump-to-failure sub-anchor),
//! NEVER log bytes. The `trust_tier` is the value stamped at TRIGGER time (the dispatch's CI-P10
//! stamp, read off the run facts — NEVER recomputed here, X-1). The `run_attempt` is monotonic so
//! Git's last-writer-wins is on the attempt, not wall-clock.
//!
//! ## NAMED FLOORS (recorded here, filled later)
//!
//! - **The `SCHEDULE_AND_RUN_JOB` dispatch HANDSHAKE into the live runner / the scheduler** (the
//!   `idem_token` mint at dispatch, the `job_queue` enqueue, the runner lease + `job.done` delivery)
//!   is CI-P16 (P-359). This body composes the FROZEN engine idiom over a [`myelin_flow::JobRunner`]
//!   seam; CI-P16 binds that seam onto the CI scheduler + the unified runner (`ToolHands::exec`, 8.4),
//!   GATED by AG-D4 (no untrusted code runs until the sandbox-escape gate is green).
//! - **The per-stage reserve/settle METERING into `cost_event`** is now SHIPPED in [`crate::metering`]
//!   (CI-P17 / P-360): the resource-second meter taxonomy ([`crate::metering::Meter`]) is the
//!   wholesale unit, [`crate::metering::CostEventRow`] is the CI `cost_event` schema row (wholesale +
//!   markup SEPARATE columns, `kind ∈ {ci, agent}`), and [`crate::metering::CiMeter`] wraps the engine
//!   bookend so a stage `reserve_budget()`s (refuse-to-start) + `settle_budget()`s its resource-seconds
//!   on `job.done`. The body composes the engine bookend over a per-stage `MinorUnits` cost; the
//!   resource-second → credit/price MARKUP mapping remains Commercial's (arch 06 R-2).
//! - **The `check_attempt` monotonic counter + the `ci.check.updated` PRODUCER plumbing into the
//!   outbox** is CI-P18 (P-361); **the `ci.result` rollup signal end-to-end with Git's merge queue
//!   (GIT-D10 / CI-D8)** is CI-P19 (P-362). This body PRODUCES the facts at the right body positions;
//!   those prompts wire the monotonic counter + the end-to-end seam GATE.

use myelin_events::check_seam::{check_updated_draft, ci_result_draft, rollup_ci_result, CiResult};
use myelin_events::{ArtifactRef, EventDraft, EventType};
use myelin_events::{DataRole, Visibility};
use myelin_flow::{CiPipelineSpec, JobRunner, PipelineOutcome, WfCtx, WfResult};

/// The frozen `wf_type` the `ci.pipeline` workflow registers under (contract 9.1). Re-exported from
/// the engine ([`myelin_flow::CI_PIPELINE_WF_TYPE`]) AND the dispatch (`myelin_ci_dispatch`) so the
/// dispatcher's `StartSpec.wf_type`, the executor's registered definition name, and this body's
/// registration all agree on ONE name by construction (no second name language).
pub const CI_PIPELINE_WF_TYPE: &str = myelin_flow::CI_PIPELINE_WF_TYPE;

// The X-1 producer event tokens (contract 5.9 / the §1 ci.* taxonomy) — re-exported from the canonical
// `myelin-ci-sandbox` constants (one source of truth; this module emits them, it does not re-name them).
use myelin_ci_sandbox::events::{CI_DEPLOYMENT_REJECTED, CI_RUN_FAILED, CI_RUN_SUCCEEDED};

/// **One CI-level stage of a `ci.pipeline` run (arch §3.1).** Wraps the engine's
/// [`myelin_flow::CiStage`] (the `SCHEDULE_AND_RUN_JOB` long-park: name / target / cost / SLA) with
/// the CI-level concern the engine does NOT own: whether the stage is a **protected-env / manual
/// GATE** (a `wait_for_signal("approval:<stage>")` that may wait days, 9.4). A gate stage is NOT
/// dispatched to a runner — it parks on a human/automation approval signal, then the body proceeds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineStage {
    /// The engine stage (the long-park spec) — `name` / `target` / `cost` / `timeout_secs`.
    pub engine: myelin_flow::CiStage,
    /// **Is this stage a protected-env / manual GATE (9.4)?** A gate stage parks on
    /// `approval:<name>` (a human/automation decision that may take days); a non-gate stage dispatches
    /// to the runner (the `SCHEDULE_AND_RUN_JOB` long-park). The gate's approval `window` is the
    /// engine stage's `timeout_secs` (a None window waits indefinitely; a Some window auto-DENIES on
    /// timeout, AG-8 — 0 mutation on a stalled approval).
    pub gate: bool,
}

impl PipelineStage {
    /// A normal (runner-dispatched) stage from an engine [`myelin_flow::CiStage`].
    pub fn job(engine: myelin_flow::CiStage) -> PipelineStage {
        PipelineStage {
            engine,
            gate: false,
        }
    }

    /// A protected-env / manual GATE stage (parks on `approval:<name>`, 9.4).
    pub fn gate(engine: myelin_flow::CiStage) -> PipelineStage {
        PipelineStage { engine, gate: true }
    }
}

/// **The `ci.pipeline` run definition the body executes (arch §3.1).** The resolved+pinned snapshot,
/// expressed as the ordered CI-level stages (gates + runner stages) + the per-context check seam the
/// run REPORTS (X-1). Built by the dispatcher's resolve/start (`myelin_ci_dispatch::resolve`,
/// CI-P11) from the CAS snapshot; carried into the body as the `StartSpec.input`. PII-free
/// (references-not-payloads): a repo ref, a commit OID, context names, an approval-window cost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineRun {
    /// The ordered stages (gates interleaved with runner stages) — run IN ORDER, fail-fast.
    pub stages: Vec<PipelineStage>,
    /// The per-context check seam this run REPORTS (X-1: one `ci.check.updated` per context). The
    /// merge gate keys on `(commit_oid, context)`; the body emits a terminal fact per context.
    pub contexts: Vec<String>,
    /// The X-1 emit context — the repo / commit / run ref / trust posture the check facts ride.
    pub facts: CheckFacts,
}

/// **The X-1 producer emit context (contract 5.9 / arch §4).** The provenance the per-context
/// `ci.check.updated` facts + the `ci.result` rollup carry that the engine stage spec does NOT — the
/// repo + commit the checks key on, the producing run ref, the monotonic `run_attempt`, the
/// trust_tier STAMPED at trigger time (read off the run, NEVER recomputed), and the `ci.result`
/// rollup `idem_token` the merge queue echoes (the no-coordination dedup agreement, OQ-F). All
/// PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckFacts {
    /// The canonical repo root from which the check seam derives its commit/check subject (X-1).
    pub repo: String,
    /// The commit OID the run ran against (the `(commit_oid, context)` key half).
    pub commit_oid: String,
    /// The producing CI run ref (`myelin://<tenant>/ci/run/<id>`) — the supersession provenance.
    pub run_ref: String,
    /// The monotonic `run_attempt` per `(commit_oid, context)` — Git's last-writer-wins key (NOT
    /// wall-clock). A re-run bumps it; the body stamps the SAME value onto every context's fact.
    pub run_attempt: u32,
    /// The `trust_tier` token STAMPED at trigger time (`trusted` / `untrusted_fork`) — read off the
    /// run's CI-P10 stamp, NEVER recomputed here (X-1). Carried onto every `ci.check.updated`.
    pub trust_tier: String,
    /// The `ci.result` rollup `idem_token` (= the merge-attempt id the merge queue minted, OQ-F). The
    /// body echoes it on the rollup so a double-delivered `ci.result` wakes the merge queue ONCE.
    pub merge_idem_token: String,
}

/// **The terminal verdict the `ci.pipeline` body reached (the X-1 producer output).** A run either
/// SUCCEEDED (every stage passed → success checks + `ci.run.succeeded` + `ci.result{success}`),
/// FAILED at a named stage (failure checks + `ci.run.failed` + `ci.result{failure}`), was REJECTED at
/// a named gate (a denied/timed-out approval → `ci.deployment.rejected`, the deploy never ran), or
/// PARKED (a stage/gate is waiting — the body returns promptly, the dispatcher re-drives it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunVerdict {
    /// Every stage passed — the run is GREEN. Carries the count of stages that completed.
    Succeeded {
        /// the number of runner stages that completed `pass`.
        stages_completed: usize,
    },
    /// A stage FAILED (a `job.done` reported `fail`, or a vanished runner timed out) — fail-fast.
    /// Carries the failed stage's name. The failure checks + `ci.run.failed` + `ci.result{failure}`
    /// were emitted.
    Failed {
        /// the name of the failed stage.
        stage: String,
    },
    /// A protected-env / manual GATE was DENIED or TIMED OUT — `ci.deployment.rejected` was emitted
    /// and the gated stages never ran (0 wasted spend on a rejected deploy).
    Rejected {
        /// the name of the rejected gate stage.
        stage: String,
    },
    /// A stage or gate is WAITING (dispatched + parked on `job.done`, or parked on `approval`). The
    /// run holds NO runtime; the body returns promptly and the dispatcher re-drives it when the
    /// signal arrives (the body replays the journaled prefix to this point).
    Parked,
}

/// **The STRUCTURED `ci.run.failed` triage hook (arch §4 / §3.1 — the deliberate agent-native input).**
///
/// A failing run's `ci.run.failed` does NOT carry a bare "it failed" — it carries the structured,
/// PII-free triage signal an agent reads to know *what* to do without re-deriving it from raw logs:
/// **which stage** failed (always present — the body's deterministic verdict), and, when the job result
/// surfaced them, **which step** (the `#step-<n>` jump-to-failure anchor), **which test** (the failing
/// test id — a machine token, never free-text body), and a **log-excerpt reference** (an `ArtifactRef`
/// into CI's log tier — references-not-payloads, never inline log bytes). This is the E2E-2 flagship's
/// load-bearing input: the (mock) triage agent reads THIS struct off the bus to file a precise issue
/// ("test `<id>` failed at step `<n>` in stage `<stage>`; see `<log_excerpt_ref>`"), NOT the firehose.
///
/// **references-not-payloads / PII-free.** Every field is a machine token or an `ArtifactRef`; the log
/// excerpt is a POINTER into the log tier (resolved per-viewer, ACL-checked), never the bytes — so the
/// failure fact stays a small, leak-free bus event (ADR-04.5; the durable bus never carries log bytes).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StructuredFailure {
    /// The failed stage's name (always present — the body's journaled verdict). The coarsest triage key.
    pub failed_stage: String,
    /// The failing step index (the `#step-<n>` jump-to-failure anchor) iff the job result surfaced it.
    pub failed_step: Option<u32>,
    /// The failing test id (a machine token, e.g. `crate::module::test_name`) iff the job result named
    /// it — NEVER a free-text test body (references-not-payloads; the body is in the log tier).
    pub failed_test: Option<String>,
    /// An `ArtifactRef` into CI's log tier at the failing excerpt (resolved per-viewer, ACL-checked) —
    /// the agent's RAG/triage read target, NEVER inline log bytes on the bus event.
    pub log_excerpt_ref: Option<String>,
}

impl StructuredFailure {
    /// The triage hook from a bare stage verdict (the body's always-available signal — no step/test/log
    /// detail surfaced). The richer constructor is [`structured_failure`].
    pub fn for_stage(stage: impl Into<String>) -> StructuredFailure {
        StructuredFailure {
            failed_stage: stage.into(),
            ..StructuredFailure::default()
        }
    }

    /// Render the triage hook as the FROZEN PII-free `ci.run.failed.structured_failure` payload object
    /// (arch §3.1). `failed_stage` is always present; the optional triage detail is included ONLY when
    /// the job result surfaced it (a `null`-free object — absent detail is an absent key, so a replay
    /// re-builds a byte-identical payload, CI-D9). The agent reads these keys directly.
    pub fn to_payload(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "failed_stage".to_string(),
            serde_json::Value::String(self.failed_stage.clone()),
        );
        if let Some(step) = self.failed_step {
            obj.insert("failed_step".to_string(), serde_json::json!(step));
        }
        if let Some(test) = &self.failed_test {
            obj.insert(
                "failed_test".to_string(),
                serde_json::Value::String(test.clone()),
            );
        }
        if let Some(log_ref) = &self.log_excerpt_ref {
            obj.insert(
                "log_excerpt_ref".to_string(),
                serde_json::Value::String(log_ref.clone()),
            );
        }
        serde_json::Value::Object(obj)
    }
}

/// **Build the structured `ci.run.failed` triage hook (arch §4 — "which step, which test, log
/// excerpt").** A pure function of the failed stage + the (optional) job-result triage detail. The
/// stage is always present; the step/test/log-excerpt are threaded through ONLY when a job result
/// surfaced them. PII-free (machine tokens + an `ArtifactRef`); never inline log bytes (the log lives
/// in the T3 log tier, referenced not carried).
pub fn structured_failure(
    failed_stage: &str,
    failed_step: Option<u32>,
    failed_test: Option<&str>,
    log_excerpt_ref: Option<&str>,
) -> StructuredFailure {
    StructuredFailure {
        failed_stage: failed_stage.to_string(),
        failed_step,
        failed_test: failed_test.map(str::to_string),
        log_excerpt_ref: log_excerpt_ref.map(str::to_string),
    }
}

/// Build the FROZEN 5.9 `CheckStatus`-shaped opaque payload value for a terminal `ci.check.updated`
/// fact (X-1, §4) — **assembled THROUGH the [`crate::check_emitter`] producer (CI-P18, P-361), the
/// ONE producer shape (no divergence, EI-01 §7 reconcile-in-place).** The Bus carries the CI-owned
/// struct OPAQUE; this is the byte-identical shape Git's `myelin_git::check_status::CheckStatus`
/// decodes off the payload. A PURE function of the facts + the terminal state, so replay re-builds a
/// BYTE-IDENTICAL payload (the CI-D9 property).
///
/// The `trust_tier` is STAMPED from the run's provenance (`CheckFacts.trust_tier`, read off the
/// CI-P10 dispatch stamp, NEVER recomputed). The `summary` is a HumanisedRef (7.3, never a raw
/// string). `cost_settled` is `Unsettled` on the terminal-but-not-yet-settled fact — a check is NOT
/// "final" until the reserve/settle bookend closes (the terminal-SETTLED re-emit on `job.done` settle
/// carries `cost_settled: true`; this body emits the terminal verdict fact, the settle re-emit is the
/// metering follow-on).
fn terminal_check_status(facts: &CheckFacts, context: &str, success: bool) -> serde_json::Value {
    let state = if success {
        crate::check_emitter::CheckState::Success
    } else {
        crate::check_emitter::CheckState::Failure
    };
    let emit_ctx = crate::check_emitter::CheckEmitContext {
        tenant: tenant_of(&facts.run_ref),
        repo: facts.repo.clone(),
        commit_oid: facts.commit_oid.clone(),
        run_ref: facts.run_ref.clone(),
        run_attempt: facts.run_attempt,
        // STAMPED from provenance (read off the run's CI-P10 stamp), NEVER recomputed — a fork run is
        // recorded faithfully but CI never endorses it (the poisoned-pipeline defence, X-1).
        trust_tier: crate::check_emitter::TrustTier::from_stamp(&facts.trust_tier),
        // The body's run window — the per-step started/completed wall-clock is NOT the supersession
        // authority; the body carries the run's deterministic timestamps (display columns only).
        started_at: "1970-01-01T00:00:00Z".to_string(),
        completed_at: Some("1970-01-01T00:00:00Z".to_string()),
    };
    crate::check_emitter::check_status_payload(
        &emit_ctx,
        crate::check_emitter::CheckProvider::Ci,
        context,
        state,
        // CI's REPORT/echo of `required` — Git's branch-protection policy is authoritative (CI
        // reports, Git decides). The body echoes `true` for the run's emitted contexts.
        true,
        // terminal but NOT settled until CI-P17's reserve/settle bookend closes (the X-1 cost gate).
        crate::check_emitter::CostPosture::Unsettled,
        // the failing-step index resolves through CI-P21's log index; the body anchors on the run.
        None,
    )
}

/// The tenant token from a `myelin://<tenant>/ci/run/<id>` run ref (the projection partition key,
/// EI-02 §1). A PURE parse over the PII-free ref grammar (no IO); defaults to the ref itself if the
/// shape is unexpected (loud-but-safe — the partition key is never silently empty).
fn tenant_of(run_ref: &str) -> String {
    run_ref
        .strip_prefix("myelin://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(run_ref)
        .to_string()
}

// @workflow-body — the `ci.pipeline` durable workflow body (the flow-determinism lint scans this
// file; it reads NO clock/RNG/IO outside WfCtx — every emit/wait/dispatch rides the journaled WfCtx
// surface so replay is bit-identical, CI-D9).

/// **`run_ci_pipeline_body(ctx, run, runner)` — the `ci.pipeline` DURABLE WORKFLOW BODY (CI-P15, arch
/// §3.1).** The deterministic body the durable executor drives under [`CI_PIPELINE_WF_TYPE`]. It
/// composes the FROZEN engine substrate ([`WfCtx::run_ci_pipeline`] — the `SCHEDULE_AND_RUN_JOB`
/// long-park per stage, the reserve/settle bookend, the SLA timer wheel) and adds CI's X-1 PRODUCER
/// side. (A free function, not an inherent `WfCtx` method — the orphan rule keeps the body in CI's
/// crate; it drives `ctx` through the FROZEN public `WfCtx` surface.)
///
/// **In order (arch §3.1):**
/// 1. **Each leading GATE stage parks on `approval:<stage>` (9.4).** A denied/timed-out approval emits
///    `ci.deployment.rejected` and returns [`RunVerdict::Rejected`] (the gated stages never run — 0
///    wasted spend); a parked gate returns [`RunVerdict::Parked`] (the run holds no runtime until the
///    approval arrives).
/// 2. **The runner stages run through the engine's [`WfCtx::run_ci_pipeline`]** (the substrate fixture
///    the CI-D9/CI-D1 drills prove). The engine handles dispatch/park/settle/SLA + the stage verdict;
///    the body reads the [`PipelineOutcome`].
/// 3. **On SUCCESS** — `emit_check(success)` PER context + `ci.run.succeeded` +
///    `signal_merge_queue(ci.result{success})`.
/// 4. **On FAILURE / TIMEOUT** — `emit_check(failure)` PER context + `ci.run.failed` +
///    `signal_merge_queue(ci.result{failure})`.
/// 5. **On PARKED** — return promptly (the run holds no runtime; no terminal facts yet).
///
/// **Determinism (CI-D9).** No clock/RNG/IO outside `WfCtx`: every gate is a journaled
/// `wait_for_signal`, every stage is a journaled `SCHEDULE_AND_RUN_JOB`, every X-1 fact rides
/// `ctx.emit` (the co-committed outbox). The per-context check facts + the rollup are a PURE function
/// of the snapshot's context set + the journaled stage verdict, so a REPLAY is BIT-IDENTICAL and ONLY
/// a journaled `job.done` (the stage verdict) feeds the body.
pub fn run_ci_pipeline_body<R>(
    ctx: &mut WfCtx,
    run: &PipelineRun,
    runner: &R,
) -> WfResult<RunVerdict>
where
    R: JobRunner,
{
    // ── 1. The protected-env / manual GATES (9.4). The arch §3.1 pseudocode gates the deploy stages
    // on a `wait_for_signal("approval:<stage>", window)` BEFORE the stage's jobs run. A gate parks
    // (holds no runtime, may wait DAYS) until a human/automation approves; a deny or a window timeout
    // REJECTS the deploy fast. The leading gates run first, then the runner stages.
    for stage in &run.stages {
        if !stage.gate {
            // The first non-gate stage starts the runner-stage sequence below. (Interleaved gates
            // between runner stages are CI-P16's deploy-stage shape; CI-P15 ships the leading-gate
            // protected-env shape the arch pseudocode names — the deploy gate before the run's jobs.)
            break;
        }
        match gate_stage(ctx, &stage.engine.name, stage.engine.timeout_secs)? {
            GateOutcome::Approved => { /* the gate passed — proceed to the next stage */ }
            GateOutcome::Rejected => {
                // A DENY or a window TIMEOUT → ci.deployment.rejected + fail the run fast. The gated
                // runner stages NEVER dispatch (0 wasted spend on a rejected deploy).
                emit_deployment_rejected(ctx, &run.facts, &stage.engine.name)?;
                return Ok(RunVerdict::Rejected {
                    stage: stage.engine.name.clone(),
                });
            }
            GateOutcome::Parked => return Ok(RunVerdict::Parked),
        }
    }

    // ── 2. The runner stages — the FROZEN engine substrate (`SCHEDULE_AND_RUN_JOB` per stage,
    // reserve/settle, SLA timers). Build the engine spec from the non-gate stages, in order. The
    // engine owns the dispatch/park/settle/verdict; the body owns the X-1 producer emits below.
    let engine_spec = CiPipelineSpec::new(
        run.stages
            .iter()
            .filter(|s| !s.gate)
            .map(|s| s.engine.clone())
            .collect(),
    );
    let outcome = ctx.run_ci_pipeline(&engine_spec, runner)?;

    // ── 3/4. The X-1 PRODUCER side — emit the terminal facts per the verdict (arch §4).
    match outcome {
        PipelineOutcome::Succeeded { stages_completed } => {
            // SUCCESS: a success check PER context + ci.run.succeeded + ci.result{success}.
            emit_terminal_checks(ctx, &run.facts, &run.contexts, true)?;
            emit_run_terminal(ctx, &run.facts, true, None)?;
            emit_ci_result(ctx, &run.facts, &run.contexts, true)?;
            Ok(RunVerdict::Succeeded { stages_completed })
        }
        PipelineOutcome::Failed { stage } => {
            // FAILURE: a failure check PER context + ci.run.failed(structured) + ci.result{failure}.
            emit_terminal_checks(ctx, &run.facts, &run.contexts, false)?;
            emit_run_terminal(ctx, &run.facts, false, Some(&stage))?;
            emit_ci_result(ctx, &run.facts, &run.contexts, false)?;
            Ok(RunVerdict::Failed { stage })
        }
        PipelineOutcome::TimedOut { stage } => {
            // A vanished runner is a failure for the gate's purposes (never a silent pass): the same
            // failure producer path, with the timed-out stage named in the structured failure.
            emit_terminal_checks(ctx, &run.facts, &run.contexts, false)?;
            emit_run_terminal(ctx, &run.facts, false, Some(&stage))?;
            emit_ci_result(ctx, &run.facts, &run.contexts, false)?;
            Ok(RunVerdict::Failed { stage })
        }
        PipelineOutcome::Parked => {
            // The run parked on a stage's job.done — holds no runtime, no terminal facts yet. The
            // dispatcher re-drives when the signal arrives; the body replays to this point.
            Ok(RunVerdict::Parked)
        }
    }
}

/// One protected-env / manual gate: `wait_for_signal("approval:<stage>", window)` (9.4). The approval
/// decision rides the signal payload (a `decline` marker → REJECTED); a window timeout → REJECTED
/// (auto-deny, AG-8 — 0 mutation on a stalled approval). A parked gate holds no runtime.
fn gate_stage(ctx: &mut WfCtx, stage: &str, window_secs: Option<i64>) -> WfResult<GateOutcome> {
    let name = myelin_flow::approval_wait_name(stage);
    match ctx.wait_for_signal(&name, window_secs)? {
        myelin_flow::WaitOutcome::Signalled { payload, .. } => {
            // The approval decision is references-not-payloads: a `decline` marker in the payload refs
            // is a DENY; anything else is an APPROVE (the §6.4 per-effect approval rule).
            let declined = payload
                .iter()
                .any(|r| r.0.contains(myelin_flow::DECLINE_MARKER));
            if declined {
                Ok(GateOutcome::Rejected)
            } else {
                Ok(GateOutcome::Approved)
            }
        }
        myelin_flow::WaitOutcome::TimedOut => Ok(GateOutcome::Rejected),
        myelin_flow::WaitOutcome::Parked => Ok(GateOutcome::Parked),
    }
}

/// Emit one terminal `ci.check.updated` PER context (X-1, §4) via `ctx.emit` (the journaled outbox).
/// REUSES the FROZEN [`check_updated_draft`] so the subject/aggregate grammar is byte-identical to
/// what Git's gate consumes (0 drift). The per-context order is the snapshot's context order, so
/// replay re-emits at the SAME command positions (CI-D9).
fn emit_terminal_checks(
    ctx: &mut WfCtx,
    facts: &CheckFacts,
    contexts: &[String],
    success: bool,
) -> WfResult<()> {
    let repo = myelin_refs::parse_scoped(&facts.repo).map_err(|_| {
        myelin_flow::WfError::CoCommit("invalid canonical Git repository root".into())
    })?;
    if repo.subsystem != "git" || repo.type_ != "repo" || repo.sub.is_some() {
        return Err(myelin_flow::WfError::CoCommit(
            "invalid canonical Git repository root".into(),
        ));
    }
    let commit = myelin_events::check_seam::CheckCommit::from_repo_root(
        &repo.artifact_ref,
        &facts.commit_oid,
    )
    .map_err(|_| myelin_flow::WfError::CoCommit("invalid canonical Git commit root".into()))?;
    for context in contexts {
        let status = terminal_check_status(facts, context, success);
        let draft = check_updated_draft(&commit, context, status)
            .map_err(|_| myelin_flow::WfError::CoCommit("invalid canonical CI check ref".into()))?;
        ctx.emit(draft, None)?;
    }
    Ok(())
}

/// Emit the run-terminal lifecycle event (`ci.run.succeeded` / `ci.run.failed`) via `ctx.emit`. The
/// failure carries the STRUCTURED failure (which stage) — the agent-native triage hook (§3.1).
/// PII-free (references-not-payloads: a run ref + a stage name).
fn emit_run_terminal(
    ctx: &mut WfCtx,
    facts: &CheckFacts,
    success: bool,
    failed_stage: Option<&str>,
) -> WfResult<()> {
    let type_ = if success {
        CI_RUN_SUCCEEDED
    } else {
        CI_RUN_FAILED
    };
    let mut payload = serde_json::json!({
        "run": facts.run_ref,
        "commit_oid": facts.commit_oid,
    });
    if let Some(stage) = failed_stage {
        // structured_failure (§3.1 / §4): the agent-native triage hook — which stage failed (always),
        // plus which step / which test / a log-excerpt ref when the job result surfaced them. The body
        // here knows the stage (its journaled verdict); the richer step/test/log detail is threaded by
        // the runner's job result (the E2E-2 flagship reads the full struct). Byte-identical to the
        // prior shape when only the stage is known (the `failed_stage` key is unchanged, CI-D9).
        payload["structured_failure"] = StructuredFailure::for_stage(stage).to_payload();
    }
    let draft = run_aggregate_draft(type_, &facts.run_ref, payload);
    ctx.emit(draft, None)?;
    Ok(())
}

/// Emit `ci.deployment.rejected` (a denied/timed-out protected-env gate, §3.1) via `ctx.emit`.
fn emit_deployment_rejected(ctx: &mut WfCtx, facts: &CheckFacts, stage: &str) -> WfResult<()> {
    let payload = serde_json::json!({
        "run": facts.run_ref,
        "commit_oid": facts.commit_oid,
        "stage": stage,
    });
    let draft = run_aggregate_draft(CI_DEPLOYMENT_REJECTED, &facts.run_ref, payload);
    ctx.emit(draft, None)?;
    Ok(())
}

/// Emit the `ci.result` rollup signal (the X-1 rollup that wakes Git's merge queue, §4 step 4) via
/// `ctx.emit`. REUSES the FROZEN [`rollup_ci_result`] (the deterministic verdict over the required
/// context set) + [`ci_result_draft`] (the canonical envelope on the `(repo, commit_oid)` aggregate,
/// so the rollup linearises AFTER the per-context checks it rolls up). The `idem_token` is the
/// merge-attempt id the merge queue echoes (a double-delivery wakes it ONCE).
fn emit_ci_result(
    ctx: &mut WfCtx,
    facts: &CheckFacts,
    contexts: &[String],
    success: bool,
) -> WfResult<()> {
    // The post-supersession current verdict: on this single-attempt body, every context shares the
    // run's overall verdict (the body emitted them all success or all failure above). The rollup is
    // over the run's context set (the REQUIRED set Git enforces; the body passes its contexts).
    let current: std::collections::BTreeMap<String, bool> =
        contexts.iter().map(|c| (c.clone(), success)).collect();
    let required: Vec<String> = contexts.to_vec();
    let result: CiResult = rollup_ci_result(
        &facts.commit_oid,
        &current,
        &required,
        &facts.merge_idem_token,
    );
    let repo = myelin_refs::parse_scoped(&facts.repo).map_err(|_| {
        myelin_flow::WfError::CoCommit("invalid canonical Git repository root".into())
    })?;
    if repo.subsystem != "git" || repo.type_ != "repo" || repo.sub.is_some() {
        return Err(myelin_flow::WfError::CoCommit(
            "invalid canonical Git repository root".into(),
        ));
    }
    let commit = myelin_events::check_seam::CheckCommit::from_repo_root(
        &repo.artifact_ref,
        &facts.commit_oid,
    )
    .map_err(|_| myelin_flow::WfError::CoCommit("invalid canonical Git commit root".into()))?;
    let draft = ci_result_draft(&commit, &result)
        .map_err(|_| myelin_flow::WfError::CoCommit("invalid canonical CI result ref".into()))?;
    ctx.emit(draft, None)?;
    Ok(())
}

/// The outcome of a protected-env / manual gate (`wait_for_signal("approval:<stage>")`, 9.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateOutcome {
    /// The gate was APPROVED (the runner stages may proceed).
    Approved,
    /// The gate was DENIED or its window TIMED OUT (→ ci.deployment.rejected, the deploy never runs).
    Rejected,
    /// The gate PARKED (no decision yet — the run holds no runtime until the approval arrives).
    Parked,
}

/// Build an `EventDraft` on the `ci/run/<run>` aggregate (the run-lifecycle / deployment events).
/// Controller-classed (the FACT that a run reached a verdict is platform metadata), Internal-visible
/// (it drives the repo's members' run view), PII-free (references-not-payloads).
fn run_aggregate_draft(type_: &str, run_ref: &str, payload: serde_json::Value) -> EventDraft {
    let subject = ArtifactRef(format!("ci/run/{run_ref}"));
    let aggregate = myelin_events::AggregateKey(format!("ci/run/{run_ref}"));
    EventDraft {
        type_: EventType(type_.to_string()),
        subject,
        aggregate,
        payload,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

#[cfg(test)]
#[path = "ci_pipeline_tests.rs"]
mod tests;
