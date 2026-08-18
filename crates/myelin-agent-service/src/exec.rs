use crate::escape_gate::AgentExecGate;
use myelin_agent::{Command, EffectKind, ToolDef, ToolHands, ToolResult};
use myelin_ci_sandbox::{
    agent_job, CompletionSettlementOwner, EgressPolicy, EnvVar, IdemToken, ImageRef, JobKind,
    JobSpec, MeterTarget, ResourceLimits, RunTokenCredential, RunnerHooks, SandboxBackend,
    SandboxLaunchError, SecretRef, SpecError, TrustTier, WorkspaceSpec,
};

pub const PLATFORM_TOKEN_ENV: &str = "MYELIN_PLATFORM_TOKEN";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolRoute {
    Direct,
    Sandbox,
    EffectApi,
}

pub const fn route_of(kind: EffectKind) -> ToolRoute {
    match kind {
        EffectKind::Read => ToolRoute::Direct,
        EffectKind::Compute => ToolRoute::Sandbox,
        EffectKind::Mutate | EffectKind::External => ToolRoute::EffectApi,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoutingError {
    NotComputeBound {
        tool: String,
        actual_route: ToolRoute,
    },
    SpecRejected(SpecError),
}

impl std::fmt::Display for RoutingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoutingError::NotComputeBound { tool, actual_route } => write!(
                f,
                "tool `{tool}` routes to {actual_route:?}, not the sandbox - a non-`compute` effect \
                 cannot reach ToolHands::exec (the routing split is the safety boundary, §5.0/X-6 #3)"
            ),
            RoutingError::SpecRejected(e) => write!(
                f,
                "the kind=agent JobSpec was rejected fail-closed: {e} (the hardening profile is \
                 non-negotiable, X-6 #4 / CI-1)"
            ),
        }
    }
}

impl std::error::Error for RoutingError {}

impl From<SpecError> for RoutingError {
    fn from(e: SpecError) -> Self {
        RoutingError::SpecRejected(e)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxJob {
    spec: JobSpec,
}

impl SandboxJob {
    #[allow(clippy::too_many_arguments)]
    pub fn for_compute(
        def: &ToolDef,
        image: ImageRef,
        command: Vec<String>,
        env: Vec<EnvVar>,
        secret_refs: Vec<SecretRef>,
        egress: EgressPolicy,
        limits: ResourceLimits,
        trust_tier: TrustTier,
        run_token: RunTokenCredential,
        meter_to: MeterTarget,
        idem_token: IdemToken,
    ) -> Result<SandboxJob, RoutingError> {
        let route = route_of(def.effect_kind);
        if route != ToolRoute::Sandbox {
            return Err(RoutingError::NotComputeBound {
                tool: def.name.0.clone(),
                actual_route: route,
            });
        }

        let scrubbed = Self::scrub_platform_token(env);

        let spec = agent_job(
            image,
            command,
            scrubbed,
            secret_refs,
            egress,
            limits,
            trust_tier,
            run_token,
            meter_to,
            idem_token,
        )?;
        debug_assert_eq!(spec.kind, JobKind::Agent);
        debug_assert_eq!(spec.workspace, WorkspaceSpec::default());
        Ok(SandboxJob { spec })
    }

    pub fn scrub_platform_token(env: Vec<EnvVar>) -> Vec<EnvVar> {
        env.into_iter()
            .filter(|e| e.name != PLATFORM_TOKEN_ENV)
            .collect()
    }

    pub fn spec(&self) -> &JobSpec {
        &self.spec
    }

    pub fn with_dispatch_idem_token(mut self, idem_token: IdemToken) -> SandboxJob {
        self.spec.idem_token = idem_token;
        self
    }

    pub fn scrubbed_env(&self) -> &[EnvVar] {
        &self.spec.env
    }
}

pub struct SandboxToolHands<'a, B: SandboxBackend> {
    gate: AgentExecGate,
    backend: &'a B,
    hooks: RunnerHooks,
    image: ImageRef,
    run_token: RunTokenCredential,
    meter_to: MeterTarget,
    idem_token: IdemToken,
    trust_tier: TrustTier,
    limits: ResourceLimits,
    egress: EgressPolicy,
    secret_refs: Vec<SecretRef>,
}

impl<'a, B: SandboxBackend> SandboxToolHands<'a, B> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gate: AgentExecGate,
        backend: &'a B,
        hooks: RunnerHooks,
        image: ImageRef,
        run_token: RunTokenCredential,
        meter_to: MeterTarget,
        idem_token: IdemToken,
        trust_tier: TrustTier,
        limits: ResourceLimits,
        egress: EgressPolicy,
        secret_refs: Vec<SecretRef>,
    ) -> Self {
        SandboxToolHands {
            gate,
            backend,
            hooks,
            image,
            run_token,
            meter_to,
            idem_token,
            trust_tier,
            limits,
            egress,
            secret_refs,
        }
    }

    pub fn gate(&self) -> &AgentExecGate {
        &self.gate
    }

    pub fn dispatch_compute(&self, job: &SandboxJob) -> Result<ToolResult, ExecError<B::Error>> {
        if self.hooks.completion_settlement_owner() != CompletionSettlementOwner::Hook {
            return Err(ExecError::SettlementOwnerNotHook);
        }
        let launch = self
            .backend
            .launch(job.spec(), &self.hooks)
            .map_err(ExecError::Launch)?;
        self.backend.kill(&launch.handle).map_err(ExecError::Kill)?;
        Ok(ToolResult::Succeeded(format!(
            "sandbox:{}",
            launch.handle.guest_id
        )))
    }

    pub fn build_compute_job(
        &self,
        def: &ToolDef,
        cmd: &Command,
    ) -> Result<SandboxJob, RoutingError> {
        SandboxJob::for_compute(
            def,
            self.image.clone(),
            vec![cmd.0.clone()],
            Vec::new(),
            self.secret_refs.clone(),
            self.egress.clone(),
            self.limits,
            self.trust_tier,
            self.run_token.clone(),
            self.meter_to.clone(),
            self.idem_token.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecError<E> {
    Launch(SandboxLaunchError<E>),
    Kill(E),
    SettlementOwnerNotHook,
}

impl<E: std::fmt::Display> std::fmt::Display for ExecError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::Launch(e) => write!(f, "sandbox launch refused: {e}"),
            ExecError::Kill(e) => write!(f, "sandbox teardown (whole-guest kill) failed: {e}"),
            ExecError::SettlementOwnerNotHook => write!(
                f,
                "direct sandbox dispatch requires Hook-owned completion settlement (no terminal \
                 reporter exists on this path to route a retryable attempt through)"
            ),
        }
    }
}

impl<E: std::error::Error> std::error::Error for ExecError<E> {}

impl<B: SandboxBackend> ToolHands for SandboxToolHands<'_, B> {
    fn exec(&self, cmd: Command) -> ToolResult {
        let def = compute_tool_def();
        match self.build_compute_job(&def, &cmd) {
            Ok(job) => match self.dispatch_compute(&job) {
                Ok(res) => res,
                Err(e) => ToolResult::Refused {
                    refused: format!("exec-refused:{e}"),
                },
            },
            Err(e) => ToolResult::Refused {
                refused: format!("exec-rejected:{e}"),
            },
        }
    }
}

pub fn compute_tool_def() -> ToolDef {
    ToolDef {
        name: myelin_agent::ToolName("agent.exec".into()),
        subsystem: "agent".into(),
        version: 1,
        input_schema: "{}".into(),
        required_caps: Vec::new(),
        effect_kind: EffectKind::Compute,
        side_effecting: false,
        requires_approval: false,
        exposed_over_mcp: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::escape_gate::ProductionBackendId;
    use myelin_agent::ToolName;
    use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
    use myelin_ci_sandbox::{
        parse_console, Backend, BackendRun, EscapeAttestation, HookError, ReserveHandle,
        ResourceUsage, SandboxHandle, SandboxLaunch, SandboxResult, CORPUS, CORPUS_VERSION,
    };
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    fn pinned() -> ImageRef {
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef000000000000000000000000000000000000000000000000").unwrap()
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

    fn credential(jti: &str) -> RunTokenCredential {
        RunTokenCredential::new(format!("test-bearer:{jti}"), jti, 300).unwrap()
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

    #[test]
    fn route_of_maps_each_effect_kind_to_its_route() {
        assert_eq!(route_of(EffectKind::Read), ToolRoute::Direct);
        assert_eq!(route_of(EffectKind::Compute), ToolRoute::Sandbox);
        assert_eq!(route_of(EffectKind::Mutate), ToolRoute::EffectApi);
        assert_eq!(route_of(EffectKind::External), ToolRoute::EffectApi);
    }

    #[test]
    fn only_compute_reaches_the_sandbox() {
        let to_sandbox: Vec<EffectKind> = [
            EffectKind::Read,
            EffectKind::Compute,
            EffectKind::Mutate,
            EffectKind::External,
        ]
        .into_iter()
        .filter(|k| route_of(*k) == ToolRoute::Sandbox)
        .collect();
        assert_eq!(to_sandbox, vec![EffectKind::Compute]);
    }

    #[test]
    fn a_mutate_effect_can_never_reach_exec() {
        let mutate = def("issue.create", EffectKind::Mutate, true);
        let r = SandboxJob::for_compute(
            &mutate,
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
    fn external_and_read_effects_also_cannot_reach_exec() {
        for (name, kind, se, route) in [
            (
                "ci.deploy",
                EffectKind::External,
                true,
                ToolRoute::EffectApi,
            ),
            ("issue.read", EffectKind::Read, false, ToolRoute::Direct),
        ] {
            let d = def(name, kind, se);
            let r = SandboxJob::for_compute(
                &d,
                pinned(),
                vec!["sh".into()],
                vec![],
                vec![],
                EgressPolicy::deny_all(),
                limits(),
                TrustTier::Trusted,
                credential("j"),
                MeterTarget {
                    reserve_id: "r".into(),
                },
                IdemToken("i".into()),
            );
            assert_eq!(
                r.unwrap_err(),
                RoutingError::NotComputeBound {
                    tool: name.into(),
                    actual_route: route,
                }
            );
        }
    }

    #[test]
    fn a_compute_effect_builds_a_kind_agent_job() {
        let compute = def("agent.run_tests", EffectKind::Compute, false);
        let job = SandboxJob::for_compute(
            &compute,
            pinned(),
            vec!["cargo".into(), "test".into()],
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            limits(),
            TrustTier::UntrustedFork,
            credential("agent-jti"),
            MeterTarget {
                reserve_id: "agent-res".into(),
            },
            IdemToken("agent-idem".into()),
        )
        .unwrap();
        assert_eq!(job.spec().kind, JobKind::Agent);
    }

    #[test]
    fn the_kind_agent_job_carries_the_full_hardening_profile() {
        let compute = def("agent.lint", EffectKind::Compute, false);
        let job = SandboxJob::for_compute(
            &compute,
            pinned(),
            vec!["ruff".into()],
            vec![],
            vec![SecretRef {
                name: "NPM_TOKEN".into(),
                handle: "broker://job/npm".into(),
            }],
            EgressPolicy::deny_all(),
            limits(),
            TrustTier::UntrustedFork,
            credential("jti"),
            MeterTarget {
                reserve_id: "res".into(),
            },
            IdemToken("idem".into()),
        )
        .unwrap();
        let spec = job.spec();
        assert_eq!(spec.kind, JobKind::Agent);
        assert!(spec.image.digest_pinned());
        assert!(spec.egress.allow.is_empty());
        assert!(spec.limits.pids_max > 0);
        assert!(spec.limits.timeout_secs > 0);
        assert_eq!(spec.workspace, WorkspaceSpec::default());
        assert_eq!(spec.secret_refs.len(), 1);
        assert_eq!(spec.secret_refs[0].name, "NPM_TOKEN");
        assert_eq!(spec.run_token.jti, "jti");
        assert_eq!(spec.meter_to.reserve_id, "res");
    }

    #[test]
    fn an_undigested_image_fails_closed() {
        let compute = def("agent.run", EffectKind::Compute, false);
        let r = SandboxJob::for_compute(
            &compute,
            ImageRef {
                reference: "registry/runner:latest".into(),
            },
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
        assert!(matches!(
            r.unwrap_err(),
            RoutingError::SpecRejected(SpecError::UndigestedImage { .. })
        ));
    }

    #[test]
    fn zero_pids_max_and_zero_timeout_fail_closed() {
        let compute = def("agent.run", EffectKind::Compute, false);
        let mut l = limits();
        l.pids_max = 0;
        let r = SandboxJob::for_compute(
            &compute,
            pinned(),
            vec!["sh".into()],
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            l,
            TrustTier::UntrustedFork,
            credential("j"),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        );
        assert!(matches!(
            r.unwrap_err(),
            RoutingError::SpecRejected(SpecError::NoPidsMax)
        ));
    }

    #[test]
    fn the_shared_platform_token_is_scrubbed_from_the_child_env() {
        let compute = def("agent.run", EffectKind::Compute, false);
        let env = vec![
            EnvVar {
                name: PLATFORM_TOKEN_ENV.into(),
                value: "super-secret-platform-token".into(),
            },
            EnvVar {
                name: "LANG".into(),
                value: "C".into(),
            },
        ];
        let job = SandboxJob::for_compute(
            &compute,
            pinned(),
            vec!["sh".into()],
            env,
            vec![],
            EgressPolicy::deny_all(),
            limits(),
            TrustTier::UntrustedFork,
            credential("per-run"),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        )
        .unwrap();
        assert!(job
            .scrubbed_env()
            .iter()
            .all(|e| e.name != PLATFORM_TOKEN_ENV));
        assert!(job.scrubbed_env().iter().any(|e| e.name == "LANG"));
        assert_eq!(job.spec().run_token.jti, "per-run");
    }

    #[test]
    fn scrub_is_idempotent() {
        let once = SandboxJob::scrub_platform_token(vec![EnvVar {
            name: PLATFORM_TOKEN_ENV.into(),
            value: "x".into(),
        }]);
        assert!(once.is_empty());
        let twice = SandboxJob::scrub_platform_token(once);
        assert!(twice.is_empty());
    }

    struct RecordingBackend {
        order: Arc<Mutex<Vec<&'static str>>>,
        kills: Arc<AtomicU32>,
    }

    impl SandboxBackend for RecordingBackend {
        type Error = HookError;
        fn launch(
            &self,
            spec: &JobSpec,
            hooks: &RunnerHooks,
        ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
            (|| -> Result<SandboxLaunch, HookError> {
                hooks.enforce_isolation_floor(spec)?;
                self.order.lock().unwrap().push("isolation_floor");
                let res = hooks.reserve(spec)?;
                self.order.lock().unwrap().push("reserve");
                if let Err(error) = hooks.attribute(spec) {
                    hooks.release_unused(spec, &res)?;
                    return Err(error);
                }
                self.order.lock().unwrap().push("attribute");
                let result = SandboxResult::stub_ok(ResourceUsage {
                    cpu_seconds: 1,
                    mem_byte_seconds: 1,
                });
                hooks.settle_completed(spec, &res, result.usage)?;
                self.order.lock().unwrap().push("settle");
                Ok(SandboxLaunch {
                    handle: SandboxHandle {
                        guest_id: "agent-guest".into(),
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
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        )
    }

    fn hands<'a>(
        backend: &'a RecordingBackend,
        hooks: RunnerHooks,
    ) -> SandboxToolHands<'a, RecordingBackend> {
        SandboxToolHands::new(
            green_gate(),
            backend,
            hooks,
            pinned(),
            credential("agent-jti"),
            MeterTarget {
                reserve_id: "agent-res".into(),
            },
            IdemToken("agent-idem".into()),
            TrustTier::UntrustedFork,
            limits(),
            EgressPolicy::deny_all(),
            Vec::new(),
        )
    }

    #[test]
    fn exec_dispatches_a_kind_agent_job_driving_the_four_guarantees_in_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let kills = Arc::new(AtomicU32::new(0));
        let backend = RecordingBackend {
            order: order.clone(),
            kills: kills.clone(),
        };
        let hands = hands(&backend, working_hooks());
        let out = hands.exec(Command("cargo test".into()));
        assert_eq!(out, ToolResult::Succeeded("sandbox:agent-guest".into()));
        assert_eq!(
            *order.lock().unwrap(),
            vec!["isolation_floor", "reserve", "attribute", "settle"]
        );
        assert_eq!(kills.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn exec_refuses_to_start_on_an_exhausted_wallet_never_running_the_guest() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let backend = RecordingBackend {
            order: order.clone(),
            kills: Arc::new(AtomicU32::new(0)),
        };
        let hooks = RunnerHooks::new(
            myelin_ci_sandbox::CompletionSettlementOwner::Hook,
            Box::new(|_spec| Err(HookError("wallet exhausted - refuse to start".into()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let hands = hands(&backend, hooks);
        let out = hands.exec(Command("cargo test".into()));
        assert!(
            out.is_refused() && out.content().starts_with("exec-refused:"),
            "a refused dispatch surfaces LOUD: {out:?}"
        );
        assert!(!order.lock().unwrap().contains(&"settle"));
    }

    #[test]
    fn exec_fails_closed_when_the_isolation_floor_is_not_met() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let backend = RecordingBackend {
            order: order.clone(),
            kills: Arc::new(AtomicU32::new(0)),
        };
        let hooks = RunnerHooks::new(
            myelin_ci_sandbox::CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Err(HookError("hardening profile not met".into()))),
        );
        let hands = hands(&backend, hooks);
        let out = hands.exec(Command("sh".into()));
        assert!(
            out.is_refused() && out.content().starts_with("exec-refused:"),
            "{out:?}"
        );
        assert!(
            order.lock().unwrap().is_empty(),
            "no guarantee fired past the isolation floor"
        );
    }

    #[test]
    fn dispatch_compute_surfaces_a_typed_launch_error() {
        let backend = RecordingBackend {
            order: Arc::new(Mutex::new(Vec::new())),
            kills: Arc::new(AtomicU32::new(0)),
        };
        let hooks = RunnerHooks::new(
            myelin_ci_sandbox::CompletionSettlementOwner::Hook,
            Box::new(|_spec| Err(HookError("exhausted".into()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let hands = hands(&backend, hooks);
        let job = hands
            .build_compute_job(&compute_tool_def(), &Command("sh".into()))
            .unwrap();
        let err = hands.dispatch_compute(&job).unwrap_err();
        assert!(matches!(
            err,
            ExecError::Launch(SandboxLaunchError::Failed(HookError(_)))
        ));
    }

    #[test]
    fn dispatch_compute_refuses_reporter_owned_hooks_before_reserve_or_launch() {
        let reserve_called = Arc::new(AtomicU32::new(0));
        let reserve_called_at = reserve_called.clone();
        let backend = RecordingBackend {
            order: Arc::new(Mutex::new(Vec::new())),
            kills: Arc::new(AtomicU32::new(0)),
        };
        let hooks = RunnerHooks::new(
            myelin_ci_sandbox::CompletionSettlementOwner::TerminalReporter,
            Box::new(move |spec| {
                reserve_called_at.fetch_add(1, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let hands = hands(&backend, hooks);
        let job = hands
            .build_compute_job(&compute_tool_def(), &Command("sh".into()))
            .unwrap();
        let err = hands.dispatch_compute(&job).unwrap_err();
        assert!(matches!(err, ExecError::SettlementOwnerNotHook));
        assert_eq!(
            reserve_called.load(Ordering::SeqCst),
            0,
            "reporter-owned hooks must refuse before reserve is ever called"
        );
        assert!(
            backend.order.lock().unwrap().is_empty(),
            "the backend's launch must never be driven under reporter-owned hooks here"
        );
    }


    #[test]
    fn the_exec_hands_carry_a_green_ag_d4_gate_by_construction() {
        let backend = RecordingBackend {
            order: Arc::new(Mutex::new(Vec::new())),
            kills: Arc::new(AtomicU32::new(0)),
        };
        let hands = hands(&backend, working_hooks());
        assert_eq!(
            hands.gate().backend_id().backend,
            Backend::FirecrackerMicrovm
        );
        assert!(hands.gate().open_line().starts_with("[AG-D4 GATE OPEN]"));
    }

    #[test]
    fn without_a_green_attestation_no_gate_can_be_built_so_no_hands_dispatch() {
        let id = ProductionBackendId {
            backend: Backend::FirecrackerMicrovm,
            rootfs_sha256: "rootfs-digest".into(),
            kernel_sha256: "kernel-digest".into(),
            corpus_version: CORPUS_VERSION,
        };
        assert!(
            AgentExecGate::admit(None, &id).is_err(),
            "no green attestation ⇒ no AgentExecGate ⇒ no SandboxToolHands ⇒ no untrusted compute"
        );
    }

    #[test]
    fn routing_error_display_is_loud_and_self_describing() {
        let e = RoutingError::NotComputeBound {
            tool: "issue.create".into(),
            actual_route: ToolRoute::EffectApi,
        };
        let s = e.to_string();
        assert!(s.contains("issue.create"));
        assert!(s.contains("routing split"));
    }
}
