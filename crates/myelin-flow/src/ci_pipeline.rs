use crate::job::{JobKind, JobOutcome, JobRunner, JobSpec};
use crate::wfctx::{WfCtx, WfError, WfResult};
use myelin_refs::ArtifactRef;
use myelin_storage::reserve_settle::{MeteredUnit, MicroUsd};

pub const CI_PIPELINE_WF_TYPE: &str = "ci.pipeline";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiStage {
    pub name: String,
    pub target: String,
    pub cost: MicroUsd,
    pub timeout_secs: Option<i64>,
}

impl CiStage {
    pub fn new(
        name: impl Into<String>,
        target: impl Into<String>,
        cost: MicroUsd,
        timeout_secs: Option<i64>,
    ) -> Self {
        Self {
            name: name.into(),
            target: target.into(),
            cost,
            timeout_secs,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiPipelineSpec {
    pub stages: Vec<CiStage>,
}

impl CiPipelineSpec {
    pub fn new(stages: Vec<CiStage>) -> Self {
        Self { stages }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineOutcome {
    Succeeded { stages_completed: usize },
    Failed { stage: String },
    TimedOut { stage: String },
    Parked,
}

pub fn stage_verdict_marker(stage: &str, pass: bool) -> ArtifactRef {
    let verdict = if pass { "pass" } else { "fail" };
    ArtifactRef(format!("ci.stage.verdict:{verdict}:{stage}"))
}

pub fn read_stage_verdict(result: &[ArtifactRef]) -> Option<(String, bool)> {
    for r in result {
        if let Some(rest) = r.0.strip_prefix("ci.stage.verdict:") {
            let (verdict, stage) = rest.split_once(':')?;
            return match verdict {
                "pass" => Some((stage.to_string(), true)),
                "fail" => Some((stage.to_string(), false)),
                _ => None,
            };
        }
    }
    None
}

impl WfCtx {
    pub fn run_ci_pipeline<R>(
        &mut self,
        spec: &CiPipelineSpec,
        runner: &R,
    ) -> WfResult<PipelineOutcome>
    where
        R: JobRunner,
    {
        let mut stages_completed = 0usize;
        for stage in &spec.stages {
            let units = vec![MeteredUnit {
                unit: "ci.stage",
                wholesale: stage.cost,
                markup: MicroUsd(0),
            }];
            let outcome = self.metered_schedule_and_run_job(
                JobSpec::new(JobKind::Ci, stage.target.clone()),
                runner,
                stage.timeout_secs,
                stage.cost,
                units,
            )?;

            match outcome {
                JobOutcome::Completed { result, .. } => {
                    let (verdict_stage, pass) = read_stage_verdict(&result).ok_or_else(|| {
                        WfError::CoCommit(format!(
                            "ci.pipeline stage `{}` job.done carried no verdict marker (the \
                                 runner did not report pass/fail, §4.9)",
                            stage.name
                        ))
                    })?;
                    if verdict_stage != stage.name {
                        return Err(WfError::CoCommit(format!(
                            "ci.pipeline stage `{}` job.done reported a verdict for stage \
                             `{verdict_stage}` (the runner mis-attributed the verdict, §4.9)",
                            stage.name
                        )));
                    }
                    if !pass {
                        return Ok(PipelineOutcome::Failed {
                            stage: stage.name.clone(),
                        });
                    }
                    stages_completed += 1;
                }
                JobOutcome::TimedOut => {
                    return Ok(PipelineOutcome::TimedOut {
                        stage: stage.name.clone(),
                    });
                }
                JobOutcome::Parked => {
                    return Ok(PipelineOutcome::Parked);
                }
            }
        }
        Ok(PipelineOutcome::Succeeded { stages_completed })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{SignalRow, SignalStore};
    use crate::{BudgetGate, Wallet, WfJournal};
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

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
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }
    fn minter() -> std::sync::Arc<dyn IdMinter> {
        std::sync::Arc::new(MonotonicMinter::new())
    }

    #[derive(Default)]
    struct RecordingCiRunner {
        dispatched: Mutex<Vec<JobSpec>>,
        calls: AtomicUsize,
    }
    impl JobRunner for RecordingCiRunner {
        fn dispatch(&self, spec: &JobSpec) -> Result<(), crate::ActivityError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                spec.kind,
                JobKind::Ci,
                "a CI pipeline dispatches kind=ci jobs"
            );
            self.dispatched.lock().unwrap().push(spec.clone());
            Ok(())
        }
    }

    fn pipeline() -> CiPipelineSpec {
        CiPipelineSpec::new(vec![
            CiStage::new(
                "build",
                "pipeline://acme/ci/pr-7#build",
                MicroUsd(10),
                Some(3600),
            ),
            CiStage::new(
                "test",
                "pipeline://acme/ci/pr-7#test",
                MicroUsd(20),
                Some(3600),
            ),
            CiStage::new(
                "lint",
                "pipeline://acme/ci/pr-7#lint",
                MicroUsd(5),
                Some(600),
            ),
        ])
    }

    fn begin_metered(
        outbox: &OutboxStore,
        journal: WfJournal,
        signals: SignalStore,
        timers: crate::TimerStore,
        balance: MicroUsd,
        now_secs: i64,
    ) -> WfCtx {
        WfCtx::begin(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            CI_PIPELINE_WF_TYPE,
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_signals(signals)
        .with_timers(timers, 0, now_secs)
        .with_budget(BudgetGate::new(Wallet::new(balance)))
    }

    fn deliver_stage_done(signals: &SignalStore, idem_token: &str, stage: &str, pass: bool) {
        signals.deliver(SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: crate::JOB_DONE_SIGNAL.into(),
            idem_key: idem_token.into(),
            payload: vec![stage_verdict_marker(stage, pass)],
            payload_key_ref: None,
            received_unix_ms: 0,
            consumed_seq: None,
        });
    }

    fn stage_token(stage_idx: usize) -> String {
        crate::job_idem_token("R1", &format!("{CI_PIPELINE_WF_TYPE}:{}", stage_idx * 2))
    }

    #[test]
    fn pipeline_parks_at_the_first_stage_holding_no_runtime() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        let mut ctx = begin_metered(&outbox, journal, signals, timers, MicroUsd(1000), 1000);
        let out = ctx
            .run_ci_pipeline(&pipeline(), &runner)
            .expect("dispatch the first stage + park");

        assert_eq!(
            out,
            PipelineOutcome::Parked,
            "parks on the build stage's job.done"
        );
        assert!(
            ctx.parked_on_signal(),
            "the run holds no runtime (state=waiting)"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            1,
            "ONE stage dispatched"
        );
        let dispatched = runner.dispatched.lock().unwrap();
        assert_eq!(dispatched[0].kind, JobKind::Ci, "kind=ci");
        assert_eq!(
            dispatched[0].idem_token,
            stage_token(0),
            "the deterministic dispatch idem_token (the build stage)"
        );
    }

    #[test]
    fn all_stages_pass_pipeline_succeeds_each_stage_metered() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        deliver_stage_done(&signals, &stage_token(0), "build", true);
        deliver_stage_done(&signals, &stage_token(1), "test", true);
        deliver_stage_done(&signals, &stage_token(2), "lint", true);

        let mut ctx = begin_metered(&outbox, journal, signals, timers, MicroUsd(1000), 1000);
        let out = ctx
            .run_ci_pipeline(&pipeline(), &runner)
            .expect("the whole pipeline runs green");

        assert_eq!(
            out,
            PipelineOutcome::Succeeded {
                stages_completed: 3
            },
            "all three stages passed"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            3,
            "THREE stages dispatched, one each"
        );
        assert_eq!(
            ctx.consumed_signals().len(),
            3,
            "THREE job.done consumed, one per stage"
        );
    }

    #[test]
    fn a_failing_stage_stops_the_pipeline_fast_later_stages_never_dispatched() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        deliver_stage_done(&signals, &stage_token(0), "build", true);
        deliver_stage_done(&signals, &stage_token(1), "test", false);

        let mut ctx = begin_metered(&outbox, journal, signals, timers, MicroUsd(1000), 1000);
        let out = ctx
            .run_ci_pipeline(&pipeline(), &runner)
            .expect("the pipeline fails fast at test");

        assert_eq!(
            out,
            PipelineOutcome::Failed {
                stage: "test".into()
            },
            "the pipeline failed at the test stage"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            2,
            "ONLY build + test dispatched - lint was NEVER dispatched (0 wasted spend)"
        );
    }

    #[test]
    fn a_vanished_runner_times_the_stage_out_and_fails_the_pipeline() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        let mut c1 = begin_metered(
            &outbox,
            journal.clone(),
            signals.clone(),
            timers.clone(),
            MicroUsd(1000),
            1000,
        );
        let out1 = c1
            .run_ci_pipeline(&pipeline(), &runner)
            .expect("dispatch + park");
        assert_eq!(out1, PipelineOutcome::Parked, "parked on build's job.done");
        c1.commit()
            .expect("co-commit the dispatch + the timeout-timer");
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            CI_PIPELINE_WF_TYPE,
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals)
        .with_timers(timers, 0, 10_000)
        .with_budget(BudgetGate::new(Wallet::new(MicroUsd(1000))));
        let out2 = c2
            .run_ci_pipeline(&pipeline(), &runner)
            .expect("the timeout drive");
        assert_eq!(
            out2,
            PipelineOutcome::TimedOut {
                stage: "build".into()
            },
            "the build runner vanished → the pipeline timed out"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            1,
            "the build stage dispatched ONCE - the replay did not re-dispatch it"
        );
    }

    #[test]
    fn a_double_delivered_stage_done_wakes_the_pipeline_once() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        let single = CiPipelineSpec::new(vec![CiStage::new(
            "build",
            "pipeline://acme/ci/pr-7#build",
            MicroUsd(10),
            Some(3600),
        )]);

        deliver_stage_done(&signals, &stage_token(0), "build", true);
        deliver_stage_done(&signals, &stage_token(0), "build", true);
        assert_eq!(
            signals.buffered_depth(),
            1,
            "the double delivery deduped to ONE row"
        );

        let mut ctx = begin_metered(
            &outbox,
            journal,
            signals.clone(),
            timers,
            MicroUsd(1000),
            1000,
        );
        let out = ctx
            .run_ci_pipeline(&single, &runner)
            .expect("the pipeline completes");
        assert_eq!(
            out,
            PipelineOutcome::Succeeded {
                stages_completed: 1
            }
        );
        assert_eq!(
            ctx.consumed_signals().len(),
            1,
            "ONE wake (the double-delivery deduped)"
        );
        assert_eq!(signals.buffered_depth(), 0, "the one row consumed once");
    }

    #[test]
    fn a_mis_attributed_stage_verdict_is_a_loud_error() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        deliver_stage_done(&signals, &stage_token(0), "the-wrong-stage", true);

        let mut ctx = begin_metered(&outbox, journal, signals, timers, MicroUsd(1000), 1000);
        let err = ctx
            .run_ci_pipeline(&pipeline(), &runner)
            .expect_err("a mis-attributed verdict is loud");
        assert!(
            matches!(err, WfError::CoCommit(ref m) if m.contains("mis-attributed the verdict")),
            "the mis-attribution is a loud CoCommit error, got {err:?}"
        );
    }

    #[test]
    fn no_balance_means_the_stage_is_never_dispatched() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::TimerStore::new();
        let runner = RecordingCiRunner::default();

        let mut ctx = begin_metered(&outbox, journal, signals, timers, MicroUsd(5), 1000);
        let err = ctx
            .run_ci_pipeline(&pipeline(), &runner)
            .expect_err("an exhausted wallet refuses the dispatch");
        assert!(
            matches!(err, WfError::CoCommit(ref m) if m.contains("never dispatched")),
            "the refused reserve is loud, got {err:?}"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            0,
            "the runner was NEVER called (no balance → no dispatch)"
        );
    }

    #[test]
    fn stage_verdict_codec_round_trips() {
        assert_eq!(
            read_stage_verdict(&[stage_verdict_marker("build", true)]),
            Some(("build".to_string(), true))
        );
        assert_eq!(
            read_stage_verdict(&[stage_verdict_marker("test", false)]),
            Some(("test".to_string(), false))
        );
        assert_eq!(
            read_stage_verdict(&[ArtifactRef("not-a-verdict".into())]),
            None,
            "a non-verdict result decodes to None (the loud-error trigger)"
        );
    }
}
