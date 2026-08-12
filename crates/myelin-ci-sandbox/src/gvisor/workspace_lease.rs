use super::*;
use crate::hardening::HardeningProfile;
use crate::user_namespace::{
    RunscInvocationMode, UserNamespaceAllocator, UserNamespaceBindError, UserNamespaceLease,
    UserNamespaceQuiescenceProof, UserNamespaceRefusal,
};
use crate::workspace_manager::{
    CapacityLease, CapacityRefusal, DeleteWorkspaceError, ManagedWorkspace, WorkspaceManager,
    WorkspaceProvisionError,
};
use crate::workspace_storage::WorkspaceStorageError;
use crate::JobSpec;
use std::path::PathBuf;

pub(super) enum WorkspaceIntegration {
    Disabled,
    Enabled {
        workspace_manager: WorkspaceManager,
        userns_allocator: UserNamespaceAllocator,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LeaseBindState {
    Allocated,
    Bound {
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    },
    Unreleasable,
}

pub(super) struct EnabledLaunchContext {
    pub(super) workspace: ManagedWorkspace,
    pub(super) lease: UserNamespaceLease,
    pub(super) bind_state: LeaseBindState,
}

pub(super) enum RuntimeBinding<'a> {
    Rootless,
    Enabled {
        expected_root_identity: (u64, u64),
        context: &'a mut EnabledLaunchContext,
    },
    EnabledPrepared {
        expected_root_identity: (u64, u64),
        lease: &'a mut crate::user_namespace::UserNamespaceLease,
        session: &'a mut crate::user_namespace::CheckoutPreparationSession,
        bind_state: &'a mut LeaseBindState,
    },
}

pub(super) struct RuntimePreparation<'a> {
    pub(super) prepared_mode: PreparedRuntimeMode,
    pub(super) mode: RunscInvocationMode,
    pub(super) binding: RuntimeBinding<'a>,
}

impl<'a> RuntimePreparation<'a> {
    pub(super) fn new(cfg: &OciConfig, binding: RuntimeBinding<'a>) -> Result<Self, String> {
        let prepared_mode = match &binding {
            RuntimeBinding::Rootless => PreparedRuntimeMode::Rootless,
            RuntimeBinding::Enabled {
                expected_root_identity,
                context,
            } => PreparedRuntimeMode::ExplicitUserNamespace {
                config: context.lease.config(),
                expected_root_identity: *expected_root_identity,
            },
            RuntimeBinding::EnabledPrepared {
                expected_root_identity,
                lease,
                ..
            } => PreparedRuntimeMode::ExplicitUserNamespace {
                config: lease.config(),
                expected_root_identity: *expected_root_identity,
            },
        };
        let mode = require_oci_layout_matches_prepared_mode(cfg, &prepared_mode)?;
        Ok(RuntimePreparation {
            prepared_mode,
            mode,
            binding,
        })
    }
}

#[derive(Debug)]
pub(super) enum WorkspaceDeletionOutcome {
    ProvenAbsent { diagnostic: Option<String> },
    NotProvenAbsent { diagnostic: String },
}

pub(super) fn classify_workspace_deletion(
    result: Result<(), DeleteWorkspaceError>,
) -> WorkspaceDeletionOutcome {
    match result {
        Ok(()) => WorkspaceDeletionOutcome::ProvenAbsent { diagnostic: None },
        Err(DeleteWorkspaceError::InternalInvariantViolated { reason }) => {
            WorkspaceDeletionOutcome::ProvenAbsent {
                diagnostic: Some(format!(
                    "workspace delete succeeded despite an internal invariant violation: {reason}"
                )),
            }
        }
        Err(DeleteWorkspaceError::Storage(e)) => WorkspaceDeletionOutcome::NotProvenAbsent {
            diagnostic: format!(
                "workspace delete/sync failed ({e}) - the userns lease is left unreleased \
                 (quarantined) since disk absence is not proven"
            ),
        },
        Err(DeleteWorkspaceError::WrongManager { .. }) => {
            WorkspaceDeletionOutcome::NotProvenAbsent {
                diagnostic:
                    "workspace delete refused (WrongManager - structurally unexpected) - the \
                         userns lease is left unreleased (quarantined) since disk absence is not \
                         proven"
                        .to_string(),
            }
        }
    }
}

fn delete_workspace_then_release_lease_if_absent(
    workspace: ManagedWorkspace,
    lease: UserNamespaceLease,
    delete_workspace: impl FnOnce(ManagedWorkspace) -> Result<(), DeleteWorkspaceError>,
) -> Vec<String> {
    let mut diagnostics = Vec::new();
    match classify_workspace_deletion(delete_workspace(workspace)) {
        WorkspaceDeletionOutcome::ProvenAbsent { diagnostic } => {
            diagnostics.extend(diagnostic);
            if let Err(e) = lease.release_unused() {
                diagnostics.push(format!("releasing the unused userns lease failed: {e}"));
            }
        }
        WorkspaceDeletionOutcome::NotProvenAbsent { diagnostic } => {
            diagnostics.push(diagnostic);
            drop(lease);
        }
    }
    diagnostics
}

pub(super) fn join_diagnostics(base: String, diagnostics: &[String]) -> String {
    diagnostics
        .iter()
        .fold(base, |acc, d| format!("{acc} AND {d}"))
}

#[derive(Debug)]
pub(crate) struct AcquisitionFailure {
    pub message: String,
    pub reconciliation_required: bool,
}

impl AcquisitionFailure {
    pub(super) fn clean(message: String) -> Self {
        Self {
            message,
            reconciliation_required: false,
        }
    }
    fn reconcile(message: String) -> Self {
        Self {
            message,
            reconciliation_required: true,
        }
    }
    pub(super) fn from_rollback_diagnostics(base: String, diagnostics: Vec<String>) -> Self {
        let reconciliation_required = !diagnostics.is_empty();
        Self {
            message: join_diagnostics(base, &diagnostics),
            reconciliation_required,
        }
    }
}

pub(super) fn acquire_enabled_workspace(
    spec: &JobSpec,
    profile: &HardeningProfile,
    container_id: &str,
    absolute_rootfs: PathBuf,
    workspace_manager: &WorkspaceManager,
    userns_allocator: &UserNamespaceAllocator,
    cargo_vendor: Option<crate::asset_registry::VerifiedCargoVendor>,
) -> Result<(OciConfig, EnabledLaunchContext), AcquisitionFailure> {
    let (cfg, context) = acquire_enabled_workspace_given(
        spec,
        profile,
        container_id,
        absolute_rootfs,
        |bytes| workspace_manager.acquire_capacity(bytes),
        || userns_allocator.lease(),
        |job_key, quota, uid, gid, capacity| {
            workspace_manager.create_workspace(job_key, quota, uid, gid, capacity)
        },
        |workspace| workspace_manager.delete_workspace(workspace),
    )?;
    let Some(cargo_vendor) = cargo_vendor else {
        return Ok((cfg, context));
    };
    match cfg.with_cargo_vendor(cargo_vendor) {
        Ok(cfg) => Ok((cfg, context)),
        Err(reason) => {
            let diagnostics = cleanup_pre_bind_failure(context, workspace_manager);
            Err(AcquisitionFailure::from_rollback_diagnostics(
                format!("attaching the structured Cargo vendor boundary failed: {reason}"),
                diagnostics,
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn acquire_enabled_workspace_given(
    spec: &JobSpec,
    profile: &HardeningProfile,
    container_id: &str,
    absolute_rootfs: PathBuf,
    acquire_capacity: impl FnOnce(u64) -> Result<CapacityLease, CapacityRefusal>,
    lease_fn: impl FnOnce() -> Result<UserNamespaceLease, UserNamespaceRefusal>,
    create_workspace: impl FnOnce(
        &str,
        u64,
        u32,
        u32,
        CapacityLease,
    ) -> Result<ManagedWorkspace, WorkspaceProvisionError>,
    delete_workspace: impl FnOnce(ManagedWorkspace) -> Result<(), DeleteWorkspaceError>,
) -> Result<(OciConfig, EnabledLaunchContext), AcquisitionFailure> {
    let capacity = acquire_capacity(spec.limits.disk_bytes).map_err(|refusal| {
        AcquisitionFailure::clean(format!("workspace capacity refused: {refusal}"))
    })?;
    let lease = match lease_fn() {
        Ok(lease) => lease,
        Err(refusal) => {
            capacity.release();
            return Err(AcquisitionFailure::clean(format!(
                "userns lease refused: {refusal}"
            )));
        }
    };
    let workspace = match create_workspace(
        container_id,
        spec.limits.disk_bytes,
        lease.host_uid(),
        lease.host_gid(),
        capacity,
    ) {
        Ok(workspace) => workspace,
        Err(WorkspaceProvisionError::Refused(refusal)) => {
            let message = format!("workspace creation refused: {refusal}");
            refusal.into_capacity().release();
            return Err(match lease.release_unused() {
                Ok(()) => AcquisitionFailure::clean(message),
                Err(e) => AcquisitionFailure::reconcile(format!(
                    "{message} AND releasing the unused userns lease also failed: {e}"
                )),
            });
        }
        Err(WorkspaceProvisionError::Storage(WorkspaceStorageError::UnrecoverableLeak {
            path,
            ..
        })) => {
            drop(lease);
            return Err(AcquisitionFailure::reconcile(format!(
                "workspace creation left an unrecoverable leak at {path:?} - the userns lease is \
                 left unreleased (quarantined) since disk absence is not proven"
            )));
        }
        Err(WorkspaceProvisionError::Storage(e)) => {
            let message = format!("workspace-storage provisioning failed: {e}");
            return Err(match lease.release_unused() {
                Ok(()) => AcquisitionFailure::clean(message),
                Err(release_error) => AcquisitionFailure::reconcile(format!(
                    "{message} AND releasing the unused userns lease also failed: {release_error}"
                )),
            });
        }
        Err(WorkspaceProvisionError::InternalInvariantViolated { reason, workspace }) => {
            let diagnostics =
                delete_workspace_then_release_lease_if_absent(*workspace, lease, delete_workspace);
            return Err(AcquisitionFailure::from_rollback_diagnostics(
                format!("workspace creation violated an internal invariant: {reason}"),
                diagnostics,
            ));
        }
    };
    let cfg =
        match OciWorkspaceMount::from_managed_workspace(&workspace).and_then(|workspace_mount| {
            OciConfig::from_spec(spec, profile).with_explicit_user_namespace_and_workspace(
                lease.config(),
                workspace_mount,
                absolute_rootfs,
            )
        }) {
            Ok(cfg) => cfg,
            Err(reason) => {
                let diagnostics = delete_workspace_then_release_lease_if_absent(
                    workspace,
                    lease,
                    delete_workspace,
                );
                return Err(AcquisitionFailure::from_rollback_diagnostics(
                    format!("building the explicit-userns workspace OCI layout failed: {reason}"),
                    diagnostics,
                ));
            }
        };
    Ok((
        cfg,
        EnabledLaunchContext {
            workspace,
            lease,
            bind_state: LeaseBindState::Allocated,
        },
    ))
}

pub(super) fn cleanup_pre_bind_failure(
    context: EnabledLaunchContext,
    workspace_manager: &WorkspaceManager,
) -> Vec<String> {
    let EnabledLaunchContext {
        workspace,
        lease,
        bind_state,
    } = context;
    match bind_state {
        LeaseBindState::Allocated => {
            delete_workspace_then_release_lease_if_absent(workspace, lease, |w| {
                workspace_manager.delete_workspace(w)
            })
        }
        LeaseBindState::Unreleasable => {
            drop(lease);
            match classify_workspace_deletion(workspace_manager.delete_workspace(workspace)) {
                WorkspaceDeletionOutcome::ProvenAbsent { diagnostic } => {
                    diagnostic.into_iter().collect()
                }
                WorkspaceDeletionOutcome::NotProvenAbsent { diagnostic } => vec![diagnostic],
            }
        }
        LeaseBindState::Bound { .. } => {
            drop(workspace);
            drop(lease);
            vec![
                "an outer launch failure occurred AFTER a successful userns lease bind - this \
                 should be structurally impossible; the workspace and lease are both abandoned \
                 (quarantined) rather than acted on, since no finalization evidence exists \
                 proving the runtime cannot still access them"
                    .to_string(),
            ]
        }
    }
}

pub(super) fn settle_enabled_workspace_and_lease(
    context: EnabledLaunchContext,
    workspace_manager: &WorkspaceManager,
    evidence: &RuntimeQuiescenceEvidence,
) -> Result<(), String> {
    let EnabledLaunchContext {
        workspace,
        lease,
        bind_state,
    } = context;
    let LeaseBindState::Bound {
        container_id,
        runsc_root_identity,
        cgroup_identity,
    } = bind_state
    else {
        return Err(format!(
            "the runtime finalized, but this lease's locally-recorded bind state was \
             {bind_state:?} (not Bound) - refusing to trust evidence against an unrecorded binding"
        ));
    };
    let expected_namespace = RuntimeNamespaceQuiescence::ExplicitUserNamespace {
        runsc_root_identity,
    };
    if evidence.container_id() != container_id || evidence.namespace() != expected_namespace {
        return Err(format!(
            "runtime quiescence evidence ({:?}, {:?}) does not match the recorded binding \
             ({container_id:?}, {expected_namespace:?})",
            evidence.container_id(),
            evidence.namespace()
        ));
    }
    if evidence.cgroup().cgroup_identity() != cgroup_identity {
        return Err(format!(
            "runtime quiescence evidence's cgroup identity {:?} does not match the recorded \
             bind-time cgroup identity {cgroup_identity:?}",
            evidence.cgroup().cgroup_identity()
        ));
    }
    let proof = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, evidence)
        .map_err(|e| format!("failed to mint a userns quiescence proof: {e}"))?;
    match classify_workspace_deletion(workspace_manager.delete_workspace(workspace)) {
        WorkspaceDeletionOutcome::ProvenAbsent { diagnostic } => {
            if let Err(e) = lease.release(proof) {
                let base = diagnostic.unwrap_or_else(|| "workspace deleted".to_string());
                return Err(format!(
                    "{base}, but releasing the userns lease also failed: {e}"
                ));
            }
            match diagnostic {
                Some(diagnostic) => Err(diagnostic),
                None => Ok(()),
            }
        }
        WorkspaceDeletionOutcome::NotProvenAbsent { diagnostic } => {
            drop(lease);
            Err(diagnostic)
        }
    }
}

pub(super) fn settle_enabled_finalization(
    finalization: RuntimeFinalization<Result<ContainerRun, RunFailure>>,
    enabled_context: Option<EnabledLaunchContext>,
    workspace_manager: Option<&WorkspaceManager>,
) -> Result<ContainerRun, RunFailure> {
    let enabled_cleanup_failure = match (enabled_context, &finalization) {
        (Some(context), RuntimeFinalization::Finalized(finalized)) => match workspace_manager {
            Some(workspace_manager) => {
                settle_enabled_workspace_and_lease(context, workspace_manager, &finalized.evidence)
                    .err()
            }
            None => {
                drop(context);
                Some(
                        "an enabled runtime finalized without its workspace manager; the workspace and userns lease were quarantined"
                            .to_string(),
                    )
            }
        },
        (Some(context), RuntimeFinalization::Failed { .. }) => {
            drop(context);
            None
        }
        (None, _) => None,
    };

    let settled = settle_finalization(
        finalization,
        |run: &ContainerRun| run.result.usage,
        discard_container_run_after_teardown_failure,
    );
    match enabled_cleanup_failure {
        None => settled,
        Some(diagnostic) => augment_settled_result_with_enabled_cleanup_failure(
            settled,
            |run: &ContainerRun| run.result.usage,
            |run: ContainerRun| discard_container_run(run, false),
            diagnostic,
        ),
    }
}

fn bind_enabled_lease_given(
    lease: &mut UserNamespaceLease,
    bind_state: &mut LeaseBindState,
    expected_root_identity: (u64, u64),
    container_id: &str,
    cgroup_identity: (u64, u64),
    revalidate_root_identity: impl FnOnce() -> Result<(u64, u64), String>,
) -> Result<(), String> {
    let current = revalidate_root_identity()?;
    if current != expected_root_identity {
        return Err(format!(
            "runsc-root identity drifted before bind (expected {expected_root_identity:?}, \
             found {current:?})"
        ));
    }
    match lease.bind(container_id.to_string(), current, cgroup_identity) {
        Ok(()) => {
            *bind_state = LeaseBindState::Bound {
                container_id: container_id.to_string(),
                runsc_root_identity: current,
                cgroup_identity,
            };
            Ok(())
        }
        Err(bind_error) => {
            *bind_state = match bind_error {
                UserNamespaceBindError::InvalidContainerId
                | UserNamespaceBindError::MarkerTooLarge => LeaseBindState::Allocated,
                UserNamespaceBindError::MarkerMismatch
                | UserNamespaceBindError::Poisoned
                | UserNamespaceBindError::InvalidSessionState
                | UserNamespaceBindError::LeaseMismatch => LeaseBindState::Unreleasable,
            };
            Err(format!("durable lease bind failed: {bind_error}"))
        }
    }
}

pub(super) fn bind_prepared_lease_given(
    lease: &mut crate::user_namespace::UserNamespaceLease,
    session: &mut crate::user_namespace::CheckoutPreparationSession,
    bind_state: &mut LeaseBindState,
    expected_root_identity: (u64, u64),
    container_id: &str,
    cgroup_identity: (u64, u64),
    revalidate_root_identity: impl FnOnce() -> Result<(u64, u64), String>,
) -> Result<(), String> {
    let current = revalidate_root_identity()?;
    if current != expected_root_identity {
        return Err(format!(
            "runsc-root identity drifted before workload bind (expected \
             {expected_root_identity:?}, found {current:?})"
        ));
    }
    match session.bind_workload(lease, container_id.to_string(), current, cgroup_identity) {
        Ok(identity) => {
            let (container_id, runsc_root_identity, cgroup_identity) = identity.into_parts();
            *bind_state = LeaseBindState::Bound {
                container_id,
                runsc_root_identity,
                cgroup_identity,
            };
            Ok(())
        }
        Err(bind_error) => {
            *bind_state = match bind_error {
                crate::user_namespace::UserNamespaceBindError::InvalidContainerId
                | crate::user_namespace::UserNamespaceBindError::MarkerTooLarge => {
                    LeaseBindState::Allocated
                }
                crate::user_namespace::UserNamespaceBindError::MarkerMismatch
                | crate::user_namespace::UserNamespaceBindError::Poisoned
                | crate::user_namespace::UserNamespaceBindError::InvalidSessionState
                | crate::user_namespace::UserNamespaceBindError::LeaseMismatch => {
                    LeaseBindState::Unreleasable
                }
            };
            Err(format!("durable workload lease bind failed: {bind_error}"))
        }
    }
}

pub(super) fn bind_then_continue<T>(
    enabled: Option<(&mut UserNamespaceLease, &mut LeaseBindState, (u64, u64))>,
    container_id: &str,
    cgroup_identity: (u64, u64),
    revalidate_root_identity: impl FnOnce() -> Result<(u64, u64), String>,
    continuation: impl FnOnce() -> T,
) -> Result<T, String> {
    if let Some((lease, bind_state, expected_root_identity)) = enabled {
        bind_enabled_lease_given(
            lease,
            bind_state,
            expected_root_identity,
            container_id,
            cgroup_identity,
            revalidate_root_identity,
        )?;
    }
    Ok(continuation())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "test-support")]
    use crate::gvisor::test_fixtures::*;
    #[cfg(feature = "test-support")]
    use crate::workspace_manager::WorkspaceStorageMode;
    #[cfg(feature = "test-support")]
    use std::sync::Arc;

    use crate::workspace_manager::DeleteWorkspaceError;

    use crate::workspace_storage::WorkspaceStorageError;

    #[test]
    fn classify_workspace_deletion_ok_proves_absence_with_no_diagnostic() {
        let outcome = classify_workspace_deletion(Ok(()));
        assert!(matches!(
            outcome,
            WorkspaceDeletionOutcome::ProvenAbsent { diagnostic: None }
        ));
    }

    #[test]
    fn classify_workspace_deletion_internal_invariant_violated_proves_absence_but_surfaces_it() {
        let outcome =
            classify_workspace_deletion(Err(DeleteWorkspaceError::InternalInvariantViolated {
                reason: "bookkeeping corruption".to_string(),
            }));
        match outcome {
            WorkspaceDeletionOutcome::ProvenAbsent {
                diagnostic: Some(diagnostic),
            } => assert!(diagnostic.contains("bookkeeping corruption")),
            other => {
                panic!("expected ProvenAbsent with a diagnostic, got a different shape: {other:?}")
            }
        }
    }

    #[test]
    fn classify_workspace_deletion_storage_failure_does_not_prove_absence() {
        let outcome = classify_workspace_deletion(Err(DeleteWorkspaceError::Storage(
            WorkspaceStorageError::ZeroQuota,
        )));
        assert!(matches!(
            outcome,
            WorkspaceDeletionOutcome::NotProvenAbsent { .. }
        ));
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn delete_workspace_then_release_lease_if_absent_releases_a_real_lease_on_internal_invariant_violated(
    ) {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("integrated-invariant-violated")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("integrated-invariant-violated")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let lease = userns_allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let capacity = workspace_manager
            .acquire_capacity(8 << 20)
            .expect("capacity must be available against a fresh 1 GiB ceiling");
        let workspace = workspace_manager
            .create_workspace(
                "integrated-invariant-violated-job",
                8 << 20,
                lease.host_uid(),
                lease.host_gid(),
                capacity,
            )
            .expect("create_workspace must succeed against a real, privileged Btrfs backend");
        let host_path = workspace.host_path().unwrap().to_path_buf();

        let diagnostics = delete_workspace_then_release_lease_if_absent(workspace, lease, |w| {
            workspace_manager.delete_workspace(w).expect(
                "the real delete must succeed for this test to model a genuine invariant \
                 violation atop an otherwise-successful deletion",
            );
            Err(DeleteWorkspaceError::InternalInvariantViolated {
                reason: "synthetic bookkeeping corruption for this test".to_string(),
            })
        });

        assert_eq!(
            diagnostics.len(),
            1,
            "the failure must be surfaced: {diagnostics:?}"
        );
        assert!(diagnostics[0].contains("synthetic bookkeeping corruption"));
        assert!(
            !host_path.exists(),
            "the real subvolume must genuinely be gone -- this variant's whole premise is that \
             disk absence IS proven, just alongside a separately-surfaced bookkeeping failure"
        );
        assert!(
            userns_allocator.is_healthy(),
            "InternalInvariantViolated proves disk absence -- the real lease must have released \
             cleanly, not been quarantined"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn delete_workspace_then_release_lease_if_absent_quarantines_a_real_lease_on_a_storage_failure()
    {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("integrated-storage-failure")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("integrated-storage-failure")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let lease = userns_allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let capacity = workspace_manager
            .acquire_capacity(8 << 20)
            .expect("capacity must be available against a fresh 1 GiB ceiling");
        let workspace = workspace_manager
            .create_workspace(
                "integrated-storage-failure-job",
                8 << 20,
                lease.host_uid(),
                lease.host_gid(),
                capacity,
            )
            .expect("create_workspace must succeed against a real, privileged Btrfs backend");
        let host_path = workspace.host_path().unwrap().to_path_buf();

        let diagnostics = delete_workspace_then_release_lease_if_absent(workspace, lease, |_w| {
            Err(DeleteWorkspaceError::Storage(
                WorkspaceStorageError::ZeroQuota,
            ))
        });

        assert_eq!(
            diagnostics.len(),
            1,
            "the failure must be surfaced: {diagnostics:?}"
        );
        assert!(diagnostics[0].contains("delete/sync failed"));
        assert!(
            !userns_allocator.is_healthy(),
            "a Storage failure does NOT prove disk absence -- the real lease must be quarantined, \
             never released"
        );

        drop(workspace_manager);
        let sink2: crate::workspace_manager::IncidentSink =
            Arc::new(|msg: &str| eprintln!("[piece7c workspace incident] {msg}"));
        let fresh = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: workspace_base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink2,
        )
        .expect("a fresh manager's own boot reconciliation must clean up the orphan and succeed");
        assert!(!host_path.exists());
        drop(fresh);
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_enabled_lease_given_binds_and_records_bound_on_success() {
        let Some((allocator, leases_dir)) = real_userns_allocator_for_tests("bind-ok") else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let expected_root_identity = (11, 22);
        let cgroup_identity = (33, 44);
        let result = bind_enabled_lease_given(
            &mut lease,
            &mut bind_state,
            expected_root_identity,
            "bind-ok-container",
            cgroup_identity,
            || Ok(expected_root_identity),
        );
        assert!(result.is_ok());
        assert_eq!(
            bind_state,
            LeaseBindState::Bound {
                container_id: "bind-ok-container".to_string(),
                runsc_root_identity: expected_root_identity,
                cgroup_identity,
            }
        );
        let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            "bind-ok-container".to_string(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity: expected_root_identity,
            },
            CgroupQuiescenceEvidence::assert_for_tests(cgroup_identity),
        );
        let proof = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, &evidence)
            .expect("a matching evidence must mint a proof");
        lease
            .release(proof)
            .expect("release must succeed after a real bind");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_enabled_lease_given_refuses_before_touching_the_lease_when_identity_drifted() {
        let Some((allocator, leases_dir)) = real_userns_allocator_for_tests("bind-identity-drift")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let result = bind_enabled_lease_given(
            &mut lease,
            &mut bind_state,
            (11, 22),
            "bind-drift-container",
            (33, 44),
            || Ok((99, 99)),
        );
        assert!(result.is_err());
        assert_eq!(
            bind_state,
            LeaseBindState::Allocated,
            "an identity-drift refusal must never touch bind_state -- the lease was never bound"
        );
        lease
            .release_unused()
            .expect("an un-bound, un-touched lease must still release cleanly");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_enabled_lease_given_classifies_an_invalid_container_id_as_still_allocated() {
        let Some((allocator, leases_dir)) = real_userns_allocator_for_tests("bind-invalid-id")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let expected_root_identity = (11, 22);
        let result = bind_enabled_lease_given(
            &mut lease,
            &mut bind_state,
            expected_root_identity,
            "",
            (33, 44),
            || Ok(expected_root_identity),
        );
        assert!(result.is_err());
        assert_eq!(
            bind_state,
            LeaseBindState::Allocated,
            "InvalidContainerId is a caller bug, not a global-trust failure -- nothing touched \
             disk, so the lease remains safely Allocated and reusable"
        );
        lease.release_unused().expect(
            "an Allocated lease untouched by a caller-bug refusal must still release cleanly",
        );
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_enabled_lease_given_classifies_a_marker_mismatch_as_unreleasable() {
        let Some((allocator, leases_dir)) = real_userns_allocator_for_tests("bind-marker-mismatch")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let expected_root_identity = (11, 22);
        lease
            .bind(
                "already-bound".to_string(),
                expected_root_identity,
                (33, 44),
            )
            .expect("the first bind against a fresh Allocated lease must succeed");
        let mut bind_state = LeaseBindState::Bound {
            container_id: "already-bound".to_string(),
            runsc_root_identity: expected_root_identity,
            cgroup_identity: (33, 44),
        };
        let result = bind_enabled_lease_given(
            &mut lease,
            &mut bind_state,
            expected_root_identity,
            "second-bind-attempt",
            (55, 66),
            || Ok(expected_root_identity),
        );
        assert!(result.is_err());
        assert_eq!(
            bind_state,
            LeaseBindState::Unreleasable,
            "MarkerMismatch means the on-disk state no longer agrees with this in-memory lease -- \
             ambiguous and never safe to release"
        );
        assert!(
            !allocator.is_healthy(),
            "MarkerMismatch must globally poison the allocator (a global-trust failure, not a \
             caller bug)"
        );
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[test]
    fn bind_then_continue_always_invokes_the_continuation_when_rootless() {
        let calls = std::cell::Cell::new(0u32);
        let result = bind_then_continue(
            None,
            "rootless-container",
            (33, 44),
            || panic!("Rootless must never need to revalidate a root identity"),
            || {
                calls.set(calls.get() + 1);
                "captured"
            },
        );
        assert_eq!(result, Ok("captured"));
        assert_eq!(calls.get(), 1);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_then_continue_invokes_the_continuation_exactly_once_after_a_successful_bind() {
        let Some((allocator, leases_dir)) =
            real_userns_allocator_for_tests("bind-then-continue-ok")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let expected_root_identity = (11, 22);
        let calls = std::cell::Cell::new(0u32);
        let result = bind_then_continue(
            Some((&mut lease, &mut bind_state, expected_root_identity)),
            "bind-then-continue-ok-container",
            (33, 44),
            || Ok(expected_root_identity),
            || {
                calls.set(calls.get() + 1);
                "captured"
            },
        );
        assert_eq!(result, Ok("captured"));
        assert_eq!(
            calls.get(),
            1,
            "a successful bind must invoke the continuation exactly once"
        );
        assert_eq!(
            bind_state,
            LeaseBindState::Bound {
                container_id: "bind-then-continue-ok-container".to_string(),
                runsc_root_identity: expected_root_identity,
                cgroup_identity: (33, 44),
            }
        );
        let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            "bind-then-continue-ok-container".to_string(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity: expected_root_identity,
            },
            CgroupQuiescenceEvidence::assert_for_tests((33, 44)),
        );
        let proof = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, &evidence)
            .expect("a matching evidence must mint a proof");
        lease
            .release(proof)
            .expect("release must succeed after a real bind");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_then_continue_never_invokes_the_continuation_when_identity_drifted() {
        let Some((allocator, leases_dir)) =
            real_userns_allocator_for_tests("bind-then-continue-drift")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let mut bind_state = LeaseBindState::Allocated;
        let calls = std::cell::Cell::new(0u32);
        let result = bind_then_continue(
            Some((&mut lease, &mut bind_state, (11, 22))),
            "bind-then-continue-drift-container",
            (33, 44),
            || Ok((99, 99)),
            || {
                calls.set(calls.get() + 1);
                "captured"
            },
        );
        assert!(result.is_err());
        assert_eq!(
            calls.get(),
            0,
            "an identity-drift refusal must NEVER invoke the capture/spawn continuation"
        );
        assert_eq!(bind_state, LeaseBindState::Allocated);
        lease
            .release_unused()
            .expect("an un-bound, un-touched lease must still release cleanly");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn bind_then_continue_never_invokes_the_continuation_on_a_real_bind_failure() {
        let Some((allocator, leases_dir)) =
            real_userns_allocator_for_tests("bind-then-continue-bind-fail")
        else {
            return;
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");
        let expected_root_identity = (11, 22);
        let calls = std::cell::Cell::new(0u32);
        let result = bind_then_continue(
            Some((
                &mut lease,
                &mut LeaseBindState::Allocated,
                expected_root_identity,
            )),
            "",
            (33, 44),
            || Ok(expected_root_identity),
            || {
                calls.set(calls.get() + 1);
                "captured"
            },
        );
        assert!(result.is_err());
        assert_eq!(
            calls.get(),
            0,
            "a real bind failure must NEVER invoke the capture/spawn continuation"
        );
        lease.release_unused().expect(
            "an Allocated lease untouched by a caller-bug bind refusal must still release cleanly",
        );
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_then_settle_releases_cleanly_on_a_matching_evidence() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("acquire-settle-ok")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("acquire-settle-ok")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let container_id = "acquire-settle-ok-container";
        let (cfg, mut context) = acquire_enabled_workspace(
            &command_spec,
            &profile,
            container_id,
            PathBuf::from("/abs/staged-rootfs"),
            &workspace_manager,
            &userns_allocator,
            None,
        )
        .expect("acquisition must succeed against a healthy real manager/allocator");
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::ExplicitUserNamespace(context.lease.config())
        );
        assert_eq!(context.bind_state, LeaseBindState::Allocated);

        let runsc_root_identity = (11, 22);
        let cgroup_identity = (33, 44);
        context
            .lease
            .bind(
                container_id.to_string(),
                runsc_root_identity,
                cgroup_identity,
            )
            .expect("bind must succeed for a fresh Allocated lease");
        context.bind_state = LeaseBindState::Bound {
            container_id: container_id.to_string(),
            runsc_root_identity,
            cgroup_identity,
        };
        let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            container_id.to_string(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity,
            },
            CgroupQuiescenceEvidence::assert_for_tests(cgroup_identity),
        );
        settle_enabled_workspace_and_lease(context, &workspace_manager, &evidence)
            .expect("settling a matching evidence against a Bound lease must succeed");
        assert!(workspace_manager.is_healthy());
        assert!(userns_allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn settle_enabled_workspace_and_lease_refuses_evidence_disagreeing_with_the_recorded_binding() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("settle-mismatch")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("settle-mismatch")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let container_id = "settle-mismatch-container";
        let (_cfg, mut context) = acquire_enabled_workspace(
            &command_spec,
            &profile,
            container_id,
            PathBuf::from("/abs/staged-rootfs"),
            &workspace_manager,
            &userns_allocator,
            None,
        )
        .expect("acquisition must succeed");
        let runsc_root_identity = (11, 22);
        let cgroup_identity = (33, 44);
        context
            .lease
            .bind(
                container_id.to_string(),
                runsc_root_identity,
                cgroup_identity,
            )
            .expect("bind must succeed");
        context.bind_state = LeaseBindState::Bound {
            container_id: container_id.to_string(),
            runsc_root_identity,
            cgroup_identity,
        };
        let host_path = context.workspace.host_path().unwrap().to_path_buf();
        let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
            container_id.to_string(),
            RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity,
            },
            CgroupQuiescenceEvidence::assert_for_tests((99, 99)),
        );
        let result = settle_enabled_workspace_and_lease(context, &workspace_manager, &evidence);
        assert!(
            result.is_err(),
            "evidence disagreeing with the recorded binding must refuse, not silently release"
        );
        assert!(
            host_path.exists(),
            "the abandoned subvolume must still be real and on disk"
        );
        drop(workspace_manager);
        let sink2: crate::workspace_manager::IncidentSink =
            Arc::new(|msg: &str| eprintln!("[piece7c workspace incident] {msg}"));
        let fresh = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: workspace_base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink2,
        )
        .expect("a fresh manager's own boot reconciliation must clean up the orphan and succeed");
        assert!(fresh.is_healthy());
        assert!(
            !host_path.exists(),
            "boot reconciliation must have deleted the abandoned subvolume for real"
        );
        drop(fresh);
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn cleanup_pre_bind_failure_abandons_both_resources_when_bind_state_is_bound() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_for_tests("bound-abandons-both")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("bound-abandons-both")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let container_id = "bound-abandons-both-container";
        let (_cfg, mut context) = acquire_enabled_workspace(
            &command_spec,
            &profile,
            container_id,
            PathBuf::from("/abs/staged-rootfs"),
            &workspace_manager,
            &userns_allocator,
            None,
        )
        .expect("acquisition must succeed");
        let runsc_root_identity = (11, 22);
        let cgroup_identity = (33, 44);
        context
            .lease
            .bind(
                container_id.to_string(),
                runsc_root_identity,
                cgroup_identity,
            )
            .expect("bind must succeed");
        context.bind_state = LeaseBindState::Bound {
            container_id: container_id.to_string(),
            runsc_root_identity,
            cgroup_identity,
        };
        let host_path = context.workspace.host_path().unwrap().to_path_buf();

        let diagnostics = cleanup_pre_bind_failure(context, &workspace_manager);

        assert_eq!(
            diagnostics.len(),
            1,
            "a Bound outer error must always surface exactly one invariant-violation diagnostic, \
             never an empty vec: {diagnostics:?}"
        );
        assert!(diagnostics[0].contains("structurally impossible"));
        assert!(
            host_path.exists(),
            "the workspace must be ABANDONED, not deleted, when bind_state was Bound"
        );
        assert!(
            !workspace_manager.is_healthy(),
            "abandoning the workspace without deleting it must poison the manager"
        );
        assert!(
            !userns_allocator.is_healthy(),
            "abandoning a Bound lease without releasing it must poison the allocator too"
        );

        drop(workspace_manager);
        let sink2: crate::workspace_manager::IncidentSink =
            Arc::new(|msg: &str| eprintln!("[piece7c workspace incident] {msg}"));
        let fresh = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: workspace_base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink2,
        )
        .expect("a fresh manager's own boot reconciliation must clean up the orphan and succeed");
        assert!(!host_path.exists());
        drop(fresh);
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_refuses_when_capacity_is_exhausted_and_touches_nothing_else() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_without_qgroup_probe_for_tests("capacity-exhausted")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("capacity-exhausted")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let holder = workspace_manager
            .acquire_capacity(1 << 30)
            .expect("the fresh manager's own full ceiling must be leasable once");
        let mut command_spec = spec(vec![]);
        command_spec.limits.disk_bytes = 1;
        let profile = HardeningProfile::derive(&command_spec);
        let result = acquire_enabled_workspace(
            &command_spec,
            &profile,
            "capacity-exhausted-container",
            PathBuf::from("/abs/staged-rootfs"),
            &workspace_manager,
            &userns_allocator,
            None,
        );
        assert!(
            result.is_err(),
            "an exhausted ceiling must refuse acquisition"
        );
        assert!(
            userns_allocator.quarantined_slots().is_empty(),
            "acquire_enabled_workspace must never have leased (and left quarantined) a userns \
             slot when capacity refused first: {:?}",
            userns_allocator.quarantined_slots()
        );
        holder.release();
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_given_releases_capacity_when_userns_lease_is_refused() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_without_qgroup_probe_for_tests("userns-refused")
        else {
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let result = acquire_enabled_workspace_given(
            &command_spec,
            &profile,
            "container-userns-refused",
            PathBuf::from("/abs/staged-rootfs"),
            |bytes| workspace_manager.acquire_capacity(bytes),
            || Err(UserNamespaceRefusal::PoolExhausted { pool_size: 0 }),
            |_, _, _, _, _| {
                panic!("create_workspace must never run when the lease is refused first")
            },
            |_| panic!("delete_workspace must never run on this path"),
        );
        assert!(
            result.is_err(),
            "a refused userns lease must refuse acquisition"
        );
        let holder = workspace_manager
            .acquire_capacity(1 << 30)
            .expect("capacity must have been released back to the pool after the userns refusal");
        holder.release();
        let _ = std::fs::remove_dir_all(&workspace_base);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_given_releases_the_lease_on_a_recoverable_provisioning_failure() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_without_qgroup_probe_for_tests("recoverable-storage-failure")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("recoverable-storage-failure")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let result = acquire_enabled_workspace_given(
            &command_spec,
            &profile,
            "container-recoverable-failure",
            PathBuf::from("/abs/staged-rootfs"),
            |bytes| workspace_manager.acquire_capacity(bytes),
            || userns_allocator.lease(),
            |_, _, _, _, capacity: CapacityLease| {
                capacity.release();
                Err(WorkspaceProvisionError::Storage(
                    WorkspaceStorageError::ZeroQuota,
                ))
            },
            |_| panic!("delete_workspace must never run - no workspace was ever created"),
        );
        assert!(
            result.is_err(),
            "a recoverable provisioning failure must refuse acquisition"
        );
        assert!(
            userns_allocator.quarantined_slots().is_empty(),
            "a recoverable failure must release_unused() the lease, not quarantine it: {:?}",
            userns_allocator.quarantined_slots()
        );
        assert!(
            workspace_manager.is_healthy(),
            "a recoverable failure must leave the workspace manager healthy (capacity released \
             cleanly, not abandoned)"
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn acquire_enabled_workspace_given_quarantines_the_lease_on_an_unrecoverable_leak() {
        let Some((workspace_manager, workspace_base)) =
            real_workspace_manager_without_qgroup_probe_for_tests("unrecoverable-leak")
        else {
            return;
        };
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("unrecoverable-leak")
        else {
            let _ = std::fs::remove_dir_all(&workspace_base);
            return;
        };
        let command_spec = spec(vec![]);
        let profile = HardeningProfile::derive(&command_spec);
        let result = acquire_enabled_workspace_given(
            &command_spec,
            &profile,
            "container-unrecoverable-leak",
            PathBuf::from("/abs/staged-rootfs"),
            |bytes| workspace_manager.acquire_capacity(bytes),
            || userns_allocator.lease(),
            |_, _, _, _, _capacity| {
                Err(WorkspaceProvisionError::Storage(
                    WorkspaceStorageError::UnrecoverableLeak {
                        path: PathBuf::from("/fake/leaked/path"),
                        subvol_id: None,
                        provisioning_error: "synthetic provisioning error".to_string(),
                        cleanup_error: "synthetic cleanup error".to_string(),
                    },
                ))
            },
            |_| panic!("delete_workspace must never run - no workspace was ever created"),
        );
        assert!(
            result.is_err(),
            "an unrecoverable leak must refuse acquisition"
        );
        assert_eq!(
            userns_allocator.quarantined_slots().len(),
            1,
            "an UnrecoverableLeak must quarantine (never release_unused()) the lease: {:?}",
            userns_allocator.quarantined_slots()
        );
        let _ = std::fs::remove_dir_all(&workspace_base);
        let _ = std::fs::remove_dir_all(&leases_dir);
    }
}
