//! # The CDC pair for contract 8.4 — `ToolHands::exec` = the unified sandbox (CI runner-seam half)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.4
//! (`ToolHands::exec(Command) -> ToolResult` — sandboxed computation; **no host-exec bypass**; **=
//! the CI runner's `kind=agent` job** on the unified sandbox; the real-kernel escape drill gates
//! both kinds; only `compute`/`external` untrusted code here, mutation goes through `EffectApi`;
//! the **four uniform guarantees** — cost gate, per-run-token attribution, HITL withhold, isolation
//! floor + drill). Owning architecture (byte-authoritative):
//! `continuous-integration/architecture/01-tech-and-data-model.md` §2 (the JobSpec + the
//! SandboxBackend/FleetProvider seam) + `02-internals-and-algorithms.md` §5.2 (the four uniform
//! guarantees). Reconciliation: `00-reconciliation-decisions.md` X-6.
//!
//! ## What this pair pins (the CI runner-seam half of 8.4; CI-P1 / P-129)
//! Row 8.4 is a co-defined seam (EI-01 §7): CI **owns the runner** (the JobSpec shape + the
//! SandboxBackend trait + the four-guarantee hooks), and the agent fabric is the **consumer** that
//! dispatches `ToolHands::exec` onto it. This file pins the **CI runner-seam half**:
//!
//! - the **PROVIDER** is the CI runner seam — it accepts ANY well-formed `JobSpec` (either `kind`)
//!   through `SandboxBackend::launch`, drives the four-guarantee `RunnerHooks` in the mandated
//!   order (isolation floor → reserve → final attribution → … → settle), and **only ever launches a
//!   digest-pinned, pids-capped, timeout-bounded spec** (the fail-closed non-negotiables) — there
//!   is no host-exec bypass (`no-host-exec`, X-6/AG-2);
//! - the **CONSUMER** is a dispatcher (the agent fabric's `ToolHands::exec`, AG-P8 → P-226; or the
//!   CI scheduler) — it builds a `JobSpec` (the agent case via `agent_job`, wiring the X-6
//!   equivalence `ToolHands::exec(Command)` IS `launch(JobSpec{kind:Agent})`) and hands it to the
//!   provider; it never reaches the host directly, and an un-digested image it tries to run is
//!   rejected fail-closed BEFORE launch.
//!
//! The full agent-fabric consumer half (the real `ToolHands::exec` body + the dispatch path) is
//! AG-P8 (→ P-226); this is the runner-seam half the CI-P1 TESTS field names. The ZERO-escapes
//! real-kernel GATE is CI-P5 (→ P-239); the Firecracker backend is CI-P2 (→ P-237).

use myelin_ci_sandbox::{
    agent_job, EgressPolicy, HookError, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget,
    ReserveHandle, ResourceLimits, ResourceUsage, RunTokenCredential, RunnerHooks, SandboxBackend,
    SandboxHandle, SandboxLaunch, SandboxResult, SpecError, TrustTier, WorkspaceSpec,
};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

fn credential(jti: &str) -> RunTokenCredential {
    RunTokenCredential::new(format!("test-bearer:{jti}"), jti, 300).unwrap()
}

fn limits() -> ResourceLimits {
    ResourceLimits {
        cpu_millis: 1000,
        mem_bytes: 256 << 20,
        disk_bytes: 1 << 30,
        pids_max: 128,
        timeout_secs: 300,
    }
}

fn pinned() -> ImageRef {
    ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap()
}

/// **PROVIDER side of 8.4 (CI runner seam).** A backend that records the ORDER it drives the four
/// guarantees so the consumer can assert the mandated sequence. It launches only specs that already
/// passed the fail-closed `JobSpec::new` invariants (the type guarantees that), and drives:
/// #4 isolation floor → #1a reserve → #2 final attribution → (guest runs) → #1b settle. There is no
/// host-execution path — all execution goes through this `launch` seam (X-6 / AG-2).
struct RunnerSeam {
    /// Bitset of which guarantees fired, in call order encoded as a sequence.
    order: Arc<std::sync::Mutex<Vec<&'static str>>>,
    /// Whether the cost gate should refuse (exhausted wallet) — to drill refuse-to-start.
    exhausted: bool,
}

impl SandboxBackend for RunnerSeam {
    type Error = HookError;

    fn launch(&self, spec: &JobSpec, hooks: &RunnerHooks) -> Result<SandboxLaunch, Self::Error> {
        // #4 isolation floor FIRST — the hardening profile must hold before any code runs.
        hooks.enforce_isolation_floor(spec)?;
        self.order.lock().unwrap().push("isolation_floor");
        // #1a cost gate — reserve at dispatch; refuse-to-start on exhaustion (never starts).
        let mut reserve_spec = spec.clone();
        if self.exhausted {
            reserve_spec.meter_to = MeterTarget {
                reserve_id: "__exhausted__".into(),
            };
        }
        let reserve = hooks.reserve(&reserve_spec)?;
        self.order.lock().unwrap().push("reserve");
        // #2 final attribution — immediately before untrusted code would spawn.
        if let Err(error) = hooks.attribute(spec) {
            hooks.release_unused(&reserve_spec, &reserve)?;
            return Err(error);
        }
        self.order.lock().unwrap().push("attribute");
        // ... the hardened guest would run the (compute/external) command here; the seam carries
        // the result back (RESHAPE-001 / CT-001 stub) ...
        let result = SandboxResult::stub_ok(ResourceUsage {
            cpu_seconds: 2,
            mem_byte_seconds: 4,
        });
        // #1b settle — release the unused reserve on completion (never interrupt in-flight).
        hooks.settle_completed(&reserve_spec, &reserve, result.usage)?;
        self.order.lock().unwrap().push("settle");
        Ok(SandboxLaunch {
            handle: SandboxHandle {
                guest_id: "guest-1".into(),
            },
            result,
            output_complete: true,
        })
    }

    fn kill(&self, _h: &SandboxHandle) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// **CONSUMER side of 8.4.** A dispatcher building a `kind=ci` `JobSpec`. The consumer never
/// reaches the host; it hands a fully-scoped spec to the provider's `launch`.
fn consumer_builds_ci_spec() -> JobSpec {
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
        credential("run-token-jti"),
        MeterTarget {
            reserve_id: "reserve-1".into(),
        },
        IdemToken("idem-1".into()),
    )
    .expect("the consumer only dispatches a well-formed, fail-closed spec")
}

/// **CONSUMER side of 8.4 (agent fabric, the X-6 equivalence).** The agent fabric's
/// `ToolHands::exec(Command)` IS `launch(JobSpec{kind:Agent})` — built via `agent_job`.
fn consumer_builds_agent_exec_spec() -> JobSpec {
    agent_job(
        pinned(),
        vec!["python".into(), "-c".into(), "print(1)".into()],
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        limits(),
        TrustTier::UntrustedFork,
        credential("agent-jti"),
        MeterTarget {
            reserve_id: "agent-reserve".into(),
        },
        IdemToken("agent-idem".into()),
    )
    .expect("the agent-fabric consumer dispatches a well-formed kind=agent spec")
}

fn working_hooks() -> RunnerHooks {
    RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    )
}

/// The PROVIDER admits the CONSUMER's CI spec and drives all four guarantees in the mandated order.
#[test]
fn provider_launches_consumer_ci_spec_driving_four_guarantees_in_order() {
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let provider = RunnerSeam {
        order: order.clone(),
        exhausted: false,
    };
    let spec = consumer_builds_ci_spec();
    let launch = provider.launch(&spec, &working_hooks()).unwrap();
    assert_eq!(launch.handle.guest_id, "guest-1");
    assert_eq!(
        launch.result.exit_code,
        Some(0),
        "the seam carries the result"
    );
    provider.kill(&launch.handle).unwrap();
    assert_eq!(
        *order.lock().unwrap(),
        vec!["isolation_floor", "reserve", "attribute", "settle"],
        "the four uniform guarantees must fire in the mandated order (X-6 §5.2)"
    );
}

/// The X-6 equivalence: an agent `ToolHands::exec` is the SAME launch path with `kind: Agent`.
#[test]
fn provider_launches_the_agent_exec_spec_on_the_same_seam() {
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let provider = RunnerSeam {
        order: order.clone(),
        exhausted: false,
    };
    let spec = consumer_builds_agent_exec_spec();
    assert_eq!(spec.kind, JobKind::Agent);
    provider.launch(&spec, &working_hooks()).unwrap();
    // The agent exec inherits ALL four guarantees by construction — same seam, same order.
    assert_eq!(
        *order.lock().unwrap(),
        vec!["isolation_floor", "reserve", "attribute", "settle"]
    );
}

/// Guarantee #1 (cost gate): the provider refuses to start when the consumer's reserve is
/// exhausted — fail-closed, the job never runs (never-interrupt-in-flight's dual).
#[test]
fn provider_refuses_to_start_when_the_cost_gate_is_exhausted() {
    let fired = Arc::new(AtomicU8::new(0));
    let fired2 = fired.clone();
    let provider = RunnerSeam {
        order: Arc::new(std::sync::Mutex::new(Vec::new())),
        exhausted: true,
    };
    let hooks = RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(move |spec| {
            if spec.meter_to.reserve_id == "__exhausted__" {
                Err(HookError("wallet exhausted — refuse to start".into()))
            } else {
                fired2.store(1, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }
        }),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    );
    let r = provider.launch(&consumer_builds_ci_spec(), &hooks);
    assert!(
        r.is_err(),
        "an exhausted wallet must refuse-to-start (11.7)"
    );
    assert_eq!(fired.load(Ordering::SeqCst), 0, "the job must never start");
}

/// The CONSUMER cannot dispatch an un-digested image — it is rejected fail-closed BEFORE it can
/// reach the provider's launch (CI-1 / the no-host-exec safety boundary). Both kinds.
#[test]
fn consumer_cannot_dispatch_an_undigested_image_to_the_provider() {
    let ci = JobSpec::new(
        JobKind::Ci,
        ImageRef {
            reference: "registry/runner:latest".into(),
        },
        vec![],
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        limits(),
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        credential("j"),
        MeterTarget {
            reserve_id: "r".into(),
        },
        IdemToken("i".into()),
    );
    assert!(matches!(ci, Err(SpecError::UndigestedImage { .. })));

    let agent = agent_job(
        ImageRef {
            reference: "registry/runner:latest".into(),
        },
        vec![],
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        limits(),
        TrustTier::UntrustedFork,
        credential("j"),
        MeterTarget {
            reserve_id: "r".into(),
        },
        IdemToken("i".into()),
    );
    assert!(matches!(agent, Err(SpecError::UndigestedImage { .. })));
}
