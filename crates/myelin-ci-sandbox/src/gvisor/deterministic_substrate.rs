use super::*;
use crate::SandboxResult;

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub(crate) struct SubstitutedCheckoutObservation {
    pub(crate) hopb_write_ok: bool,
    pub(crate) used_after_hopb: u64,
    pub(crate) used_at_workload_checkpoint: u64,
    pub(crate) mount_source_matched_workspace: bool,
    pub(crate) sentinel_read_through_mount: bool,
    pub(crate) settled_ok: bool,
    pub(crate) settle_error: Option<String>,
}

#[cfg(any(test, feature = "test-support"))]
struct SubstituteWorkloadChild;

#[cfg(any(test, feature = "test-support"))]
impl RunscChild for SubstituteWorkloadChild {
    fn kill(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn wait(&mut self) -> Result<i32, String> {
        Ok(0)
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(super) fn finalized_for_test_support(
    evidence: RuntimeQuiescenceEvidence,
) -> RuntimeFinalization<Result<ContainerRun, RunFailure>> {
    RuntimeFinalization::Finalized(FinalizedRun {
        primary: Ok(ContainerRun {
            child: Box::new(SubstituteWorkloadChild),
            bundle_dir: std::env::temp_dir()
                .join("myelin-substituted-checkout-fake-bundle-does-not-exist"),
            result: SandboxResult::stub_ok(crate::ResourceUsage {
                cpu_seconds: 3,
                mem_byte_seconds: 7,
            }),
            run_error: None,
        }),
        evidence,
    })
}

#[cfg(any(test, feature = "test-support"))]
pub(super) fn substitute_checkout_spec() -> crate::JobSpec {
    crate::JobSpec::new(
        crate::JobKind::Ci,
        crate::ImageRef::pinned(format!("test.local/substrate@sha256:{}", "a".repeat(64))).unwrap(),
        vec!["true".into()],
        vec![],
        vec![],
        crate::EgressPolicy { allow: vec![] },
        crate::ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 64,
            timeout_secs: 120,
        },
        crate::WorkspaceSpec {
            repo_ref: Some("myelin://acme/git/repo/widgets".to_string()),
            commit: Some("a".repeat(40)),
        },
        crate::TrustTier::UntrustedFork,
        crate::RunTokenCredential::new("test-bearer", "j", 300).unwrap(),
        crate::MeterTarget {
            reserve_id: "r".into(),
        },
        crate::IdemToken("idem-6e1b".into()),
    )
    .unwrap()
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn deterministic_workspace_manager_for_tests(
    base_dir: std::path::PathBuf,
    host_capacity_bytes: u64,
) -> Result<
    crate::workspace_manager::WorkspaceManager,
    crate::workspace_manager::WorkspaceManagerError,
> {
    let sink: crate::workspace_manager::IncidentSink =
        std::sync::Arc::new(|msg: &str| eprintln!("[6e.1b workspace incident] {msg}"));
    crate::workspace_manager::WorkspaceManager::try_new(
        crate::workspace_manager::WorkspaceStorageMode::LocalDevelopmentDirectory {
            base_dir,
            host_capacity_bytes,
        },
        sink,
    )
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn deterministic_userns_allocator_for_tests(
    base_dir: &std::path::Path,
    pool_size: u32,
) -> Result<
    crate::user_namespace::UserNamespaceAllocator,
    crate::user_namespace::UserNamespaceAllocatorError,
> {
    let uid = unsafe { libc::geteuid() };
    let subuid = base_dir.join("subuid");
    let subgid = base_dir.join("subgid");
    std::fs::write(&subuid, format!("{uid}:100000:{pool_size}\n")).expect("write fixture subuid");
    std::fs::write(&subgid, format!("{uid}:200000:{pool_size}\n")).expect("write fixture subgid");
    let leases_dir = base_dir.join("leases");
    let sink: crate::user_namespace::IncidentSink =
        std::sync::Arc::new(|msg: &str| eprintln!("[6e.1b userns incident] {msg}"));
    crate::user_namespace::UserNamespaceAllocator::try_new_for_tests(
        leases_dir, &subuid, &subgid, pool_size, sink,
    )
}

#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::type_complexity)]
fn acquire_deterministic_checkout_capsule(
    root: &std::path::Path,
) -> (
    AcquiredCheckoutRuntime,
    crate::workspace_manager::WorkspaceManager,
    std::path::PathBuf,
    crate::user_namespace::UserNamespaceAllocator,
    std::path::PathBuf,
) {
    let workspace_base = root.join("workspace");
    let userns_base = root.join("userns");
    std::fs::create_dir_all(&workspace_base).expect("mk workspace base");
    std::fs::create_dir_all(&userns_base).expect("mk userns base");
    let workspace_manager =
        deterministic_workspace_manager_for_tests(workspace_base.clone(), 1 << 30)
            .expect("deterministic directory workspace manager must construct");
    let userns_allocator = deterministic_userns_allocator_for_tests(&userns_base, 1)
        .expect("deterministic userns allocator must construct (a NON-root user is required)");
    let spec = substitute_checkout_spec();
    let profile = crate::hardening::HardeningProfile::derive(&spec);
    let runtime = AcquiredCheckoutRuntime::acquire(
        &spec,
        &profile,
        std::path::PathBuf::from("/abs/staged-rootfs"),
        &workspace_manager,
        &userns_allocator,
        WorkspaceProcessIdentity::Isolated,
        None,
    )
    .expect("acquisition must succeed against a healthy deterministic manager/allocator");
    (
        runtime,
        workspace_manager,
        workspace_base,
        userns_allocator,
        userns_base,
    )
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubstitutedEvidenceMode {
    DerivedFromBind,
    #[cfg(test)]
    MismatchedRunscRoot,
}

#[cfg(any(test, feature = "test-support"))]
pub(super) struct NoOpTestSupportAuthority;

#[cfg(any(test, feature = "test-support"))]
impl NoOpTestSupportAuthority {
    fn authorization_context() -> crate::RunTokenAuthorizationContext {
        crate::RunTokenAuthorizationContext::CiJob(crate::CiJobAuthorizationContext {
            tenant_id: "acme".to_string(),
            region: "fr-par".to_string(),
            principal_id: "p".to_string(),
            project_id: "00000000-0000-0000-0000-000000000001".to_string(),
            wf_run_id: "wf".to_string(),
            job_id: "j".to_string(),
            lease_owner: "o".to_string(),
            lease_epoch: 1,
            claim_nonce: "n".to_string(),
            claim_started_at_epoch_secs: 0,
            claim_expires_at_epoch_secs: 1,
            reserve_id: "r".to_string(),
            required_capabilities: vec![],
            checkout_scope: None,
            credential_binding: None,
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
impl crate::checkout_orchestration::AttemptAuthority for NoOpTestSupportAuthority {
    fn begin_phase(
        &self,
        _phase: crate::runner::PreparationPhase,
    ) -> Result<(), crate::checkout_orchestration::AttemptAuthorityError> {
        Ok(())
    }
    fn complete_phase(
        &self,
        _phase: crate::runner::PreparationPhase,
        _usage: crate::ResourceUsage,
    ) -> Result<(), crate::checkout_orchestration::AttemptAuthorityError> {
        Ok(())
    }
    fn seal_phase(
        &self,
        _phase: crate::runner::PreparationPhase,
    ) -> Result<(), crate::checkout_orchestration::AttemptAuthorityError> {
        Ok(())
    }
    fn renew_preparation_lease(&self) -> Result<(), crate::runner::PreparationLeaseLost> {
        Ok(())
    }
    fn mint_phase_credential(
        &self,
        phase: crate::CheckoutPhase,
    ) -> Result<
        crate::checkout_orchestration::PhaseCredentialCarrier,
        crate::checkout_orchestration::AttemptAuthorityError,
    > {
        let seed = match phase {
            crate::CheckoutPhase::Advertise => "advertise",
            crate::CheckoutPhase::Fetch => "fetch",
            crate::CheckoutPhase::Materialization => "materialization",
        };
        Ok(crate::checkout_orchestration::PhaseCredentialCarrier::new(
            crate::RunTokenCredential::new("bearer", format!("jti-noop-{seed}"), 300).unwrap(),
            Self::authorization_context(),
            format!("gen-noop-{seed}"),
        ))
    }
    fn mint_workload_credential(
        &self,
    ) -> Result<
        crate::checkout_orchestration::WorkloadCredentialCarrier,
        crate::checkout_orchestration::AttemptAuthorityError,
    > {
        Ok(
            crate::checkout_orchestration::WorkloadCredentialCarrier::new(
                crate::RunTokenCredential::new("bearer", "jti-noop-workload", 300).unwrap(),
                Self::authorization_context(),
                "gen-noop-workload",
            ),
        )
    }
    fn should_requeue(&self) -> bool {
        true
    }
}

#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::type_complexity)]
fn run_substituted_checkout_inner(
    root: &std::path::Path,
    sentinel_name: &str,
    sentinel_bytes: &[u8],
    evidence_mode: SubstitutedEvidenceMode,
) -> (
    SubstitutedCheckoutObservation,
    crate::workspace_manager::WorkspaceManager,
    std::path::PathBuf,
    crate::user_namespace::UserNamespaceAllocator,
    std::path::PathBuf,
) {
    let (runtime, workspace_manager, workspace_base, userns_allocator, userns_base) =
        acquire_deterministic_checkout_capsule(root);
    let base_spec = substitute_checkout_spec();
    let authority = NoOpTestSupportAuthority;
    let hooks = crate::RunnerHooks::new(
        crate::CompletionSettlementOwner::TerminalReporter,
        Box::new(|s| Ok(crate::ReserveHandle(s.meter_to.reserve_id.clone()))),
        Box::new(|_, _, _| Ok(())),
        Box::new(|_| Ok(())),
        Box::new(|_| Ok(())),
    )
    .with_checkout_phase_authorization(Box::new(|_spec, _scope, _phase| {
        Ok(crate::LaunchPermit::immediate())
    }));
    let scope = crate::derive_checkout_authorization_scope(base_spec.kind, &base_spec.workspace)
        .expect("scope derives")
        .expect("the substituted checkout spec is checkout-bearing");
    let expected_commit = crate::workspace_intent::ExpectedGitCommitId::new(
        scope.commit_hex().to_string(),
        scope.commit_format(),
    )
    .expect("the capsule scope's commit is a well-formed expected commit id");
    let carrier = crate::checkout_orchestration::PhaseCredentialCarrier::new(
        crate::RunTokenCredential::new("bearer", "materialization-6e1b-jti", 300).unwrap(),
        NoOpTestSupportAuthority::authorization_context(),
        "gen-mat-6e1b",
    );
    let (mat_run_token, mat_authorization) =
        crate::checkout_orchestration::authorize_phase_generation(
            &hooks,
            &base_spec,
            &scope,
            crate::CheckoutPhase::Materialization,
            carrier,
        )
        .expect("mint the materialization phase authorization");
    let (prepared, hopb_write_ok, used_after_hopb) = match runtime
        .substituted_hop_b_for_test_support(
            &mat_run_token,
            mat_authorization,
            &expected_commit,
            sentinel_name,
            sentinel_bytes,
            None,
        ) {
        Ok(triple) => triple,
        Err((_capsule, error)) => panic!(
            "the matching materialization authorization must resolve and Hop B must prepare the \
             capsule, but it was refused: {error}"
        ),
    };
    let (
        outcome,
        used_at_workload_checkpoint,
        mount_source_matched_workspace,
        sentinel_read_through_mount,
    ) = prepared.substituted_workload_for_test_support(
        &authority,
        &hooks,
        &base_spec,
        &workspace_manager,
        sentinel_name,
        sentinel_bytes,
        evidence_mode,
    );
    let (settled_ok, settle_error) = match outcome {
        RetainedWorkloadOutcome::Ran(Ok(_)) => (true, None),
        RetainedWorkloadOutcome::Ran(Err(failure)) => (false, Some(format!("{failure:?}"))),
        RetainedWorkloadOutcome::RunFailed {
            failure,
            disposal_diagnostics,
        } => (
            false,
            Some(format!(
                "RunFailed: {failure:?}; disposal={disposal_diagnostics:?}"
            )),
        ),
        RetainedWorkloadOutcome::PhaseAuthorityFailed {
            error,
            disposal_diagnostics,
        } => (
            false,
            Some(format!(
                "PhaseAuthorityFailed: {error:?}; disposal={disposal_diagnostics:?}"
            )),
        ),
        RetainedWorkloadOutcome::LeaseLost {
            lost,
            disposal_diagnostics,
        } => (
            false,
            Some(format!(
                "LeaseLost: {lost:?}; disposal={disposal_diagnostics:?}"
            )),
        ),
        RetainedWorkloadOutcome::PermitRefused {
            message,
            disposal_diagnostics,
        } => (
            false,
            Some(format!(
                "PermitRefused: {message}; disposal={disposal_diagnostics:?}"
            )),
        ),
    };
    let observation = SubstitutedCheckoutObservation {
        hopb_write_ok,
        used_after_hopb,
        used_at_workload_checkpoint,
        mount_source_matched_workspace,
        sentinel_read_through_mount,
        settled_ok,
        settle_error,
    };
    (
        observation,
        workspace_manager,
        workspace_base,
        userns_allocator,
        userns_base,
    )
}

#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::type_complexity)]
pub(crate) fn run_substituted_checkout_success(
    root: &std::path::Path,
    sentinel_name: &str,
    sentinel_bytes: &[u8],
) -> (
    SubstitutedCheckoutObservation,
    crate::workspace_manager::WorkspaceManager,
    std::path::PathBuf,
    crate::user_namespace::UserNamespaceAllocator,
    std::path::PathBuf,
) {
    run_substituted_checkout_inner(
        root,
        sentinel_name,
        sentinel_bytes,
        SubstitutedEvidenceMode::DerivedFromBind,
    )
}

#[cfg(test)]
#[allow(clippy::type_complexity)]
pub(crate) fn run_substituted_checkout_mismatched_evidence(
    root: &std::path::Path,
    sentinel_name: &str,
    sentinel_bytes: &[u8],
) -> (
    SubstitutedCheckoutObservation,
    crate::workspace_manager::WorkspaceManager,
    std::path::PathBuf,
    crate::user_namespace::UserNamespaceAllocator,
    std::path::PathBuf,
) {
    run_substituted_checkout_inner(
        root,
        sentinel_name,
        sentinel_bytes,
        SubstitutedEvidenceMode::MismatchedRunscRoot,
    )
}

#[cfg(test)]
mod tests {

    mod deterministic_substrate_6e1b {
        use crate::gvisor::{
            acquire_enabled_workspace, classify_workspace_deletion,
            deterministic_userns_allocator_for_tests, deterministic_workspace_manager_for_tests,
            run_substituted_checkout_mismatched_evidence, run_substituted_checkout_success,
            settle_enabled_workspace_and_lease, substitute_checkout_spec, unique_suffix,
            CgroupQuiescenceEvidence, EnabledWorkspaceRequest, LeaseBindState,
            RuntimeNamespaceQuiescence, RuntimeQuiescenceEvidence, WorkspaceDeletionOutcome,
        };
        use crate::user_namespace::{
            CheckoutPreparationSession, PreparationQuiescenceProof, UserNamespaceQuiescenceProof,
        };
        use crate::workspace_manager::{
            DeleteWorkspaceError, WorkspaceAdmission, WorkspaceManagerError,
            WorkspaceProvisionError,
        };
        use crate::workspace_storage::{
            DirectoryWorkspaceStorage, PreparedWorkspace, WorkspaceStorageError,
        };
        use std::path::PathBuf;

        fn temp_root(tag: &str) -> PathBuf {
            let root = std::env::temp_dir().join(format!(
                "myelin-6e1b-{tag}-{}-{}",
                std::process::id(),
                unique_suffix()
            ));
            std::fs::create_dir_all(&root).expect("mk temp root");
            root
        }

        fn open_directory_backend(tag: &str) -> (DirectoryWorkspaceStorage, PathBuf) {
            let base = temp_root(tag).join("dir-backend");
            std::fs::create_dir_all(&base).expect("mk base");
            let backend = DirectoryWorkspaceStorage::open(&base)
                .expect("directory backend opens over an exclusively-owned dir");
            let canonical = backend.base_dir().to_path_buf();
            (backend, canonical)
        }

        #[test]
        fn directory_create_write_read_delete_absence() {
            let base = temp_root("t1").join("workspace");
            std::fs::create_dir_all(&base).unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 1 << 30).unwrap();
            let cap = wm.acquire_capacity(1 << 20).expect("capacity");
            let ws = wm
                .create_workspace("job-t1", 1 << 20, 0, 0, cap)
                .expect("create directory workspace");
            let host = ws.host_path().unwrap().to_path_buf();
            assert!(host.is_dir(), "a fresh leaf directory exists");
            ws.checked_test_quota_write("checkout.sentinel", b"provenance")
                .expect("the checked byte-accounted write succeeds under quota");
            assert_eq!(
                std::fs::read(host.join("checkout.sentinel")).unwrap(),
                b"provenance",
                "the sentinel reads back byte-identical"
            );
            let refusal = ws.checked_test_quota_write("huge", &vec![0u8; (1 << 20) + 1]);
            assert!(
                matches!(
                    refusal,
                    Err(WorkspaceStorageError::DirectoryQuotaExceeded { .. })
                ),
                "an over-quota checked write refuses, got {refusal:?}"
            );
            assert!(
                !host.join("huge").exists(),
                "the refused over-quota write left nothing behind"
            );
            wm.delete_workspace(ws).expect("delete proves absence");
            assert!(
                !host.exists(),
                "the leaf is gone after a proven-absence delete"
            );
            assert_eq!(
                wm.capacity_used_bytes(),
                0,
                "capacity released after delete"
            );
            let _ = std::fs::remove_dir_all(&base);
        }

        #[test]
        fn capacity_leased_then_released_and_reusable() {
            let base = temp_root("t2").join("workspace");
            std::fs::create_dir_all(&base).unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 4 << 20).unwrap();
            let cap = wm
                .acquire_capacity(4 << 20)
                .expect("lease the whole ceiling");
            assert_eq!(wm.capacity_used_bytes(), 4 << 20);
            assert!(wm.acquire_capacity(1).is_err(), "the ceiling is exhausted");
            let ws = wm
                .create_workspace("job-t2", 4 << 20, 0, 0, cap)
                .expect("create consumes the lease");
            wm.delete_workspace(ws)
                .expect("delete releases the capacity");
            assert_eq!(wm.capacity_used_bytes(), 0, "capacity fully returned");
            let again = wm
                .acquire_capacity(4 << 20)
                .expect("reuse the freed ceiling");
            again.release();
            let _ = std::fs::remove_dir_all(&base);
        }

        #[test]
        fn real_userns_preparation_bind_workload_release_transitions() {
        if crate::fake_root_test_environment_skip("real userns lease semantics") {
            return;
        }
            let base = temp_root("t3").join("userns");
            std::fs::create_dir_all(&base).unwrap();
            let alloc = deterministic_userns_allocator_for_tests(&base, 1)
                .expect("a NON-root user builds the fixture allocator");
            let mut lease = alloc.lease().expect("a fresh pool leases");
            let mut session = CheckoutPreparationSession::new();
            let (prep_root, prep_cgroup) = ((1_u64, 2_u64), (3_u64, 4_u64));
            session
                .bind_preparation(&mut lease, "c-prep".to_string(), prep_root, prep_cgroup)
                .expect("Allocated -> PreparationBound");
            let prep_ev = RuntimeQuiescenceEvidence::assert_for_tests(
                "c-prep".to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: prep_root,
                },
                CgroupQuiescenceEvidence::assert_for_tests(prep_cgroup),
            );
            let prep_proof = PreparationQuiescenceProof::from_runtime_evidence(&lease, &prep_ev)
                .expect("a matching prep evidence mints a proof");
            session
                .confirm_prepared(&mut lease, prep_proof)
                .expect("PreparationBound -> Prepared");
            let (wl_root, wl_cgroup) = ((5_u64, 6_u64), (7_u64, 8_u64));
            session
                .bind_workload(&mut lease, "c-workload".to_string(), wl_root, wl_cgroup)
                .expect("Prepared -> Bound");
            let wl_ev = RuntimeQuiescenceEvidence::assert_for_tests(
                "c-workload".to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: wl_root,
                },
                CgroupQuiescenceEvidence::assert_for_tests(wl_cgroup),
            );
            let proof = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, &wl_ev)
                .expect("the workload evidence mints a release proof");
            lease.release(proof).expect("release with a matching proof");
            assert!(alloc.is_healthy(), "the allocator stays healthy");
            let probe = alloc.lease().expect("the slot is reusable after release");
            probe
                .release_unused()
                .expect("the probe lease releases cleanly");
            assert!(
                alloc.is_healthy(),
                "the allocator is STILL clean after the probe"
            );
            let _ = std::fs::remove_dir_all(&base);
        }

        #[test]
        fn full_capsule_substituted_hopb_and_workload_then_real_settle() {
        if crate::fake_root_test_environment_skip("real userns lease semantics") {
            return;
        }
            let root = temp_root("t4");
            let (obs, wm, workspace_base, alloc, userns_base) =
                run_substituted_checkout_success(&root, "checkout.sentinel", b"shared-provenance");
            assert!(
                obs.hopb_write_ok,
                "the checked Hop B sentinel write succeeded"
            );
            assert!(
                obs.used_after_hopb >= "shared-provenance".len() as u64,
                "the byte-accounted checkpoint saw Hop B's bytes: {}",
                obs.used_after_hopb
            );
            assert_eq!(
                obs.used_at_workload_checkpoint, obs.used_after_hopb,
                "the re-scan at the workload checkpoint agrees"
            );
            assert!(
                obs.mount_source_matched_workspace,
                "the retained OCI mount source equals the capsule workspace host path"
            );
            assert!(
                obs.sentinel_read_through_mount,
                "the substituted workload read the sentinel THROUGH the OCI-recorded mount"
            );
            assert!(
                obs.settled_ok,
                "the real settle tail succeeded: {:?}",
                obs.settle_error
            );
            let child_dirs = std::fs::read_dir(&workspace_base)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .count();
            assert_eq!(child_dirs, 0, "the workspace leaf was deleted by settle");
            assert_eq!(wm.capacity_used_bytes(), 0, "capacity zero after settle");
            assert!(wm.is_healthy(), "the workspace manager stays healthy");
            assert!(alloc.is_healthy(), "the userns allocator stays healthy");
            let probe = alloc.lease().expect("the userns slot is reusable");
            probe
                .release_unused()
                .expect("the probe lease releases cleanly");
            assert!(
                alloc.is_healthy(),
                "the allocator is STILL clean after the probe"
            );
            let _ = std::fs::remove_dir_all(&workspace_base);
            let _ = std::fs::remove_dir_all(&userns_base);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn cross_backend_base_inode_and_symlink_substitutions_refuse_without_deleting() {
            let (mut backend, canonical) = open_directory_backend("t5a");
            let btrfs_cap =
                PreparedWorkspace::for_tests(canonical.join("x"), 42, canonical.clone());
            assert!(
                matches!(
                    backend.delete_workspace(btrfs_cap),
                    Err(WorkspaceStorageError::BackendMismatch { .. })
                ),
                "a Btrfs capability is refused by the directory backend"
            );

            let (mut backend_a, _) = open_directory_backend("t5b-a");
            let (mut backend_b, _) = open_directory_backend("t5b-b");
            let ws_a = backend_a.create_workspace("job", 1 << 20, 0, 0).unwrap();
            let leaf_a = ws_a.host_path().to_path_buf();
            assert!(
                matches!(
                    backend_b.delete_workspace(ws_a),
                    Err(WorkspaceStorageError::WrongStorage { .. })
                ),
                "backend B refuses backend A's capability"
            );
            assert!(
                leaf_a.exists(),
                "the refused wrong-base delete removed nothing"
            );
            backend_a
                .list_orphaned_workspaces(&std::collections::BTreeSet::new())
                .and_then(|orphans| {
                    orphans
                        .into_iter()
                        .try_for_each(|o| backend_a.delete_orphan(o))
                })
                .unwrap();

            let (mut backend_c, _) = open_directory_backend("t5c");
            let ws_c = backend_c.create_workspace("job", 1 << 20, 0, 0).unwrap();
            let leaf_c = ws_c.host_path().to_path_buf();
            std::fs::remove_dir_all(&leaf_c).unwrap();
            std::fs::create_dir(&leaf_c).unwrap();
            assert!(
                matches!(
                    backend_c.delete_workspace(ws_c),
                    Err(WorkspaceStorageError::DirectoryAbsenceUnproven { .. })
                ),
                "an inode-substituted leaf refuses deletion (absence unproven)"
            );
            assert!(
                leaf_c.exists(),
                "the substituted replacement dir was NOT deleted"
            );

            let (mut backend_d, _) = open_directory_backend("t5d");
            let ws_d = backend_d.create_workspace("job", 1 << 20, 0, 0).unwrap();
            let leaf_d = ws_d.host_path().to_path_buf();
            let decoy = leaf_d.with_file_name("decoy-target");
            std::fs::create_dir(&decoy).unwrap();
            std::fs::write(decoy.join("keep"), b"do not delete").unwrap();
            std::fs::remove_dir_all(&leaf_d).unwrap();
            std::os::unix::fs::symlink(&decoy, &leaf_d).unwrap();
            assert!(
                matches!(
                    backend_d.delete_workspace(ws_d),
                    Err(WorkspaceStorageError::DirectoryAbsenceUnproven { .. })
                ),
                "a symlink-substituted leaf refuses deletion"
            );
            assert!(
                decoy.join("keep").exists(),
                "the symlink was never followed - the decoy target is intact"
            );
        }

        #[test]
        fn injected_delete_failure_retains_capacity_and_poisons_the_manager() {
            let base = temp_root("t6").join("workspace");
            std::fs::create_dir_all(&base).unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 1 << 30).unwrap();
            let cap = wm.acquire_capacity(1 << 20).unwrap();
            let ws = wm.create_workspace("job-t6", 1 << 20, 0, 0, cap).unwrap();
            let host = ws.host_path().unwrap().to_path_buf();
            std::fs::remove_dir_all(&host).unwrap();
            std::fs::create_dir(&host).unwrap();
            let result = wm.delete_workspace(ws);
            assert!(
                matches!(result, Err(DeleteWorkspaceError::Storage(_))),
                "the delete surfaced a storage failure, got {result:?}"
            );
            assert!(
                matches!(wm.admission(), WorkspaceAdmission::Poisoned { .. }),
                "an absence-unproven delete poisons the manager"
            );
            assert_eq!(
                wm.capacity_used_bytes(),
                1 << 20,
                "capacity is RETAINED (never silently freed) on an unproven delete"
            );
            let outcome = classify_workspace_deletion(Err(DeleteWorkspaceError::Storage(
                WorkspaceStorageError::DirectoryAbsenceUnproven {
                    path: host.clone(),
                    reason: "injected".to_string(),
                },
            )));
            assert!(
                matches!(outcome, WorkspaceDeletionOutcome::NotProvenAbsent { .. }),
                "an unproven delete leaves the userns lease unreleased/quarantined"
            );
            let _ = std::fs::remove_dir_all(&base);
        }

        #[test]
        fn boot_orphan_reconciliation_deletes_orphans_and_refuses_malformed_entries() {
            let base = temp_root("t7-ok").join("workspace");
            std::fs::create_dir_all(base.join("orphan-a")).unwrap();
            std::fs::create_dir_all(base.join("orphan-b")).unwrap();
            std::fs::write(base.join("orphan-a").join("junk"), b"stale").unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 1 << 30)
                .expect("construction reconciles orphans");
            assert!(matches!(wm.admission(), WorkspaceAdmission::Healthy));
            let remaining = std::fs::read_dir(&base)
                .unwrap()
                .filter_map(Result::ok)
                .count();
            assert_eq!(
                remaining, 0,
                "every boot orphan was deleted before admission"
            );
            drop(wm);
            let _ = std::fs::remove_dir_all(&base);

            let base2 = temp_root("t7-bad").join("workspace");
            std::fs::create_dir_all(&base2).unwrap();
            std::fs::write(base2.join("not-a-dir"), b"stray").unwrap();
            let result = deterministic_workspace_manager_for_tests(base2.clone(), 1 << 30);
            assert!(
                matches!(
                    result,
                    Err(WorkspaceManagerError::Storage(
                        WorkspaceStorageError::UnexpectedEntry { .. }
                    ))
                ),
                "a non-directory boot entry is a loud UnexpectedEntry refusal"
            );
            drop(result);
            let _ = std::fs::remove_dir_all(&base2);
        }

        #[test]
        fn injected_create_failure_retains_capacity_and_poisons_without_a_healthy_residual() {
            let base = temp_root("t9").join("workspace");
            std::fs::create_dir_all(&base).unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 1 << 30).unwrap();
            let canonical = std::fs::canonicalize(&base).unwrap();
            std::fs::create_dir(canonical.join("job-t9")).unwrap();
            let cap = wm.acquire_capacity(1 << 20).unwrap();
            let result = wm.create_workspace("job-t9", 1 << 20, 0, 0, cap);
            assert!(
                matches!(
                    result,
                    Err(WorkspaceProvisionError::Storage(
                        WorkspaceStorageError::UnrecoverableLeak { .. }
                    ))
                ),
                "a pre-existing untracked leaf is an UnrecoverableLeak, got {result:?}"
            );
            assert!(
                matches!(wm.admission(), WorkspaceAdmission::Poisoned { .. }),
                "the manager is poisoned (NOT healthy) while the residual survives"
            );
            assert_eq!(
                wm.capacity_used_bytes(),
                1 << 20,
                "capacity is RETAINED - never released while a residual directory exists"
            );
            assert!(
                canonical.join("job-t9").exists(),
                "the residual leaf is still present - surfaced via poison, not silently released"
            );
            let _ = std::fs::remove_dir_all(&base);
        }

        #[test]
        fn injected_delete_failure_quarantines_the_paired_userns_lease() {
        if crate::fake_root_test_environment_skip("real userns lease semantics") {
            return;
        }
            let root = temp_root("t10");
            let workspace_base = root.join("workspace");
            let userns_base = root.join("userns");
            std::fs::create_dir_all(&workspace_base).unwrap();
            std::fs::create_dir_all(&userns_base).unwrap();
            let wm =
                deterministic_workspace_manager_for_tests(workspace_base.clone(), 1 << 30).unwrap();
            let alloc = deterministic_userns_allocator_for_tests(&userns_base, 1).unwrap();
            let spec = substitute_checkout_spec();
            let profile = crate::hardening::HardeningProfile::derive(&spec);
            let container_id = "job-t10-workload";
            let (_cfg, mut ctx) = acquire_enabled_workspace(
                EnabledWorkspaceRequest::new(
                    &spec,
                    &profile,
                    container_id,
                    PathBuf::from("/abs/staged-rootfs"),
                    crate::gvisor::WorkspaceProcessIdentity::Isolated,
                ),
                &wm,
                &alloc,
            )
            .expect("acquire a real paired workspace + userns lease");
            let (root_id, cgroup_id) = ((5_u64, 6_u64), (7_u64, 8_u64));
            ctx.lease
                .bind(container_id.to_string(), root_id, cgroup_id)
                .expect("bind the lease to a workload runtime");
            ctx.bind_state = LeaseBindState::Bound {
                container_id: container_id.to_string(),
                runsc_root_identity: root_id,
                cgroup_identity: cgroup_id,
            };
            let host = ctx.workspace.host_path().unwrap().to_path_buf();
            std::fs::remove_dir_all(&host).unwrap();
            std::fs::create_dir(&host).unwrap();
            let evidence = RuntimeQuiescenceEvidence::assert_for_tests(
                container_id.to_string(),
                RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                    runsc_root_identity: root_id,
                },
                CgroupQuiescenceEvidence::assert_for_tests(cgroup_id),
            );
            let result = settle_enabled_workspace_and_lease(ctx, &wm, &evidence);
            assert!(
                result.is_err(),
                "an unproven workspace delete makes the paired settlement fail"
            );
            assert!(
                matches!(wm.admission(), WorkspaceAdmission::Poisoned { .. }),
                "the workspace manager is poisoned by the unproven delete"
            );
            assert_eq!(
                wm.capacity_used_bytes(),
                1 << 30,
                "workspace capacity is retained on the unproven delete"
            );
            assert!(
                alloc.lease().is_err(),
                "the quarantined userns slot CANNOT be reissued after an unproven delete"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn substituted_workload_mismatched_evidence_is_rejected_at_settle() {
        if crate::fake_root_test_environment_skip("real userns lease semantics") {
            return;
        }
            let root = temp_root("t11");
            let (obs, wm, workspace_base, alloc, userns_base) =
                run_substituted_checkout_mismatched_evidence(
                    &root,
                    "checkout.sentinel",
                    b"shared-provenance",
                );
            assert!(
                obs.hopb_write_ok,
                "the checked Hop B sentinel write still succeeded"
            );
            assert!(
                obs.mount_source_matched_workspace,
                "the retained OCI mount still equals the capsule workspace host path"
            );
            assert!(
                obs.sentinel_read_through_mount,
                "the substituted workload still read the sentinel through the OCI-recorded mount"
            );
            assert!(
                !obs.settled_ok,
                "mismatched evidence must NOT settle clean (got settled_ok=true)"
            );
            let error = obs
                .settle_error
                .as_deref()
                .expect("a refused settle carries a diagnostic");
            assert!(
                error.contains("does not match the recorded binding"),
                "the rejection must come from the SETTLE-tail evidence-vs-recorded-binding provenance \
                 check, got: {error}"
            );
            let child_dirs = std::fs::read_dir(&workspace_base)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .count();
            assert_eq!(
                child_dirs, 1,
                "the workspace leaf must SURVIVE a refused settle (never deleted)"
            );
            assert_ne!(
                wm.capacity_used_bytes(),
                0,
                "capacity must be RETAINED on a refused settle (never silently freed)"
            );
            assert!(
                matches!(wm.admission(), WorkspaceAdmission::Poisoned { .. }),
                "dropping the still-live workspace on a refused settle poisons the manager"
            );
            assert!(
                alloc.lease().is_err(),
                "the quarantined userns slot CANNOT be reissued after a refused settle"
            );
            let _ = std::fs::remove_dir_all(&workspace_base);
            let _ = std::fs::remove_dir_all(&userns_base);
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}
