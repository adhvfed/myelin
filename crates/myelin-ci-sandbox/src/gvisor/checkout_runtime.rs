use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(all(test, feature = "test-support"))]
use super::RuntimePreparation;
use super::{
    acquire_enabled_workspace, checkout_cleanup_plan, execute_cleanup_plan,
    resolve_checkout_preparation_permit, run_checkout_preparation_inner,
    settle_enabled_finalization, unique_suffix, AcquisitionFailure, BoundWorkloadRefusal,
    CheckoutPreparationError, CheckoutPreparationSpec, ContainerRun, EnabledLaunchContext,
    EnabledWorkspaceRequest, LeaseBindState, OciConfig, PreparedCheckoutEvidence,
    RealCheckoutCleanupExecutor, RetainedWorkloadOutcome, RunFailure, RuntimeFinalization,
    WorkloadRotatedSpec, WorkspaceProcessIdentity,
};
#[cfg(any(test, feature = "test-support"))]
use super::{
    bind_prepared_lease_given, finalized_for_test_support, CgroupQuiescenceEvidence,
    RuntimeNamespaceQuiescence, RuntimeQuiescenceEvidence, SubstitutedEvidenceMode,
};
use crate::checkout_orchestration::AttemptAuthority;
use crate::hardening::HardeningProfile;
use crate::runner::PreparationPhase;
use crate::user_namespace::{
    CheckoutPreparationSession, UserNamespaceAllocator, UserNamespaceLease,
};
use crate::workspace_manager::WorkspaceManager;
#[cfg(all(test, feature = "test-support"))]
use crate::LaunchPermit;
use crate::{
    CheckoutAuthorizationScope, JobSpec, PhaseAuthorization, RunTokenCredential, RunnerHooks,
    SandboxCancellation, SandboxOutputSink,
};

pub(super) struct AcquiredCheckoutRuntime {
    workload_container_id: String,
    checkout_scope: CheckoutAuthorizationScope,
    process_identity: WorkspaceProcessIdentity,
    enabled_context: EnabledLaunchContext,
    session: CheckoutPreparationSession,
    workload_cfg: OciConfig,
}

pub(super) struct PreparedCheckoutRuntime {
    acquired: AcquiredCheckoutRuntime,
    prepared_checkout_evidence: PreparedCheckoutEvidence,
}

impl AcquiredCheckoutRuntime {
    pub(super) fn acquire(
        spec: &JobSpec,
        profile: &HardeningProfile,
        absolute_rootfs: PathBuf,
        workspace_manager: &WorkspaceManager,
        userns_allocator: &UserNamespaceAllocator,
        process_identity: WorkspaceProcessIdentity,
        cargo_vendor: Option<crate::asset_registry::VerifiedCargoVendor>,
    ) -> Result<AcquiredCheckoutRuntime, AcquisitionFailure> {
        let checkout_scope =
            match crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace) {
                Ok(Some(scope)) => scope,
                Ok(None) => {
                    return Err(AcquisitionFailure::clean(
                        "AcquiredCheckoutRuntime::acquire called for a non-checkout job - its \
                         workspace names neither repo_ref nor commit"
                            .to_string(),
                    ))
                }
                Err(reason) => {
                    return Err(AcquisitionFailure::clean(format!(
                    "deriving the checkout authorization scope from the job spec failed: {reason}"
                )))
                }
            };
        let workload_container_id =
            format!("myelin-prod-{}-{}", std::process::id(), unique_suffix());
        let (workload_cfg, enabled_context) = acquire_enabled_workspace(
            EnabledWorkspaceRequest::new(
                spec,
                profile,
                &workload_container_id,
                absolute_rootfs,
                process_identity,
            )
            .with_optional_cargo_vendor(cargo_vendor),
            workspace_manager,
            userns_allocator,
        )?;
        let runtime = AcquiredCheckoutRuntime {
            workload_container_id,
            checkout_scope,
            process_identity,
            enabled_context,
            session: CheckoutPreparationSession::new(),
            workload_cfg,
        };
        if runtime.enabled_context.workspace.job_key() != runtime.workload_container_id {
            let observed = runtime.enabled_context.workspace.job_key().to_string();
            let expected = runtime.workload_container_id.clone();
            let diagnostics = runtime.dispose_checkout_runtime(workspace_manager);
            return Err(AcquisitionFailure::from_rollback_diagnostics(
                format!(
                    "acquired workspace job_key {observed:?} does not equal the stable workload \
                     container id {expected:?} - the just-acquired workspace and lease were disposed"
                ),
                diagnostics,
            ));
        }
        Ok(runtime)
    }

    pub(super) fn dispose_checkout_runtime(
        self,
        workspace_manager: &WorkspaceManager,
    ) -> Vec<String> {
        let plan = checkout_cleanup_plan(self.session.cleanup_disposition());
        let EnabledLaunchContext {
            workspace,
            lease,
            bind_state: _,
        } = self.enabled_context;
        let mut executor = RealCheckoutCleanupExecutor {
            workspace: Some(workspace),
            lease: Some(lease),
            checkout_session: Some(self.session),
            workspace_manager,
        };
        execute_cleanup_plan(plan, &mut executor)
    }
}

impl PreparedCheckoutRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_retained_workload(
        self,
        authority: &dyn AttemptAuthority,
        hooks: &RunnerHooks,
        spec: &JobSpec,
        workspace_manager: &WorkspaceManager,
        rootfs: &Path,
        cancellation: SandboxCancellation,
        output: Option<Arc<dyn SandboxOutputSink>>,
    ) -> RetainedWorkloadOutcome {
        self.run_retained_workload_inner(
            authority,
            spec,
            workspace_manager,
            move |workload_spec, cfg, container_id, lease, session, bind_state| {
                workload_spec.acquire_permit_and_run(
                    hooks,
                    cfg,
                    container_id,
                    rootfs,
                    lease,
                    session,
                    bind_state,
                    output,
                    cancellation,
                )
            },
        )
    }

    #[allow(clippy::too_many_arguments, private_interfaces, private_bounds)]
    fn run_retained_workload_inner<R>(
        mut self,
        authority: &dyn AttemptAuthority,
        base_spec: &JobSpec,
        workspace_manager: &WorkspaceManager,
        run_workload: R,
    ) -> RetainedWorkloadOutcome
    where
        R: FnOnce(
            &WorkloadRotatedSpec,
            &OciConfig,
            &str,
            &mut UserNamespaceLease,
            &mut CheckoutPreparationSession,
            &mut LeaseBindState,
        ) -> Result<
            Result<RuntimeFinalization<Result<ContainerRun, RunFailure>>, RunFailure>,
            BoundWorkloadRefusal,
        >,
    {
        if let Err(message) = self
            .acquired
            .workload_cfg
            .bind_materialized_cargo_lock(self.prepared_checkout_evidence.cargo_lock_sha256_hex())
        {
            let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
            return RetainedWorkloadOutcome::RunFailed {
                failure: RunFailure::uncommitted(message),
                disposal_diagnostics,
            };
        }
        let materialization_usage = self.prepared_checkout_evidence.preparation_usage();
        if let Err(error) = authority.complete_phase(
            PreparationPhase::CheckoutMaterialization,
            materialization_usage,
        ) {
            let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
            return RetainedWorkloadOutcome::PhaseAuthorityFailed {
                error,
                disposal_diagnostics,
            };
        }
        if let Err(lost) = authority.renew_preparation_lease() {
            let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
            return RetainedWorkloadOutcome::LeaseLost {
                lost,
                disposal_diagnostics,
            };
        }
        let workload_carrier = match authority.mint_workload_credential() {
            Ok(carrier) => carrier,
            Err(error) => {
                let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
                return RetainedWorkloadOutcome::PhaseAuthorityFailed {
                    error,
                    disposal_diagnostics,
                };
            }
        };
        let workload_spec = WorkloadRotatedSpec::from_carrier(&workload_carrier, base_spec);
        let outer_result = match run_workload(
            &workload_spec,
            &self.acquired.workload_cfg,
            &self.acquired.workload_container_id,
            &mut self.acquired.enabled_context.lease,
            &mut self.acquired.session,
            &mut self.acquired.enabled_context.bind_state,
        ) {
            Ok(outer_result) => outer_result,
            Err(BoundWorkloadRefusal::PermitRefused(message)) => {
                let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
                return RetainedWorkloadOutcome::PermitRefused {
                    message,
                    disposal_diagnostics,
                };
            }
            Err(BoundWorkloadRefusal::PrepModeMismatch(message)) => {
                let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
                return RetainedWorkloadOutcome::RunFailed {
                    failure: RunFailure::uncommitted(message),
                    disposal_diagnostics,
                };
            }
        };
        match outer_result {
            Err(failure) => {
                let disposal_diagnostics = self.dispose_checkout_runtime(workspace_manager);
                RetainedWorkloadOutcome::RunFailed {
                    failure,
                    disposal_diagnostics,
                }
            }
            Ok(finalization) => {
                let PreparedCheckoutRuntime { acquired, .. } = self;
                let AcquiredCheckoutRuntime {
                    enabled_context, ..
                } = acquired;
                let settled = settle_enabled_finalization(
                    finalization,
                    Some(enabled_context),
                    Some(workspace_manager),
                );
                RetainedWorkloadOutcome::Ran(settled)
            }
        }
    }

    pub(super) fn dispose_checkout_runtime(
        self,
        workspace_manager: &WorkspaceManager,
    ) -> Vec<String> {
        self.acquired.dispose_checkout_runtime(workspace_manager)
    }
}

#[allow(clippy::result_large_err, clippy::too_many_arguments)]
pub(super) fn run_checkout_preparation_v2(
    mut runtime: AcquiredCheckoutRuntime,
    spec: CheckoutPreparationSpec,
    run_token: RunTokenCredential,
    authorization: PhaseAuthorization,
    cancellation: &std::sync::atomic::AtomicBool,
    output: Option<Arc<dyn SandboxOutputSink>>,
) -> Result<PreparedCheckoutRuntime, (AcquiredCheckoutRuntime, CheckoutPreparationError)> {
    let permit = match resolve_checkout_preparation_permit(
        authorization,
        &run_token,
        &runtime.checkout_scope,
        &spec.expected_commit,
    ) {
        Ok(permit) => permit,
        Err(error) => return Err((runtime, error)),
    };
    let outcome = run_checkout_preparation_inner(
        &mut runtime.enabled_context.lease,
        &mut runtime.session,
        &runtime.enabled_context.workspace,
        runtime.process_identity,
        runtime.workload_cfg.has_cargo_vendor(),
        spec,
        permit,
        cancellation,
        output,
    );
    match outcome {
        Ok(evidence) => Ok(PreparedCheckoutRuntime {
            acquired: runtime,
            prepared_checkout_evidence: evidence,
        }),
        Err(error) => Err((runtime, error)),
    }
}

#[cfg(all(test, feature = "test-support"))]
impl PreparedCheckoutRuntime {
    #[allow(clippy::too_many_arguments, private_interfaces, private_bounds)]
    pub(crate) fn run_retained_workload_given<F>(
        self,
        authority: &dyn AttemptAuthority,
        hooks: &RunnerHooks,
        spec: &JobSpec,
        workspace_manager: &WorkspaceManager,
        rootfs: &Path,
        execute: F,
    ) -> RetainedWorkloadOutcome
    where
        F: FnOnce(
            &JobSpec,
            &OciConfig,
            LaunchPermit,
            &Path,
            &str,
            RuntimePreparation<'_>,
        )
            -> Result<RuntimeFinalization<Result<ContainerRun, RunFailure>>, RunFailure>,
    {
        self.run_retained_workload_inner(
            authority,
            spec,
            workspace_manager,
            move |workload_spec, cfg, container_id, lease, session, bind_state| {
                workload_spec.acquire_permit_and_run_given(
                    hooks,
                    cfg,
                    container_id,
                    rootfs,
                    lease,
                    session,
                    bind_state,
                    execute,
                )
            },
        )
    }
}

#[cfg(all(test, feature = "test-support"))]
impl AcquiredCheckoutRuntime {
    pub(crate) fn drive_session_for_tests(
        &mut self,
        target: crate::user_namespace::CheckoutSessionCleanup,
    ) {
        use crate::user_namespace::{CheckoutSessionCleanup, PreparationQuiescenceProof};
        let container = "myelin-checkout-drive".to_string();
        let root = (11_u64, 22_u64);
        let cgroup = (33_u64, 44_u64);
        if target == CheckoutSessionCleanup::NeverBound {
            return;
        }
        self.session
            .bind_preparation(
                &mut self.enabled_context.lease,
                container.clone(),
                root,
                cgroup,
            )
            .expect("bind_preparation must succeed on a fresh Allocated lease");
        if target == CheckoutSessionCleanup::TeardownUnproven {
            return;
        }
        let nonce = self.enabled_context.lease.nonce_for_tests();
        let proof =
            PreparationQuiescenceProof::assert_for_tests(nonce, container.clone(), root, cgroup);
        self.session
            .confirm_prepared(&mut self.enabled_context.lease, proof)
            .expect("confirm_prepared must succeed with a matching proof");
        if target == CheckoutSessionCleanup::Prepared {
            return;
        }
        if target == CheckoutSessionCleanup::WorkloadBound {
            self.session
                .bind_workload(
                    &mut self.enabled_context.lease,
                    "myelin-prod-workload".to_string(),
                    root,
                    cgroup,
                )
                .expect("bind_workload must succeed from Prepared");
        }
    }

    pub(crate) fn into_prepared_for_tests(
        mut self,
        evidence: PreparedCheckoutEvidence,
    ) -> PreparedCheckoutRuntime {
        use crate::user_namespace::CheckoutSessionCleanup;
        self.drive_session_for_tests(CheckoutSessionCleanup::Prepared);
        let disposition = self.session.cleanup_disposition();
        assert_eq!(
            disposition,
            CheckoutSessionCleanup::Prepared,
            "into_prepared_for_tests must leave the session durably Prepared"
        );
        PreparedCheckoutRuntime {
            acquired: self,
            prepared_checkout_evidence: evidence,
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl AcquiredCheckoutRuntime {
    #[allow(clippy::result_large_err)]
    pub(crate) fn substituted_hop_b_for_test_support(
        mut self,
        run_token: &RunTokenCredential,
        authorization: PhaseAuthorization,
        expected_commit: &crate::workspace_intent::ExpectedGitCommitId,
        sentinel_name: &str,
        sentinel_bytes: &[u8],
        injected_disposition: Option<crate::runner::PreparationAttemptDisposition>,
    ) -> Result<
        (PreparedCheckoutRuntime, bool, u64),
        (AcquiredCheckoutRuntime, CheckoutPreparationError),
    > {
        use crate::user_namespace::PreparationQuiescenceProof;

        let permit = match resolve_checkout_preparation_permit(
            authorization,
            run_token,
            &self.checkout_scope,
            expected_commit,
        ) {
            Ok(permit) => permit,
            Err(error) => return Err((self, error)),
        };

        let prep_container = format!("myelin-checkout-substituted-{}", self.workload_container_id);
        let prep_root = (0x0011_u64, 0x0022_u64);
        let prep_cgroup = (0x0033_u64, 0x0044_u64);

        self.session
            .bind_preparation(
                &mut self.enabled_context.lease,
                prep_container.clone(),
                prep_root,
                prep_cgroup,
            )
            .expect("bind_preparation must succeed on a fresh Allocated lease");

        if let Err(error) = permit.commit_and_release() {
            let prep_evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                prep_container.clone(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: prep_root,
                },
                CgroupQuiescenceEvidence::assert_for_tests(prep_cgroup),
            );
            let prep_proof = PreparationQuiescenceProof::from_runtime_evidence(
                &self.enabled_context.lease,
                &prep_evidence,
            )
            .expect("a matching preparation evidence mints a proof");
            self.session
                .confirm_prepared(&mut self.enabled_context.lease, prep_proof)
                .expect("confirm_prepared with matching evidence must succeed");
            return Err((
                self,
                CheckoutPreparationError::RejectedAfterQuiescence {
                    message: format!("materialization launch permit commit failed: {error}"),
                    usage: crate::ResourceUsage {
                        cpu_seconds: 0,
                        mem_byte_seconds: 0,
                    },
                    disposition:
                        crate::runner::PreparationAttemptDisposition::RetryableInfrastructure {
                            phase: crate::runner::PreparationPhase::CheckoutMaterialization,
                        },
                },
            ));
        }

        let hopb_write_ok = injected_disposition.is_none()
            && self
                .enabled_context
                .workspace
                .checked_test_quota_write(sentinel_name, sentinel_bytes)
                .is_ok();
        let used_after_hopb = self
            .enabled_context
            .workspace
            .scan_used_bytes()
            .unwrap_or(0);

        let prep_evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            prep_container.clone(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity: prep_root,
            },
            CgroupQuiescenceEvidence::assert_for_tests(prep_cgroup),
        );
        let prep_proof = PreparationQuiescenceProof::from_runtime_evidence(
            &self.enabled_context.lease,
            &prep_evidence,
        )
        .expect("a matching preparation evidence mints a proof");
        self.session
            .confirm_prepared(&mut self.enabled_context.lease, prep_proof)
            .expect("confirm_prepared with a matching proof must succeed");

        if let Some(disposition) = injected_disposition {
            return Err((
                self,
                CheckoutPreparationError::RejectedAfterQuiescence {
                    message: "6e.2 injected Hop-B failure (test-support)".to_string(),
                    usage: crate::ResourceUsage {
                        cpu_seconds: 2,
                        mem_byte_seconds: 5,
                    },
                    disposition,
                },
            ));
        }

        let prepared = PreparedCheckoutRuntime {
            acquired: self,
            prepared_checkout_evidence: PreparedCheckoutEvidence::for_tests(crate::ResourceUsage {
                cpu_seconds: 2,
                mem_byte_seconds: 5,
            }),
        };
        Ok((prepared, hopb_write_ok, used_after_hopb))
    }
}

#[cfg(any(test, feature = "test-support"))]
impl PreparedCheckoutRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn substituted_workload_for_test_support(
        self,
        authority: &dyn AttemptAuthority,
        hooks: &RunnerHooks,
        base_spec: &JobSpec,
        workspace_manager: &WorkspaceManager,
        sentinel_name: &str,
        sentinel_bytes: &[u8],
        evidence_mode: SubstitutedEvidenceMode,
    ) -> (RetainedWorkloadOutcome, u64, bool, bool) {
        let workspace_host = self
            .acquired
            .enabled_context
            .workspace
            .host_path()
            .unwrap()
            .to_path_buf();
        let used_at_workload_checkpoint = self
            .acquired
            .enabled_context
            .workspace
            .scan_used_bytes()
            .unwrap_or(0);
        let mount_source_matched_workspace = std::cell::Cell::new(false);
        let sentinel_read_through_mount = std::cell::Cell::new(false);

        let outcome = self.run_retained_workload_inner(
            authority,
            base_spec,
            workspace_manager,
            |workload_spec, cfg, container_id, lease, session, bind_state| {
                let permit = match workload_spec.acquire_launch_permit_for_test_support(hooks) {
                    Ok(permit) => permit,
                    Err(refusal) => return Err(refusal),
                };
                let synth_root = (0x0111_u64, 0x0222_u64);
                let synth_cgroup = (0x0333_u64, 0x0444_u64);

                let mount_source = cfg.workspace_host_source_for_tests().map(Path::to_path_buf);
                mount_source_matched_workspace
                    .set(mount_source.as_deref() == Some(workspace_host.as_path()));
                sentinel_read_through_mount.set(match &mount_source {
                    Some(src) => std::fs::read(src.join(sentinel_name))
                        .map(|bytes| bytes == sentinel_bytes)
                        .unwrap_or(false),
                    None => false,
                });

                if let Err(message) = bind_prepared_lease_given(
                    lease,
                    session,
                    bind_state,
                    synth_root,
                    container_id,
                    synth_cgroup,
                    || Ok(synth_root),
                ) {
                    return Ok(Err(RunFailure::uncommitted(message)));
                }
                let (bound_container, bound_root, bound_cgroup) = match &*bind_state {
                    LeaseBindState::Bound {
                        container_id,
                        runsc_root_identity,
                        cgroup_identity,
                    } => (container_id.clone(), *runsc_root_identity, *cgroup_identity),
                    other => {
                        return Ok(Err(RunFailure::uncommitted(format!(
                            "substituted workload bind did not reach Bound: {other:?}"
                        ))))
                    }
                };
                let evidence_root = match evidence_mode {
                    SubstitutedEvidenceMode::DerivedFromBind => bound_root,
                    #[cfg(test)]
                    SubstitutedEvidenceMode::MismatchedRunscRoot => {
                        (bound_root.0 ^ 0xDEAD_BEEF, bound_root.1 ^ 0x0F0F)
                    }
                };
                let workload_evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                    bound_container,
                    RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                        runsc_root_identity: evidence_root,
                    },
                    CgroupQuiescenceEvidence::assert_for_tests(bound_cgroup),
                );
                if let Err(error) = permit.commit_and_release() {
                    return Ok(Err(RunFailure::uncommitted(error.to_string())));
                }
                Ok(Ok(finalized_for_test_support(workload_evidence)))
            },
        );

        (
            outcome,
            used_at_workload_checkpoint,
            mount_source_matched_workspace.get(),
            sentinel_read_through_mount.get(),
        )
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "test-support")]
    use super::{PreparedCheckoutEvidence, RetainedWorkloadOutcome, RunFailure};
    #[cfg(feature = "test-support")]
    use crate::gvisor::test_fixtures::*;
    #[cfg(feature = "test-support")]
    use crate::user_namespace::CheckoutSessionCleanup;
    #[cfg(feature = "test-support")]
    use crate::CompletionSettlementOwner;
    #[cfg(feature = "test-support")]
    use crate::ReserveHandle;
    #[cfg(feature = "test-support")]
    use crate::ResourceUsage;
    #[cfg(feature = "test-support")]
    use crate::RunnerHooks;
    #[cfg(feature = "test-support")]
    use std::sync::Arc;
    #[cfg(feature = "test-support")]
    use std::sync::Mutex;

    #[cfg(feature = "test-support")]
    #[test]
    fn into_prepared_for_tests_drives_the_real_lease_to_prepared() {
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("into-prepared")
        else {
            return;
        };
        let prepared =
            runtime.into_prepared_for_tests(PreparedCheckoutEvidence::for_tests(ResourceUsage {
                cpu_seconds: 2,
                mem_byte_seconds: 3,
            }));
        let diagnostics = prepared.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics.is_empty(),
            "a clean Prepared disposal must produce no diagnostics, got {diagnostics:?}"
        );
        assert!(workspace_manager.is_healthy());
        assert!(
            userns_allocator.lease().is_ok(),
            "release_prepared must return the slot to the pool"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn run_retained_workload_threads_the_workload_generation() {
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("workload-generation-threading")
        else {
            return;
        };
        let prepared =
            runtime.into_prepared_for_tests(PreparedCheckoutEvidence::for_tests(ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 1,
            }));
        let spec = checkout_spec();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        );
        let authority = FakeAttemptAuthority::new(true);
        let seen_workload_jti = Arc::new(Mutex::new(None::<String>));
        let seen = seen_workload_jti.clone();

        let outcome = prepared.run_retained_workload_given(
            &authority,
            &hooks,
            &spec,
            &workspace_manager,
            std::path::Path::new("/abs/staged-rootfs"),
            move |workload_spec, _cfg, permit, _rootfs, _container_id, _prep| {
                *seen.lock().unwrap() = Some(workload_spec.run_token.jti.clone());
                drop(permit);
                Err(RunFailure::uncommitted(
                    "synthetic assert-only workload executor",
                ))
            },
        );

        if seen_workload_jti.lock().unwrap().is_none() {
            let _ = outcome;
            let _ = std::fs::remove_dir_all(&workspace_base);
            let _ = std::fs::remove_dir_all(&leases_dir);
            return;
        }
        assert_eq!(
            seen_workload_jti.lock().unwrap().as_deref(),
            Some("jti-Workload"),
            "the workload must run under its OWN rotated generation spec (step 21 mint)"
        );
        assert!(matches!(outcome, RetainedWorkloadOutcome::RunFailed { .. }));
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.lease().is_ok());
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn dispose_never_bound_deletes_workspace_and_frees_the_slot() {
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("dispose-never-bound")
        else {
            return;
        };
        let diagnostics = runtime.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics.is_empty(),
            "a clean NeverBound disposal must produce no diagnostics, got {diagnostics:?}"
        );
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.is_healthy());
        assert!(
            userns_allocator.lease().is_ok(),
            "release_unused must return the slot to the pool - a fresh lease must now succeed"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn dispose_prepared_deletes_workspace_and_release_prepared_frees_the_slot() {
        let Some((mut runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("dispose-prepared")
        else {
            return;
        };
        runtime.drive_session_for_tests(CheckoutSessionCleanup::Prepared);
        let diagnostics = runtime.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics.is_empty(),
            "a clean Prepared disposal must produce no diagnostics, got {diagnostics:?}"
        );
        assert!(workspace_manager.is_healthy());
        assert!(
            userns_allocator.lease().is_ok(),
            "release_prepared must return the slot to the pool - a fresh lease must now succeed"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn dispose_teardown_unproven_quarantines_both() {
        let Some((mut runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("dispose-teardown-unproven")
        else {
            return;
        };
        runtime.drive_session_for_tests(CheckoutSessionCleanup::TeardownUnproven);
        let diagnostics = runtime.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics.iter().any(|d| d.contains("quarantined")),
            "a TeardownUnproven disposal must report quarantine, got {diagnostics:?}"
        );
        assert!(
            !workspace_manager.is_healthy(),
            "dropping the still-live workspace (never delete, never release) must poison the manager"
        );
        assert!(
            userns_allocator.lease().is_err(),
            "a quarantined slot must NOT be reissued - the pool stays exhausted"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn dispose_workload_bound_abandons_both_with_an_invariant_violation() {
        let Some((mut runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("dispose-workload-bound")
        else {
            return;
        };
        runtime.drive_session_for_tests(CheckoutSessionCleanup::WorkloadBound);
        let diagnostics = runtime.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.contains("structurally impossible")),
            "disposing a WorkloadBound capsule must surface an invariant violation, got {diagnostics:?}"
        );
        assert!(!workspace_manager.is_healthy());
        assert!(
            userns_allocator.lease().is_err(),
            "an abandoned slot must NOT be reissued"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }
}
