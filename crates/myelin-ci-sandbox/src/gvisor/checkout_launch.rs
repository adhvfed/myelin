use super::*;
use crate::hardening::HardeningProfile;
use crate::runner::{PreparationPhase, PreparationTerminalDisposition, RetryableAttemptCause};
use crate::workspace_manager::WorkspaceManager;
use crate::{
    CheckoutAuthorizationScope, HookError, JobSpec, PhaseAuthorization, ResourceUsage,
    RunTokenCredential, RunnerHooks, SandboxCancellation, SandboxHandle, SandboxLaunch,
    SandboxOutputSink,
};
use std::path::Path;
use std::sync::Arc;

struct NotStartedCapsuleGuard<'a> {
    capsule: Option<checkout_runtime::AcquiredCheckoutRuntime>,
    workspace_manager: &'a WorkspaceManager,
}

impl<'a> NotStartedCapsuleGuard<'a> {
    pub(super) fn new(
        capsule: checkout_runtime::AcquiredCheckoutRuntime,
        workspace_manager: &'a WorkspaceManager,
    ) -> Self {
        Self {
            capsule: Some(capsule),
            workspace_manager,
        }
    }

    fn disarm(mut self) -> checkout_runtime::AcquiredCheckoutRuntime {
        self.capsule
            .take()
            .expect("the guard still holds the capsule")
    }
}

impl Drop for NotStartedCapsuleGuard<'_> {
    fn drop(&mut self) {
        if let Some(capsule) = self.capsule.take() {
            let _diagnostics = capsule.dispose_checkout_runtime(self.workspace_manager);
        }
    }
}

impl GvisorBackend {
    #[allow(clippy::too_many_arguments, clippy::result_large_err)]
    fn launch_checkout_continuation(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        authority: &dyn crate::checkout_orchestration::AttemptAuthority,
        report_claim: &crate::runner::PreparationReportClaim,
        scope: &CheckoutAuthorizationScope,
        runtime: checkout_runtime::AcquiredCheckoutRuntime,
        preparation_spec: CheckoutPreparationSpec,
        workspace_manager: &WorkspaceManager,
        rootfs: &Path,
        cancellation: &SandboxCancellation,
        output: Option<Arc<dyn SandboxOutputSink>>,
    ) -> Result<
        crate::checkout_orchestration::CheckoutContinuationOutcome,
        crate::checkout_orchestration::CheckoutOrchestrationError,
    > {
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
            |runtime, prep_spec, run_token, authorization| {
                checkout_runtime::run_checkout_preparation_v2(
                    runtime,
                    prep_spec,
                    run_token,
                    authorization,
                    cancellation.as_atomic(),
                    output.clone(),
                )
            },
            |prepared, authority, hooks, spec, workspace_manager, rootfs| {
                prepared.run_retained_workload(
                    authority,
                    hooks,
                    spec,
                    workspace_manager,
                    rootfs,
                    cancellation.clone(),
                    output.clone(),
                )
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn launch_checkout_continuation_given<HopB, RunWorkload>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        authority: &dyn crate::checkout_orchestration::AttemptAuthority,
        report_claim: &crate::runner::PreparationReportClaim,
        scope: &CheckoutAuthorizationScope,
        runtime: checkout_runtime::AcquiredCheckoutRuntime,
        preparation_spec: CheckoutPreparationSpec,
        workspace_manager: &WorkspaceManager,
        rootfs: &Path,
        hop_b: HopB,
        run_workload: RunWorkload,
    ) -> Result<
        crate::checkout_orchestration::CheckoutContinuationOutcome,
        crate::checkout_orchestration::CheckoutOrchestrationError,
    >
    where
        HopB: FnOnce(
            checkout_runtime::AcquiredCheckoutRuntime,
            CheckoutPreparationSpec,
            RunTokenCredential,
            PhaseAuthorization,
        ) -> Result<
            checkout_runtime::PreparedCheckoutRuntime,
            (
                checkout_runtime::AcquiredCheckoutRuntime,
                CheckoutPreparationError,
            ),
        >,
        RunWorkload: FnOnce(
            checkout_runtime::PreparedCheckoutRuntime,
            &dyn crate::checkout_orchestration::AttemptAuthority,
            &RunnerHooks,
            &JobSpec,
            &WorkspaceManager,
            &Path,
        ) -> RetainedWorkloadOutcome,
    {
        use crate::checkout_orchestration::{
            route_after_disposal, CheckoutContinuationOutcome, CheckoutOrchestrationError,
        };
        use crate::CheckoutPhase;

        const MATERIALIZATION: PreparationPhase = PreparationPhase::CheckoutMaterialization;

        let capsule_guard = NotStartedCapsuleGuard::new(runtime, workspace_manager);

        let prepare_materialization =
            || -> Result<(RunTokenCredential, PhaseAuthorization), bool> {
                authority.begin_phase(MATERIALIZATION).map_err(|_| false)?;
                let carrier = authority
                    .mint_phase_credential(CheckoutPhase::Materialization)
                    .map_err(|_| true)?;
                crate::checkout_orchestration::authorize_phase_generation(
                    hooks,
                    spec,
                    scope,
                    CheckoutPhase::Materialization,
                    carrier,
                )
                .map_err(|_| true)
            };
        let (run_token, authorization) = match prepare_materialization() {
            Ok(pair) => pair,
            Err(phase_was_begun) => {
                return Ok(resolve_post_acquisition_authority_failure(
                    authority,
                    report_claim,
                    capsule_guard.disarm(),
                    workspace_manager,
                    phase_was_begun,
                ));
            }
        };

        let prepared = match hop_b(
            capsule_guard.disarm(),
            preparation_spec,
            run_token,
            authorization,
        ) {
            Ok(prepared) => prepared,
            Err((runtime, error)) => {
                let disposition = error.attempt_disposition();
                let usage = checkout_preparation_error_usage(&error);
                let diagnostic = checkout_preparation_error_diagnostic(&error).to_owned();
                let diagnostics = runtime.dispose_checkout_runtime(workspace_manager);
                return Ok(crate::checkout_orchestration::resolve_hop_b_failure(
                    authority,
                    report_claim,
                    disposition,
                    usage,
                    Some(diagnostic),
                    diagnostics,
                )?);
            }
        };

        let outcome = run_workload(prepared, authority, hooks, spec, workspace_manager, rootfs);
        match outcome {
            RetainedWorkloadOutcome::Ran(Ok(container_run)) => {
                let ContainerRun {
                    child,
                    bundle_dir,
                    result,
                    run_error,
                } = container_run;
                let guest_id = format!("runsc-{}", spec.idem_token.0);
                self.live
                    .lock()
                    .unwrap()
                    .insert(guest_id.clone(), RunscProc { child, bundle_dir });
                Ok(CheckoutContinuationOutcome::WorkloadLaunched(
                    SandboxLaunch {
                        handle: SandboxHandle { guest_id },
                        result,
                        output_complete: run_error.is_none(),
                    },
                ))
            }
            RetainedWorkloadOutcome::Ran(Err(run_failure)) => Ok(classify_bound_workload_failure(
                authority,
                report_claim,
                run_failure,
            )),
            RetainedWorkloadOutcome::RunFailed {
                failure,
                disposal_diagnostics,
            } => Ok(route_after_disposal(
                disposal_diagnostics,
                MATERIALIZATION,
                classify_bound_workload_failure(authority, report_claim, failure),
            )),
            RetainedWorkloadOutcome::PermitRefused {
                message,
                disposal_diagnostics,
            } => Ok(route_after_disposal(
                disposal_diagnostics,
                MATERIALIZATION,
                crate::checkout_orchestration::requeue_or_exhausted_with_diagnostic(
                    authority,
                    report_claim,
                    MATERIALIZATION,
                    Some(message),
                ),
            )),
            RetainedWorkloadOutcome::PhaseAuthorityFailed {
                error,
                disposal_diagnostics,
            } => {
                if disposal_diagnostics.is_empty() {
                    Err(CheckoutOrchestrationError::Authority(error))
                } else {
                    Ok(CheckoutContinuationOutcome::ReconciliationRequired {
                        phase: MATERIALIZATION,
                        teardown_unproven: true,
                        usage_unrepresentable: false,
                        quarantine_required: true,
                    })
                }
            }
            RetainedWorkloadOutcome::LeaseLost {
                lost,
                disposal_diagnostics,
            } => {
                if disposal_diagnostics.is_empty() {
                    Err(CheckoutOrchestrationError::LeaseLost(lost))
                } else {
                    Ok(CheckoutContinuationOutcome::ReconciliationRequired {
                        phase: MATERIALIZATION,
                        teardown_unproven: true,
                        usage_unrepresentable: false,
                        quarantine_required: true,
                    })
                }
            }
        }
    }
}

fn resolve_begun_transport_failure(
    authority: &dyn crate::checkout_orchestration::AttemptAuthority,
    report_claim: &crate::runner::PreparationReportClaim,
) -> crate::checkout_orchestration::CheckoutContinuationOutcome {
    use crate::checkout_orchestration::CheckoutContinuationOutcome;
    if authority
        .complete_phase(
            PreparationPhase::CheckoutTransport,
            ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            },
        )
        .is_err()
    {
        return CheckoutContinuationOutcome::ReconciliationRequired {
            phase: PreparationPhase::CheckoutTransport,
            teardown_unproven: false,
            usage_unrepresentable: false,
            quarantine_required: false,
        };
    }
    crate::checkout_orchestration::requeue_or_exhausted(
        authority,
        report_claim,
        PreparationPhase::CheckoutTransport,
    )
}

fn resolve_post_acquisition_authority_failure(
    authority: &dyn crate::checkout_orchestration::AttemptAuthority,
    report_claim: &crate::runner::PreparationReportClaim,
    runtime: checkout_runtime::AcquiredCheckoutRuntime,
    workspace_manager: &WorkspaceManager,
    phase_was_begun: bool,
) -> crate::checkout_orchestration::CheckoutContinuationOutcome {
    let diagnostics = runtime.dispose_checkout_runtime(workspace_manager);
    crate::checkout_orchestration::route_post_acquisition_authority_failure(
        authority,
        report_claim,
        diagnostics,
        phase_was_begun,
    )
}

fn checkout_preparation_error_usage(error: &CheckoutPreparationError) -> ResourceUsage {
    let zero = ResourceUsage {
        cpu_seconds: 0,
        mem_byte_seconds: 0,
    };
    match error {
        CheckoutPreparationError::Refused(_) => zero,
        CheckoutPreparationError::Unreleasable { usage, .. } => usage.unwrap_or(zero),
        CheckoutPreparationError::TeardownUnproven { usage, .. } => *usage,
        CheckoutPreparationError::RejectedAfterQuiescence { usage, .. } => *usage,
    }
}

fn checkout_preparation_error_diagnostic(error: &CheckoutPreparationError) -> &str {
    match error {
        CheckoutPreparationError::Refused(message)
        | CheckoutPreparationError::Unreleasable { message, .. }
        | CheckoutPreparationError::TeardownUnproven { message, .. }
        | CheckoutPreparationError::RejectedAfterQuiescence { message, .. } => message,
    }
}

fn checkout_transport_error_usage(error: &CheckoutTransportError) -> ResourceUsage {
    let zero = ResourceUsage {
        cpu_seconds: 0,
        mem_byte_seconds: 0,
    };
    match error {
        CheckoutTransportError::Refused { .. } => zero,
        CheckoutTransportError::Failed { usage, .. }
        | CheckoutTransportError::TeardownUnproven { usage, .. }
        | CheckoutTransportError::UsageUnrepresentable { usage, .. } => *usage,
    }
}

fn classify_bound_workload_failure(
    authority: &dyn crate::checkout_orchestration::AttemptAuthority,
    report_claim: &crate::runner::PreparationReportClaim,
    failure: RunFailure,
) -> crate::checkout_orchestration::CheckoutContinuationOutcome {
    use crate::checkout_orchestration::{requeue_or_exhausted, CheckoutContinuationOutcome};
    let zero = ResourceUsage {
        cpu_seconds: 0,
        mem_byte_seconds: 0,
    };
    match failure {
        RunFailure::Uncommitted { .. } => requeue_or_exhausted(
            authority,
            report_claim,
            PreparationPhase::CheckoutMaterialization,
        ),
        RunFailure::CommitOutcomeUnknown { .. } => {
            CheckoutContinuationOutcome::ReconciliationRequired {
                phase: PreparationPhase::CheckoutMaterialization,
                teardown_unproven: true,
                usage_unrepresentable: false,
                quarantine_required: false,
            }
        }
        RunFailure::Executed { usage, message } => CheckoutContinuationOutcome::WorkloadRetryable {
            cause: RetryableAttemptCause::SandboxInfrastructure,
            usage,
            message,
        },
        RunFailure::CommittedButNotExecuted { message } => {
            CheckoutContinuationOutcome::WorkloadRetryable {
                cause: RetryableAttemptCause::SandboxInfrastructure,
                usage: zero,
                message,
            }
        }
    }
}

impl GvisorBackend {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn launch_checkout_orchestrated_with(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        repo_root: &Path,
        cancellation: &SandboxCancellation,
        output: Option<Arc<dyn SandboxOutputSink>>,
    ) -> Result<
        crate::checkout_orchestration::CheckoutContinuationOutcome,
        crate::checkout_orchestration::CheckoutOrchestrationError,
    > {
        self.launch_checkout_orchestrated_with_given(
            spec,
            hooks,
            repo_root,
            cancellation,
            &|job, cfg, stdin, rootfs, cancellation, permit| {
                run_git_wire_container_raw(job, cfg, stdin, rootfs, cancellation, permit)
            },
            move |authority,
                  report_claim,
                  scope,
                  runtime,
                  preparation_spec,
                  workspace_manager,
                  rootfs| {
                self.launch_checkout_continuation(
                    spec,
                    hooks,
                    authority,
                    report_claim,
                    scope,
                    runtime,
                    preparation_spec,
                    workspace_manager,
                    rootfs,
                    cancellation,
                    output,
                )
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn launch_checkout_orchestrated_with_given<Continue>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        repo_root: &Path,
        cancellation: &SandboxCancellation,
        transport_execute: GitWireHopExecutor,
        continue_with: Continue,
    ) -> Result<
        crate::checkout_orchestration::CheckoutContinuationOutcome,
        crate::checkout_orchestration::CheckoutOrchestrationError,
    >
    where
        Continue: FnOnce(
            &dyn crate::checkout_orchestration::AttemptAuthority,
            &crate::runner::PreparationReportClaim,
            &CheckoutAuthorizationScope,
            checkout_runtime::AcquiredCheckoutRuntime,
            CheckoutPreparationSpec,
            &WorkspaceManager,
            &Path,
        ) -> Result<
            crate::checkout_orchestration::CheckoutContinuationOutcome,
            crate::checkout_orchestration::CheckoutOrchestrationError,
        >,
    {
        use crate::checkout_orchestration::{
            authorize_phase_generation, requeue_or_exhausted, route_after_disposal,
            route_preparation_disposition, AttemptAuthorityLeaseCheckpoint,
            CheckoutContinuationOutcome, CheckoutOrchestrationError, ParentAttemptAdmission,
        };
        use crate::CheckoutPhase;

        hooks
            .enforce_isolation_floor(spec)
            .map_err(CheckoutOrchestrationError::Hook)?;
        spec.validate_secret_coverage().map_err(|error| {
            CheckoutOrchestrationError::Hook(HookError(format!(
                "secret injection refused: {error}"
            )))
        })?;
        let profile = HardeningProfile::derive(spec);
        profile
            .assert_enforced()
            .map_err(|e| CheckoutOrchestrationError::Hook(HookError(e.to_string())))?;
        let registry = self.registry.as_ref().ok_or_else(|| {
            CheckoutOrchestrationError::Hook(HookError(
                "checkout orchestration requires an asset registry for the workload image"
                    .to_string(),
            ))
        })?;
        let verified_rootfs = registry
            .resolve(&spec.image)
            .map_err(|e| CheckoutOrchestrationError::Hook(HookError(e.to_string())))?;
        let checkout_guest_root = self
            .materialize_job_guest_root(
                verified_rootfs,
                &format!("checkout-{}-{}", std::process::id(), unique_suffix()),
            )
            .map_err(|e| {
                CheckoutOrchestrationError::Hook(HookError(format!("per-job rootfs overlay: {e}")))
            })?;
        let cargo_vendor = selected_cargo_vendor(spec, registry)
            .map_err(|e| CheckoutOrchestrationError::Hook(HookError(e)))?;
        let (workspace_manager, userns_allocator) = match &self.workspace_integration {
            WorkspaceIntegration::Enabled {
                workspace_manager,
                userns_allocator,
            } => {
                workspace_manager.check_health().map_err(|e| {
                    CheckoutOrchestrationError::Hook(HookError(format!(
                        "workspace manager health check failed: {e}"
                    )))
                })?;
                userns_allocator.check_identity().map_err(|e| {
                    CheckoutOrchestrationError::Hook(HookError(format!(
                        "userns allocator identity check failed: {e}"
                    )))
                })?;
                (workspace_manager, userns_allocator)
            }
            WorkspaceIntegration::Disabled => {
                return Err(CheckoutOrchestrationError::Hook(HookError(
                    "checkout orchestration requires the Enabled workspace integration".to_string(),
                )))
            }
        };

        let scope = match crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace) {
            Ok(Some(scope)) => scope,
            Ok(None) => {
                return Err(CheckoutOrchestrationError::Hook(HookError(
                    "checkout orchestration called for a non-checkout job".to_string(),
                )))
            }
            Err(reason) => {
                return Err(CheckoutOrchestrationError::Hook(HookError(format!(
                    "deriving the checkout scope failed: {reason}"
                ))))
            }
        };
        let region =
            match &spec.run_token_authorization {
                Some(crate::RunTokenAuthorizationContext::CiJob(ctx)) => ctx.region.clone(),
                None => return Err(CheckoutOrchestrationError::Hook(HookError(
                    "checkout orchestration requires a resolved run-token authorization context \
                     (for the region)"
                        .to_string(),
                ))),
            };
        let tenant = scope.tenant().0.clone();
        let repo = scope.repo_id().to_string();
        let expected = crate::workspace_intent::ExpectedGitCommitId::new(
            scope.commit_hex().to_string(),
            scope.commit_format(),
        )
        .map_err(|e| CheckoutOrchestrationError::Hook(HookError(e)))?;

        let (report_claim, _reserve, attempt_authority) = match hooks
            .reserve_parent_attempt(spec)
            .map_err(
            CheckoutOrchestrationError::Hook,
        )? {
            ParentAttemptAdmission::Admitted {
                claim,
                reserve,
                attempt_authority,
            } => (claim, reserve, attempt_authority),
            ParentAttemptAdmission::AttemptsExhausted {
                claim,
                reserve: _reserve,
            } => {
                return Ok(CheckoutContinuationOutcome::PreparationTerminal {
                    claim,
                    disposition: PreparationTerminalDisposition::AttemptsExhausted,
                    diagnostic: None,
                });
            }
        };
        let authority = attempt_authority.as_ref();
        let report_claim = &report_claim;

        if authority
            .begin_phase(PreparationPhase::CheckoutTransport)
            .is_err()
        {
            return Ok(requeue_or_exhausted(
                authority,
                report_claim,
                PreparationPhase::CheckoutTransport,
            ));
        }

        let advertise_carrier = match authority.mint_phase_credential(CheckoutPhase::Advertise) {
            Ok(carrier) => carrier,
            Err(_error) => return Ok(resolve_begun_transport_failure(authority, report_claim)),
        };
        let (advertise_credential, advertise_authorization) = match authorize_phase_generation(
            hooks,
            spec,
            &scope,
            CheckoutPhase::Advertise,
            advertise_carrier,
        ) {
            Ok(pair) => pair,
            Err(_error) => return Ok(resolve_begun_transport_failure(authority, report_claim)),
        };

        let mut fetch_leg = || -> Result<(RunTokenCredential, PhaseAuthorization), HookError> {
            let carrier = authority
                .mint_phase_credential(CheckoutPhase::Fetch)
                .map_err(|e| HookError(e.to_string()))?;
            authorize_phase_generation(hooks, spec, &scope, CheckoutPhase::Fetch, carrier)
        };

        let lease_checkpoint = AttemptAuthorityLeaseCheckpoint(authority);

        let transport = fetch_checkout_pack_within_parent_attempt_v2_given(
            repo_root,
            &tenant,
            &region,
            &repo,
            &expected,
            spec.limits,
            advertise_credential,
            advertise_authorization,
            &mut fetch_leg,
            cancellation.as_atomic(),
            Some(&lease_checkpoint),
            transport_execute,
        );
        let (pack, transport_usage) = match transport {
            Ok(outcome) => outcome.into_parts(),
            Err(error) => {
                let disposition = error.attempt_disposition();
                let usage = checkout_transport_error_usage(&error);
                return Ok(route_preparation_disposition(
                    authority,
                    report_claim,
                    disposition,
                    usage,
                    None,
                )?);
            }
        };

        authority.complete_phase(PreparationPhase::CheckoutTransport, transport_usage)?;
        if let Err(lost) = authority.renew_preparation_lease() {
            return Err(CheckoutOrchestrationError::LeaseLost(lost));
        }

        let runtime = match checkout_runtime::AcquiredCheckoutRuntime::acquire(
            spec,
            &profile,
            checkout_guest_root.path().to_path_buf(),
            workspace_manager,
            userns_allocator,
            cargo_vendor,
        ) {
            Ok(runtime) => runtime,
            Err(failure) => {
                if failure.reconciliation_required {
                    return Ok(CheckoutContinuationOutcome::ReconciliationRequired {
                        phase: PreparationPhase::CheckoutMaterialization,
                        teardown_unproven: true,
                        usage_unrepresentable: false,
                        quarantine_required: true,
                    });
                }
                return Ok(requeue_or_exhausted(
                    authority,
                    report_claim,
                    PreparationPhase::CheckoutMaterialization,
                ));
            }
        };

        let preparation_spec = match CheckoutPreparationSpec::new(expected, pack, spec.limits) {
            Ok(spec) => spec,
            Err(_reason) => {
                let diagnostics = runtime.dispose_checkout_runtime(workspace_manager);
                return Ok(route_after_disposal(
                    diagnostics,
                    PreparationPhase::CheckoutMaterialization,
                    requeue_or_exhausted(
                        authority,
                        report_claim,
                        PreparationPhase::CheckoutMaterialization,
                    ),
                ));
            }
        };

        continue_with(
            authority,
            report_claim,
            &scope,
            runtime,
            preparation_spec,
            workspace_manager,
            checkout_guest_root.path(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "test-support")]
    use crate::runner::PreparationAttemptDisposition;
    #[cfg(feature = "test-support")]
    use std::sync::Mutex;

    use crate::runner::PreparationPhase;

    use std::sync::Arc;

    use crate::gvisor::test_fixtures::*;
    use crate::{
        CompletionSettlementOwner, HookError, LaunchPermit, ReserveHandle, ResourceUsage,
        RunnerHooks, SandboxBackend, SandboxCancellation, SandboxLaunchError, SandboxOutputSink,
    };

    #[test]
    fn reserve_parent_attempt_refuses_in_legacy_mode() {
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        );
        assert!(matches!(
            hooks.reserve_parent_attempt(&spec(vec![])),
            Err(HookError(_))
        ));
    }

    #[test]
    fn reserve_parent_attempt_returns_the_installed_admission() {
        use crate::checkout_orchestration::ParentAttemptAdmission;
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        )
        .with_parent_attempt_reservation(Box::new(|_spec| {
            Ok(ParentAttemptAdmission::Admitted {
                claim: report_claim(),
                reserve: ReserveHandle("ci-reserve:v2:a".to_string()),
                attempt_authority: Box::new(FakeAttemptAuthority::new(true)),
            })
        }));
        match hooks
            .reserve_parent_attempt(&spec(vec![]))
            .expect("admitted")
        {
            ParentAttemptAdmission::Admitted { reserve, .. } => {
                assert_eq!(reserve.0, "ci-reserve:v2:a")
            }
            ParentAttemptAdmission::AttemptsExhausted { .. } => panic!("expected Admitted"),
        }
    }

    #[test]
    fn classify_bound_workload_failure_splits_pre_and_post_cas() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let authority = FakeAttemptAuthority::new(true);

        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::uncommitted("gate failed"),
        );
        assert!(
            matches!(
                out,
                CheckoutContinuationOutcome::PreparationRetryable {
                    phase: PreparationPhase::CheckoutMaterialization,
                    ..
                }
            ),
            "a pre-CAS Uncommitted workload failure is a preparation requeue, got {out:?}"
        );

        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::commit_outcome_unknown("ambiguous"),
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::ReconciliationRequired { .. }
        ));

        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::committed_but_not_executed("never execed"),
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::WorkloadRetryable {
                usage: ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0
                },
                ..
            }
        ));

        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::executed(
                "teardown infra failed",
                ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 6,
                },
            ),
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::WorkloadRetryable {
                usage: ResourceUsage {
                    cpu_seconds: 5,
                    mem_byte_seconds: 6
                },
                ..
            }
        ));
    }

    #[test]
    fn classify_uncommitted_terminalizes_when_attempts_are_exhausted() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let authority = FakeAttemptAuthority::new(false);
        let out = classify_bound_workload_failure(
            &authority,
            &report_claim(),
            RunFailure::uncommitted("gate failed"),
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::PreparationTerminal {
                disposition: crate::runner::PreparationTerminalDisposition::AttemptsExhausted,
                ..
            }
        ));
    }

    #[test]
    fn resolve_begun_transport_failure_completes_zero_then_requeues() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let authority = FakeAttemptAuthority::new(true);
        let out = resolve_begun_transport_failure(&authority, &report_claim());
        assert_eq!(
            authority.ops.lock().unwrap().clone(),
            vec!["complete:CheckoutTransport:0"],
            "the begun transport phase is completed with zero"
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::PreparationRetryable {
                phase: PreparationPhase::CheckoutTransport,
                ..
            }
        ));
    }

    #[cfg(feature = "test-support")]
    #[allow(clippy::result_large_err)]
    #[test]
    fn continuation_routes_a_terminal_hop_b_failure_and_disposes_the_prepared_capsule() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("continuation-terminal-hopb")
        else {
            return;
        };
        let backend = GvisorBackend::new(test_registry());
        let spec = checkout_spec();
        let scope = crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace)
            .expect("scope derives")
            .expect("checkout-bearing");
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        )
        .with_checkout_phase_authorization(Box::new(|_spec, _scope, _phase| {
            Ok(LaunchPermit::immediate())
        }));
        let authority = FakeAttemptAuthority::new(false);
        let preparation_spec = CheckoutPreparationSpec::new(
            crate::workspace_intent::ExpectedGitCommitId::new(
                scope.commit_hex().to_string(),
                scope.commit_format(),
            )
            .unwrap(),
            PrefetchedCheckoutPack::for_tests(),
            spec.limits,
        )
        .unwrap();

        let outcome = backend
            .launch_checkout_continuation_given(
                &spec,
                &hooks,
                &authority,
                &report_claim(),
                &scope,
                runtime,
                preparation_spec,
                &workspace_manager,
                std::path::Path::new("/abs/staged-rootfs"),
                |runtime, _spec, _run_token, _authorization| {
                    Err((
                        runtime,
                        CheckoutPreparationError::RejectedAfterQuiescence {
                            message: "injected terminal checkout rejection".to_string(),
                            usage: ResourceUsage {
                                cpu_seconds: 4,
                                mem_byte_seconds: 8,
                            },
                            disposition: PreparationAttemptDisposition::Terminal(
                                PreparationTerminalDisposition::Failed {
                                    phase: PreparationPhase::CheckoutMaterialization,
                                },
                            ),
                        },
                    ))
                },
                |_prepared, _authority, _hooks, _spec, _wm, _rootfs| {
                    panic!("the workload transition must not run after a Hop B failure")
                },
            )
            .expect("the continuation routes a terminal Hop B failure without a structural error");

        assert!(
            matches!(
                outcome,
                CheckoutContinuationOutcome::PreparationTerminal {
                    disposition: PreparationTerminalDisposition::Failed {
                        phase: PreparationPhase::CheckoutMaterialization
                    },
                    diagnostic: Some(ref diagnostic),
                    ..
                }
                if diagnostic == "injected terminal checkout rejection"
            ),
            "a terminal Hop B failure retains its diagnostic in the preparation-terminal outcome, got {outcome:?}"
        );
        let ops = authority.ops.lock().unwrap().clone();
        assert!(
            ops.contains(&"begin:CheckoutMaterialization".to_string())
                && ops.contains(&"mint:Materialization".to_string())
                && ops.contains(&"complete:CheckoutMaterialization:4".to_string()),
            "the continuation began, authorized, and completed the materialization phase, got {ops:?}"
        );
        assert!(workspace_manager.is_healthy());
        assert!(
            userns_allocator.lease().is_ok(),
            "disposing the capsule must return the slot to the pool"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn not_started_capsule_guard_disposes_safely_on_any_early_exit() {
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("guard-early-exit")
        else {
            return;
        };
        {
            let _guard = NotStartedCapsuleGuard::new(runtime, &workspace_manager);
        }
        assert!(
            workspace_manager.is_healthy(),
            "the guard's Drop must NOT poison the manager - it performs the safe NotStarted cleanup"
        );
        assert!(
            userns_allocator.lease().is_ok(),
            "the guard's Drop must release_unused the slot - the pool slot is reusable"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn not_started_capsule_guard_disarm_hands_back_the_capsule() {
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("guard-disarm")
        else {
            return;
        };
        let runtime = NotStartedCapsuleGuard::new(runtime, &workspace_manager).disarm();
        let diagnostics = runtime.dispose_checkout_runtime(&workspace_manager);
        assert!(
            diagnostics.is_empty(),
            "a clean NotStarted disposal, got {diagnostics:?}"
        );
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.lease().is_ok());
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[allow(clippy::result_large_err)]
    #[test]
    fn continuation_full_success_threads_materialization_generation_and_launches() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
            acquire_real_checkout_capsule("continuation-full-success")
        else {
            return;
        };
        let backend = GvisorBackend::new(test_registry());
        let spec = checkout_spec();
        let scope = crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace)
            .expect("scope derives")
            .expect("checkout-bearing");
        let seen_materialization_jti = Arc::new(Mutex::new(None::<String>));
        let seen = seen_materialization_jti.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        )
        .with_checkout_phase_authorization(Box::new(move |s, _scope, phase| {
            if phase == crate::CheckoutPhase::Materialization {
                *seen.lock().unwrap() = Some(s.run_token.jti.clone());
            }
            Ok(LaunchPermit::immediate())
        }));
        let authority = FakeAttemptAuthority::new(false);
        let preparation_spec = CheckoutPreparationSpec::new(
            crate::workspace_intent::ExpectedGitCommitId::new(
                scope.commit_hex().to_string(),
                scope.commit_format(),
            )
            .unwrap(),
            PrefetchedCheckoutPack::for_tests(),
            spec.limits,
        )
        .unwrap();

        let outcome = backend
            .launch_checkout_continuation_given(
                &spec,
                &hooks,
                &authority,
                &report_claim(),
                &scope,
                runtime,
                preparation_spec,
                &workspace_manager,
                std::path::Path::new("/abs/staged-rootfs"),
                |runtime, _spec, _run_token, _authorization| {
                    Ok(runtime.into_prepared_for_tests(PreparedCheckoutEvidence::for_tests(
                        ResourceUsage {
                            cpu_seconds: 3,
                            mem_byte_seconds: 7,
                        },
                    )))
                },
                |prepared, _authority, _hooks, _spec, wm, _rootfs| {
                    let diagnostics = prepared.dispose_checkout_runtime(wm);
                    assert!(
                        diagnostics.is_empty(),
                        "the Prepared capsule disposes cleanly (release_prepared), got {diagnostics:?}"
                    );
                    RetainedWorkloadOutcome::Ran(Ok(fake_run()))
                },
            )
            .expect("the full-success continuation returns a launched workload");

        assert!(
            matches!(outcome, CheckoutContinuationOutcome::WorkloadLaunched(_)),
            "the full success sequence launches the workload, got {outcome:?}"
        );
        assert_eq!(
            seen_materialization_jti.lock().unwrap().as_deref(),
            Some("jti-Materialization"),
            "the materialization phase authorized against its OWN rotated spec, not the advertise base"
        );
        let ops = authority.ops.lock().unwrap().clone();
        assert!(
            ops.contains(&"begin:CheckoutMaterialization".to_string())
                && ops.contains(&"mint:Materialization".to_string()),
            "the continuation began + minted the materialization generation, got {ops:?}"
        );
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.lease().is_ok());
        drop(backend);
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[allow(clippy::result_large_err)]
    #[test]
    fn continuation_disposes_capsule_on_authority_failure_without_poisoning() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome;
        for (label, authority) in [
            ("begin_phase", FakeAttemptAuthority::failing_begin_phase()),
            ("mint_phase", FakeAttemptAuthority::failing_mint_phase()),
        ] {
            let Some((runtime, workspace_manager, workspace_base, userns_allocator, leases_dir)) =
                acquire_real_checkout_capsule(&format!("continuation-authfail-{label}"))
            else {
                return;
            };
            let backend = GvisorBackend::new(test_registry());
            let spec = checkout_spec();
            let scope = crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace)
                .unwrap()
                .unwrap();
            let hooks = RunnerHooks::new(
                CompletionSettlementOwner::TerminalReporter,
                Box::new(|s| Ok(ReserveHandle(s.meter_to.reserve_id.clone()))),
                Box::new(|_, _, _| Ok(())),
                Box::new(|_| Ok(())),
                Box::new(|_| Ok(())),
            )
            .with_checkout_phase_authorization(Box::new(|_s, _scope, _phase| {
                Ok(LaunchPermit::immediate())
            }));
            let preparation_spec = CheckoutPreparationSpec::new(
                crate::workspace_intent::ExpectedGitCommitId::new(
                    scope.commit_hex().to_string(),
                    scope.commit_format(),
                )
                .unwrap(),
                PrefetchedCheckoutPack::for_tests(),
                spec.limits,
            )
            .unwrap();

            let outcome = backend
                .launch_checkout_continuation_given(
                    &spec,
                    &hooks,
                    &authority,
                    &report_claim(),
                    &scope,
                    runtime,
                    preparation_spec,
                    &workspace_manager,
                    std::path::Path::new("/abs/staged-rootfs"),
                    |_runtime, _spec, _rt, _auth| {
                        panic!("Hop B must not run after an authority failure")
                    },
                    |_prepared, _a, _h, _s, _wm, _r| panic!("the workload must not run"),
                )
                .unwrap_or_else(|e| {
                    panic!("{label}: authority failure must be a typed outcome, not {e:?}")
                });

            assert!(
                matches!(
                    outcome,
                    CheckoutContinuationOutcome::PreparationRetryable { .. }
                        | CheckoutContinuationOutcome::PreparationTerminal { .. }
                ),
                "{label}: a clean-disposal authority failure yields a typed requeue/terminal, got {outcome:?}"
            );
            assert!(
                workspace_manager.is_healthy(),
                "{label}: the manager must NOT be poisoned by a dropped capsule"
            );
            assert!(
                userns_allocator.lease().is_ok(),
                "{label}: the slot must be released (not quarantined) - workspace admission stays open"
            );
            drop(backend);
            let _ = std::fs::remove_dir_all(&workspace_base);
            let _ = std::fs::remove_dir_all(&leases_dir);
        }
    }

    mod orchestrated_active_path_6e2 {
        use super::*;
        use crate::checkout_orchestration::ParentAttemptAdmission;
        use crate::gvisor::checkout_transport_test_support::{
            checkout_spec_for_backend, deterministic_enabled_backend_for_tests,
        };

        fn unique_root(tag: &str) -> std::path::PathBuf {
            let root = std::env::temp_dir().join(format!(
                "myelin-6e2-{tag}-{}-{}",
                std::process::id(),
                unique_suffix()
            ));
            std::fs::create_dir_all(&root).unwrap();
            root
        }

        fn admitting_hooks() -> RunnerHooks {
            ok_hooks()
                .with_checkout_phase_authorization(Box::new(|_spec, _scope, _phase| {
                    Ok(LaunchPermit::immediate())
                }))
                .with_parent_attempt_reservation(Box::new(|_spec| {
                    Ok(ParentAttemptAdmission::Admitted {
                        claim: report_claim(),
                        reserve: ReserveHandle("ci-reserve:v2:6e2".to_string()),
                        attempt_authority: Box::new(NoOpTestSupportAuthority),
                    })
                }))
        }

        #[cfg(feature = "test-support")]
        #[test]
        #[cfg_attr(
            not(feature = "privileged-host-tests"),
            ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
        )]
        fn orchestrated_checkout_drives_two_gated_hops_to_a_clean_workload_launch() {
            use crate::checkout_orchestration::CheckoutContinuationOutcome;
            use crate::gvisor::checkout_transport_test_support::stage_checkout_repo_root;
            let root = unique_root("orchestrated");
            let (backend, image) = deterministic_enabled_backend_for_tests(&root);
            let repo_root = stage_checkout_repo_root(&root.join("repos"));
            let spec = checkout_spec_for_backend(image);
            let hooks = admitting_hooks();

            let (result, recorded) = backend.drive_checkout_cycle_with_substituted_runsc_given(
                &spec,
                &hooks,
                &repo_root,
                "checkout.sentinel",
                b"6e2-provenance-sentinel",
            );

            assert_eq!(
                recorded.len(),
                2,
                "exactly two transport hops must spawn (advertise then fetch): {recorded:?}"
            );
            assert_ne!(
                recorded[0].0, recorded[1].0,
                "advertise and fetch must spawn under DISTINCT jtis: {recorded:?}"
            );
            assert!(
                recorded[0].1 && recorded[1].1,
                "both transport permits must commit: {recorded:?}"
            );

            match result {
                Ok(CheckoutContinuationOutcome::WorkloadLaunched(launch)) => {
                    assert!(
                        launch.output_complete,
                        "the substituted workload must complete cleanly"
                    );
                    assert_eq!(
                        launch.result.usage,
                        crate::ResourceUsage {
                            cpu_seconds: 3,
                            mem_byte_seconds: 7,
                        },
                        "the settled workload carries exactly the substituted workload usage"
                    );
                }
                other => panic!("expected a clean WorkloadLaunched, got {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn run_cycle_selects_the_gvisor_arm_on_workspace_shape_before_reserve_or_spawn() {
            let root = unique_root("selector");
            let (backend, image) = deterministic_enabled_backend_for_tests(&root);
            let sink: Arc<dyn SandboxOutputSink> = Arc::new(RecordingOutput::default());

            let checkout_spec = checkout_spec_for_backend(image.clone());
            let err = backend
                .run_cycle(
                    &checkout_spec,
                    &admitting_hooks(),
                    sink.clone(),
                    SandboxCancellation::new(),
                )
                .expect_err("a checkout job on a checkout-disabled backend fails closed");
            match err {
                SandboxLaunchError::Failed(GvisorError::Hook(HookError(msg))) => assert!(
                    msg.contains("enabled checkout repository root"),
                    "checkout arm selected; got: {msg}"
                ),
                other => panic!("expected the checkout-arm fail-closed refusal, got {other:?}"),
            }

            let mut malformed = checkout_spec_for_backend(image.clone());
            malformed.workspace.commit = None;
            let err = backend
                .run_cycle(
                    &malformed,
                    &admitting_hooks(),
                    sink.clone(),
                    SandboxCancellation::new(),
                )
                .expect_err("a malformed workspace is refused");
            match err {
                SandboxLaunchError::Failed(GvisorError::Hook(HookError(msg))) => assert!(
                    msg.contains("malformed workspace"),
                    "malformed arm selected; got: {msg}"
                ),
                other => panic!("expected the malformed-workspace refusal, got {other:?}"),
            }

            let mut compute_spec = checkout_spec_for_backend(image);
            compute_spec.workspace = crate::WorkspaceSpec::default();
            let err = backend
                .run_cycle(&compute_spec, &ok_hooks(), sink, SandboxCancellation::new())
                .expect_err("compute under legacy hooks refuses at parent-attempt admission");
            match err {
                SandboxLaunchError::Failed(GvisorError::Hook(HookError(msg))) => assert!(
                    msg.contains("parent-attempt"),
                    "compute arm selected (reached reserve_parent_attempt); got: {msg}"
                ),
                other => panic!("expected the compute-arm reserve refusal, got {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}
