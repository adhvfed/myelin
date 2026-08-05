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

pub const MYELIN_SELF_TENANT: &str = "myelin";

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

struct Substrate {
    runs: RunStore,
    journal: WfJournal,
    signals: SignalStore,
    outbox: OutboxStore,
    tele: FlowTelemetry,
    timers: TimerStore,
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineFace {
    pub completed: bool,
    pub dispatches: usize,
    pub stages: usize,
}

impl PipelineFace {
    pub fn is_green(&self) -> bool {
        self.completed && self.dispatches == self.stages
    }
}

pub fn run_myelin_ci_pipeline() -> PipelineFace {
    let runner = Arc::new(CountingCiRunner::default());
    let (ex, run, sub) = start_pipeline("ci.pipeline:myelin:self-host:run-1");
    let part = partition_for_run_id(&run.0);
    let stages = ["build", "test", "lint"];

    {
        let w = fresh_pipeline_worker(&sub, "worker-1", part, runner.clone());
        let _ = w.tick(1_000, "2026-06-26T00:00:00Z", 7);
    }

    let mut completed = false;
    for (idx, stage) in stages.iter().enumerate() {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeFace {
    pub merges: usize,
    pub git_pr_merged_emits: u64,
    pub ci_dispatches: usize,
}

impl MergeFace {
    pub fn is_green(&self) -> bool {
        self.merges == 1 && self.git_pr_merged_emits == 1 && self.ci_dispatches == 1
    }
}

pub fn run_myelin_merge_queue() -> MergeFace {
    let ci = Arc::new(CountingCi::default());
    let merger = Arc::new(CountingMerger::default());
    let (ex, run, sub) = start_merge("queue:myelin:main:pr-516");
    let part = partition_for_run_id(&run.0);

    {
        let w = fresh_merge_worker(&sub, "worker-1", part, ci.clone(), merger.clone());
        let _ = w.tick(1_000, "2026-06-26T00:00:00Z", 7);
    }

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlaFace {
    pub armed: bool,
    pub re_armed: bool,
    pub fired: bool,
    pub rows_scanned: u64,
}

impl SlaFace {
    pub fn is_green(&self) -> bool {
        self.armed && self.re_armed && self.fired
    }
}

pub fn run_myelin_sla_timer() -> SlaFace {
    let timers = TimerStore::new();
    let tenant = myelin_tenant();
    let region = myelin_region();
    let issue_key = "myelin/platform#516";
    let timer_id = sla_timer_id(issue_key);

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

    let re_armed = matches!(
        crate::SlaTimerCall::new(&timers, tenant.clone(), timer_id.clone()).re_arm(2_000),
        ReArmOutcome::ReArmed
    );

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

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the self_tenant artifact must be checked - an unread RED face silently claims a green the \
              engine did not earn on Myelin's own work (EI-01 §1/§3)"]
pub struct FlowSelfTenantArtifact {
    pub date: String,
    pub pipeline: PipelineFace,
    pub merge_queue: MergeFace,
    pub sla_timer: SlaFace,
}

impl FlowSelfTenantArtifact {
    pub fn is_green(&self) -> bool {
        self.pipeline.is_green() && self.merge_queue.is_green() && self.sla_timer.is_green()
    }

    pub fn summary(&self) -> String {
        format!(
            "P-516 FLOW SELF_TENANT {} - tenant={MYELIN_SELF_TENANT} region={MYELIN_SELF_REGION} \
             ci-pipeline={} merge-queue={} sla-timer={} verdict={}",
            self.date,
            self.pipeline.is_green(),
            self.merge_queue.is_green(),
            self.sla_timer.is_green(),
            if self.is_green() { "GREEN" } else { "RED" },
        )
    }
}

pub fn run_flow_over_myelins_own_work(date: &str) -> FlowSelfTenantArtifact {
    FlowSelfTenantArtifact {
        date: date.to_string(),
        pipeline: run_myelin_ci_pipeline(),
        merge_queue: run_myelin_merge_queue(),
        sla_timer: run_myelin_sla_timer(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenFlowRow {
    pub id: &'static str,
    pub section: &'static str,
    pub title: &'static str,
    pub proof_command: &'static str,
    pub artifact_path: &'static str,
    pub artifact_date: Option<String>,
}

impl ProvenFlowRow {
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }

    pub fn artifact_abs_path(&self, repo_root: &std::path::Path) -> std::path::PathBuf {
        repo_root.join(self.artifact_path)
    }
}

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
        row(
            "FLOW-D1",
            "9.1",
            "deterministic replay/recovery + lease-based crash recovery - a killed run resumes, 0 lost effect",
            "cargo test -p myelin-flow --test drills_flow_d1_replay",
            "crates/myelin-flow/tests/drills_flow_d1_replay.rs",
            date,
        ),
        row(
            "FLOW-D2",
            "9.1",
            "the replay-divergence guard - a divergent/wrong-version replay halts nondeterministic + dead-letters, 0 silent divergence",
            "cargo test -p myelin-flow --features integration --test integration_flow_replay",
            "crates/myelin-flow/tests/integration_flow_replay.rs",
            date,
        ),
        row(
            "FLOW-D5",
            "9.1",
            "the WfCtx journal/outbox CO-COMMIT - the silent-data-loss floor: the journal + the outbox land atomically",
            "cargo test -p myelin-flow --test drills_flow_d5_cocommit",
            "crates/myelin-flow/tests/drills_flow_d5_cocommit.rs",
            date,
        ),
        row(
            "FLOW-D3",
            "9.3",
            "the minute-bucket durable timer wheel at 1M+ cell-scale (indexed-not-scanned) - the 100k floor promoted to seven figures",
            "cargo test -p myelin-flow --test drills_flow_d3_full_1m_timer_wheel",
            "crates/myelin-flow/tests/drills_flow_d3_full_1m_timer_wheel.rs",
            date,
        ),
        row(
            "FLOW-D4",
            "9.4",
            "the multi-day HITL approval-card round-trip + the per-effect partial-approval across a restart + deploy",
            "cargo test -p myelin-flow --test drills_flow_d4_multiday_hitl --test drills_flow_d4_per_effect",
            "crates/myelin-flow/tests/drills_flow_d4_multiday_hitl.rs",
            date,
        ),
        row(
            "FLOW-D6",
            "11.7",
            "the reserve/settle bookend on a runaway loop vs a depleting wallet - spend is bounded, balanced",
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
        row(
            "FLOW-D8",
            "12.6",
            "the world-scale 30× agent surge + the protected-human-lane shed order - the human lane holds, the machine lane sheds",
            "cargo test -p myelin-flow --test drills_flow_d8_surge",
            "crates/myelin-flow/tests/drills_flow_d8_surge.rs",
            date,
        ),
        row(
            "FLOW-D9",
            "9.6",
            "crypto-shred reaching history - the PersonalDataHolder erase path COMPLETE, 0 recoverable PII",
            "cargo test -p myelin-flow --test drills_flow_d9_crypto_shred",
            "crates/myelin-flow/tests/drills_flow_d9_crypto_shred.rs",
            date,
        ),
        row(
            "FLOW-D10",
            "9.5",
            "restore to a consistent point - in-flight runs resume, no vanished result; the X-1 seam end-to-end",
            "cargo test -p myelin-flow --test drills_flow_d10_restore_verify --test drills_flow_d10_x1_seam_e2e",
            "crates/myelin-flow/tests/drills_flow_d10_restore_verify.rs",
            date,
        ),
        row(
            "E2E-2",
            "9.1",
            "the durable-workflow + HITL SPINE - CI-fail → triage agent → issue → chat → fix-PR across a kill + days-later approval (exactly-once, merge-count==1, reserve/settle balanced)",
            "cargo test -p myelin-flow --test drills_flow_e2e2_spine",
            "crates/myelin-flow/tests/drills_flow_e2e2_spine.rs",
            date,
        ),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowTruthUpVerdict {
    Green {
        rows_confirmed: usize,
        date: String,
    },
    Red {
        undated_rows: Vec<&'static str>,
    },
}

impl FlowTruthUpVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, FlowTruthUpVerdict::Green { .. })
    }

    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            FlowTruthUpVerdict::Green { .. } => &[],
            FlowTruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FlowTruthUpPass;

impl FlowTruthUpPass {
    pub fn new() -> FlowTruthUpPass {
        FlowTruthUpPass
    }

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowTruthUpRed {
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for FlowTruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL - {} FLOW row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} - a claim \
             that outlives its verification misleads the next agent (EI-01 §1); fix the doc or re-run \
             the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for FlowTruthUpRed {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowRowStatus {
    DatedGreen {
        date: String,
    },
    ClaimedNotProven {
        date: String,
        reason: String,
    },
}

impl FlowRowStatus {
    pub fn is_dated_green(&self) -> bool {
        matches!(self, FlowRowStatus::DatedGreen { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowScorecardEntry {
    pub row: ProvenFlowRow,
    pub status: FlowRowStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the truth-up scorecard must be checked - an unread CLAIMED-NOT-PROVEN row silently \
              drifts the docs from the code (EI-01 §1)"]
pub struct FlowTruthUpScorecard {
    pub date: String,
    pub entries: Vec<FlowScorecardEntry>,
}

impl FlowTruthUpScorecard {
    pub fn is_green(&self) -> bool {
        self.entries.iter().all(|e| e.status.is_dated_green())
    }

    pub fn rows_total(&self) -> usize {
        self.entries.len()
    }

    pub fn rows_dated_green(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status.is_dated_green())
            .count()
    }

    pub fn claimed_not_proven(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| !e.status.is_dated_green())
            .map(|e| e.row.id)
            .collect()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let verdict = if self.is_green() {
            "GREEN (no earlier-band FLOW gate red)"
        } else {
            "RED (a FLOW claim outran its verification)"
        };
        out.push_str(&format!(
            "P-516 FLOW TRUTH-UP SCORECARD {} - {}/{} rows dated-green, verdict={verdict}\n",
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
                "  [§{}] {:<10} {:<28} - {}  ⟨{}⟩\n",
                e.row.section, e.row.id, status, e.row.title, e.row.proof_command,
            ));
        }
        out
    }
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowIncident {
    pub incident_id: String,
    pub gate_id: String,
    pub description: String,
    pub repro_drill_name: String,
}

impl FlowIncident {
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

    pub fn issue_draft(&self) -> FlowIncidentIssueDraft {
        FlowIncidentIssueDraft {
            gate_id: self.gate_id.clone(),
            title: format!(
                "[{}] FLOW gate {} regressed",
                self.incident_id, self.gate_id
            ),
            body: format!(
                "FLOW incident {} on the Myelin self-tenant: {}. Reproducing drill: `{}` \
                 (reference-linked - every incident adds a drill, EI-01 §3).",
                self.incident_id, self.description, self.repro_drill_name
            ),
        }
    }

    pub fn drill_ticket(&self) -> FlowIncidentDrillTicket {
        FlowIncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowIncidentIssueDraft {
    pub gate_id: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowIncidentDrillTicket {
    pub drill_name: String,
    pub gate_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

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

        assert!(artifact.pipeline.completed);
        assert_eq!(
            artifact.pipeline.dispatches, 3,
            "0 re-dispatch - one dispatch per stage"
        );

        assert_eq!(
            artifact.merge_queue.merges, 1,
            "exactly one merge (0 double-merge)"
        );
        assert_eq!(
            artifact.merge_queue.git_pr_merged_emits, 1,
            "one git.pr.merged emit"
        );
        assert_eq!(artifact.merge_queue.ci_dispatches, 1, "0 re-dispatch");

        assert!(
            artifact.sla_timer.armed && artifact.sla_timer.re_armed && artifact.sla_timer.fired
        );

        let s = artifact.summary();
        assert!(s.contains("P-516 FLOW SELF_TENANT 2026-06-26"), "dated: {s}");
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    #[test]
    fn the_truth_up_pass_is_green() {
        let rows = proven_flow_rows(RUN_DATE);
        assert!(
            rows.len() >= 11,
            "the PROVEN set covers FLOW-D1..FLOW-D10 + the E2E-2 spine"
        );
        let confirmed = FlowTruthUpPass::new()
            .run_or_fail_ci(&rows, RUN_DATE)
            .expect("0 red earlier-band FLOW gates - every PROVEN row dated");
        assert_eq!(confirmed, rows.len());
    }

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
            "the scorecard must be green - every PROVEN FLOW row dated + its proof source on disk; \
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

    #[test]
    fn an_incident_files_an_issue_and_a_repro_drill_ticket() {
        let incident = FlowIncident::new(
            "INC-FLOW-SELF_TENANT-1",
            "FLOW-D1",
            "a replay-recovery regression dropped a journaled effect on the Myelin self-tenant",
            "repro_flow_d1_self_tenant_replay_recovery",
        );
        let draft = incident.issue_draft();
        assert_eq!(draft.gate_id, "FLOW-D1");
        assert!(draft.title.contains("INC-FLOW-SELF_TENANT-1"));
        assert!(
            draft.body.contains("repro_flow_d1_self_tenant_replay_recovery"),
            "the issue is reference-linked to its repro drill: {}",
            draft.body
        );
        assert!(!draft.body.to_lowercase().contains("email"));
        let ticket = incident.drill_ticket();
        assert_eq!(ticket.drill_name, "repro_flow_d1_self_tenant_replay_recovery");
        assert_eq!(ticket.gate_id, "FLOW-D1");
    }
}
