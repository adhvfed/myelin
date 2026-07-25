//! # The CDC pair for contracts 9.2 / 9.4 — the `SCHEDULE_AND_RUN_JOB` long-park idiom
//! CONSUMED by the Agent-Fabric (AG-P16 → P-228, M2-C)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` rows 9.2
//! (`WfCtx::schedule_and_run_job` — the long-park idiom: dispatch-and-return) + 9.4 (the `job.done`
//! durable signal idempotent on `idem_token`). Owning architecture: `agent-fabric.md` §5.6 (a run is
//! a durable workflow + the `SCHEDULE_AND_RUN_JOB` long-park idiom (C5) — *the Fabric CONSUMES this
//! idiom; it does not reinvent durable waits*).
//!
//! The Agent-Fabric long-park ([`dispatch_long_compute`]) is the CONSUMER of the 9.2/9.4 idiom: it
//! dispatches a long `kind=agent` `ToolHands::exec` job through the engine's `schedule_and_run_job`,
//! the run PARKS holding no runtime, and a durable `job.done` signal resumes it. This pair stands the
//! PRODUCER side (the runner that runs the job for hours and then delivers `job.done`) against the
//! consumer and proves the **no-coordination dedup agreement**: the runner derives the SAME
//! deterministic `idem_token` from `(run_id, command_id)` that the workflow keyed its wait on (via the
//! exposed [`job_idem_token`]), echoes it on `job.done`, and a DOUBLE delivery wakes the run EXACTLY
//! once. The parked run holds no runtime (the worker is freed between dispatch and completion).

use myelin_agent::{Command, EffectKind, ToolDef, ToolName};
use myelin_agent_service::escape_gate::{AgentExecGate, ProductionBackendId};
use myelin_agent_service::{dispatch_long_compute, LongComputeProfile};
use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
use myelin_ci_sandbox::{
    parse_console, Backend, BackendRun, EgressPolicy, EscapeAttestation, IdemToken, ImageRef,
    MeterTarget, ResourceLimits, ResourceUsage, RunTokenCredential, SandboxBackend, SandboxHandle,
    SandboxLaunch, SandboxResult, SpecError, TrustTier, CORPUS, CORPUS_VERSION,
};
use myelin_ci_sandbox::{JobSpec as SandboxJobSpec, RunnerHooks};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
};
use myelin_flow::engine::{SignalRow, SignalStore};
use myelin_flow::{job_idem_token, WfCtx, WfJournal, JOB_DONE_SIGNAL};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// **PRODUCER side of 9.2/9.4 (the unified-sandbox runner, async-dispatch role).** It accepts the
/// hardened `kind=agent` job asynchronously (the dispatch returns immediately — the run parks) and
/// RECORDS the dispatch `idem_token` the workflow stamped on it. The producer (the runner) and the
/// consumer (the workflow) derive the SAME deterministic token from `(run_id, command_id)` WITHOUT a
/// coordination round-trip — this fixture asserts the stamped token equals the one the workflow keys
/// its `wait_for_signal` on. A real runner would run the guest for hours; here we record the accept.
#[derive(Default)]
struct UnifiedRunnerProducer {
    accepted_tokens: Mutex<Vec<String>>,
    calls: AtomicUsize,
}

impl SandboxBackend for UnifiedRunnerProducer {
    type Error = SpecError;
    fn launch(
        &self,
        _spec: &SandboxJobSpec,
        _hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, Self::Error> {
        // the in-line launch is unused on the long-park path.
        Ok(SandboxLaunch {
            handle: SandboxHandle {
                guest_id: "unused".into(),
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        // RECORD the dispatch idem_token the workflow stamped (the no-coordination dedup key the
        // runner will echo on job.done).
        self.accepted_tokens
            .lock()
            .unwrap()
            .push(spec.idem_token.0.clone());
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
fn minter() -> std::sync::Arc<dyn IdMinter> {
    std::sync::Arc::new(MonotonicMinter::new())
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
fn compute_def() -> ToolDef {
    ToolDef {
        name: ToolName("agent.long_test".into()),
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
/// A real GREEN AG-D4 gate (AG-P17 → P-229) — the structural fail-closed prerequisite the long-park
/// dispatcher requires to exist at all. Minted from the corpus parser (never hardcoded).
fn green_gate() -> AgentExecGate {
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

fn profile() -> LongComputeProfile {
    LongComputeProfile {
        image: ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef000000000000000000000000000000000000000000000000").unwrap(),
        command: vec!["cargo".into(), "test".into(), "--release".into()],
        secret_refs: vec![],
        egress: EgressPolicy::deny_all(),
        limits: ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs: 7200,
        },
        trust_tier: TrustTier::UntrustedFork,
        run_token: RunTokenCredential::new("test-bearer", "agent-jti", 300).unwrap(),
        meter_to: MeterTarget {
            reserve_id: "agent-res".into(),
        },
        idem_token: IdemToken("agent-idem".into()),
    }
}

/// **CONSUMER drives the 9.2 dispatch-and-return; the run PARKS holding no runtime (§5.6).** A long
/// compute with no buffered `job.done` dispatches the hardened job onto the runner (one async accept)
/// and the run parks (`state=waiting`) — the worker is freed (it did NOT block on the two-hour job).
/// The runner RECORDED the deterministic dispatch `idem_token` the consumer will key its wait on.
#[test]
fn long_park_dispatches_and_parks_holding_no_runtime() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();
    let runner = UnifiedRunnerProducer::default();

    let mut ctx = begin(&outbox, journal, signals);
    let out = dispatch_long_compute(
        &mut ctx,
        green_gate(),
        &runner,
        &compute_def(),
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
        runner.calls.load(Ordering::SeqCst),
        1,
        "dispatched exactly once"
    );

    // PRODUCER + CONSUMER agree on the deterministic dedup key WITHOUT coordination: the token the
    // runner RECORDED equals the one the workflow keyed its wait on (derived from run_id+command_id).
    let consumer_token = job_idem_token("R1", "agent.run:0");
    let accepted = runner.accepted_tokens.lock().unwrap();
    assert_eq!(accepted.len(), 1, "one async dispatch");
    assert_eq!(
        accepted[0], consumer_token,
        "the runner echoes the SAME deterministic idem_token the workflow keyed its wait on (§4.9)"
    );
}

/// **CONSUMER + PRODUCER prove 9.4 — a DOUBLY-delivered `job.done` wakes the long-parked run EXACTLY
/// once (§5.6).** The runner finishes the (long) job and delivers `signal(run, "job.done", {result},
/// idem_key = idem_token)` — delivered TWICE (at-least-once under the bus, both on the SAME
/// deterministic token). The engine's `wf_signal` PK dedups to ONE buffered row; the consumer wakes
/// EXACTLY once and consumes the result once. 1 wake per job, never two.
#[test]
fn a_doubly_delivered_job_done_wakes_the_long_parked_run_exactly_once() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();
    let runner = UnifiedRunnerProducer::default();

    // The PRODUCER derives the SAME deterministic token (no coordination) and delivers job.done TWICE.
    let token = job_idem_token("R1", "agent.run:0");
    let result = vec![ArtifactRef("myelin://acme/agent/trace/green".into())];
    for _ in 0..2 {
        signals.deliver(SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: JOB_DONE_SIGNAL.into(),
            idem_key: token.clone(),
            payload: result.clone(),
            payload_key_ref: None,
            consumed_seq: None,
            received_unix_ms: 0,
        });
    }
    assert_eq!(
        signals.buffered_depth(),
        1,
        "the double delivery deduped to ONE buffered row (9.4 PK)"
    );

    let mut ctx = begin(&outbox, journal, signals.clone());
    let out = dispatch_long_compute(
        &mut ctx,
        green_gate(),
        &runner,
        &compute_def(),
        &Command("--workspace".into()),
        profile(),
        None,
    )
    .expect("build")
    .expect("dispatch + complete");

    match out {
        myelin_agent_service::LongParkOutcome::Completed {
            idem_token,
            result: got,
        } => {
            assert_eq!(
                idem_token, token,
                "the runner echoed the dispatch token (the dedup agreement held)"
            );
            assert_eq!(
                got, result,
                "the references-not-payloads result threads back"
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
    assert_eq!(
        ctx.consumed_signals().len(),
        1,
        "EXACTLY ONE wake per job (the double delivery deduped)"
    );
    assert_eq!(
        signals.buffered_depth(),
        0,
        "the one buffered row is consumed once"
    );
}
