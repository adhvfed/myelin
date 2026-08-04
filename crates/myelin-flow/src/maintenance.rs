use crate::job::{JobKind, JobOutcome, JobRunner, JobSpec};
use crate::wfctx::{ActivityError, RetryPolicy, WfCtx, WfResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaintenanceOp {
    Gc,
    Repack,
    BundleGen,
    HistoryRewrite,
}

impl MaintenanceOp {
    pub fn as_str(self) -> &'static str {
        match self {
            MaintenanceOp::Gc => "gc",
            MaintenanceOp::Repack => "repack",
            MaintenanceOp::BundleGen => "bundle-gen",
            MaintenanceOp::HistoryRewrite => "history-rewrite",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheNamespace {
    Fork,
    Mirror,
    CloneBundle,
}

impl CacheNamespace {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheNamespace::Fork => "fork",
            CacheNamespace::Mirror => "mirror",
            CacheNamespace::CloneBundle => "clone-bundle",
        }
    }

    pub const FANOUT_ORDER: [CacheNamespace; 3] = [
        CacheNamespace::Fork,
        CacheNamespace::Mirror,
        CacheNamespace::CloneBundle,
    ];
}

pub trait MaintenancePerformer {
    fn perform_step(
        &self,
        op: MaintenanceOp,
        step_index: usize,
        idem_token: &str,
    ) -> Result<(), ActivityError>;

    fn invalidate_namespace(
        &self,
        namespace: CacheNamespace,
        idem_token: &str,
    ) -> Result<(), ActivityError>;
}

impl WfCtx {
    pub fn run_maintenance<P>(
        &mut self,
        op: MaintenanceOp,
        step_count: usize,
        performer: &P,
    ) -> WfResult<usize>
    where
        P: MaintenancePerformer,
    {
        let ran_before = self.side_effects_executed();
        for step_index in 0..step_count {
            let marker = maintenance_step_marker(op, step_index);
            self.activity(RetryPolicy::default_policy(), move |idem, _attempt| {
                performer.perform_step(op, step_index, idem)?;
                Ok(vec![marker.clone()])
            })?;
        }
        Ok((self.side_effects_executed() - ran_before) as usize)
    }

    pub fn run_heavy_maintenance<R>(
        &mut self,
        op: MaintenanceOp,
        target: impl Into<String>,
        runner: &R,
        timeout_secs: Option<i64>,
    ) -> WfResult<JobOutcome>
    where
        R: JobRunner,
    {
        let spec = JobSpec::new(
            JobKind::Ci,
            format!("maintenance:{}:{}", op.as_str(), target.into()),
        );
        self.schedule_and_run_job(spec, runner, timeout_secs)
    }

    pub fn run_history_rewrite_invalidation<P>(
        &mut self,
        namespaces: &[CacheNamespace],
        performer: &P,
    ) -> WfResult<usize>
    where
        P: MaintenancePerformer,
    {
        let ran_before = self.side_effects_executed();
        for &namespace in namespaces {
            let marker = invalidation_marker(namespace);
            self.activity(RetryPolicy::default_policy(), move |idem, _attempt| {
                performer.invalidate_namespace(namespace, idem)?;
                Ok(vec![marker.clone()])
            })?;
        }
        Ok((self.side_effects_executed() - ran_before) as usize)
    }
}

pub fn maintenance_step_marker(op: MaintenanceOp, step_index: usize) -> myelin_refs::ArtifactRef {
    myelin_refs::ArtifactRef(format!("maintenance:{}:step:{step_index}", op.as_str()))
}

pub fn invalidation_marker(namespace: CacheNamespace) -> myelin_refs::ArtifactRef {
    myelin_refs::ArtifactRef(format!(
        "history-rewrite:invalidated:{}",
        namespace.as_str()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{history_kind, WfJournal};
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
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
    fn begin(outbox: &OutboxStore, journal: WfJournal) -> WfCtx {
        WfCtx::begin(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "git.maintenance",
            "2026-06-21T00:00:00Z",
            7,
        )
    }
    fn resume(
        outbox: &OutboxStore,
        journal: WfJournal,
        history: Vec<crate::schema::WfHistoryRow>,
    ) -> WfCtx {
        WfCtx::resume(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "git.maintenance",
            "2026-06-21T00:00:00Z",
            7,
            history,
        )
    }

    #[derive(Default)]
    struct RecordingPerformer {
        steps: Mutex<Vec<usize>>,
        namespaces: Mutex<Vec<CacheNamespace>>,
        step_calls: AtomicUsize,
        ns_calls: AtomicUsize,
        fail_step_once: Option<usize>,
        failed: Mutex<Vec<usize>>,
    }
    impl MaintenancePerformer for RecordingPerformer {
        fn perform_step(
            &self,
            _op: MaintenanceOp,
            step_index: usize,
            _idem: &str,
        ) -> Result<(), ActivityError> {
            self.step_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_step_once == Some(step_index) {
                let mut failed = self.failed.lock().unwrap();
                if !failed.contains(&step_index) {
                    failed.push(step_index);
                    return Err(ActivityError("transient maintenance failure".into()));
                }
            }
            self.steps.lock().unwrap().push(step_index);
            Ok(())
        }
        fn invalidate_namespace(
            &self,
            namespace: CacheNamespace,
            _idem: &str,
        ) -> Result<(), ActivityError> {
            self.ns_calls.fetch_add(1, Ordering::SeqCst);
            self.namespaces.lock().unwrap().push(namespace);
            Ok(())
        }
    }

    #[test]
    fn maintenance_runs_each_step_as_a_journaled_activity() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let performer = RecordingPerformer::default();

        let mut ctx = begin(&outbox, journal.clone());
        let ran = ctx
            .run_maintenance(MaintenanceOp::Repack, 8, &performer)
            .expect("the repack runs");
        assert_eq!(ran, 8, "all 8 steps ran live this drive");
        assert_eq!(
            *performer.steps.lock().unwrap(),
            vec![0, 1, 2, 3, 4, 5, 6, 7]
        );
        ctx.commit().expect("co-commit the journaled steps");

        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(hist.len(), 8, "8 journaled activity_completed rows");
        assert!(
            hist.iter()
                .all(|r| r.kind == history_kind::ACTIVITY_COMPLETED),
            "each step is a journaled activity"
        );
        assert_eq!(
            hist[0].result.as_ref().unwrap()[0],
            maintenance_step_marker(MaintenanceOp::Repack, 0),
            "the journaled step carries the references-not-payloads marker"
        );
    }

    #[test]
    fn crash_mid_repack_replays_to_the_un_journaled_step_zero_re_execution() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();

        let performer1 = RecordingPerformer::default();
        let mut c1 = begin(&outbox, journal.clone());
        let ran1 = c1
            .run_maintenance(MaintenanceOp::Repack, 3, &performer1)
            .expect("drive 1");
        assert_eq!(ran1, 3, "3 steps ran before the crash");
        c1.commit()
            .expect("the 3 steps co-commit (durable before the crash)");
        assert_eq!(*performer1.steps.lock().unwrap(), vec![0, 1, 2]);
        let history = journal.history_for(&tenant(), "R1");
        assert_eq!(history.len(), 3, "3 journaled at the crash point");

        let performer2 = RecordingPerformer::default();
        let mut c2 = resume(&outbox, journal.clone(), history);
        let ran2 = c2
            .run_maintenance(MaintenanceOp::Repack, 8, &performer2)
            .expect("the resume drive");

        assert_eq!(
            ran2, 5,
            "resumed at step 3 - only steps 3..=7 ran live (5 steps)"
        );
        assert_eq!(
            *performer2.steps.lock().unwrap(),
            vec![3, 4, 5, 6, 7],
            "0..=2 replayed (0 re-execution), 3..=7 ran - replay to the un-journaled step"
        );
        assert_eq!(
            performer2.step_calls.load(Ordering::SeqCst),
            5,
            "0 re-executed side effect - the journaled prefix's perform_step was NEVER called"
        );
        c2.commit().expect("co-commit the resumed tail");
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            8,
            "8 journaled, 0 lost, 0 duplicate"
        );
    }

    #[test]
    fn invalidation_fan_out_is_a_journaled_sequence() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let performer = RecordingPerformer::default();

        let mut ctx = begin(&outbox, journal.clone());
        let invalidated = ctx
            .run_history_rewrite_invalidation(&CacheNamespace::FANOUT_ORDER, &performer)
            .expect("the fan-out runs");
        assert_eq!(invalidated, 3, "all 3 namespaces invalidated live");
        assert_eq!(
            *performer.namespaces.lock().unwrap(),
            vec![
                CacheNamespace::Fork,
                CacheNamespace::Mirror,
                CacheNamespace::CloneBundle
            ],
            "the fan-out visits the trust-scoped namespaces in the FROZEN order"
        );
        ctx.commit().expect("co-commit the fan-out");
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(hist.len(), 3, "3 journaled invalidation activities");
        assert_eq!(
            hist[0].result.as_ref().unwrap()[0],
            invalidation_marker(CacheNamespace::Fork),
            "the journaled step carries the namespace marker"
        );
    }

    #[test]
    fn invalidation_fan_out_replays_from_the_last_journaled_step() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();

        let performer1 = RecordingPerformer::default();
        let mut c1 = begin(&outbox, journal.clone());
        let n1 = c1
            .run_history_rewrite_invalidation(&[CacheNamespace::Fork], &performer1)
            .expect("drive 1");
        assert_eq!(n1, 1, "Fork invalidated before the crash");
        c1.commit().expect("the Fork invalidation co-commits");
        let history = journal.history_for(&tenant(), "R1");
        assert_eq!(history.len(), 1, "1 journaled at the crash point");

        let performer2 = RecordingPerformer::default();
        let mut c2 = resume(&outbox, journal.clone(), history);
        let n2 = c2
            .run_history_rewrite_invalidation(&CacheNamespace::FANOUT_ORDER, &performer2)
            .expect("the resume drive");

        assert_eq!(
            n2, 2,
            "resumed from Mirror - only Mirror + CloneBundle ran live"
        );
        assert_eq!(
            *performer2.namespaces.lock().unwrap(),
            vec![CacheNamespace::Mirror, CacheNamespace::CloneBundle],
            "Fork replayed (0 re-invalidation), the fan-out resumed from the last journaled step"
        );
        assert_eq!(
            performer2.ns_calls.load(Ordering::SeqCst),
            2,
            "0 re-invalidation - the already-purged Fork scope's invalidate was NEVER re-called"
        );
        c2.commit().expect("co-commit the resumed fan-out tail");
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            3,
            "3 journaled, 0 duplicate"
        );
    }

    #[test]
    fn a_failed_maintenance_step_retries() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let performer = RecordingPerformer {
            fail_step_once: Some(2),
            ..Default::default()
        };

        let mut ctx = begin(&outbox, journal.clone());
        let ran = ctx
            .run_maintenance(MaintenanceOp::Gc, 4, &performer)
            .expect("the gc runs despite a transient failure");
        assert_eq!(ran, 4, "all 4 steps complete (step 2 retried)");
        assert_eq!(
            *performer.steps.lock().unwrap(),
            vec![0, 1, 2, 3],
            "step 2 succeeded on retry"
        );
        assert_eq!(
            performer.step_calls.load(Ordering::SeqCst),
            5,
            "one retry of step 2"
        );
        ctx.commit().expect("co-commit");
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            4,
            "step 2 journaled exactly once"
        );
    }

    #[test]
    fn heavy_maintenance_rides_the_long_park() {
        use crate::engine::SignalStore;

        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();

        struct AcceptingRunner {
            dispatched: Mutex<Vec<JobSpec>>,
        }
        impl JobRunner for AcceptingRunner {
            fn dispatch(&self, spec: &JobSpec) -> Result<(), ActivityError> {
                self.dispatched.lock().unwrap().push(spec.clone());
                Ok(())
            }
        }
        let runner = AcceptingRunner {
            dispatched: Mutex::new(Vec::new()),
        };

        let mut ctx = begin(&outbox, journal).with_signals(signals).with_timers(
            crate::TimerStore::new(),
            0,
            1_000,
        );
        let out = ctx
            .run_heavy_maintenance(
                MaintenanceOp::Repack,
                "repo://acme/giant",
                &runner,
                Some(7200),
            )
            .expect("dispatch + park");
        assert_eq!(
            out,
            JobOutcome::Parked,
            "the heavy repack parks on job.done (holds no runtime)"
        );
        assert!(ctx.parked_on_signal(), "the run is waiting on the runner");
        let dispatched = runner.dispatched.lock().unwrap();
        assert_eq!(dispatched.len(), 1, "one heavy job dispatched");
        assert_eq!(
            dispatched[0].target, "maintenance:repack:repo://acme/giant",
            "the job target carries the op + the opaque repo descriptor"
        );
        assert_eq!(
            dispatched[0].kind,
            JobKind::Ci,
            "a heavy maintenance job runs on the CI batch lane"
        );
    }

    #[test]
    fn op_and_namespace_tokens_are_stable() {
        assert_eq!(MaintenanceOp::Gc.as_str(), "gc");
        assert_eq!(MaintenanceOp::Repack.as_str(), "repack");
        assert_eq!(MaintenanceOp::BundleGen.as_str(), "bundle-gen");
        assert_eq!(MaintenanceOp::HistoryRewrite.as_str(), "history-rewrite");
        assert_eq!(CacheNamespace::Fork.as_str(), "fork");
        assert_eq!(CacheNamespace::Mirror.as_str(), "mirror");
        assert_eq!(CacheNamespace::CloneBundle.as_str(), "clone-bundle");
        assert_eq!(
            CacheNamespace::FANOUT_ORDER.len(),
            3,
            "the three trust-scoped namespaces"
        );
    }
}
