use super::*;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore};
use myelin_flow::engine::{SignalRow, SignalStore};
use myelin_flow::{job_idem_token, stage_verdict_marker, TimerStore, WfJournal, JOB_DONE_SIGNAL};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

use crate::{
    CiManifestLaneV1, CiManifestLimitsV1, CiManifestSchedulingV1, CiManifestWorkspaceV1,
    CiMergeWaiterV1, CiRunFinalizationOutcome, CiRunFinalizationWrite, CiRunStoreError,
};

const RUN_ID: &str = "33333333-3333-8333-8333-333333333333";
const JOB_A: &str = "11111111-1111-8111-8111-111111111111";
const JOB_B: &str = "22222222-2222-8222-8222-222222222222";
const JOB_C: &str = "55555555-5555-8555-8555-555555555555";

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn region() -> Region {
    Region("fr-par".into())
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("ci-controlplane".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: myelin_events::Timestamp("2026-07-21T12:34:56Z".into()),
        recorded_at: myelin_events::Timestamp("2026-07-21T12:34:56Z".into()),
        caused_by: Some(CausedBy("event-1".into())),
    }
}

fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

fn job(id: &str, name: &str, needs: Vec<String>) -> GrantedCiJobV1 {
    GrantedCiJobV1 {
        job_id: id.into(),
        stage: name.into(),
        name: name.into(),
        check_context: name.into(),
        needs,
        matrix_key: BTreeMap::new(),
        image: format!("registry.example/{name}@sha256:{}", "a".repeat(64)),
        command: vec!["/bin/true".into()],
        env: BTreeMap::new(),
        secret_handles: BTreeMap::new(),
        egress_allow: Vec::new(),
        limits: CiManifestLimitsV1 {
            cpu_millis: 1_000,
            mem_bytes: 1_073_741_824,
            disk_bytes: 2_147_483_648,
            pids_max: 128,
            timeout_secs: 600,
        },
        workspace: CiManifestWorkspaceV1 {
            repo_ref: "myelin://acme/git/repo/core".into(),
            commit_oid: "deadbeef".into(),
            read_only_root: true,
            tmpfs_scratch: true,
        },
        scheduling: CiManifestSchedulingV1 {
            lane: CiManifestLaneV1::Batch,
            labels: vec!["linux".into()],
            concurrency_group: None,
            fair_key: "project:core".into(),
        },
        reserve_handle: format!("reserve:{id}"),
        token_authority_handle: format!("mint:{id}"),
        continue_on_error: false,
    }
}

fn manifest() -> CiDriveManifestV1 {
    CiDriveManifestV1 {
        schema_version: 1,
        tenant_id: tenant().0,
        region: region().0,
        project_id: "55555555-5555-4555-8555-555555555555".into(),
        wf_run_id: RUN_ID.into(),
        ci_run_id: "44444444-4444-8444-8444-444444444444".into(),
        source_snapshot_ref: format!(
            "myelin://acme/ci/artifact/snapshot-blake3:{}",
            "a".repeat(64)
        ),
        source_plan_schema_version: 2,
        launch_request_digest: format!("blake3:{}", "b".repeat(64)),
        workflow_type: CI_PIPELINE_WF_TYPE.into(),
        workflow_definition_version: 1,
        workflow_code_hash: format!("blake3:{}", "c".repeat(64)),
        authority_policy_revision: "ci-policy-2026-07-21".into(),
        repo_ref: "myelin://acme/git/repo/core".into(),
        source_ref: Some("refs/heads/main".into()),
        commit_oid: "deadbeef".into(),
        run_ref: "myelin://acme/ci/run/44444444-4444-8444-8444-444444444444".into(),
        started_at: "2026-07-21T12:34:56.000000Z".into(),
        trust_tier: CiManifestTrustTierV1::Trusted,
        check_attempts: BTreeMap::from([
            ("build-a".into(), 7),
            ("build-b".into(), 4),
            ("package".into(), 9),
        ]),
        merge_waiter: None,
        jobs: vec![
            job(JOB_A, "build-a", vec![]),
            job(JOB_B, "build-b", vec![]),
            job(JOB_C, "package", vec![JOB_A.into(), JOB_B.into()]),
        ],
    }
}

#[derive(Default)]
struct RecordingRunner {
    targets: Mutex<Vec<String>>,
}

impl JobRunner for RecordingRunner {
    fn dispatch(&self, spec: &JobSpec) -> Result<(), myelin_flow::ActivityError> {
        self.targets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(spec.target.clone());
        Ok(())
    }
}

impl RecordingRunner {
    fn targets(&self) -> Vec<String> {
        self.targets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

#[derive(Default)]
struct RecordingFinalizer {
    finalizations: Mutex<Vec<CiRunFinalization>>,
}

impl CiRunFinalizer for RecordingFinalizer {
    fn finalize(
        &self,
        finalization: &CiRunFinalization,
    ) -> Result<CiRunFinalizationOutcome, CiRunStoreError> {
        self.finalizations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(finalization.clone());
        Ok(CiRunFinalizationOutcome {
            write: CiRunFinalizationWrite::Finalized,
            completed_at: "2026-07-21T12:59:59.000000Z".into(),
        })
    }
}

impl RecordingFinalizer {
    fn finalizations(&self) -> Vec<CiRunFinalization> {
        self.finalizations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

struct RefusingFinalizer;

impl CiRunFinalizer for RefusingFinalizer {
    fn finalize(
        &self,
        _finalization: &CiRunFinalization,
    ) -> Result<CiRunFinalizationOutcome, CiRunStoreError> {
        Err(CiRunStoreError::IncompleteTerminalAccounting)
    }
}

fn begin(
    outbox: &OutboxStore,
    journal: WfJournal,
    signals: SignalStore,
    timers: TimerStore,
) -> WfCtx {
    WfCtx::begin(
        outbox,
        minter(),
        journal,
        ctx_base(),
        RUN_ID,
        CI_PIPELINE_WF_TYPE,
        "2026-07-21T13:00:00Z",
        7,
    )
    .with_signals(signals)
    .with_timers(timers, 0, 1_753_101_600)
}

fn resume(
    outbox: &OutboxStore,
    journal: WfJournal,
    signals: SignalStore,
    timers: TimerStore,
) -> WfCtx {
    let history = journal.history_for(&tenant(), RUN_ID);
    WfCtx::resume_versioned(
        outbox,
        minter(),
        journal,
        ctx_base(),
        RUN_ID,
        CI_PIPELINE_WF_TYPE,
        "2026-07-21T13:00:01Z",
        7,
        history,
        1,
        1,
    )
    .with_signals(signals)
    .with_timers(timers, 0, 1_753_101_601)
}

fn dispatch_token(command: usize) -> String {
    job_idem_token(RUN_ID, &format!("{CI_PIPELINE_WF_TYPE}:{command}"))
}

fn deliver(signals: &SignalStore, token: &str, concrete_name: &str, pass: bool, at: i64) {
    signals.deliver(SignalRow {
        tenant: tenant(),
        region: region(),
        run_id: RUN_ID.into(),
        signal_name: JOB_DONE_SIGNAL.into(),
        idem_key: token.into(),
        payload: vec![stage_verdict_marker(concrete_name, pass)],
        payload_key_ref: None,
        received_unix_ms: at,
        consumed_seq: None,
    });
}

#[test]
fn dag_dispatches_roots_together_replays_out_of_order_completions_then_launches_descendant() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();
    let finalizer = RecordingFinalizer::default();
    let manifest = manifest();

    let mut first = begin(&outbox, journal.clone(), signals.clone(), timers.clone());
    assert_eq!(
        run_ci_manifest_pipeline(&mut first, &manifest, &runner, &finalizer).unwrap(),
        CiManifestPipelineOutcome::Parked
    );
    first.commit().unwrap();
    assert_eq!(runner.targets(), vec![JOB_A.to_string(), JOB_B.to_string()]);

    deliver(&signals, &dispatch_token(1), "build-b", true, 2);
    deliver(&signals, &dispatch_token(0), "build-a", true, 3);
    let mut second = resume(&outbox, journal.clone(), signals.clone(), timers.clone());
    assert_eq!(
        run_ci_manifest_pipeline(&mut second, &manifest, &runner, &finalizer).unwrap(),
        CiManifestPipelineOutcome::Parked
    );
    second.commit().unwrap();
    assert_eq!(
        runner.targets(),
        vec![JOB_A.to_string(), JOB_B.to_string(), JOB_C.to_string()]
    );

    deliver(&signals, &dispatch_token(4), "package", true, 4);
    let mut third = resume(&outbox, journal.clone(), signals, timers);
    assert_eq!(
        run_ci_manifest_pipeline(&mut third, &manifest, &runner, &finalizer).unwrap(),
        CiManifestPipelineOutcome::Succeeded { jobs_completed: 3 }
    );
    third.commit().unwrap();
    assert_eq!(runner.targets().len(), 3, "replay never redispatched a job");
    assert_eq!(finalizer.finalizations().len(), 1);
    assert_eq!(
        finalizer.finalizations()[0].terminal_state,
        CiRunTerminalState::Succeeded
    );
    let terminal = outbox
        .committed_rows()
        .into_iter()
        .find(|row| row.envelope.type_.0 == myelin_ci_sandbox::events::CI_RUN_SUCCEEDED)
        .expect("the successful run publishes its terminal fact");
    assert_eq!(
        terminal.envelope.payload["source_ref"], "refs/heads/main",
        "an automation can match the branch that actually triggered this run"
    );
    assert_eq!(
        terminal.envelope.payload["repo_ref"], manifest.repo_ref,
        "an automation can stay inside the repository that owns the run"
    );

    let check_attempts: BTreeMap<String, u64> = outbox
        .committed_rows()
        .into_iter()
        .filter(|row| row.envelope.type_.0 == myelin_ci_sandbox::events::CI_CHECK_UPDATED)
        .map(|row| {
            (
                row.envelope.payload["context"]["name"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                row.envelope.payload["run_attempt"].as_u64().unwrap(),
            )
        })
        .collect();
    assert!(outbox
        .committed_rows()
        .iter()
        .filter(|row| row.envelope.type_.0 == myelin_ci_sandbox::events::CI_CHECK_UPDATED)
        .all(|row| {
            row.envelope.payload["cost_settled"] == true
                && row.envelope.payload["completed_at"] == "2026-07-21T12:59:59.000000Z"
        }));
    assert_eq!(
        check_attempts,
        BTreeMap::from([
            ("build-a".into(), 7),
            ("build-b".into(), 4),
            ("package".into(), 9)
        ])
    );
}

#[test]
fn failed_frontier_drains_dispatched_sibling_and_never_dispatches_descendant() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();
    let finalizer = RecordingFinalizer::default();
    let mut failed_manifest = manifest();
    failed_manifest.merge_waiter = Some(CiMergeWaiterV1 {
        workflow_run_id: RUN_ID.into(),
        idem_token: "merge-required-sibling".into(),
        required_contexts: vec!["build-b".into()],
    });
    deliver(&signals, &dispatch_token(1), "build-b", true, 2);
    deliver(&signals, &dispatch_token(0), "build-a", false, 3);

    let mut ctx = begin(&outbox, journal, signals, timers);
    assert_eq!(
        run_ci_manifest_pipeline(&mut ctx, &failed_manifest, &runner, &finalizer).unwrap(),
        CiManifestPipelineOutcome::Failed {
            job: "build-a".into(),
            timed_out: false,
        }
    );
    assert_eq!(runner.targets(), vec![JOB_A.to_string(), JOB_B.to_string()]);
    assert_eq!(
        ctx.staged_history_len(),
        6,
        "two dispatches, two joins, terminal clock, and finalization activity"
    );
    assert_eq!(
        finalizer.finalizations()[0].terminal_state,
        CiRunTerminalState::Failed
    );
    assert_eq!(
        finalizer.finalizations()[0]
            .jobs
            .iter()
            .map(|job| (job.job_id.as_str(), job.flow_dispatched))
            .collect::<BTreeMap<_, _>>(),
        BTreeMap::from([(JOB_A, true), (JOB_B, true), (JOB_C, false)])
    );
    ctx.commit().unwrap();
    let check_states: BTreeMap<String, (String, bool)> = outbox
        .committed_rows()
        .into_iter()
        .filter(|row| row.envelope.type_.0 == myelin_ci_sandbox::events::CI_CHECK_UPDATED)
        .map(|row| {
            (
                row.envelope.payload["context"]["name"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                (
                    row.envelope.payload["state"].as_str().unwrap().to_owned(),
                    row.envelope.payload["required"].as_bool().unwrap(),
                ),
            )
        })
        .collect();
    assert_eq!(
        check_states,
        BTreeMap::from([
            ("build-a".into(), ("failure".into(), false)),
            ("build-b".into(), ("success".into(), true)),
            ("package".into(), ("cancelled".into(), false)),
        ])
    );
    let rollup = outbox
        .committed_rows()
        .into_iter()
        .find(|row| row.envelope.type_.0 == myelin_events::taxonomy::new_tokens::CI_RESULT)
        .expect("merge attempt receives one rollup");
    assert_eq!(rollup.envelope.payload["overall"], "success");
    assert_eq!(
        rollup.envelope.payload["contexts"],
        serde_json::json!(["build-b"])
    );
}

#[test]
fn accounting_refusal_prevents_every_terminal_outward_fact() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();
    let manifest = manifest();
    deliver(&signals, &dispatch_token(1), "build-b", true, 2);
    deliver(&signals, &dispatch_token(0), "build-a", true, 3);
    deliver(&signals, &dispatch_token(4), "package", true, 4);

    let mut ctx = begin(&outbox, journal, signals, timers);
    assert!(matches!(
        run_ci_manifest_pipeline(&mut ctx, &manifest, &runner, &RefusingFinalizer),
        Err(WfError::ActivityExhausted(_))
    ));
    ctx.commit().unwrap();
    assert!(
        outbox.committed_rows().is_empty(),
        "checks, run terminal events, and merge results all follow durable finalization"
    );
}

#[test]
fn malformed_job_verdict_is_a_protocol_error_never_a_user_failure() {
    for payload in [
        vec![stage_verdict_marker("another-job", true)],
        vec![ArtifactRef("ci.artifact:without-verdict".into())],
    ] {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = TimerStore::new();
        let runner = RecordingRunner::default();
        let finalizer = RecordingFinalizer::default();
        signals.deliver(SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: RUN_ID.into(),
            signal_name: JOB_DONE_SIGNAL.into(),
            idem_key: dispatch_token(0),
            payload,
            payload_key_ref: None,
            received_unix_ms: 2,
            consumed_seq: None,
        });

        let mut ctx = begin(&outbox, journal, signals, timers);
        assert!(matches!(
            run_ci_manifest_pipeline(&mut ctx, &manifest(), &runner, &finalizer),
            Err(WfError::CoCommit(_))
        ));
        ctx.commit().unwrap();
        assert!(outbox.committed_rows().is_empty());
        assert!(finalizer.finalizations().is_empty());
    }
}

#[test]
fn flow_timeout_parks_until_late_accounting_then_emits_timed_out_once() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();
    let finalizer = RecordingFinalizer::default();
    let mut manifest = manifest();
    manifest.jobs.truncate(1);
    manifest.jobs[0].limits.timeout_secs = 1;
    manifest
        .check_attempts
        .retain(|context, _| context == "build-a");

    let mut first = begin(&outbox, journal.clone(), signals.clone(), timers.clone());
    assert_eq!(
        run_ci_manifest_pipeline(&mut first, &manifest, &runner, &finalizer).unwrap(),
        CiManifestPipelineOutcome::Parked
    );
    first.commit().unwrap();

    let mut deadline = resume(&outbox, journal.clone(), signals.clone(), timers.clone());
    assert_eq!(
        run_ci_manifest_pipeline(&mut deadline, &manifest, &runner, &finalizer).unwrap(),
        CiManifestPipelineOutcome::Parked,
        "the workflow verdict times out but money truth still waits for the runner receipt"
    );
    deadline.commit().unwrap();
    assert!(outbox.committed_rows().is_empty());
    assert!(finalizer.finalizations().is_empty());

    deliver(&signals, &dispatch_token(0), "build-a", false, 2);
    let mut accounted = resume(&outbox, journal, signals, timers);
    assert_eq!(
        run_ci_manifest_pipeline(&mut accounted, &manifest, &runner, &finalizer).unwrap(),
        CiManifestPipelineOutcome::Failed {
            job: "build-a".into(),
            timed_out: true,
        }
    );
    accounted.commit().unwrap();

    let finalizations = finalizer.finalizations();
    assert_eq!(finalizations.len(), 1);
    assert_eq!(
        finalizations[0].terminal_state,
        CiRunTerminalState::TimedOut
    );
    assert!(finalizations[0].jobs[0].flow_timed_out);
    let rows = outbox.committed_rows();
    assert_eq!(
        rows.iter()
            .filter(|row| row.envelope.type_.0 == myelin_ci_sandbox::events::CI_RUN_TIMED_OUT)
            .count(),
        1
    );
    assert!(!rows
        .iter()
        .any(|row| row.envelope.type_.0 == myelin_ci_sandbox::events::CI_RUN_FAILED));
}

#[test]
fn frontier_timeout_dominates_an_earlier_failure_and_matches_accounting() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();
    let finalizer = RecordingFinalizer::default();
    let mut mixed = manifest();
    mixed.jobs.truncate(2);
    for job in &mut mixed.jobs {
        job.limits.timeout_secs = 1;
    }
    mixed
        .check_attempts
        .retain(|context, _| context != "package");

    let mut first = begin(&outbox, journal.clone(), signals.clone(), timers.clone());
    assert_eq!(
        run_ci_manifest_pipeline(&mut first, &mixed, &runner, &finalizer).unwrap(),
        CiManifestPipelineOutcome::Parked
    );
    first.commit().unwrap();

    deliver(&signals, &dispatch_token(0), "build-a", false, 2);
    let mut deadline = resume(&outbox, journal.clone(), signals.clone(), timers.clone());
    assert_eq!(
        run_ci_manifest_pipeline(&mut deadline, &mixed, &runner, &finalizer).unwrap(),
        CiManifestPipelineOutcome::Parked,
        "the timed-out sibling still waits for its accounting receipt"
    );
    deadline.commit().unwrap();

    deliver(&signals, &dispatch_token(1), "build-b", false, 3);
    let mut accounted = resume(&outbox, journal, signals, timers);
    assert_eq!(
        run_ci_manifest_pipeline(&mut accounted, &mixed, &runner, &finalizer).unwrap(),
        CiManifestPipelineOutcome::Failed {
            job: "build-b".into(),
            timed_out: true,
        }
    );
    accounted.commit().unwrap();

    let finalizations = finalizer.finalizations();
    assert_eq!(finalizations.len(), 1);
    assert_eq!(
        finalizations[0].terminal_state,
        CiRunTerminalState::TimedOut
    );
    assert_eq!(
        finalizations[0]
            .jobs
            .iter()
            .filter(|job| job.flow_timed_out)
            .map(|job| job.job_id.as_str())
            .collect::<Vec<_>>(),
        vec![JOB_B]
    );
    assert_eq!(runner.targets(), vec![JOB_A.to_string(), JOB_B.to_string()]);
}
