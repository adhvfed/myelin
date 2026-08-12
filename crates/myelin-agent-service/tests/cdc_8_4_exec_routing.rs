use myelin_agent::{Command, EffectKind, ToolDef, ToolHands, ToolName};
use myelin_agent_service::escape_gate::{AgentExecGate, ProductionBackendId};
use myelin_agent_service::exec::{
    route_of, RoutingError, SandboxJob, SandboxToolHands, ToolRoute, PLATFORM_TOKEN_ENV,
};
use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
use myelin_ci_sandbox::{
    parse_console, Backend, BackendRun, EgressPolicy, EnvVar, EscapeAttestation, HookError,
    IdemToken, ImageRef, JobKind, JobSpec, MeterTarget, ReserveHandle, ResourceLimits,
    ResourceUsage, RunTokenCredential, RunnerHooks, SandboxBackend, SandboxHandle, SandboxLaunch,
    SandboxLaunchError, SandboxResult, SecretRef, TrustTier, CORPUS, CORPUS_VERSION,
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

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
        timeout_secs: 300,
    }
}

fn def(name: &str, kind: EffectKind, side_effecting: bool) -> ToolDef {
    ToolDef {
        name: ToolName(name.into()),
        subsystem: "issues".into(),
        version: 1,
        input_schema: "{}".into(),
        required_caps: vec![],
        effect_kind: kind,
        side_effecting,
        requires_approval: false,
        exposed_over_mcp: false,
    }
}

fn credential(jti: &str) -> RunTokenCredential {
    RunTokenCredential::new(format!("test-bearer:{jti}"), jti, 300).unwrap()
}

struct RunnerSeam {
    order: Arc<Mutex<Vec<&'static str>>>,
    kills: Arc<AtomicU32>,
    reserve_exhausted: bool,
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
            if self.reserve_exhausted {
                reserve_spec.meter_to = MeterTarget {
                    reserve_id: "__exhausted__".into(),
                };
            }
            let res = hooks.reserve(&reserve_spec)?;
            self.order.lock().unwrap().push("reserve");
            if let Err(error) = hooks.attribute(spec) {
                hooks.release_unused(&reserve_spec, &res)?;
                return Err(error);
            }
            self.order.lock().unwrap().push("attribute");
            let result = SandboxResult::stub_ok(ResourceUsage {
                cpu_seconds: 2,
                mem_byte_seconds: 4,
            });
            hooks.settle_completed(&reserve_spec, &res, result.usage)?;
            self.order.lock().unwrap().push("settle");
            Ok(SandboxLaunch {
                handle: SandboxHandle {
                    guest_id: "fabric-guest".into(),
                },
                result,
                output_complete: true,
            })
        })()
        .map_err(SandboxLaunchError::Failed)
    }
    fn kill(&self, _h: &SandboxHandle) -> Result<(), Self::Error> {
        self.kills.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn working_hooks() -> RunnerHooks {
    RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| {
            if spec.meter_to.reserve_id == "__exhausted__" {
                Err(HookError("wallet exhausted - refuse to start".into()))
            } else {
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }
        }),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    )
}

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

fn fabric_hands<'a>(
    backend: &'a RunnerSeam,
    hooks: RunnerHooks,
) -> SandboxToolHands<'a, RunnerSeam> {
    SandboxToolHands::new(
        green_gate(),
        backend,
        hooks,
        pinned(),
        credential("agent-jti"),
        MeterTarget {
            reserve_id: "agent-reserve".into(),
        },
        IdemToken("agent-idem".into()),
        TrustTier::UntrustedFork,
        limits(),
        EgressPolicy::deny_all(),
        Vec::new(),
    )
}

#[test]
fn fabric_exec_dispatches_kind_agent_job_through_the_four_guarantees() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let kills = Arc::new(AtomicU32::new(0));
    let backend = RunnerSeam {
        order: order.clone(),
        kills: kills.clone(),
        reserve_exhausted: false,
    };
    let hands = fabric_hands(&backend, working_hooks());
    let out = hands.exec(Command("cargo test".into()));
    assert_eq!(
        out,
        myelin_agent::ToolResult::Succeeded("sandbox:fabric-guest".into())
    );
    assert_eq!(
        *order.lock().unwrap(),
        vec!["isolation_floor", "reserve", "attribute", "settle"],
        "the four uniform guarantees fire in the mandated order (X-6 §5.2)"
    );
    assert_eq!(
        kills.load(Ordering::SeqCst),
        1,
        "whole-guest kill on teardown (guarantee #4)"
    );
}

#[test]
fn fabric_exec_refuses_to_start_on_exhausted_reserve_11_7() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let backend = RunnerSeam {
        order: order.clone(),
        kills: Arc::new(AtomicU32::new(0)),
        reserve_exhausted: true,
    };
    let hands = fabric_hands(&backend, working_hooks());
    let out = hands.exec(Command("cargo test".into()));
    assert!(
        out.is_refused() && out.content().starts_with("exec-refused:"),
        "refuse-to-start surfaces LOUD: {out:?}"
    );
    assert!(
        !order.lock().unwrap().contains(&"settle"),
        "the guest never ran - refuse-to-start (11.7), never interrupt in-flight's dual"
    );
}

#[test]
fn a_mutate_effect_can_never_reach_the_sandbox() {
    assert_eq!(route_of(EffectKind::Mutate), ToolRoute::EffectApi);
    let r = SandboxJob::for_compute(
        &def("issue.create", EffectKind::Mutate, true),
        pinned(),
        vec!["sh".into()],
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
    assert_eq!(
        r.unwrap_err(),
        RoutingError::NotComputeBound {
            tool: "issue.create".into(),
            actual_route: ToolRoute::EffectApi,
        }
    );
}

#[test]
fn the_consumer_feeds_the_full_hardening_profile_and_fails_closed_on_an_undigested_tag() {
    let compute = def("agent.lint", EffectKind::Compute, false);
    let job = SandboxJob::for_compute(
        &compute,
        pinned(),
        vec!["ruff".into()],
        vec![EnvVar {
            name: PLATFORM_TOKEN_ENV.into(),
            value: "leak-me".into(),
        }],
        vec![SecretRef {
            name: "NPM_TOKEN".into(),
            handle: "broker://job/npm".into(),
        }],
        EgressPolicy::deny_all(),
        limits(),
        TrustTier::UntrustedFork,
        credential("per-run"),
        MeterTarget {
            reserve_id: "res".into(),
        },
        IdemToken("idem".into()),
    )
    .unwrap();
    let spec = job.spec();
    assert_eq!(spec.kind, JobKind::Agent);
    assert!(spec.image.digest_pinned());
    assert!(spec.egress.allow.is_empty(), "default-deny egress");
    assert!(spec.limits.pids_max > 0 && spec.limits.timeout_secs > 0);
    assert!(spec.env.iter().all(|e| e.name != PLATFORM_TOKEN_ENV));
    assert_eq!(spec.run_token.jti, "per-run");
    assert_eq!(spec.secret_refs[0].name, "NPM_TOKEN");

    let bad = SandboxJob::for_compute(
        &compute,
        ImageRef {
            reference: "registry/runner:latest".into(),
        },
        vec!["ruff".into()],
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
    assert!(matches!(bad, Err(RoutingError::SpecRejected(_))));
}
