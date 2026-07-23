//! # The CDC pair for the RUNNER SIDE of `job.done` — contracts 9.2 / 9.4 (runner PROVIDER ↔
//! engine CONSUMER), CI-P3 → P-238.
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md` rows 9.2/9.4
//! (the `SCHEDULE_AND_RUN_JOB` long-park idiom + the `job.done` durable signal, idempotent on
//! `idem_token`) — CONSUMED by CI's runner. Owning architecture:
//! `continuous-integration/architecture/00-overview.md` §4 (the runner reports terminal via
//! `job.done`) + `02-internals-and-algorithms.md` §3.3 + recon §OQ-F.
//!
//! ## What this pair pins (the runner ↔ engine agreement of 9.2/9.4)
//!
//! **The RUNNER side (PROVIDER of `job.done` — CI-P3):** after running the job, the runner reports
//! terminal by delivering the `job.done` signal ECHOING the `idem_token` the workflow stamped on the
//! `JobSpec` at dispatch — `signal_name = JOB_DONE_SIGNAL`, `idem_key = idem_token`. It can deliver
//! "done" TWICE (at-least-once); it does NOT build a second signal path.
//!
//! **The ENGINE side (CONSUMER — the exactly-once wake, shipped P-FLOW-09 / P-205):**
//! `DurableExecutor::signal` buffers the `job.done` via `INSERT … ON CONFLICT (tenant, run_id,
//! signal_name, idem_key) DO NOTHING` — a double-delivery buffers ONCE; the parked workflow wakes
//! ONCE. The agreement: the SAME `idem_token` flows engine → JobSpec → runner → `job.done` → engine,
//! and the engine's PK dedup makes the wake exactly-once. The runner REUSES this; it never forks it.

use myelin_ci_sandbox::{
    CompletionClaim, CountingFirehose, EgressPolicy, EngineTerminalReporter, IdemToken, ImageRef,
    JobKind, JobLeaseStore, JobSpec, MeterTarget, QueuedJob, ReserveHandle, ResourceLimits,
    ResourceUsage, RunnerAgent, RunnerHooks, SandboxBackend, SandboxHandle, SandboxLaunch,
    SandboxResult, TerminalReport, TerminalReporter, TrustTier, WorkspaceSpec,
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

/// A no-op backend that drives the four-guarantee seam (the runner DRIVES the sandbox; it does not
/// reimplement it). No host-exec path.
struct NoopBackend;
impl SandboxBackend for NoopBackend {
    type Error = myelin_ci_sandbox::HookError;
    fn launch(&self, spec: &JobSpec, h: &RunnerHooks) -> Result<SandboxLaunch, Self::Error> {
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
        })
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

/// **PROVIDER (the runner) ↔ CONSUMER (the engine): the runner echoes the engine's `idem_token` on
/// `job.done`; the engine wakes exactly once.** The runner runs the job (claimed via the lease
/// handshake) and reports terminal onto the ENGINE's `DurableExecutor::signal` — keyed on the SAME
/// `idem_token` the workflow stamped on the spec. A first delivery buffers (wakes); a re-delivery is
/// a no-op (the workflow wakes once). The runner reuses the engine signal path — no fork.
#[test]
fn runner_echoes_idem_token_engine_wakes_exactly_once() {
    let ex = FlowExecutor::new(Arc::new(MonotonicMinter::new()), tenant(), region());
    let run = started_run(&ex);
    // the engine minted the deterministic idem_token at the dispatch position; the runner ECHOES it.
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

    // the runner DERIVES the report from the backend's clean result (no longer an input).
    let out = agent.run_one(1000).expect("the runner runs + reports");
    assert_eq!(
        out.signal_outcome,
        SignalOutcome::Buffered,
        "the runner's FIRST job.done (echoing the engine idem_token) wakes the parked workflow"
    );

    // the runner RE-delivers (at-least-once) — the engine's ON CONFLICT DO NOTHING makes it a no-op.
    // The re-delivery passes the derived report directly (the job row is already settled).
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

    // EXACTLY ONE buffered job.done row — the agreement: same idem_token, engine wakes once.
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        1,
        "the runner ↔ engine agreement: double-deliver buffers once (double-effect = 0)"
    );
}

/// **The CONSUMER's frozen dedup key is `(tenant, run_id, signal_name, idem_key)`; the PROVIDER must
/// key `signal_name = JOB_DONE_SIGNAL`, `idem_key = idem_token`.** This pins the exact tuple the
/// runner's `EngineTerminalReporter` builds against the engine's `SignalSpec` — a drift in either end
/// (a different signal name, or keying on something other than the idem_token) breaks the wake-once
/// agreement and fails THIS test.
#[test]
fn the_provider_keys_job_done_on_the_frozen_consumer_tuple() {
    let ex = FlowExecutor::new(Arc::new(MonotonicMinter::new()), tenant(), region());
    let run = started_run(&ex);
    let idem = job_idem_token(&run.0, "ci.pipeline:0");

    // the runner-side reporter (PROVIDER) delivers exactly the SignalSpec the engine (CONSUMER) dedups
    // on — proven by the buffered row being addressable by (tenant, run, JOB_DONE_SIGNAL, idem_token).
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

    // the engine buffered it under the frozen tuple (signal_name = job.done, idem_key = idem_token).
    let row = ex
        .signals()
        .get(&tenant(), &run.0, JOB_DONE_SIGNAL, &idem)
        .expect("the job.done is addressable by the frozen consumer dedup tuple");
    assert_eq!(row.signal_name, JOB_DONE_SIGNAL);
    assert_eq!(
        row.idem_key, idem,
        "keyed on the echoed idem_token (no coordination round-trip)"
    );

    // and the same SignalSpec re-delivered (what the engine's signal CONSUMER sees) is a duplicate —
    // proving the runner's path IS the engine's signal path (no second mechanism).
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
