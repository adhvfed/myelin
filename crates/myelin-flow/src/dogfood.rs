//! # `dogfood` — Myelin's OWN pipelines / merge queue / SLA timers as myelin-flow workflows (P-FLOW-29 / P-516, M6)
//!
//! **The myelin-flow M6 dogfood prompt.** FLOW-M6 promotes NOTHING and freezes NO new contract — it
//! *exercises* the production-hardened durable-workflow engine (the M2 heartbeat, hardened through M5)
//! on **real (self-)tenant data**: the platform's own development. The cheapest, most honest load
//! generator is the team's own work (master-sequencing §2 M6; EI-01 §5 — *the ratchet runs on the
//! builders' own work*), and the moat is only real once **Myelin's own CI pipelines, merge queue, and
//! SLA timers run as myelin-flow workflows on the self-hosting platform** (refined arch §1; roadmap §2
//! M6). The dogfood loop exercises every engine path (replay, timers, signals, the long-park, the
//! merge-queue wake) on the platform's own commits.
//!
//! ## What this module IS (the dogfood DRIVER over the EXISTING engine — EI-01 §7)
//! This is a **caller that drives the already-shipped myelin-flow surface over the Myelin self-tenant**
//! — never a second engine, executor, timer wheel, or merge-queue body. It REUSES:
//! - [`crate::WfCtx::run_ci_pipeline`] (P-FLOW-22, contract 9.2/9.4/11.7) — the CI-pipeline-as-workflow
//!   substrate, reframed onto **Myelin's own build/test/lint pipeline** running as a `ci.pipeline`
//!   workflow on the real durable substrate (a [`crate::FlowDispatcher`] over a `RunStore` + journal +
//!   signal buffer + outbox + timer wheel). Every long stage is a `SCHEDULE_AND_RUN_JOB` long-park; the
//!   pipeline parks holding NO runtime, resumes on the journaled `job.done`, and runs to completion.
//! - [`crate::WfCtx::run_merge_attempt`] (P-FLOW-19/23, contract 5.9/7.3) — the merge-queue workflow
//!   body, reframed onto **a real Myelin PR**: the queue dispatches CI, parks on `ci.result`, and on the
//!   green rollup merges the PR EXACTLY ONCE (0 double-merge) + emits `git.pr.merged` exactly once,
//!   even across a worker kill + an at-least-once double-delivery.
//! - [`crate::TimerStore`] + [`crate::SlaTimerCall`] (P-FLOW-13/14, contract 9.3) — the cheap SLA-timer
//!   arm/re-arm/disarm/fire path, reframed onto **a real Myelin issue's SLA deadline**: the breach timer
//!   arms, a comment slides it (cheap re-arm), and when the deadline passes the wheel FIRES it (a real
//!   Myelin SLA timer fires on a real Myelin issue).
//!
//! ## What this module wires (the dogfood loop is live)
//! - **The myelin-flow drills run as Myelin CI jobs on Myelin's own commits** — wired into the frozen
//!   `myelin_harness::self_hosting_ci::self_hosting_jobs` graph (the FLOW-P29 band; see the harness
//!   module). The dogfood loop is live: the three faces + the truth-up pass run on every Myelin commit.
//! - **The truth-up pass** ([`FlowTruthUpPass`] over [`proven_flow_rows`]) — every PROVEN FLOW row
//!   (FLOW-D1..FLOW-D10 + the E2E-2 spine) rests on a DATED green artifact whose proof SOURCE exists on
//!   disk; no earlier-band FLOW gate is red. A row that names a vanished artifact is surfaced LOUDLY,
//!   never trusted on faith (EI-01 §1, code-wins-over-docs).
//! - **The every-incident-adds-a-drill loop** ([`FlowIncident`]) — a FLOW incident files a PII-free
//!   Myelin issue draft AND a reproducing-drill ticket; the integration drill registers the repro into
//!   the harness `DrillRegistry` (the T-3 `register_drill` hook) so it re-runs forever.
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/durable-workflow.md` §1
//! (the engine paths the dogfood loop exercises). **Roadmap:**
//! `planning/06-roadmaps/shared/durable-workflow.md` §2 FLOW-M6 (dogfood + truth-up).
//! **Master sequencing:** `planning/06-roadmaps/00-master-sequencing.md` §2/§4 M6 (the dogfood band +
//! the done-bar: 0 red earlier-band gate; the truth-up pass). **Doctrine:**
//! `external-insights/01-process-and-quality-doctrine.md` §1 (code-wins-over-docs — the truth-up pass),
//! §4 (drive the real thing — the dogfood loop IS the test), §5 (the ratchet runs on the builders' own
//! work). **VISION §5** (dogfooding).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use myelin_events::check_seam::CiOverall;
use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::reserve_settle::MicroUsd;
use myelin_tenancy::{Region, TenantId};

use crate::merge_queue::{encode_ci_result, merge_attempt_id, MergeRequest};
use crate::timer::sla::sla_timer_id;
use crate::{
    partition_for_run_id, run_state, stage_verdict_marker, ActivityError, CiDispatch, CiDispatcher,
    CiPipelineSpec, CiStage, DriveOutcome, DurableExecutor, FlowDispatcher, FlowExecutor,
    FlowTelemetry, JobKind, JobRunner, JobSpec, MergeOutcome, MergePerformer, ReArmOutcome,
    RunStore, SignalSpec, SignalStore, TimerRow, TimerStore, WfCtx, WfJournal, WorkflowBody,
    CI_PIPELINE_WF_TYPE, CI_RESULT_SIGNAL, JOB_DONE_SIGNAL,
};

/// The Myelin self-tenant id (the platform self-hosts as exactly one cell — P-508 / CP-M6). Opaque,
/// PII-free — the dogfood workflows run over the platform's OWN work under this tenant.
pub const MYELIN_SELF_TENANT: &str = "myelin";

/// The region the Myelin self-tenant is pinned to (fr-par — the dev/prod residency pin; a config swap,
/// never a code change). The dogfood workflows dispatch cell-local in this region.
pub const MYELIN_SELF_REGION: &str = "fr-par";

fn myelin_tenant() -> TenantId {
    TenantId(MYELIN_SELF_TENANT.into())
}
fn myelin_region() -> Region {
    Region(MYELIN_SELF_REGION.into())
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: myelin_tenant(),
        region: myelin_region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            myelin_tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-26T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-26T00:00:00Z".into()),
        caused_by: None,
    }
}

/// The shared durable substrate a dogfood worker drives over (survives a worker restart — so the
/// long-park + crash-recovery paths are exercised end-to-end, exactly as in the M2..M5 drills).
struct Substrate {
    runs: RunStore,
    journal: WfJournal,
    signals: SignalStore,
    outbox: OutboxStore,
    tele: FlowTelemetry,
    timers: TimerStore,
}

// ───────────────────────────── face (1): Myelin's OWN CI pipeline as a ci.pipeline workflow ─────────────────────────────

/// A CI runner fixture standing in for CI's real runner pool (AG-D4-gated dispatch). Counts dispatches
/// so the face proves 0 re-dispatch across the park/resume — the SAME recording-runner discipline the
/// CI-D9/CI-D1 drills use (EI-01 §7; the production binding onto `ToolHands::exec` is CI's, M4).
#[derive(Default)]
struct CountingCiRunner {
    calls: AtomicUsize,
}
impl JobRunner for CountingCiRunner {
    fn dispatch(&self, spec: &JobSpec) -> Result<(), ActivityError> {
        debug_assert_eq!(spec.kind, JobKind::Ci, "a CI pipeline dispatches kind=ci");
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// **Myelin's OWN build/test/lint pipeline** as a `ci.pipeline` workflow: three ordered `kind=ci`
/// stages (`build` → `test` → `lint`), each a `SCHEDULE_AND_RUN_JOB` long-park with an SLA. This is
/// the shape Myelin's self-hosting CI runs under (the dogfood loop) — every long Myelin build stage is
/// one durable long-park.
fn myelin_pipeline() -> CiPipelineSpec {
    CiPipelineSpec::new(vec![
        CiStage::new(
            "build",
            "pipeline://myelin/ci/self-host#build",
            MicroUsd(0),
            Some(3600),
        ),
        CiStage::new(
            "test",
            "pipeline://myelin/ci/self-host#test",
            MicroUsd(0),
            Some(3600),
        ),
        CiStage::new(
            "lint",
            "pipeline://myelin/ci/self-host#lint",
            MicroUsd(0),
            Some(600),
        ),
    ])
}

fn ci_pipeline_body(runner: Arc<CountingCiRunner>) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        let out = ctx
            .run_ci_pipeline(&myelin_pipeline(), runner.as_ref())
            .map_err(|e| format!("{e:?}"))?;
        match out {
            crate::PipelineOutcome::Succeeded { stages_completed } => Ok(vec![ArtifactRef(
                format!("outcome:succeeded:{stages_completed}"),
            )]),
            crate::PipelineOutcome::Failed { stage } => {
                Ok(vec![ArtifactRef(format!("outcome:failed:{stage}"))])
            }
            crate::PipelineOutcome::TimedOut { stage } => {
                Ok(vec![ArtifactRef(format!("outcome:timedout:{stage}"))])
            }
            crate::PipelineOutcome::Parked => Ok(vec![]),
        }
    })
}

fn fresh_pipeline_worker(
    sub: &Substrate,
    worker: &str,
    partition: i16,
    runner: Arc<CountingCiRunner>,
) -> FlowDispatcher {
    let mut disp = FlowDispatcher::new(
        sub.runs.clone(),
        sub.outbox.clone(),
        sub.journal.clone(),
        sub.tele.clone(),
        minter(),
        ctx_base(),
        partition,
        worker,
        30,
    )
    .with_signals(sub.signals.clone())
    .with_timers(sub.timers.clone());
    disp.register(CI_PIPELINE_WF_TYPE, ci_pipeline_body(runner));
    disp
}

fn start_pipeline(idem: &str) -> (FlowExecutor, crate::RunId, Substrate) {
    let ex = FlowExecutor::new(minter(), myelin_tenant(), myelin_region());
    ex.register_definition(CI_PIPELINE_WF_TYPE);
    let run = ex
        .start(crate::StartSpec {
            wf_type: CI_PIPELINE_WF_TYPE.into(),
            input: vec![],
            budget: None,
            idem_key: idem.into(),
        })
        .expect("start Myelin's own ci.pipeline workflow");
    let sub = Substrate {
        runs: ex.runs().clone(),
        journal: WfJournal::new(),
        signals: ex.signals().clone(),
        outbox: OutboxStore::new(),
        tele: FlowTelemetry::new(),
        timers: TimerStore::new(),
    };
    (ex, run, sub)
}

fn stage_token(run_id: &str, stage_idx: usize) -> String {
    crate::job_idem_token(run_id, &format!("{CI_PIPELINE_WF_TYPE}:{}", stage_idx * 2))
}

fn deliver_stage_done(
    ex: &FlowExecutor,
    run: &crate::RunId,
    stage_idx: usize,
    stage: &str,
) -> crate::SignalOutcome {
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: JOB_DONE_SIGNAL.into(),
        idem_key: stage_token(&run.0, stage_idx),
        payload: vec![stage_verdict_marker(stage, true)],
        payload_key_ref: None,
    })
    .expect("deliver job.done")
}

/// The result of driving Myelin's OWN CI pipeline as a `ci.pipeline` workflow end-to-end across a
/// worker kill (the long-park + crash-recovery path). GREEN iff the pipeline ran every stage to
/// completion with 0 re-dispatch (the replay short-circuited the journaled prefix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineFace {
    /// `true` iff the pipeline completed green (all three Myelin stages ran to completion).
    pub completed: bool,
    /// how many stages the runner dispatched (must equal the stage count — 0 re-dispatch across the kill).
    pub dispatches: usize,
    /// the stage count the pipeline ran (3 — build/test/lint).
    pub stages: usize,
}

impl PipelineFace {
    /// `true` iff Myelin's own CI pipeline ran to completion with exactly one dispatch per stage.
    pub fn is_green(&self) -> bool {
        self.completed && self.dispatches == self.stages
    }
}

/// **Run Myelin's OWN CI pipeline as a `ci.pipeline` myelin-flow workflow (P-FLOW-29 face 1).** Drives
/// the production CI-pipeline substrate over the Myelin self-tenant: dispatch `build` + park, KILL the
/// worker, deliver each stage's `job.done` (at-least-once), resume on a fresh worker, replay the
/// journaled prefix (0 re-dispatch), and run every stage to completion — exercising replay, the
/// long-park, signals, and the durable timer in one workflow on Myelin's own build.
pub fn run_myelin_ci_pipeline() -> PipelineFace {
    let runner = Arc::new(CountingCiRunner::default());
    let (ex, run, sub) = start_pipeline("ci.pipeline:myelin:self-host:run-1");
    let part = partition_for_run_id(&run.0);
    let stages = ["build", "test", "lint"];

    // Worker 1: dispatch build + park. Then the worker crashes (the control plane is gone) — exactly
    // the M5 long-park-across-a-kill path, driven on Myelin's own pipeline.
    {
        let w = fresh_pipeline_worker(&sub, "worker-1", part, runner.clone());
        let _ = w.tick(1_000, "2026-06-26T00:00:00Z", 7);
    }

    let mut completed = false;
    for (idx, stage) in stages.iter().enumerate() {
        // The runner delivers this stage's job.done TWICE (at-least-once); the wf_signal PK dedups.
        let _ = deliver_stage_done(&ex, &run, idx, stage);
        let _ = deliver_stage_done(&ex, &run, idx, stage);
        sub.runs.wake(&myelin_tenant(), &run.0);
        let w = fresh_pipeline_worker(&sub, &format!("worker-{}", idx + 2), part, runner.clone());
        let out = w
            .tick((idx as i64 + 2) * 1_000, "2026-06-26T01:00:00Z", 7)
            .expect("resume the Myelin pipeline");
        if let DriveOutcome::Completed(refs) = out {
            completed = refs == vec![ArtifactRef("outcome:succeeded:3".into())];
        }
    }

    let _ = sub.runs.get(&myelin_tenant(), &run.0);
    PipelineFace {
        completed: completed
            && sub.runs.get(&myelin_tenant(), &run.0).map(|r| r.state)
                == Some(run_state::COMPLETED.to_string()),
        dispatches: runner.calls.load(Ordering::SeqCst),
        stages: stages.len(),
    }
}

// ───────────────────────────── face (2): Myelin's OWN merge queue merges a real Myelin PR ─────────────────────────────

#[derive(Default)]
struct CountingCi {
    calls: AtomicUsize,
}
impl CiDispatcher for CountingCi {
    fn dispatch(&self, _ci: &CiDispatch) -> Result<(), ActivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct CountingMerger {
    merges: AtomicUsize,
}
impl MergePerformer for CountingMerger {
    fn merge(&self, request: &MergeRequest) -> Result<String, ActivityError> {
        self.merges.fetch_add(1, Ordering::SeqCst);
        Ok(format!("merged-{}", request.speculative_commit_oid))
    }
}

/// A real Myelin PR's merge request (the platform's own monorepo, the `main` branch).
fn myelin_pr() -> MergeRequest {
    MergeRequest {
        pr_ref: "myelin://myelin/git/monorepo#pr-516".into(),
        target_ref: "refs/heads/main".into(),
        speculative_commit_oid: "feedface".into(),
        required_contexts: vec!["build".into(), "test".into()],
    }
}

fn merge_queue_body(ci: Arc<CountingCi>, merger: Arc<CountingMerger>) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        let out = ctx
            .run_merge_attempt(
                &myelin_pr(),
                ci.as_ref(),
                merger.as_ref(),
                Some(3600),
                MicroUsd(0),
                vec![],
            )
            .map_err(|e| format!("{e:?}"))?;
        match out {
            MergeOutcome::Merged {
                merged_commit_oid, ..
            } => Ok(vec![ArtifactRef(format!(
                "outcome:merged:{merged_commit_oid}"
            ))]),
            MergeOutcome::Dequeued { reason } => {
                Ok(vec![ArtifactRef(format!("outcome:dequeued:{reason}"))])
            }
            MergeOutcome::TimedOut => Ok(vec![ArtifactRef("outcome:timedout".into())]),
            MergeOutcome::Parked => Ok(vec![]),
        }
    })
}

fn fresh_merge_worker(
    sub: &Substrate,
    worker: &str,
    partition: i16,
    ci: Arc<CountingCi>,
    merger: Arc<CountingMerger>,
) -> FlowDispatcher {
    let mut disp = FlowDispatcher::new(
        sub.runs.clone(),
        sub.outbox.clone(),
        sub.journal.clone(),
        sub.tele.clone(),
        minter(),
        ctx_base(),
        partition,
        worker,
        30,
    )
    .with_signals(sub.signals.clone())
    .with_timers(sub.timers.clone());
    disp.register("merge.queue", merge_queue_body(ci, merger));
    disp
}

fn start_merge(idem: &str) -> (FlowExecutor, crate::RunId, Substrate) {
    let ex = FlowExecutor::new(minter(), myelin_tenant(), myelin_region());
    ex.register_definition("merge.queue");
    let run = ex
        .start(crate::StartSpec {
            wf_type: "merge.queue".into(),
            input: vec![],
            budget: None,
            idem_key: idem.into(),
        })
        .expect("start Myelin's own merge-queue workflow");
    let sub = Substrate {
        runs: ex.runs().clone(),
        journal: WfJournal::new(),
        signals: ex.signals().clone(),
        outbox: OutboxStore::new(),
        tele: FlowTelemetry::new(),
        timers: TimerStore::new(),
    };
    (ex, run, sub)
}

/// The result of Myelin's own merge queue merging a real Myelin PR. GREEN iff the PR merged EXACTLY
/// ONCE (0 double-merge), the `git.pr.merged` emit landed exactly once, and CI was dispatched once
/// across the kill (0 re-dispatch).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeFace {
    /// how many times the PR was merged (must be exactly 1 — 0 double-merge).
    pub merges: usize,
    /// how many `git.pr.merged` events were emitted (must be exactly 1).
    pub git_pr_merged_emits: u64,
    /// how many times CI was dispatched (must be exactly 1 — 0 re-dispatch across the kill).
    pub ci_dispatches: usize,
}

impl MergeFace {
    /// `true` iff Myelin's own merge queue merged the real Myelin PR exactly once with no double-effect.
    pub fn is_green(&self) -> bool {
        self.merges == 1 && self.git_pr_merged_emits == 1 && self.ci_dispatches == 1
    }
}

/// **Myelin's OWN merge queue merges a real Myelin PR EXACTLY ONCE (P-FLOW-29 face 2).** Drives the
/// production merge-queue workflow body over the Myelin self-tenant: dispatch CI + park on `ci.result`,
/// KILL the worker, deliver the green rollup TWICE (at-least-once), resume on a fresh worker, and merge
/// the PR — exactly once, with one `git.pr.merged` emit and 0 re-dispatch (the merge-queue wake path on
/// Myelin's own PR).
pub fn run_myelin_merge_queue() -> MergeFace {
    let ci = Arc::new(CountingCi::default());
    let merger = Arc::new(CountingMerger::default());
    let (ex, run, sub) = start_merge("queue:myelin:main:pr-516");
    let part = partition_for_run_id(&run.0);

    // Worker 1: dispatch CI + park. Then the worker crashes while parked (the merge-queue wake path).
    {
        let w = fresh_merge_worker(&sub, "worker-1", part, ci.clone(), merger.clone());
        let _ = w.tick(1_000, "2026-06-26T00:00:00Z", 7);
    }

    // CI delivers the green rollup TWICE (at-least-once double-delivery) — the wf_signal PK dedups.
    let attempt = merge_attempt_id(&run.0, "merge.queue:0");
    for _ in 0..2 {
        let result = myelin_events::check_seam::CiResult {
            commit_oid: "feedface".into(),
            overall: CiOverall::Success,
            contexts: vec!["build".into(), "test".into()],
            idem_token: attempt.clone(),
        };
        let _ = ex.signal(SignalSpec {
            run: run.clone(),
            signal_name: CI_RESULT_SIGNAL.into(),
            idem_key: attempt.clone(),
            payload: encode_ci_result(&result),
            payload_key_ref: None,
        });
    }
    sub.runs.wake(&myelin_tenant(), &run.0);

    // Worker 2 (redeployed): resume + merge the real Myelin PR.
    {
        let w = fresh_merge_worker(&sub, "worker-2", part, ci.clone(), merger.clone());
        let _ = w.tick(2_000, "2026-06-26T02:00:00Z", 7);
    }

    MergeFace {
        merges: merger.merges.load(Ordering::SeqCst),
        git_pr_merged_emits: sub.outbox.committed_count() as u64,
        ci_dispatches: ci.calls.load(Ordering::SeqCst),
    }
}

// ───────────────────────────── face (3): a real Myelin SLA timer fires on a real Myelin issue ─────────────────────────────

/// The result of a real Myelin SLA timer firing on a real Myelin issue. GREEN iff the breach timer
/// armed, the comment-slide re-armed it cheaply (one row), and the wheel FIRED it past the deadline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlaFace {
    /// `true` iff the breach timer armed on the first arm.
    pub armed: bool,
    /// `true` iff a comment slide re-armed the SAME row (cheap re-arm, 0 new wheel rows).
    pub re_armed: bool,
    /// `true` iff the wheel FIRED the SLA timer once the deadline passed (the breach fired).
    pub fired: bool,
    /// the total wheel rows scanned to fire it (the SC-11 "indexed not scanned" probe — only the due row).
    pub rows_scanned: u64,
}

impl SlaFace {
    /// `true` iff a real Myelin SLA timer armed, re-armed cheaply, and FIRED on a real Myelin issue.
    pub fn is_green(&self) -> bool {
        self.armed && self.re_armed && self.fired
    }
}

/// **A real Myelin SLA timer FIRES on a real Myelin issue (P-FLOW-29 face 3).** Drives the production
/// SLA-timer path over the Myelin self-tenant: arm the breach timer for a real Myelin issue, slide it
/// once via the cheap re-arm (a comment that moves the deadline — one row update), then advance the
/// wheel past the deadline and FIRE it. Exercises arm / cheap re-arm / the bucketed wheel scan / fire
/// on Myelin's own issue.
pub fn run_myelin_sla_timer() -> SlaFace {
    let timers = TimerStore::new();
    let tenant = myelin_tenant();
    let region = myelin_region();
    // A real Myelin issue's breach SLA timer — the deterministic, PII-free `sla/<issue_key>` handle.
    let issue_key = "myelin/platform#516";
    let timer_id = sla_timer_id(issue_key);

    // Arm the breach timer for the Myelin issue (deadline at t=1000s — a bare SLA timer, no run wake).
    let armed = timers.arm(TimerRow {
        tenant: tenant.clone(),
        region,
        timer_id: timer_id.clone(),
        run_id: None,
        command_id: format!("{timer_id}/arm"),
        fire_at: 1_000,
        bucket: crate::epoch_minute(1_000),
        fired: false,
        partition: 0,
    });

    // A comment on the Myelin issue slides the deadline (cheap re-arm — one row, no second wheel row).
    let re_armed = matches!(
        crate::SlaTimerCall::new(&timers, tenant.clone(), timer_id.clone()).re_arm(2_000),
        ReArmOutcome::ReArmed
    );

    // The wheel advances PAST the (new) deadline (now=3000s); the bucketed scan reads the due bucket
    // (`bucket <= now AND NOT fired`) and FIRES the breach — a real Myelin SLA timer firing.
    let due = timers.scan_due(0, 3_000, 64);
    let mut fired = false;
    if let Some(row) = due.into_iter().find(|r| r.timer_id == timer_id) {
        fired = matches!(
            timers.fire(&tenant, &row.timer_id, &WfJournal::new(), &RunStore::new()),
            crate::FireOutcome::Fired
        );
    }

    SlaFace {
        armed: matches!(armed, crate::ArmOutcome::Armed),
        re_armed,
        fired,
        rows_scanned: timers.rows_scanned(),
    }
}

// ───────────────────────────── the aggregate dogfood artifact ─────────────────────────────

/// **The named green artifact the FLOW dogfood run emits.** Myelin's own pipelines / merge queue / SLA
/// timers driven as myelin-flow workflows over the Myelin self-tenant, across the three faces:
/// - **Myelin's own CI pipeline** runs as a `ci.pipeline` workflow end-to-end (face 1);
/// - **Myelin's own merge queue** merges a real Myelin PR exactly once (face 2);
/// - **a real Myelin SLA timer** fires on a real Myelin issue (face 3).
///
/// GREEN iff every face is green — a RED face fails LOUDLY ([`Self::is_green`] is false), never a
/// claimed-but-unearned green.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the dogfood artifact must be checked — an unread RED face silently claims a green the \
              engine did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct FlowDogfoodArtifact {
    /// the date the dogfood run was asserted.
    pub date: String,
    /// face 1 — Myelin's own CI pipeline as a `ci.pipeline` workflow.
    pub pipeline: PipelineFace,
    /// face 2 — Myelin's own merge queue merges a real Myelin PR exactly once.
    pub merge_queue: MergeFace,
    /// face 3 — a real Myelin SLA timer fires on a real Myelin issue.
    pub sla_timer: SlaFace,
}

impl FlowDogfoodArtifact {
    /// `true` iff every face is green — Myelin's pipelines / merge queue / SLA timers all run as
    /// myelin-flow workflows on the self-hosting platform.
    pub fn is_green(&self) -> bool {
        self.pipeline.is_green() && self.merge_queue.is_green() && self.sla_timer.is_green()
    }

    /// The dated one-line summary (the artifact body the dogfood CI run prints).
    pub fn summary(&self) -> String {
        format!(
            "P-516 FLOW DOGFOOD {} — tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             ci-pipeline={} merge-queue={} sla-timer={} verdict={}",
            self.date,
            self.pipeline.is_green(),
            self.merge_queue.is_green(),
            self.sla_timer.is_green(),
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

/// **Run Myelin's OWN pipelines / merge queue / SLA timers as myelin-flow workflows (P-FLOW-29).** The
/// dogfood loop: drives all three production engine paths over the Myelin self-tenant, REUSING the
/// already-shipped surface (EI-01 §7, never a second engine). `date` is the run stamp.
pub fn run_flow_over_myelins_own_work(date: &str) -> FlowDogfoodArtifact {
    FlowDogfoodArtifact {
        date: date.to_string(),
        pipeline: run_myelin_ci_pipeline(),
        merge_queue: run_myelin_merge_queue(),
        sla_timer: run_myelin_sla_timer(),
    }
}

// ───────────────────────────── (2) the truth-up pass over the PROVEN FLOW rows ─────────────────────────────

/// One PROVEN FLOW row the truth-up pass enumerates: a gate/drill the ledger claims PROVEN, with the
/// proof command that emits its dated green artifact AND the repo-relative path to that proof source.
/// The truth-up pass asserts EACH row rests on a DATED green artifact whose source EXISTS on disk — a
/// row that names a vanished artifact is surfaced, never trusted on faith (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenFlowRow {
    /// the stable gate/drill id (e.g. `"FLOW-D1"`, `"E2E-2"`).
    pub id: &'static str,
    /// the contract SECTION the row's gate belongs to (the §x.y face of the durable-workflow doc).
    pub section: &'static str,
    /// a one-line human title (what the row proves).
    pub title: &'static str,
    /// the proof command that emits this row's dated green artifact.
    pub proof_command: &'static str,
    /// the repo-RELATIVE path to the proof source (the test file `proof_command` runs).
    pub artifact_path: &'static str,
    /// the DATE the row's green artifact was last emitted, if any. `None` ⇒ CLAIMED-NOT-PROVEN.
    pub artifact_date: Option<String>,
}

impl ProvenFlowRow {
    /// `true` iff this row rests on a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    /// Resolve this row's [`artifact_path`](Self::artifact_path) to an absolute path under `repo_root`.
    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

/// **The FROZEN set of PROVEN FLOW rows the truth-up pass enumerates (P-FLOW-29).** Every FLOW gate the
/// ledger claims PROVEN: the ten drills **FLOW-D1..FLOW-D10** (the engine heartbeat, the timers, the
/// signals/HITL, the long-park, the world-scale family) **plus** the whole-system **E2E-2** spine
/// (P-FLOW-28). The truth-up pass asserts EVERY id here rests on a dated green artifact whose proof
/// source exists on disk; a row without one is a loud failure. `date` is supplied by the runner.
pub fn proven_flow_rows(date: &str) -> Vec<ProvenFlowRow> {
    fn row(
        id: &'static str,
        section: &'static str,
        title: &'static str,
        cmd: &'static str,
        artifact_path: &'static str,
        date: &str,
    ) -> ProvenFlowRow {
        ProvenFlowRow {
            id,
            section,
            title,
            proof_command: cmd,
            artifact_path,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        // ── The engine heartbeat (FLOW-D1/D2/D5) — replay/recovery, divergence guard, co-commit. ──
        row(
            "FLOW-D1",
            "9.1",
            "deterministic replay/recovery + lease-based crash recovery — a killed run resumes, 0 lost effect",
            "cargo test -p myelin-flow --test drills_flow_d1_replay",
            "crates/myelin-flow/tests/drills_flow_d1_replay.rs",
            date,
        ),
        row(
            "FLOW-D2",
            "9.1",
            "the replay-divergence guard — a divergent/wrong-version replay halts nondeterministic + dead-letters, 0 silent divergence",
            "cargo test -p myelin-flow --features integration --test integration_flow_replay",
            "crates/myelin-flow/tests/integration_flow_replay.rs",
            date,
        ),
        row(
            "FLOW-D5",
            "9.1",
            "the WfCtx journal/outbox CO-COMMIT — the silent-data-loss floor: the journal + the outbox land atomically",
            "cargo test -p myelin-flow --test drills_flow_d5_cocommit",
            "crates/myelin-flow/tests/drills_flow_d5_cocommit.rs",
            date,
        ),
        // ── The durable timers (FLOW-D3) — the minute-bucket wheel at six then seven figures. ──
        row(
            "FLOW-D3",
            "9.3",
            "the minute-bucket durable timer wheel at 1M+ cell-scale (indexed-not-scanned) — the 100k floor promoted to seven figures",
            "cargo test -p myelin-flow --test drills_flow_d3_full_1m_timer_wheel",
            "crates/myelin-flow/tests/drills_flow_d3_full_1m_timer_wheel.rs",
            date,
        ),
        // ── The durable signals + HITL (FLOW-D4) — the multi-day round-trip + per-effect partial. ──
        row(
            "FLOW-D4",
            "9.4",
            "the multi-day HITL approval-card round-trip + the per-effect partial-approval across a restart + deploy",
            "cargo test -p myelin-flow --test drills_flow_d4_multiday_hitl --test drills_flow_d4_per_effect",
            "crates/myelin-flow/tests/drills_flow_d4_multiday_hitl.rs",
            date,
        ),
        // ── The long-park (FLOW-D6/D7) — reserve/settle bookend + loop safety. ──
        row(
            "FLOW-D6",
            "11.7",
            "the reserve/settle bookend on a runaway loop vs a depleting wallet — spend is bounded, balanced",
            "cargo test -p myelin-flow --test drills_flow_d6_reserve_settle",
            "crates/myelin-flow/tests/drills_flow_d6_reserve_settle.rs",
            date,
        ),
        row(
            "FLOW-D7",
            "9.2",
            "the adversarial workflow→event→workflow loop is STOPPED (the causal loop-safety ceiling)",
            "cargo test -p myelin-flow --test drills_flow_d7_loop_safety",
            "crates/myelin-flow/tests/drills_flow_d7_loop_safety.rs",
            date,
        ),
        // ── The world-scale family (FLOW-D8/D9/D10) — surge, crypto-shred, restore-verify. ──
        row(
            "FLOW-D8",
            "12.6",
            "the world-scale 30× agent surge + the protected-human-lane shed order — the human lane holds, the machine lane sheds",
            "cargo test -p myelin-flow --test drills_flow_d8_surge",
            "crates/myelin-flow/tests/drills_flow_d8_surge.rs",
            date,
        ),
        row(
            "FLOW-D9",
            "9.6",
            "crypto-shred reaching history — the PersonalDataHolder erase path COMPLETE, 0 recoverable PII",
            "cargo test -p myelin-flow --test drills_flow_d9_crypto_shred",
            "crates/myelin-flow/tests/drills_flow_d9_crypto_shred.rs",
            date,
        ),
        row(
            "FLOW-D10",
            "9.5",
            "restore to a consistent point — in-flight runs resume, no vanished result; the X-1 seam end-to-end",
            "cargo test -p myelin-flow --test drills_flow_d10_restore_verify --test drills_flow_d10_x1_seam_e2e",
            "crates/myelin-flow/tests/drills_flow_d10_restore_verify.rs",
            date,
        ),
        // ── The whole-system E2E-2 spine (P-FLOW-28) — the durable-workflow + HITL flagship. ──
        row(
            "E2E-2",
            "9.1",
            "the durable-workflow + HITL SPINE — CI-fail → triage agent → issue → chat → fix-PR across a kill + days-later approval (exactly-once, merge-count==1, reserve/settle balanced)",
            "cargo test -p myelin-flow --test drills_flow_e2e2_spine",
            "crates/myelin-flow/tests/drills_flow_e2e2_spine.rs",
            date,
        ),
    ]
}

/// The verdict of the FLOW truth-up pass — Green (every PROVEN row dated) or Red (the undated rows
/// named). Never a swallowed bool — a RED points at exactly which FLOW claim outran its verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowTruthUpVerdict {
    /// every enumerated PROVEN FLOW row rests on a dated green artifact (no earlier-band FLOW gate red).
    Green {
        /// how many PROVEN rows were confirmed dated + green.
        rows_confirmed: usize,
        /// the date the truth-up pass ran.
        date: String,
    },
    /// one or more PROVEN rows are CLAIMED-NOT-PROVEN. Names them so the failure is specific.
    Red {
        /// the ids of the rows lacking a dated green artifact.
        undated_rows: Vec<&'static str>,
    },
}

impl FlowTruthUpVerdict {
    /// `true` iff the truth-up pass is green (every PROVEN row dated).
    pub fn is_green(&self) -> bool {
        matches!(self, FlowTruthUpVerdict::Green { .. })
    }

    /// the ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass).
    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            FlowTruthUpVerdict::Green { .. } => &[],
            FlowTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

/// **The FLOW truth-up pass (P-FLOW-29 / EI-01 §1).** Enumerates every PROVEN FLOW row and confirms
/// each rests on a DATED green artifact. A row WITHOUT one is a LOUD failure ([`FlowTruthUpVerdict::Red`]),
/// never a silent pass — the code-wins-over-docs discipline made mechanical. A zero-sized orchestrator.
#[derive(Clone, Copy, Debug, Default)]
pub struct FlowTruthUpPass;

impl FlowTruthUpPass {
    /// A new truth-up pass (stateless).
    pub fn new() -> FlowTruthUpPass {
        FlowTruthUpPass
    }

    /// **Run the truth-up pass over `rows`.** Returns [`FlowTruthUpVerdict::Green`] (every row dated) or
    /// [`FlowTruthUpVerdict::Red`] (the undated rows named). `date` stamps the green verdict.
    pub fn run(&self, rows: &[ProvenFlowRow], date: &str) -> FlowTruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            FlowTruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            FlowTruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    /// **The loud-never-swallowed truth-up CI entrypoint (EI-01 §5).** Run the pass and turn a RED
    /// verdict into a process-failing `Err` — so `pass.run_or_fail_ci(&rows, date)?` FAILS the dogfood
    /// truth-up job if ANY PROVEN FLOW row lacks a dated green artifact. On GREEN it returns the count.
    pub fn run_or_fail_ci(
        &self,
        rows: &[ProvenFlowRow],
        date: &str,
    ) -> Result<usize, FlowTruthUpRed> {
        match self.run(rows, date) {
            FlowTruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            FlowTruthUpVerdict::Red { undated_rows } => Err(FlowTruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

/// A RED truth-up pass surfaced as an `Err` — the CLAIMED-NOT-PROVEN FLOW rows, loud + specific.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowTruthUpRed {
    /// the ids of the rows lacking a dated green artifact.
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for FlowTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL — {} FLOW row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} — a claim \
             that outlives its verification misleads the next agent (EI-01 §1); fix the doc or re-run \
             the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for FlowTruthUpRed {}

// ───────────────────────────── the enumerated truth-up scorecard (the green artifact) ─────────────────────────────

/// How a PROVEN FLOW row's proof stands at truth-up time: a dated green artifact, or an
/// honestly-recorded CLAIMED-NOT-PROVEN note. Either way the status carries a DATE (EI-01 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowRowStatus {
    /// the row rests on a dated green artifact whose proof source exists on disk.
    DatedGreen {
        /// the date the green artifact was last emitted.
        date: String,
    },
    /// the row is CLAIMED but NOT PROVEN — no dated green artifact, or its proof source is gone.
    ClaimedNotProven {
        /// the date the truth-up pass recorded the gap.
        date: String,
        /// why the row is not proven.
        reason: String,
    },
}

impl FlowRowStatus {
    /// `true` iff this is a dated green artifact (the per-row truth-up invariant).
    pub fn is_dated_green(&self) -> bool {
        matches!(self, FlowRowStatus::DatedGreen { .. })
    }
}

/// One scorecard line: a PROVEN FLOW row resolved to its [`FlowRowStatus`] at truth-up time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowScorecardEntry {
    /// the row this line scores.
    pub row: ProvenFlowRow,
    /// its resolved status (dated-green or claimed-not-proven, both dated).
    pub status: FlowRowStatus,
}

/// **The enumerated FLOW truth-up scorecard (the GATE/DRILLS green artifact, P-FLOW-29).** Every PROVEN
/// FLOW row → its dated green artifact (or a dated CLAIMED-NOT-PROVEN note). Rendering it produces the
/// §x.y-grouped table the prompt's GATE demands, and [`Self::is_green`] is true iff NO earlier-band FLOW
/// gate is red.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked — an unread CLAIMED-NOT-PROVEN row silently \
              drifts the docs from the code (EI-01 §1)"]
pub struct FlowTruthUpScorecard {
    /// the run date the scorecard is stamped with.
    pub date: String,
    /// one entry per PROVEN FLOW row, in section order.
    pub entries: Vec<FlowScorecardEntry>,
}

impl FlowTruthUpScorecard {
    /// `true` iff every row rests on a dated green artifact (the gate invariant: no FLOW gate red).
    pub fn is_green(&self) -> bool {
        self.entries.iter().all(|e| e.status.is_dated_green())
    }

    /// how many rows the scorecard enumerates.
    pub fn rows_total(&self) -> usize {
        self.entries.len()
    }

    /// how many rows rest on a dated green artifact.
    pub fn rows_dated_green(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status.is_dated_green())
            .count()
    }

    /// the ids of any CLAIMED-NOT-PROVEN rows (empty on a green pass) — the loud failure list.
    pub fn claimed_not_proven(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| !e.status.is_dated_green())
            .map(|e| e.row.id)
            .collect()
    }

    /// **Render the enumerated scorecard as the dated green artifact** (the §x.y-grouped table a
    /// truth-up CI run prints). CLAIMED-NOT-PROVEN rows are rendered LOUD, never elided.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let verdict = if self.is_green() {
            "GREEN (no earlier-band FLOW gate red)"
        } else {
            "RED (a FLOW claim outran its verification)"
        };
        out.push_str(&format!(
            "P-516 FLOW TRUTH-UP SCORECARD {} — {}/{} rows dated-green, verdict={verdict}\n",
            self.date,
            self.rows_dated_green(),
            self.rows_total(),
        ));
        for e in &self.entries {
            let status = match &e.status {
                FlowRowStatus::DatedGreen { date } => format!("DATED-GREEN({date})"),
                FlowRowStatus::ClaimedNotProven { date, reason } => {
                    format!("CLAIMED-NOT-PROVEN({date}: {reason})")
                }
            };
            out.push_str(&format!(
                "  [§{}] {:<10} {:<28} — {}  ⟨{}⟩\n",
                e.row.section, e.row.id, status, e.row.title, e.row.proof_command,
            ));
        }
        out
    }
}

/// **Run the FLOW truth-up pass and produce the enumerated [`FlowTruthUpScorecard`] (P-FLOW-29).** For
/// each PROVEN FLOW row this resolves a dated [`FlowRowStatus`]: a row is DATED-GREEN iff it carries an
/// `artifact_date` AND its proof source exists on disk under `repo_root`; otherwise it is recorded
/// CLAIMED-NOT-PROVEN with the run `date`. The scorecard surfaces — never swallows — any gap (EI-01 §1).
pub fn run_flow_truth_up_scorecard(
    date: &str,
    repo_root: &std::path::Path,
) -> FlowTruthUpScorecard {
    let entries = proven_flow_rows(date)
        .into_iter()
        .map(|row| {
            let status = match &row.artifact_date {
                None => FlowRowStatus::ClaimedNotProven {
                    date: date.to_string(),
                    reason: "no dated green artifact".to_string(),
                },
                Some(_) if !row.artifact_abs_path(repo_root).exists() => {
                    FlowRowStatus::ClaimedNotProven {
                        date: date.to_string(),
                        reason: format!("proof source missing on disk: {}", row.artifact_path),
                    }
                }
                Some(d) => FlowRowStatus::DatedGreen { date: d.clone() },
            };
            FlowScorecardEntry { row, status }
        })
        .collect();
    FlowTruthUpScorecard {
        date: date.to_string(),
        entries,
    }
}

// ───────────────────────────── (3) the every-incident-adds-a-drill loop ─────────────────────────────

/// **A FLOW incident on Myelin's own development (the every-incident-adds-a-drill loop, EI-01 §3/§5).**
/// A real incident ends by filing a PII-free Myelin issue draft AND a reproducing-drill ticket — both
/// reference-linked (the issue points at the drill that reproduces it). The integration drill registers
/// the repro into the harness `DrillRegistry` so it re-runs forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowIncident {
    /// the incident id (PII-free, e.g. `"INC-FLOW-DOGFOOD-1"`).
    pub incident_id: String,
    /// the FLOW gate the incident regressed (e.g. `"FLOW-D1"`).
    pub gate_id: String,
    /// a PII-free one-line description of what broke.
    pub description: String,
    /// the name of the reproducing drill the incident files.
    pub repro_drill_name: String,
}

impl FlowIncident {
    /// A new FLOW incident (every field PII-free).
    pub fn new(
        incident_id: &str,
        gate_id: &str,
        description: &str,
        repro_drill_name: &str,
    ) -> FlowIncident {
        FlowIncident {
            incident_id: incident_id.into(),
            gate_id: gate_id.into(),
            description: description.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    /// The PII-free Myelin issue draft the incident files (names the gate + the repro drill).
    pub fn issue_draft(&self) -> FlowIncidentIssueDraft {
        FlowIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!(
                "[{}] FLOW gate {} regressed",
                self.incident_id, self.gate_id
            ),
            body: format!(
                "FLOW incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked — every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    /// The reproducing-drill ticket the incident files (the test that joins the permanent suite).
    pub fn drill_ticket(&self) -> FlowIncidentDrillTicket {
        FlowIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

/// The PII-free Myelin issue draft a [`FlowIncident`] files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowIncidentIssueDraft {
    /// the FLOW gate the issue is about.
    pub gate_id: String,
    /// the issue title (PII-free).
    pub title: String,
    /// the issue body (PII-free; names the repro drill).
    pub body: String,
}

/// The reproducing-drill ticket a [`FlowIncident`] files (the drill that joins the permanent suite).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowIncidentDrillTicket {
    /// the drill name (the test that re-fires the failure).
    pub drill_name: String,
    /// the gate the drill guards.
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    /// **THE HEADLINE: Myelin's own pipelines / merge queue / SLA timers run GREEN as myelin-flow
    /// workflows on the self-hosting platform.** All three faces green over the Myelin self-tenant.
    #[test]
    fn myelins_own_workflows_green_on_the_self_hosting_platform() {
        let artifact = run_flow_over_myelins_own_work(RUN_DATE);
        assert!(
            artifact.is_green(),
            "Myelin's pipelines/merge-queue/SLA-timers must run green as myelin-flow workflows: {}",
            artifact.summary()
        );
        assert!(
            artifact.pipeline.is_green(),
            "Myelin's CI pipeline as a ci.pipeline workflow"
        );
        assert!(
            artifact.merge_queue.is_green(),
            "Myelin's merge queue merges a real Myelin PR"
        );
        assert!(
            artifact.sla_timer.is_green(),
            "a real Myelin SLA timer fires on a real Myelin issue"
        );

        // Face 1: the pipeline ran every stage to completion with 0 re-dispatch across the kill.
        assert!(artifact.pipeline.completed);
        assert_eq!(
            artifact.pipeline.dispatches, 3,
            "0 re-dispatch — one dispatch per stage"
        );

        // Face 2: exactly-once — 1 merge, 1 git.pr.merged, 0 re-dispatch.
        assert_eq!(
            artifact.merge_queue.merges, 1,
            "exactly one merge (0 double-merge)"
        );
        assert_eq!(
            artifact.merge_queue.git_pr_merged_emits, 1,
            "one git.pr.merged emit"
        );
        assert_eq!(artifact.merge_queue.ci_dispatches, 1, "0 re-dispatch");

        // Face 3: the SLA timer armed, re-armed cheaply, and FIRED.
        assert!(
            artifact.sla_timer.armed && artifact.sla_timer.re_armed && artifact.sla_timer.fired
        );

        let s = artifact.summary();
        assert!(s.contains("P-516 FLOW DOGFOOD 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    /// The truth-up pass is GREEN — every PROVEN FLOW row rests on a dated green artifact.
    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_flow_rows(RUN_DATE);
        assert!(
            rows.len() >= 11,
            "the PROVEN set covers FLOW-D1..FLOW-D10 + the E2E-2 spine"
        );
        let confirmed = FlowTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red earlier-band FLOW gates — every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

    /// A claimed-not-proven row reds the truth-up pass LOUDLY (surfaced, never swallowed).
    #[test]
    fn a_claimed_not_proven_row_reds_the_truth_up_pass() {
        let mut rows = proven_flow_rows(RUN_DATE);
        rows[0].artifact_date = None;
        let verdict = FlowTruthUpPass::new().run(&rows, RUN_DATE);
        assert!(!verdict.is_green(), "an undated row reds the pass");
        assert_eq!(verdict.undated_rows(), &[rows[0].id]);
        let err = FlowTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect_err("a claimed-not-proven row fails the CI entrypoint");
        assert!(err.to_string().contains("CLAIMED-NOT-PROVEN"));
    }

    /// The enumerated scorecard renders GREEN with every PROVEN row dated + its proof source on disk.
    #[test]
    fn the_scorecard_renders_green_with_proof_sources_on_disk() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let scorecard = run_flow_truth_up_scorecard(RUN_DATE, &repo_root);
        assert!(
            scorecard.is_green(),
            "the scorecard must be green — every PROVEN FLOW row dated + its proof source on disk; \
             claimed-not-proven: {:?}",
            scorecard.claimed_not_proven()
        );
        assert_eq!(scorecard.rows_dated_green(), scorecard.rows_total());
        let md = scorecard.render();
        assert!(md.contains("verdict=GREEN"), "rendered: {md}");
        assert!(
            md.contains("FLOW-D1") && md.contains("E2E-2"),
            "enumerated: {md}"
        );
    }

    /// A row whose proof source is missing on disk is surfaced CLAIMED-NOT-PROVEN (never trusted on faith).
    #[test]
    fn a_vanished_proof_source_is_surfaced_not_trusted() {
        let bogus_root = std::path::Path::new("/nonexistent-flow-truth-up-root");
        let scorecard = run_flow_truth_up_scorecard(RUN_DATE, bogus_root);
        assert!(
            !scorecard.is_green(),
            "a vanished proof source must red the scorecard"
        );
        assert!(
            scorecard.entries.iter().all(|e| matches!(
                &e.status,
                FlowRowStatus::ClaimedNotProven { reason, .. } if reason.contains("missing on disk")
            )),
            "every row is surfaced as proof-source-missing, never trusted on faith"
        );
    }

    /// The every-incident loop: an incident files a PII-free issue draft + a reproducing-drill ticket.
    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = FlowIncident::new(
            "INC-FLOW-DOGFOOD-1",
            "FLOW-D1",
            "a replay-recovery regression dropped a journaled effect on the Myelin self-tenant",
            "repro_flow_d1_dogfood_replay_recovery",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "FLOW-D1");
        assert!(draft.title.contains("INC-FLOW-DOGFOOD-1"));
        assert!(
            draft.body.contains("repro_flow_d1_dogfood_replay_recovery"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        assert!(!draft.body.to_lowercase().contains("email"));
        let ticket = incident.drill_ticket();
        assert_eq!(ticket.drill_name, "repro_flow_d1_dogfood_replay_recovery");
        assert_eq!(ticket.gate_id, "FLOW-D1");
    }
}
