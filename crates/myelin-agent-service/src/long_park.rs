use crate::escape_gate::AgentExecGate;
use crate::exec::{RoutingError, SandboxJob};
use myelin_agent::{Command, ToolDef};
use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, MeterTarget, ResourceLimits, RunTokenCredential,
    SandboxBackend, SecretRef, TrustTier,
};
use myelin_flow::{JobKind, JobOutcome, JobRunner, JobSpec, WfCtx, WfResult};
use myelin_refs::ArtifactRef;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LongParkOutcome {
    Completed {
        idem_token: String,
        result: Vec<ArtifactRef>,
    },
    Parked,
    TimedOut,
}

impl LongParkOutcome {
    pub fn is_completed(&self) -> bool {
        matches!(self, LongParkOutcome::Completed { .. })
    }
    pub fn is_parked(&self) -> bool {
        matches!(self, LongParkOutcome::Parked)
    }
    pub fn is_timed_out(&self) -> bool {
        matches!(self, LongParkOutcome::TimedOut)
    }

    fn from_job_outcome(out: JobOutcome) -> LongParkOutcome {
        match out {
            JobOutcome::Completed { idem_token, result } => {
                LongParkOutcome::Completed { idem_token, result }
            }
            JobOutcome::Parked => LongParkOutcome::Parked,
            JobOutcome::TimedOut => LongParkOutcome::TimedOut,
        }
    }
}

pub struct AgentJobDispatcher<'a, B: SandboxBackend> {
    gate: AgentExecGate,
    backend: &'a B,
    job: SandboxJob,
}

impl<'a, B: SandboxBackend> AgentJobDispatcher<'a, B> {
    pub fn new(gate: AgentExecGate, backend: &'a B, job: SandboxJob) -> AgentJobDispatcher<'a, B> {
        AgentJobDispatcher { gate, backend, job }
    }

    pub fn job(&self) -> &SandboxJob {
        &self.job
    }

    pub fn gate(&self) -> &AgentExecGate {
        &self.gate
    }
}

impl<B: SandboxBackend> JobRunner for AgentJobDispatcher<'_, B> {
    fn dispatch(&self, spec: &JobSpec) -> Result<(), myelin_flow::ActivityError> {
        debug_assert_eq!(
            spec.kind,
            JobKind::Agent,
            "the Agent-Fabric dispatcher accepts only kind=agent jobs (a kind=ci job is CI's own \
             merge-queue runner - the SAME §5.6 idiom, a different dispatch target)"
        );
        let dispatched = self
            .job
            .clone()
            .with_dispatch_idem_token(IdemToken(spec.idem_token.clone()));
        self.backend
            .accept_async(dispatched.spec())
            .map_err(|e| myelin_flow::ActivityError(format!("async dispatch refused: {e}")))
    }
}

#[allow(clippy::too_many_arguments)]
pub fn dispatch_long_compute<B: SandboxBackend>(
    ctx: &mut WfCtx,
    gate: AgentExecGate,
    backend: &B,
    def: &ToolDef,
    cmd: &Command,
    profile: LongComputeProfile,
    timeout_secs: Option<i64>,
) -> Result<WfResult<LongParkOutcome>, RoutingError> {
    let job = build_long_job(def, cmd, &profile)?;
    let target = long_job_target(&job);
    let dispatcher = AgentJobDispatcher::new(gate, backend, job);

    let engine_spec = JobSpec::new(JobKind::Agent, target);
    Ok(ctx
        .schedule_and_run_job(engine_spec, &dispatcher, timeout_secs)
        .map(LongParkOutcome::from_job_outcome))
}

#[allow(clippy::too_many_arguments)]
pub fn dispatch_long_compute_metered<B: SandboxBackend>(
    ctx: &mut WfCtx,
    gate: AgentExecGate,
    backend: &B,
    def: &ToolDef,
    cmd: &Command,
    profile: LongComputeProfile,
    timeout_secs: Option<i64>,
    cost: myelin_storage::reserve_settle::MicroUsd,
    units: Vec<myelin_storage::reserve_settle::MeteredUnit>,
) -> Result<WfResult<LongParkOutcome>, RoutingError> {
    let job = build_long_job(def, cmd, &profile)?;
    let target = long_job_target(&job);
    let dispatcher = AgentJobDispatcher::new(gate, backend, job);
    let engine_spec = JobSpec::new(JobKind::Agent, target);
    Ok(ctx
        .metered_schedule_and_run_job(engine_spec, &dispatcher, timeout_secs, cost, units)
        .map(LongParkOutcome::from_job_outcome))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LongComputeProfile {
    pub image: ImageRef,
    pub command: Vec<String>,
    pub secret_refs: Vec<SecretRef>,
    pub egress: EgressPolicy,
    pub limits: ResourceLimits,
    pub trust_tier: TrustTier,
    pub run_token: RunTokenCredential,
    pub meter_to: MeterTarget,
    pub idem_token: IdemToken,
}

fn build_long_job(
    def: &ToolDef,
    cmd: &Command,
    profile: &LongComputeProfile,
) -> Result<SandboxJob, RoutingError> {
    let mut command = profile.command.clone();
    command.push(cmd.0.clone());
    SandboxJob::for_compute(
        def,
        profile.image.clone(),
        command,
        Vec::new(),
        profile.secret_refs.clone(),
        profile.egress.clone(),
        profile.limits,
        profile.trust_tier,
        profile.run_token.clone(),
        profile.meter_to.clone(),
        profile.idem_token.clone(),
    )
}

fn long_job_target(job: &SandboxJob) -> String {
    format!("agent-job:{}", job.spec().idem_token.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::{EffectKind, ToolName};
    use myelin_ci_sandbox::{
        JobSpec as SandboxJobSpec, ResourceUsage, SandboxHandle, SandboxLaunch, SandboxResult,
        SpecError,
    };
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_flow::engine::{SignalRow, SignalStore};
    use myelin_flow::{
        job_idem_token, DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenLease,
        RunTokenMinter, WfJournal, JOB_DONE_SIGNAL,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingAsyncBackend {
        accepted: Mutex<Vec<SandboxJobSpec>>,
        calls: AtomicUsize,
        fail_first: bool,
    }

    impl SandboxBackend for RecordingAsyncBackend {
        type Error = SpecError;
        fn launch(
            &self,
            _spec: &SandboxJobSpec,
            _hooks: &myelin_ci_sandbox::RunnerHooks,
        ) -> Result<SandboxLaunch, myelin_ci_sandbox::SandboxLaunchError<Self::Error>> {
            Ok(SandboxLaunch {
                handle: SandboxHandle {
                    guest_id: "unused-inline".into(),
                },
                result: SandboxResult::stub_ok(ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                }),
                output_complete: true,
            })
        }
        fn kill(&self, _h: &SandboxHandle) -> Result<(), Self::Error> {
            Ok(())
        }
        fn accept_async(&self, spec: &SandboxJobSpec) -> Result<(), Self::Error> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_first && n == 0 {
                return Err(SpecError::NoTimeout);
            }
            self.accepted.lock().unwrap().push(spec.clone());
            Ok(())
        }
    }

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
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_signals(signals)
    }

    fn pinned() -> ImageRef {
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef000000000000000000000000000000000000000000000000").unwrap()
    }
    fn limits() -> ResourceLimits {
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs: 7200,
        }
    }
    fn compute_def(name: &str) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: "agent".into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec![],
            effect_kind: EffectKind::Compute,
            side_effecting: false,
            requires_approval: false,
            exposed_over_mcp: false,
        }
    }
    fn profile() -> LongComputeProfile {
        LongComputeProfile {
            image: pinned(),
            command: vec!["cargo".into(), "test".into(), "--release".into()],
            secret_refs: vec![],
            egress: EgressPolicy::deny_all(),
            limits: limits(),
            trust_tier: TrustTier::UntrustedFork,
            run_token: RunTokenCredential::new("test-bearer", "agent-jti", 300).unwrap(),
            meter_to: MeterTarget {
                reserve_id: "agent-res".into(),
            },
            idem_token: IdemToken("agent-idem".into()),
        }
    }

    fn green_gate() -> AgentExecGate {
        use crate::escape_gate::ProductionBackendId;
        use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
        use myelin_ci_sandbox::{
            parse_console, Backend, BackendRun, EscapeAttestation, CORPUS, CORPUS_VERSION,
        };
        let id = ProductionBackendId {
            backend: Backend::FirecrackerMicrovm,
            rootfs_sha256: "rootfs-digest".into(),
            kernel_sha256: "kernel-digest".into(),
            corpus_version: CORPUS_VERSION,
        };
        let mut console = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
        for atk in CORPUS {
            console.push_str(&format!("{} CONTAINED\n", atk.id));
        }
        console.push_str(&format!("{END_MARKER}\n"));
        let report = parse_console(&console);
        let att = EscapeAttestation::from_green_drill(
            "2026-06-21",
            &report,
            vec![BackendRun {
                backend: Backend::FirecrackerMicrovm,
                exercised: true,
                residual_note: None,
            }],
            Backend::FirecrackerMicrovm,
            "rootfs-digest",
            "kernel-digest",
            "6.1.168",
        )
        .unwrap();
        AgentExecGate::admit(Some(&att), &id).unwrap()
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
            consumed_seq: None,
            received_unix_ms: 0,
        });
    }

    #[test]
    fn long_compute_dispatches_and_parks_holding_no_runtime() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let backend = RecordingAsyncBackend::default();

        let mut ctx = begin(&outbox, journal, signals);
        let out = dispatch_long_compute(
            &mut ctx,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            None,
        )
        .expect("a compute tool builds a long-park job")
        .expect("dispatch + park");

        assert!(
            out.is_parked(),
            "the long-park returns Parked (the worker is freed): {out:?}"
        );
        assert!(
            ctx.parked_on_signal(),
            "the run is waiting on job.done (holds NO runtime)"
        );
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
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
    fn the_dispatched_spec_carries_the_deterministic_idem_token() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let backend = RecordingAsyncBackend::default();

        let consumer_token = job_idem_token("R1", "agent.run:0");

        let mut ctx = begin(&outbox, journal, signals);
        let _ = dispatch_long_compute(
            &mut ctx,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            None,
        )
        .expect("build")
        .expect("park");

        let accepted = backend.accepted.lock().unwrap();
        assert_eq!(accepted.len(), 1, "one async dispatch");
        assert_eq!(
            accepted[0].idem_token.0, consumer_token,
            "the engine stamped the deterministic dispatch token the runner echoes on job.done"
        );
        assert_eq!(
            accepted[0].kind,
            myelin_ci_sandbox::JobKind::Agent,
            "the hardened spec the backend received is a kind=agent job"
        );
    }

    #[test]
    fn a_doubly_delivered_job_done_wakes_the_run_exactly_once() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let backend = RecordingAsyncBackend::default();

        let token = job_idem_token("R1", "agent.run:0");
        deliver_job_done(
            &signals,
            &token,
            vec![ArtifactRef("myelin://acme/agent/trace/ok".into())],
        );
        deliver_job_done(
            &signals,
            &token,
            vec![ArtifactRef("myelin://acme/agent/trace/ok".into())],
        );
        assert_eq!(
            signals.buffered_depth(),
            1,
            "the double delivery deduped to ONE buffered row"
        );

        let mut ctx = begin(&outbox, journal, signals.clone());
        let out = dispatch_long_compute(
            &mut ctx,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            None,
        )
        .expect("build")
        .expect("dispatch + complete");

        match out {
            LongParkOutcome::Completed { idem_token, result } => {
                assert_eq!(idem_token, token, "the runner echoed the dispatch token");
                assert_eq!(
                    result,
                    vec![ArtifactRef("myelin://acme/agent/trace/ok".into())]
                );
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(
            ctx.consumed_signals().len(),
            1,
            "EXACTLY ONE wake per job (the double-delivery deduped)"
        );
        assert_eq!(
            signals.buffered_depth(),
            0,
            "the one buffered row is consumed once"
        );
    }

    #[derive(Default)]
    struct RecordingMinter {
        calls: Mutex<Vec<(String, DelegationCaveats, u64)>>,
    }
    impl RunTokenMinter for RecordingMinter {
        fn mint_run_token(
            &self,
            agent_id: &str,
            run_id: &str,
            caveats: &DelegationCaveats,
            ttl_secs: u64,
        ) -> Result<RunTokenHandle, RunTokenError> {
            let mut c = self.calls.lock().unwrap();
            let n = c.len();
            c.push((agent_id.into(), caveats.clone(), ttl_secs));
            Ok(RunTokenHandle {
                token: format!("tok:{run_id}:{n}"),
                jti: format!("jti:{run_id}:{n}"),
                ttl_secs,
            })
        }
    }

    #[test]
    fn on_wake_after_a_long_park_the_per_run_token_is_reminted() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let backend = RecordingAsyncBackend::default();
        let mint = Arc::new(RecordingMinter::default());
        let lease = RunTokenLease::new(
            mint.clone(),
            "psn:agent-7",
            DelegationCaveats(vec!["delegated:human-x".into()]),
        );

        let mut c1 =
            begin(&outbox, journal.clone(), signals.clone()).with_run_identity(lease.clone());
        let out1 = dispatch_long_compute(
            &mut c1,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            None,
        )
        .expect("build")
        .expect("dispatch + park");
        assert!(out1.is_parked(), "drive 1 parks holding no runtime");
        assert_eq!(
            c1.reminted_tokens(),
            0,
            "the cold dispatch drive does NOT re-mint (nothing resumed)"
        );
        c1.commit()
            .expect("co-commit the dispatch + the park marker");
        let history = journal.history_for(&tenant(), "R1");

        let token = job_idem_token("R1", "agent.run:0");
        deliver_job_done(
            &signals,
            &token,
            vec![ArtifactRef("myelin://acme/agent/trace/ok".into())],
        );

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals.clone())
        .with_run_identity(lease);
        let out2 = dispatch_long_compute(
            &mut c2,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            None,
        )
        .expect("build")
        .expect("the wake drive");
        assert!(
            out2.is_completed(),
            "drive 2 completes on the arrived job.done: {out2:?}"
        );
        assert_eq!(
            c2.reminted_tokens(),
            1,
            "the wake RE-MINTED a fresh per-run token (gate #3)"
        );

        let calls = mint.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "exactly one re-mint on the wake");
        let (agent, cav, ttl) = calls[0].clone();
        assert_eq!(agent, "psn:agent-7");
        assert_eq!(
            ttl,
            RunTokenLease::DEFAULT_TTL_SECS,
            "short-lived (the fail-static W, not the workflow life)"
        );
        assert!(
            cav.0.contains(&"run:R1".to_string()),
            "attenuated per-run (cannot act outside R1)"
        );
        assert!(
            cav.0.contains(&"delegated:human-x".to_string()),
            "the SAME grant chain (attenuate-only)"
        );
    }

    #[test]
    fn a_vanished_runner_times_out_and_never_parks_forever() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = myelin_flow::timer::TimerStore::new();
        let backend = RecordingAsyncBackend::default();

        let mut c1 =
            begin(&outbox, journal.clone(), signals.clone()).with_timers(timers.clone(), 0, 1000);
        let out1 = dispatch_long_compute(
            &mut c1,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            Some(3600),
        )
        .expect("build")
        .expect("dispatch + park");
        assert!(
            out1.is_parked(),
            "dispatched, parked on job.done with an SLA timer"
        );
        c1.commit().expect("co-commit the dispatch + the SLA timer");
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals.clone())
        .with_timers(timers.clone(), 0, 9000);
        let out2 = dispatch_long_compute(
            &mut c2,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            Some(3600),
        )
        .expect("build")
        .expect("the timeout drive");
        assert!(
            out2.is_timed_out(),
            "the SLA fired before the runner reported → TimedOut: {out2:?}"
        );
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "the job was dispatched ONCE - the replay short-circuit did not re-dispatch it"
        );
    }

    #[test]
    fn a_mutate_tool_can_never_long_park() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let backend = RecordingAsyncBackend::default();

        let mutate = ToolDef {
            name: ToolName("issue.create".into()),
            subsystem: "issues".into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec![],
            effect_kind: EffectKind::Mutate,
            side_effecting: true,
            requires_approval: true,
            exposed_over_mcp: false,
        };
        let mut ctx = begin(&outbox, journal, signals);
        let err = dispatch_long_compute(
            &mut ctx,
            green_gate(),
            &backend,
            &mutate,
            &Command("x".into()),
            profile(),
            None,
        )
        .expect_err("a mutate tool cannot build a long-park job");
        assert!(
            matches!(err, RoutingError::NotComputeBound { ref tool, .. } if tool == "issue.create"),
            "a non-compute tool is REFUSED LOUD (the routing split): {err:?}"
        );
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            0,
            "nothing was dispatched (0 mutate-via-exec)"
        );
    }

    #[test]
    fn long_park_outcome_predicates_are_exact() {
        let completed = LongParkOutcome::Completed {
            idem_token: "t".into(),
            result: vec![],
        };
        let parked = LongParkOutcome::Parked;
        let timed_out = LongParkOutcome::TimedOut;
        assert!(completed.is_completed() && !completed.is_parked() && !completed.is_timed_out());
        assert!(parked.is_parked() && !parked.is_completed() && !parked.is_timed_out());
        assert!(timed_out.is_timed_out() && !timed_out.is_completed() && !timed_out.is_parked());
    }

    #[test]
    fn from_job_outcome_is_a_faithful_re_tag() {
        let refs = vec![ArtifactRef("r".into())];
        assert_eq!(
            LongParkOutcome::from_job_outcome(JobOutcome::Completed {
                idem_token: "t".into(),
                result: refs.clone(),
            }),
            LongParkOutcome::Completed {
                idem_token: "t".into(),
                result: refs,
            }
        );
        assert_eq!(
            LongParkOutcome::from_job_outcome(JobOutcome::Parked),
            LongParkOutcome::Parked
        );
        assert_eq!(
            LongParkOutcome::from_job_outcome(JobOutcome::TimedOut),
            LongParkOutcome::TimedOut
        );
    }

    #[test]
    fn the_long_job_target_is_references_not_payloads() {
        let job = build_long_job(
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            &profile(),
        )
        .unwrap();
        let target = long_job_target(&job);
        assert_eq!(
            target, "agent-job:agent-idem",
            "a machine handle naming the job, no PII body"
        );
    }
}
