use crate::wfctx::{WaitOutcome, WfCtx, WfError, WfResult};

pub const JOB_DONE_SIGNAL: &str = "job.done";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Ci,
    Agent,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Ci => "ci",
            JobKind::Agent => "agent",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSpec {
    pub kind: JobKind,
    pub target: String,
    pub idem_token: String,
}

impl JobSpec {
    pub fn new(kind: JobKind, target: impl Into<String>) -> Self {
        Self {
            kind,
            target: target.into(),
            idem_token: String::new(),
        }
    }
}

pub trait JobRunner {
    fn dispatch(&self, spec: &JobSpec) -> Result<(), crate::ActivityError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobOutcome {
    Completed {
        idem_token: String,
        result: Vec<myelin_refs::ArtifactRef>,
    },
    Parked,
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchedJob {
    idem_token: String,
    deadline_unix_secs: Option<i64>,
    dispatch_command_id: String,
    spec_fingerprint: String,
}

const JOB_DISPATCH_DEADLINE_PREFIX: &str = "job:dispatch-deadline:";
const JOB_DISPATCH_SPEC_PREFIX: &str = "job:dispatch-spec:v2:";
const JOB_DISPATCH_TIMEOUT_NONE: &str = "job:dispatch-timeout:none";
const JOB_DISPATCH_TIMEOUT_SECS_PREFIX: &str = "job:dispatch-timeout-secs:";

impl DispatchedJob {
    pub fn idem_token(&self) -> &str {
        &self.idem_token
    }

    pub fn deadline_unix_secs(&self) -> Option<i64> {
        self.deadline_unix_secs
    }
}

fn dispatch_spec_fingerprint(spec: &JobSpec, timeout_secs: Option<i64>) -> String {
    let mut hasher = blake3::Hasher::new();
    let timeout = timeout_secs
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".into());
    for field in [
        spec.kind.as_str().as_bytes(),
        spec.target.as_bytes(),
        spec.idem_token.as_bytes(),
        timeout.as_bytes(),
    ] {
        hasher.update(&(field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    hasher.finalize().to_hex().to_string()
}

impl WfCtx {
    pub fn dispatch_job<R>(
        &mut self,
        spec: JobSpec,
        runner: &R,
        timeout_secs: Option<i64>,
    ) -> WfResult<DispatchedJob>
    where
        R: JobRunner,
    {
        let dispatch_command_id = self.peek_next_command_id();
        let replaying_dispatch = self.is_replaying_command(&dispatch_command_id);
        let idem_token = job_idem_token(self.run_id(), &dispatch_command_id);
        let deadline_unix_secs =
            timeout_secs.map(|timeout| self.drive_now_unix_secs().saturating_add(timeout));
        let dispatched = JobSpec {
            idem_token: idem_token.clone(),
            ..spec
        };
        let dispatch_marker = job_dispatch_marker(&idem_token, dispatched.kind);
        let spec_fingerprint = dispatch_spec_fingerprint(&dispatched, timeout_secs);
        let spec_marker =
            myelin_refs::ArtifactRef(format!("{JOB_DISPATCH_SPEC_PREFIX}{spec_fingerprint}"));
        let timeout_marker = myelin_refs::ArtifactRef(match timeout_secs {
            Some(timeout) => format!("{JOB_DISPATCH_TIMEOUT_SECS_PREFIX}{timeout}"),
            None => JOB_DISPATCH_TIMEOUT_NONE.to_string(),
        });
        let deadline_marker = deadline_unix_secs.map(|deadline| {
            myelin_refs::ArtifactRef(format!("{JOB_DISPATCH_DEADLINE_PREFIX}{deadline}"))
        });
        let spec_for_closure = dispatched.clone();
        let marker_for_closure = dispatch_marker.clone();
        let spec_marker_for_closure = spec_marker.clone();
        let timeout_marker_for_closure = timeout_marker.clone();
        let result = self.activity(
            crate::RetryPolicy::default_policy(),
            move |act_idem, _attempt| {
                debug_assert!(
                    act_idem.ends_with("/act"),
                    "the activity's own BUS-2 token is the /act token; the JOB token is /job"
                );
                runner.dispatch(&spec_for_closure)?;
                let mut result = vec![
                    marker_for_closure.clone(),
                    spec_marker_for_closure.clone(),
                    timeout_marker_for_closure.clone(),
                ];
                if let Some(deadline) = &deadline_marker {
                    result.push(deadline.clone());
                }
                Ok(result)
            },
        )?;

        if !result.iter().any(|artifact| artifact == &dispatch_marker) {
            return Err(self.diverge(format!(
                "job dispatch journal at {dispatch_command_id} does not describe `{idem_token}`"
            )));
        }
        let recorded_spec = result
            .iter()
            .find(|artifact| artifact.0.starts_with(JOB_DISPATCH_SPEC_PREFIX));
        if let Some(recorded) = recorded_spec {
            if recorded != &spec_marker {
                return Err(self.diverge(format!(
                    "job dispatch journal at {dispatch_command_id} changed kind, target, token, or timeout"
                )));
            }
        }
        let recorded_timeout = result.iter().find(|artifact| {
            artifact.0 == JOB_DISPATCH_TIMEOUT_NONE
                || artifact.0.starts_with(JOB_DISPATCH_TIMEOUT_SECS_PREFIX)
        });
        if recorded_spec.is_some() != recorded_timeout.is_some() {
            return Err(self.diverge(format!(
                "job dispatch journal at {dispatch_command_id} has a partial v2 spec/timeout binding"
            )));
        }
        match (timeout_secs, recorded_timeout) {
            (None, Some(marker)) if marker.0 == JOB_DISPATCH_TIMEOUT_NONE => {}
            (Some(timeout), Some(marker))
                if marker.0 == format!("{JOB_DISPATCH_TIMEOUT_SECS_PREFIX}{timeout}") => {}
            (None, None) if recorded_spec.is_none() => {}
            _ => {
                return Err(self.diverge(format!(
                    "job dispatch journal at {dispatch_command_id} changed timeout mode or duration"
                )))
            }
        }
        let recorded_deadline_text = result
            .iter()
            .find_map(|artifact| artifact.0.strip_prefix(JOB_DISPATCH_DEADLINE_PREFIX))
            .map(ToOwned::to_owned);
        let recorded_deadline = match recorded_deadline_text {
            Some(deadline) => Some(deadline.parse::<i64>().map_err(|_| {
                self.diverge(format!(
                    "job dispatch journal at {dispatch_command_id} has a malformed deadline"
                ))
            })?),
            None => None,
        };

        if timeout_secs.is_some() && recorded_deadline.is_none() {
            return Err(self.diverge(format!(
                "timed job dispatch journal at {dispatch_command_id} is missing its absolute deadline"
            )));
        }
        if timeout_secs.is_none() && recorded_deadline.is_some() {
            return Err(self.diverge(format!(
                "untimed job dispatch journal at {dispatch_command_id} unexpectedly has a deadline"
            )));
        }
        if let Some(deadline) = recorded_deadline.filter(|_| !replaying_dispatch) {
            self.arm_job_deadline(&dispatch_command_id, deadline)?;
        }

        let identity = (
            dispatch_command_id.clone(),
            recorded_deadline,
            spec_fingerprint.clone(),
        );
        if self
            .job_dispatches
            .insert(idem_token.clone(), identity.clone())
            .is_some_and(|existing| existing != identity)
        {
            return Err(self.diverge(format!(
                "job dispatch token `{idem_token}` was reused for a different journaled dispatch"
            )));
        }

        Ok(DispatchedJob {
            idem_token,
            deadline_unix_secs: recorded_deadline,
            dispatch_command_id,
            spec_fingerprint,
        })
    }

    pub fn join_dispatched_job(&mut self, job: &DispatchedJob) -> WfResult<JobOutcome> {
        let Some(expected) = self.job_dispatches.get(&job.idem_token) else {
            return Err(self.diverge(
                "job join refused an unregistered/foreign dispatch handle".into(),
            ));
        };
        let identity_matches = expected
            == &(
                job.dispatch_command_id.clone(),
                job.deadline_unix_secs,
                job.spec_fingerprint.clone(),
            );
        if !identity_matches {
            return Err(self.diverge(
                "job join refused a dispatch handle that differs from journaled identity".into(),
            ));
        }
        let earliest = self
            .job_dispatches
            .iter()
            .filter(|(token, _)| !self.joined_job_dispatches.contains(*token))
            .min_by(|(left_token, left), (right_token, right)| {
                left.1
                    .unwrap_or(i64::MAX)
                    .cmp(&right.1.unwrap_or(i64::MAX))
                    .then(left_token.cmp(right_token))
            })
            .map(|(token, _)| token.as_str());
        if earliest != Some(job.idem_token.as_str()) {
            return Err(self.diverge(format!(
                "unsafe job join order: `{}` is not the earliest outstanding dispatch",
                job.idem_token
            )));
        }

        let outcome = match self.wait_for_signal_exact_until_prearmed(
            JOB_DONE_SIGNAL,
            &job.idem_token,
            job.deadline_unix_secs,
        )? {
            WaitOutcome::Signalled {
                idem_key,
                payload,
                payload_key_ref: _,
            } => JobOutcome::Completed {
                idem_token: idem_key,
                result: payload,
            },
            WaitOutcome::Parked => JobOutcome::Parked,
            WaitOutcome::TimedOut => JobOutcome::TimedOut,
        };
        if !matches!(outcome, JobOutcome::Parked) {
            if job.deadline_unix_secs.is_some() {
                self.disarm_job_deadline(&job.dispatch_command_id)?;
            }
            self.joined_job_dispatches.insert(job.idem_token.clone());
        }
        Ok(outcome)
    }

    pub fn schedule_and_run_job<R>(
        &mut self,
        spec: JobSpec,
        runner: &R,
        timeout_secs: Option<i64>,
    ) -> WfResult<JobOutcome>
    where
        R: JobRunner,
    {
        let dispatched = self.dispatch_job(spec, runner, timeout_secs)?;

        self.join_dispatched_job(&dispatched)
    }

    pub fn metered_schedule_and_run_job<R>(
        &mut self,
        spec: JobSpec,
        runner: &R,
        timeout_secs: Option<i64>,
        cost: myelin_storage::reserve_settle::MicroUsd,
        units: Vec<myelin_storage::reserve_settle::MeteredUnit>,
    ) -> WfResult<JobOutcome>
    where
        R: JobRunner,
    {
        let Some(gate) = self.budget().cloned() else {
            return self.schedule_and_run_job(spec, runner, timeout_secs);
        };

        let admit =
            self.reserve_and_begin(&gate, cost, crate::budget::DispatchNoun::LONG_PARK)?;

        let outcome = self.schedule_and_run_job(spec, runner, timeout_secs)?;

        if let JobOutcome::Completed { .. } = &outcome {
            gate.settle(self.tenant_id(), &admit.ledger_run, &units)
                .map_err(|e| {
                    WfError::CoCommit(format!("schedule_and_run_job settle failed: {e}"))
                })?;
        }
        Ok(outcome)
    }
}

pub fn job_idem_token(run_id: &str, dispatch_command_id: &str) -> String {
    format!("{run_id}/{dispatch_command_id}/job")
}

pub fn job_dispatch_marker(idem_token: &str, kind: JobKind) -> myelin_refs::ArtifactRef {
    myelin_refs::ArtifactRef(format!("job:dispatched:{}:{idem_token}", kind.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{SignalRow, SignalStore};
    use crate::schema::WfHistoryRow;
    use crate::{RetryPolicy, WfJournal};
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_refs::ArtifactRef;
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

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
    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }
    fn begin(outbox: &OutboxStore, journal: WfJournal, signals: SignalStore) -> WfCtx {
        WfCtx::begin(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_signals(signals)
    }

    #[derive(Default)]
    struct RecordingRunner {
        dispatched: Mutex<Vec<JobSpec>>,
        calls: AtomicUsize,
        fail_first: bool,
    }
    impl JobRunner for RecordingRunner {
        fn dispatch(&self, spec: &JobSpec) -> Result<(), crate::ActivityError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_first && n == 0 {
                return Err(crate::ActivityError(
                    "runner transiently unreachable".into(),
                ));
            }
            self.dispatched.lock().unwrap().push(spec.clone());
            Ok(())
        }
    }

    fn deliver_job_done(signals: &SignalStore, idem_token: &str, result: Vec<ArtifactRef>) {
        signals.deliver(SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: JOB_DONE_SIGNAL.into(),
            idem_key: idem_token.into(),
            payload: result,
            payload_key_ref: None,
            received_unix_ms: 0,
            consumed_seq: None,
        });
    }

    #[test]
    fn idem_token_is_deterministic_from_command_id_producer_and_consumer_agree() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();

        let consumer_token = job_idem_token("R1", "merge.queue:0");

        let mut ctx = begin(&outbox, journal.clone(), signals.clone());
        let out = ctx
            .schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                None,
            )
            .expect("dispatch + park");
        assert_eq!(out, JobOutcome::Parked, "no job.done yet → the run parks");

        let dispatched = runner.dispatched.lock().unwrap();
        assert_eq!(dispatched.len(), 1, "one dispatch");
        let producer_token = dispatched[0].idem_token.clone();

        assert_eq!(
            producer_token, consumer_token,
            "producer + consumer derive the SAME idem_token without coordination"
        );
        assert_eq!(
            producer_token, "R1/merge.queue:0/job",
            "deterministic on position"
        );
        ctx.commit()
            .expect("co-commit the dispatch + the park marker");
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(
            hist[0].kind,
            crate::history_kind::ACTIVITY_COMPLETED,
            "the dispatch is journaled"
        );
        assert_eq!(
            hist[0].result.as_ref().unwrap()[0],
            job_dispatch_marker("R1/merge.queue:0/job", JobKind::Ci),
            "the journaled dispatch carries job_dispatched: true + the idem_token"
        );
    }

    #[test]
    fn dispatch_returns_immediately_and_the_workflow_parks() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .schedule_and_run_job(
                JobSpec::new(JobKind::Agent, "agent://acme/job/x"),
                &runner,
                None,
            )
            .expect("dispatch + park");

        assert_eq!(
            out,
            JobOutcome::Parked,
            "the long-park returns Parked (the worker is freed)"
        );
        assert!(
            ctx.parked_on_signal(),
            "the run is waiting on job.done (holds no runtime)"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            1,
            "the job was dispatched exactly once"
        );
        assert_eq!(
            ctx.consumed_signals().len(),
            0,
            "nothing consumed - the job is still running"
        );
    }

    #[test]
    fn buffered_job_done_completes_with_the_result() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();

        let token = job_idem_token("R1", "merge.queue:0");
        deliver_job_done(
            &signals,
            &token,
            vec![ArtifactRef("myelin://acme/ci/result/green".into())],
        );

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                None,
            )
            .expect("dispatch + complete");

        match out {
            JobOutcome::Completed { idem_token, result } => {
                assert_eq!(idem_token, token, "the runner echoed the dispatch token");
                assert_eq!(
                    result,
                    vec![ArtifactRef("myelin://acme/ci/result/green".into())]
                );
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(
            ctx.consumed_signals().len(),
            1,
            "exactly ONE job.done consumed"
        );
    }

    #[test]
    fn double_delivered_job_done_wakes_the_workflow_once() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();

        let token = job_idem_token("R1", "merge.queue:0");
        deliver_job_done(
            &signals,
            &token,
            vec![ArtifactRef("myelin://acme/ci/result/green".into())],
        );
        deliver_job_done(
            &signals,
            &token,
            vec![ArtifactRef("myelin://acme/ci/result/green".into())],
        );
        assert_eq!(
            signals.buffered_depth(),
            1,
            "the double delivery deduped to ONE buffered row"
        );

        let mut ctx = begin(&outbox, journal, signals.clone());
        let out = ctx
            .schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                None,
            )
            .expect("dispatch + complete");
        assert!(
            matches!(out, JobOutcome::Completed { .. }),
            "the run completes, got {out:?}"
        );
        assert_eq!(
            ctx.consumed_signals().len(),
            1,
            "ONE wake per job (the double-delivery deduped)"
        );
        assert_eq!(
            signals.buffered_depth(),
            0,
            "the one buffered row is consumed once"
        );
    }

    #[test]
    fn vanished_runner_timeout_branch_fires_and_bounds_the_wait() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::timer::TimerStore::new();
        let runner = RecordingRunner::default();

        let mut c1 =
            begin(&outbox, journal.clone(), signals.clone()).with_timers(timers.clone(), 0, 1000);
        let out1 = c1
            .schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                Some(100),
            )
            .expect("dispatch + park");
        assert_eq!(
            out1,
            JobOutcome::Parked,
            "dispatched, parked on job.done with an SLA timer"
        );
        c1.commit()
            .expect("co-commit the dispatch + the timeout-timer");
        assert_eq!(
            timers.armed_count(),
            1,
            "the vanished-runner SLA timeout-timer is armed"
        );
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals.clone())
        .with_timers(timers.clone(), 0, 2000);
        let out2 = c2
            .schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                Some(100),
            )
            .expect("the timeout drive");
        assert_eq!(
            out2,
            JobOutcome::TimedOut,
            "the SLA fired before the runner reported → TimedOut"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            1,
            "the job was dispatched ONCE - the replay short-circuit did not re-dispatch it"
        );
    }

    #[test]
    fn replay_short_circuits_dispatch_and_completion_with_zero_re_dispatch() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();

        let token = job_idem_token("R1", "merge.queue:0");
        deliver_job_done(
            &signals,
            &token,
            vec![ArtifactRef("myelin://acme/ci/result/green".into())],
        );

        let mut c1 = begin(&outbox, journal.clone(), signals.clone());
        let out1 = c1
            .schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                None,
            )
            .expect("drive 1");
        assert!(matches!(out1, JobOutcome::Completed { .. }));
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        deliver_job_done(
            &signals,
            "R1/other/job",
            vec![ArtifactRef("myelin://acme/ci/result/other".into())],
        );
        let depth_before = signals.buffered_depth();

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals.clone());
        let out2 = c2
            .schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                None,
            )
            .expect("the replay drive");
        match out2 {
            JobOutcome::Completed { idem_token, .. } => assert_eq!(
                idem_token, token,
                "replay returns the SAME journaled completion (the original token)"
            ),
            other => panic!("expected the journaled Completed, got {other:?}"),
        }
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            1,
            "0 RE-DISPATCH on replay (the dispatch short-circuited)"
        );
        assert_eq!(
            c2.consumed_signals().len(),
            0,
            "replay consumed NOTHING new"
        );
        assert_eq!(
            signals.buffered_depth(),
            depth_before,
            "the second job.done was NOT consumed"
        );
    }

    #[test]
    fn job_done_with_a_mismatched_idem_key_remains_buffered() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();

        deliver_job_done(
            &signals,
            "the-wrong-token",
            vec![ArtifactRef("x://y".into())],
        );

        let mut ctx = begin(&outbox, journal, signals.clone());
        let outcome = ctx
            .schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                None,
            )
            .expect("an unrelated completion is not a protocol error");
        assert_eq!(outcome, JobOutcome::Parked);
        assert_eq!(signals.buffered_depth(), 1);
    }

    #[test]
    fn dag_joins_consume_exact_keys_when_completions_arrive_out_of_order() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();
        let token_a = job_idem_token("R1", "merge.queue:0");
        let token_b = job_idem_token("R1", "merge.queue:1");

        deliver_job_done(
            &signals,
            &token_b,
            vec![ArtifactRef("myelin://acme/ci/result/b".into())],
        );
        deliver_job_done(
            &signals,
            &token_a,
            vec![ArtifactRef("myelin://acme/ci/result/a".into())],
        );

        let mut ctx = begin(&outbox, journal, signals.clone());
        let a = ctx
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/a"),
                &runner,
                None,
            )
            .unwrap();
        let b = ctx
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/b"),
                &runner,
                None,
            )
            .unwrap();
        assert_eq!(a.idem_token, token_a);
        assert_eq!(b.idem_token, token_b);

        assert_eq!(
            ctx.join_dispatched_job(&a).unwrap(),
            JobOutcome::Completed {
                idem_token: token_a,
                result: vec![ArtifactRef("myelin://acme/ci/result/a".into())],
            }
        );
        assert_eq!(
            ctx.join_dispatched_job(&b).unwrap(),
            JobOutcome::Completed {
                idem_token: token_b,
                result: vec![ArtifactRef("myelin://acme/ci/result/b".into())],
            }
        );
        assert_eq!(signals.buffered_depth(), 0);
        assert_eq!(runner.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn split_join_keeps_the_deadline_fixed_at_dispatch_time() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::timer::TimerStore::new();
        let runner = RecordingRunner::default();

        let mut first =
            begin(&outbox, journal.clone(), signals.clone()).with_timers(timers.clone(), 0, 100);
        let dispatched = first
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/a"),
                &runner,
                Some(10),
            )
            .unwrap();
        assert_eq!(dispatched.deadline_unix_secs, Some(110));
        let armed = timers.rows_for_run(&tenant(), &region(), "R1");
        assert_eq!(armed.len(), 1, "the SLA timer is armed at dispatch");
        assert_eq!(armed[0].fire_at, 110);
        assert_eq!(
            armed[0].command_id, "merge.queue:0/job-timeout",
            "the dispatch position owns the pre-armed deadline"
        );
        first.commit().unwrap();

        let history = journal.history_for(&tenant(), "R1");
        let mut resumed = WfCtx::resume(
            &outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals)
        .with_timers(timers, 0, 200);
        let replayed = resumed
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/a"),
                &runner,
                Some(10),
            )
            .unwrap();
        assert_eq!(replayed.deadline_unix_secs, Some(110));
        assert_eq!(
            resumed.join_dispatched_job(&replayed).unwrap(),
            JobOutcome::TimedOut
        );
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn v2_dispatch_replay_rejects_target_drift() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();
        let mut first = begin(&outbox, journal.clone(), signals.clone());
        first
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/a"),
                &runner,
                None,
            )
            .unwrap();
        first.commit().unwrap();

        let mut replay = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
            journal.history_for(&tenant(), "R1"),
        )
        .with_signals(signals);
        let error = replay
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/b"),
                &runner,
                None,
            )
            .unwrap_err();
        assert!(error.is_nondeterministic());
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            1,
            "replay does not dispatch"
        );
    }

    #[test]
    fn v2_dispatch_replay_rejects_none_to_timed_drift() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();
        let mut first = begin(&outbox, journal.clone(), signals.clone());
        let own_handle = first
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/a"),
                &runner,
                None,
            )
            .unwrap();
        let mut forged = own_handle.clone();
        forged.spec_fingerprint = "forged".into();
        assert!(first
            .join_dispatched_job(&forged)
            .unwrap_err()
            .is_nondeterministic());
        first.commit().unwrap();

        let mut replay = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
            journal.history_for(&tenant(), "R1"),
        )
        .with_signals(signals)
        .with_timers(crate::timer::TimerStore::new(), 0, 100);
        let error = replay
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/a"),
                &runner,
                Some(10),
            )
            .unwrap_err();
        assert!(error.is_nondeterministic());
    }

    #[test]
    fn legacy_untimed_split_dispatch_replays_under_its_pinned_definition() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let token = job_idem_token("R1", "merge.queue:0");
        journal.append_history_for_test(WfHistoryRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            seq: 0,
            kind: crate::history_kind::ACTIVITY_COMPLETED.into(),
            command_id: "merge.queue:0".into(),
            result: Some(vec![job_dispatch_marker(&token, JobKind::Ci)]),
            result_key_ref: None,
        });
        let runner = RecordingRunner::default();
        let signals = SignalStore::new();
        let mut replay = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
            journal.history_for(&tenant(), "R1"),
        )
        .with_signals(signals);
        let handle = replay
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/a"),
                &runner,
                None,
            )
            .unwrap();
        assert_eq!(handle.idem_token(), token);
        assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            replay.join_dispatched_job(&handle).unwrap(),
            JobOutcome::Parked
        );
    }

    #[test]
    fn foreign_dispatch_handle_is_rejected_as_nondeterministic() {
        let outbox = OutboxStore::new();
        let runner = RecordingRunner::default();
        let mut first = begin(&outbox, WfJournal::new(), SignalStore::new());
        first
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/a"),
                &runner,
                None,
            )
            .unwrap();
        let mut foreign = WfCtx::begin(
            &outbox,
            minter(),
            WfJournal::new(),
            ctx_base(),
            "R2",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_signals(SignalStore::new());
        let foreign_handle = foreign
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/b"),
                &runner,
                None,
            )
            .unwrap();
        assert!(first
            .join_dispatched_job(&foreign_handle)
            .unwrap_err()
            .is_nondeterministic());
    }

    #[test]
    fn joins_must_follow_earliest_deadline_then_stable_token_order() {
        let outbox = OutboxStore::new();
        let runner = RecordingRunner::default();
        let timers = crate::timer::TimerStore::new();
        let mut by_deadline =
            begin(&outbox, WfJournal::new(), SignalStore::new()).with_timers(timers, 0, 100);
        let later = by_deadline
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/later"),
                &runner,
                Some(20),
            )
            .unwrap();
        let earlier = by_deadline
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/earlier"),
                &runner,
                Some(10),
            )
            .unwrap();
        assert!(by_deadline
            .join_dispatched_job(&later)
            .unwrap_err()
            .is_nondeterministic());
        assert!(earlier.deadline_unix_secs() < later.deadline_unix_secs());

        let mut tied = begin(&outbox, WfJournal::new(), SignalStore::new()).with_timers(
            crate::timer::TimerStore::new(),
            0,
            100,
        );
        let first_token = tied
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/first"),
                &runner,
                Some(10),
            )
            .unwrap();
        let second_token = tied
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/second"),
                &runner,
                Some(10),
            )
            .unwrap();
        assert!(first_token.idem_token() < second_token.idem_token());
        assert!(tied
            .join_dispatched_job(&second_token)
            .unwrap_err()
            .is_nondeterministic());
    }

    #[test]
    fn a_failed_dispatch_retries_reusing_the_same_idem_token() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner {
            fail_first: true,
            ..Default::default()
        };

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                None,
            )
            .expect("the retried dispatch succeeds");
        assert_eq!(
            out,
            JobOutcome::Parked,
            "the retried dispatch parks on job.done"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            2,
            "one failure + one retry"
        );
        let dispatched = runner.dispatched.lock().unwrap();
        assert_eq!(dispatched.len(), 1, "one accepted dispatch (the retry)");
        assert_eq!(
            dispatched[0].idem_token, "R1/merge.queue:0/job",
            "the retry reused the SAME idem_token (the runner dedups on it)"
        );
    }

    #[test]
    fn dispatch_uses_the_default_retry_policy() {
        assert_eq!(
            RetryPolicy::default_policy().max_attempts,
            3,
            "the §4.4 retry floor"
        );
    }
}
