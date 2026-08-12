use crate::{
    JobSpec, ResourceUsage, RunnerHooks, SandboxCycleOutcome, SandboxLaunchError, SandboxResult,
};

struct FakeRunscChild;

impl super::RunscChild for FakeRunscChild {
    fn kill(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn wait(&mut self) -> Result<i32, String> {
        Ok(0)
    }
}

fn fake_workload_finalization(
    workload_usage: ResourceUsage,
) -> super::RuntimeFinalization<Result<super::ContainerRun, super::RunFailure>> {
    super::RuntimeFinalization::Finalized(super::FinalizedRun {
        primary: Ok(super::ContainerRun {
            child: Box::new(FakeRunscChild),
            bundle_dir: std::env::temp_dir().join("myelin-runsc-driver-fake-bundle-does-not-exist"),
            result: SandboxResult::stub_ok(workload_usage),
            run_error: None,
        }),
        evidence: super::RuntimeQuiescenceEvidence::assert_for_tests(
            "myelin-runsc-driver-fake".to_string(),
            super::RuntimeNamespaceQuiescence::Rootless,
            super::CgroupQuiescenceEvidence::assert_for_tests((0, 0)),
        ),
    })
}

impl super::GvisorBackend {
    pub fn drive_compute_cycle_with_substituted_runsc(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        workload_usage: ResourceUsage,
    ) -> Result<SandboxCycleOutcome, SandboxLaunchError<super::GvisorError>> {
        self.launch_compute_orchestrated_with(
            spec,
            hooks,
            move |_spec, _cfg, permit, _rootfs, _container_id, _prep| {
                permit
                    .commit_and_release()
                    .map_err(|error| super::RunFailure::uncommitted(error.to_string()))?;
                Ok(fake_workload_finalization(workload_usage))
            },
        )
    }
}

impl super::GvisorBackend {
    #[allow(clippy::unused_self)]
    pub fn drive_checkout_cycle_with_substituted_runsc(&self, root: &std::path::Path) {
        drive_checkout_cycle_with_substituted_runsc(root)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum InjectedHopBOutcome {
    Success,
    TerminalFailed,
    RetryableInfrastructure,
}

impl super::GvisorBackend {
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn drive_checkout_cycle_with_substituted_runsc_given(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        repo_root: &std::path::Path,
        sentinel_name: &str,
        sentinel_bytes: &[u8],
    ) -> (
        Result<
            crate::checkout_orchestration::CheckoutContinuationOutcome,
            crate::checkout_orchestration::CheckoutOrchestrationError,
        >,
        Vec<(String, bool)>,
    ) {
        self.drive_checkout_cycle_with_injected_hop_b(
            spec,
            hooks,
            repo_root,
            sentinel_name,
            sentinel_bytes,
            InjectedHopBOutcome::Success,
        )
    }

    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn drive_checkout_cycle_with_injected_hop_b(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        repo_root: &std::path::Path,
        sentinel_name: &str,
        sentinel_bytes: &[u8],
        hop_b: InjectedHopBOutcome,
    ) -> (
        Result<
            crate::checkout_orchestration::CheckoutContinuationOutcome,
            crate::checkout_orchestration::CheckoutOrchestrationError,
        >,
        Vec<(String, bool)>,
    ) {
        let injected_disposition = match hop_b {
            InjectedHopBOutcome::Success => None,
            InjectedHopBOutcome::TerminalFailed => {
                Some(crate::runner::PreparationAttemptDisposition::Terminal(
                    crate::runner::PreparationTerminalDisposition::Failed {
                        phase: crate::runner::PreparationPhase::CheckoutMaterialization,
                    },
                ))
            }
            InjectedHopBOutcome::RetryableInfrastructure => Some(
                crate::runner::PreparationAttemptDisposition::RetryableInfrastructure {
                    phase: crate::runner::PreparationPhase::CheckoutMaterialization,
                },
            ),
        };
        self.drive_checkout_cycle_inner(
            spec,
            hooks,
            repo_root,
            sentinel_name,
            sentinel_bytes,
            matches!(hop_b, InjectedHopBOutcome::Success),
            injected_disposition,
        )
    }

    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        clippy::too_many_arguments
    )]
    fn drive_checkout_cycle_inner(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        repo_root: &std::path::Path,
        sentinel_name: &str,
        sentinel_bytes: &[u8],
        hop_b_succeeds: bool,
        injected_disposition: Option<crate::runner::PreparationAttemptDisposition>,
    ) -> (
        Result<
            crate::checkout_orchestration::CheckoutContinuationOutcome,
            crate::checkout_orchestration::CheckoutOrchestrationError,
        >,
        Vec<(String, bool)>,
    ) {
        use super::checkout_transport_test_support::{
            advertisement_bytes, fake_git_wire_run, fetch_response_bytes, permit_recording_executor,
        };

        let commit = spec
            .workspace
            .commit
            .clone()
            .expect("a checkout spec carries a commit oid");
        let advertise = advertisement_bytes(&commit);
        let fetch = fetch_response_bytes(b"substituted-checkout-pack-payload");
        let advertise_usage = ResourceUsage {
            cpu_seconds: 3,
            mem_byte_seconds: 7,
        };
        let fetch_usage = ResourceUsage {
            cpu_seconds: 11,
            mem_byte_seconds: 13,
        };
        let (executor, seen) = permit_recording_executor(vec![
            Box::new(move || Ok((fake_git_wire_run(advertise, advertise_usage), false))),
            Box::new(move || Ok((fake_git_wire_run(fetch, fetch_usage), false))),
        ]);

        let cancellation = crate::SandboxCancellation::new();
        let result = self.launch_checkout_orchestrated_with_given(
            spec,
            hooks,
            repo_root,
            &cancellation,
            &*executor,
            |authority,
             report_claim,
             scope,
             runtime,
             preparation_spec,
             workspace_manager,
             rootfs| {
                self.launch_checkout_continuation_given(
                    spec,
                    hooks,
                    authority,
                    report_claim,
                    scope,
                    runtime,
                    preparation_spec,
                    workspace_manager,
                    rootfs,
                    |rt, prep_spec, run_token, authorization| {
                        let disposition =
                            if hop_b_succeeds {
                                None
                            } else {
                                Some(injected_disposition.expect(
                                    "a non-success Hop-B injection carries its disposition",
                                ))
                            };
                        rt.substituted_hop_b_for_test_support(
                            &run_token,
                            authorization,
                            &prep_spec.expected_commit,
                            sentinel_name,
                            sentinel_bytes,
                            disposition,
                        )
                        .map(|prepared| prepared.0)
                    },
                    |prepared, authority, hooks, spec, workspace_manager, _rootfs| {
                        prepared
                            .substituted_workload_for_test_support(
                                authority,
                                hooks,
                                spec,
                                workspace_manager,
                                sentinel_name,
                                sentinel_bytes,
                                super::SubstitutedEvidenceMode::DerivedFromBind,
                            )
                            .0
                    },
                )
            },
        );
        let recorded = seen.lock().unwrap().clone();
        (result, recorded)
    }
}

fn drive_checkout_cycle_with_substituted_runsc(root: &std::path::Path) {
    let sentinel = b"myelin-6e1b-provenance-sentinel";
    let (observation, workspace_manager, workspace_base, userns_allocator, userns_base) =
        super::run_substituted_checkout_success(root, "checkout.sentinel", sentinel);

    assert!(
        observation.hopb_write_ok,
        "the checked Hop B sentinel write must succeed"
    );
    assert!(
        observation.used_after_hopb >= sentinel.len() as u64,
        "the post-checkout usage checkpoint must include the materialized sentinel"
    );
    assert_eq!(
        observation.used_at_workload_checkpoint, observation.used_after_hopb,
        "the workload checkpoint must observe the same materialized workspace bytes"
    );
    assert!(
        observation.mount_source_matched_workspace,
        "the retained OCI mount source must equal the capsule workspace host path"
    );
    assert!(
        observation.sentinel_read_through_mount,
        "the substituted workload must read the sentinel through the OCI-recorded mount"
    );
    assert!(
        observation.settled_ok,
        "the real settle tail must succeed cleanly: {:?}",
        observation.settle_error
    );

    let residual: Vec<_> = std::fs::read_dir(&workspace_base)
        .expect("workspace base is readable")
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .collect();
    assert!(
        residual.is_empty(),
        "the workspace leaf must be gone after settle"
    );
    assert_eq!(
        workspace_manager.capacity_used_bytes(),
        0,
        "capacity must be fully released after settle"
    );
    assert!(
        workspace_manager.is_healthy(),
        "the workspace manager stays healthy"
    );
    assert!(
        userns_allocator.is_healthy(),
        "the userns allocator stays healthy"
    );
    let probe = userns_allocator
        .lease()
        .expect("the userns slot must be reusable after the lease was released");
    probe
        .release_unused()
        .expect("the probe lease releases cleanly");
    assert!(
        userns_allocator.is_healthy(),
        "the userns allocator is STILL clean after the probe"
    );

    let _ = std::fs::remove_dir_all(&workspace_base);
    let _ = std::fs::remove_dir_all(&userns_base);
}
