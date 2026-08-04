use myelin_ci_sandbox::{
    agent_job, EgressPolicy, HookError, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget,
    ReserveHandle, ResourceLimits, ResourceUsage, RunTokenCredential, RunnerHooks, SandboxBackend,
    SandboxHandle, SandboxLaunch, SandboxLaunchError, SandboxResult, SpecError, TrustTier,
    WorkspaceSpec,
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
        tmpfs_bytes: 1 << 30,
        pids_max: 128,
        timeout_secs: 300,
    }
}

fn pinned() -> ImageRef {
    ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap()
}

struct RunnerSeam {
    order: Arc<std::sync::Mutex<Vec<&'static str>>>,
    exhausted: bool,
}

impl SandboxBackend for RunnerSeam {
    type Error = HookError;

    fn launch(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
        (|| -> Result<SandboxLaunch, HookError> {
            hooks.enforce_isolation_floor(spec)?;
            self.order.lock().unwrap().push("isolation_floor");
            let mut reserve_spec = spec.clone();
            if self.exhausted {
                reserve_spec.meter_to = MeterTarget {
                    reserve_id: "__exhausted__".into(),
                };
            }
            let reserve = hooks.reserve(&reserve_spec)?;
            self.order.lock().unwrap().push("reserve");
            if let Err(error) = hooks.attribute(spec) {
                hooks.release_unused(&reserve_spec, &reserve)?;
                return Err(error);
            }
            self.order.lock().unwrap().push("attribute");
            let result = SandboxResult::stub_ok(ResourceUsage {
                cpu_seconds: 2,
                mem_byte_seconds: 4,
            });
            hooks.settle_completed(&reserve_spec, &reserve, result.usage)?;
            self.order.lock().unwrap().push("settle");
            Ok(SandboxLaunch {
                handle: SandboxHandle {
                    guest_id: "guest-1".into(),
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
    assert_eq!(
        *order.lock().unwrap(),
        vec!["isolation_floor", "reserve", "attribute", "settle"]
    );
}

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
                Err(HookError("wallet exhausted - refuse to start".into()))
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
