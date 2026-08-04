use myelin_ci_sandbox::{
    CompletionClaim, CountingFirehose, EgressPolicy, EngineTerminalReporter, IdemToken, ImageRef,
    JobKind, JobLeaseStore, JobSpec, MeterTarget, QueuedJob, ReserveHandle, ResourceLimits,
    ResourceUsage, RunnerAgent, RunnerHooks, SandboxBackend, SandboxHandle, SandboxLaunch,
    SandboxLaunchError, SandboxResult, TerminalReport, TerminalReporter, TrustTier, WorkspaceSpec,
};
use myelin_events::MonotonicMinter;
use myelin_flow::{
    job_idem_token, DurableExecutor, FlowExecutor, RunId, SignalOutcome, SignalSpec, StartSpec,
    JOB_DONE_SIGNAL,
};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn pinned() -> ImageRef {
    ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap()
}
fn limits() -> ResourceLimits {
    ResourceLimits {
        cpu_millis: 1000,
        mem_bytes: 256 << 20,
        disk_bytes: 1 << 30,
        tmpfs_bytes: 1 << 30,
        pids_max: 128,
        timeout_secs: 600,
    }
}
fn ci_spec(idem: &str) -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        pinned(),
        vec!["cargo".into(), "test".into()],
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        limits(),
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        myelin_ci_sandbox::RunTokenCredential::new("test-bearer", "jti", 300).unwrap(),
        MeterTarget {
            reserve_id: "res".into(),
        },
        IdemToken(idem.into()),
    )
    .unwrap()
}
fn hooks() -> RunnerHooks {
    RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    )
}

struct NoopBackend;
impl SandboxBackend for NoopBackend {
    type Error = myelin_ci_sandbox::HookError;
    fn launch(
        &self,
        spec: &JobSpec,
        h: &RunnerHooks,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
        (|| -> Result<SandboxLaunch, myelin_ci_sandbox::HookError> {
            h.enforce_isolation_floor(spec)?;
            let r = h.reserve(spec)?;
            if let Err(error) = h.attribute(spec) {
                h.release_unused(spec, &r)?;
                return Err(error);
            }
            let result = SandboxResult::stub_ok(ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 1,
            });
            h.settle_completed(spec, &r, result.usage)?;
            Ok(SandboxLaunch {
                handle: SandboxHandle {
                    guest_id: "g".into(),
                },
                result,
                output_complete: true,
            })
        })()
        .map_err(SandboxLaunchError::Failed)
    }
    fn kill(&self, _h: &SandboxHandle) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn started_run(ex: &FlowExecutor) -> RunId {
    ex.register_definition("ci.pipeline");
    ex.start(StartSpec {
        wf_type: "ci.pipeline".into(),
        input: vec![],
        budget: None,
        idem_key: "ci:run".into(),
    })
    .expect("start")
}

#[test]
fn runner_echoes_idem_token_engine_wakes_exactly_once() {
    let ex = FlowExecutor::new(Arc::new(MonotonicMinter::new()), tenant(), region());
    let run = started_run(&ex);
    let idem = job_idem_token(&run.0, "ci.pipeline:0");

    let q = JobLeaseStore::new();
    q.enqueue(QueuedJob::new(
        tenant(),
        region(),
        &run.0,
        "job-1",
        vec!["linux".into()],
        ci_spec(&idem),
    ));

    let backend = NoopBackend;
    let firehose = CountingFirehose::new();
    let reporter = EngineTerminalReporter::new(ex.clone());
    let agent = RunnerAgent::new(
        "worker-1",
        vec!["linux".into()],
        vec![TrustTier::Trusted],
        region(),
        30,
        q,
        &backend,
        &firehose,
        &reporter,
        hooks(),
    );

    let out = agent.run_one(1000).expect("the runner runs + reports");
    assert_eq!(
        out.signal_outcome,
        SignalOutcome::Buffered,
        "the runner's FIRST job.done (echoing the engine idem_token) wakes the parked workflow"
    );

    let again = agent
        .report_done_again(
            &CompletionClaim {
                tenant: tenant(),
                run: run.clone(),
                job_id: "job-1".into(),
                idem_token: idem.clone(),
                lease_owner: "worker-1".into(),
                lease_epoch: out.lease_epoch,
                claim_nonce: out.claim_nonce.clone(),
            },
            &out.report,
        )
        .expect("re-delivery is the idempotency working");
    assert_eq!(
        again,
        SignalOutcome::Duplicate,
        "the runner's SECOND job.done is a no-op"
    );

    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        1,
        "the runner ↔ engine agreement: double-deliver buffers once (double-effect = 0)"
    );
}

#[test]
fn the_provider_keys_job_done_on_the_frozen_consumer_tuple() {
    let ex = FlowExecutor::new(Arc::new(MonotonicMinter::new()), tenant(), region());
    let run = started_run(&ex);
    let idem = job_idem_token(&run.0, "ci.pipeline:0");

    let reporter = EngineTerminalReporter::new(ex.clone());
    let outcome = reporter
        .report_done(
            &CompletionClaim {
                tenant: tenant(),
                run: run.clone(),
                job_id: "job-1".into(),
                idem_token: idem.clone(),
                lease_owner: "worker-1".into(),
                lease_epoch: 1,
                claim_nonce: "ignored-by-generic-reporter".into(),
            },
            &TerminalReport {
                passed: true,
                timed_out: false,
                usage: ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                },
                result_refs: vec![],
            },
        )
        .expect("the runner reports job.done onto the engine signal path");
    assert_eq!(outcome, SignalOutcome::Buffered);

    let row = ex
        .signals()
        .get(&tenant(), &run.0, JOB_DONE_SIGNAL, &idem)
        .expect("the job.done is addressable by the frozen consumer dedup tuple");
    assert_eq!(row.signal_name, JOB_DONE_SIGNAL);
    assert_eq!(
        row.idem_key, idem,
        "keyed on the echoed idem_token (no coordination round-trip)"
    );

    let dup = ex
        .signal(SignalSpec {
            run: run.clone(),
            signal_name: JOB_DONE_SIGNAL.into(),
            idem_key: idem.clone(),
            payload: vec![],
            payload_key_ref: None,
        })
        .expect("re-delivery via the engine signal CONSUMER");
    assert_eq!(
        dup,
        SignalOutcome::Duplicate,
        "the runner's reporter and the engine signal CONSUMER are the SAME path (no fork)"
    );
}
