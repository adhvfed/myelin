//! CT-007 slice 5b.3-6e.1b: the deterministic checkout-capsule substrate (test-support only).

use super::*;
use crate::SandboxResult;

// ═════════════════ CT-007 slice 5b.3-6e.1b: the deterministic checkout-capsule substrate ═════════
//
// test-support ONLY. Appended AFTER the top-level `#[cfg(test)] mod tests` block so it is excluded
// from `production_source()` — its `AcquiredCheckoutRuntime::acquire(` call therefore never counts
// against the composition-root-zero pin (which scans `production_source()` only). Every item here is
// gated `#[cfg(any(test, feature = "test-support"))]`: ABSENT from ordinary builds. Nothing here is
// reachable from any production composition root.

/// Owned observations from the deterministic substituted-execution seam — the facts a caller cannot
/// see for itself (Hop B's checked write, the byte-accounted checkpoints, the OCI-mounted sentinel
/// round-trip, and the real-settle outcome). It carries NO workspace path, lease, session,
/// `OciConfig`, or evidence.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
#[allow(dead_code)] // fields are read by the in-lib 6e.1b tests; the non-test build only constructs it.
pub(crate) struct SubstitutedCheckoutObservation {
    pub(crate) hopb_write_ok: bool,
    pub(crate) used_after_hopb: u64,
    pub(crate) used_at_workload_checkpoint: u64,
    pub(crate) mount_source_matched_workspace: bool,
    pub(crate) sentinel_read_through_mount: bool,
    pub(crate) settled_ok: bool,
    pub(crate) settle_error: Option<String>,
}

/// A deterministic stand-in for the substituted workload's spawned `runsc` child: teardown
/// (`kill`/`wait`) is a clean no-op success.
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

/// Fabricate the already-`Finalized` envelope a successful workload run hands back, carrying the
/// supplied (matching) workload quiescence `evidence` + a clean exit-0 result. This is the ONLY
/// substituted runtime execution; the settle tail it feeds is REAL.
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

/// A checkout-bearing `JobSpec` for the deterministic substrate (repo_ref + commit present, so it
/// derives a checkout scope). No real registry/rootfs is consulted — `acquire` builds the OCI layout
/// from the spec + a synthetic absolute rootfs path.
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

/// Build a deterministic-directory [`WorkspaceManager`](crate::workspace_manager::WorkspaceManager)
/// under `base_dir` — the dormant `DeterministicDirectoryForTests` mode. No Btrfs / `CAP_SYS_ADMIN`.
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
        crate::workspace_manager::WorkspaceStorageMode::DeterministicDirectoryForTests {
            base_dir,
            host_capacity_bytes,
        },
        sink,
    )
}

/// Build a deterministic userns allocator from FIXTURE `subuid`/`subgid` files written under
/// `base_dir` (so it NEVER depends on this host's real `/etc/subuid`). It FAILS (never skips) if the
/// process is root — `try_new_impl` refuses a privileged runner — so the CI test surfaces a violated
/// non-root prerequisite as a hard error, exactly as Sol's 6e.1b ruling requires.
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

/// Build fresh deterministic managers under `root` and acquire a REAL checkout capsule (pool size 1)
/// — the NON-SKIPPING analog of the Btrfs-gated `acquire_real_checkout_capsule`. Returns the capsule
/// plus the managers/dirs so a caller can drive it and then probe durable state.
#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::type_complexity)]
pub(crate) fn acquire_deterministic_checkout_capsule(
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

/// Whether the substituted workload's runtime-quiescence evidence is DERIVED from the durable bind's
/// own `Bound` output (the honest positive path) or deliberately MISMATCHED on its `runsc_root_identity`
/// (the negative path proving the settle-tail provenance check is LIVE, not vacuous). CT-007 5b.3-6e.2.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // MismatchedRunscRoot is constructed only by the `#[cfg(test)]` negative test.
pub(crate) enum SubstitutedEvidenceMode {
    DerivedFromBind,
    MismatchedRunscRoot,
}

/// **CT-007 slice 5b.3-6e.2: a no-op `test-support` [`AttemptAuthority`] for the SANDBOX substituted
/// workload leg.** It records nothing and fails nothing — its phase/renew/mint ops all succeed as
/// clean no-ops — so the hardware-independent 6e.1b/§4-sandbox tests can drive the REAL
/// `run_retained_workload_inner` (materialization completion, lease renewal, workload-credential mint +
/// rotation) without a control-plane. Distinct from the `#[cfg(test)]`-only `FakeAttemptAuthority` (it
/// must be reachable from the ordinary `--features test-support` build, not just `#[cfg(test)]`) and
/// from the REAL `DurableAttemptAuthority` the live-PG §4 tests inject. It is pinned production-zero:
/// defined AFTER the top-level test module (so it is excluded from `production_source()` entirely) and
/// asserted absent from `production_source()` by the `ordinary_build_and_production_root_pins` test.
#[cfg(any(test, feature = "test-support"))]
pub(super) struct NoOpTestSupportAuthority;

#[cfg(any(test, feature = "test-support"))]
impl NoOpTestSupportAuthority {
    /// A minimal well-formed ephemeral authorization context for the workload credential mint — the
    /// substituted workload leg never verifies it (that is the control-plane's Identity gate, exercised
    /// by the live-PG §4 tests); it only needs to rotate structurally into the workload-local spec.
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
        // Phase-DISTINCT jti + generation so a composed test can prove each transport leg spawned under
        // its OWN credential and its OWN durable phase permit (advertise ≠ fetch ≠ materialization) —
        // exactly what a real `DurableAttemptAuthority` guarantees.
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

/// Run the full deterministic substituted checkout cycle under `root` in `evidence_mode`, returning
/// the owned observation plus the residual managers/dirs so the caller can assert durable state. This
/// composes the two sealed test-support halves — [`AcquiredCheckoutRuntime::substituted_hop_b_for_test_support`]
/// (real Hop B durable transitions) then [`PreparedCheckoutRuntime::substituted_workload_for_test_support`]
/// (the REAL `run_retained_workload_inner` under the no-op authority, faking ONLY the hardware-gated
/// permit/revalidation) — so the whole path is exercised, not a fabricated transition.
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
    // A minimal immediate-permit hooks: this 6e.1b provenance/settle drill has no control plane, so its
    // launch fence grants an immediate permit (a no-op `commit_and_release`) for BOTH the workload leg
    // and the materialization phase. The REAL V2 launch-fence proof is the composed §4 path
    // (`drive_checkout_cycle_*`), which threads the real `ci_runner_v2_wiring` hooks whose permits commit
    // the queue→running / row-locked CAS.
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
    // Mint the materialization `PhaseAuthorization` the fused Hop-B seam consumes — bound to the
    // capsule's OWN derived scope + the same commit it was acquired for, so `resolve_checkout_preparation_permit`
    // ACCEPTS it (a mismatched credential is what the §4 negative variant exercises). The authorization
    // can only come from `authorize_phase_generation`, exactly as the real orchestrator mints it.
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
    // Hop B half: real Allocated -> PreparationBound -> Prepared, with the REAL materialization-permit
    // resolve (consuming the authorization) and a substituted git execution.
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
    // Workload half: the REAL run_retained_workload_inner (authority phases + the real launch permit
    // acquire+commit + settle), faking only the explicit-userns revalidation + the runsc spawn.
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
    // Map the REAL outcome. `Ran(Ok(_))` is a clean real settle; `Ran(Err(_))` is the settle tail
    // refusing (the negative provenance path lands here); every other variant is a pre-settle disposal
    // (the workload never bound/renewed) — all `settled_ok == false`.
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

/// Run the full deterministic substituted checkout SUCCESS cycle under `root`, returning the owned
/// observation plus the residual managers/dirs so the caller can assert step-8 durable state (path
/// absence, capacity zero/reusable, userns slot reusable, healthy manager). The ONE entry both the
/// 6e.1b in-lib tests and the runsc-driver checkout seam call.
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

/// Run the deterministic substituted checkout cycle under `root` with DELIBERATELY MISMATCHED workload
/// evidence (a wrong `runsc_root_identity`, diverging from the durable bind's own recorded output), so
/// the REAL settle tail's evidence-vs-recorded-binding provenance check MUST reject it AT SETTLE. The
/// returned `settle_error` carries the settle-layer refusal; the residual managers let the caller
/// assert the fail-closed contract (workspace NOT deleted, capacity retained, manager poisoned, userns
/// slot not reissued). CT-007 5b.3-6e.2 — the negative pair to `run_substituted_checkout_success`.
#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::type_complexity)]
#[allow(dead_code)] // called only by the `#[cfg(test)]` negative provenance test.
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

    // ══════════ CT-007 slice 5b.3-6e.1b: the 8 mandatory deterministic-substrate tests ══════════
    //
    // These RUN (never soft-skip) given a NON-root user + a writable tmp base dir — no
    // Btrfs/CAP_SYS_ADMIN/subuid/KVM/runsc. Everything they touch is `#[cfg(any(test,
    // feature = "test-support"))]`, so they compile+run under `--lib` AND `--lib --features
    // test-support`.
    mod deterministic_substrate_6e1b {
        use crate::gvisor::{
            acquire_enabled_workspace, classify_workspace_deletion,
            deterministic_userns_allocator_for_tests, deterministic_workspace_manager_for_tests,
            run_substituted_checkout_mismatched_evidence, run_substituted_checkout_success,
            settle_enabled_workspace_and_lease, substitute_checkout_spec, unique_suffix,
            CgroupQuiescenceEvidence, LeaseBindState, RuntimeNamespaceQuiescence,
            RuntimeQuiescenceEvidence, WorkspaceDeletionOutcome,
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

        // ── Test 1: directory create → checked sentinel write/read → delete → proven absence. ──
        #[test]
        fn directory_create_write_read_delete_absence() {
            let base = temp_root("t1").join("workspace");
            std::fs::create_dir_all(&base).unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 1 << 30).unwrap();
            let cap = wm.acquire_capacity(1 << 20).expect("capacity");
            let ws = wm
                .create_workspace("job-t1", 1 << 20, 0, 0, cap)
                .expect("create directory workspace");
            let host = ws.host_path().to_path_buf();
            assert!(host.is_dir(), "a fresh leaf directory exists");
            ws.checked_test_quota_write("checkout.sentinel", b"provenance")
                .expect("the checked byte-accounted write succeeds under quota");
            assert_eq!(
                std::fs::read(host.join("checkout.sentinel")).unwrap(),
                b"provenance",
                "the sentinel reads back byte-identical"
            );
            // An over-quota checked write refuses BEFORE mutating.
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

        // ── Test 2: capacity leased, exhausted, released, then reusable — real aggregate accounting. ──
        #[test]
        fn capacity_leased_then_released_and_reusable() {
            let base = temp_root("t2").join("workspace");
            std::fs::create_dir_all(&base).unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 4 << 20).unwrap();
            let cap = wm
                .acquire_capacity(4 << 20)
                .expect("lease the whole ceiling");
            assert_eq!(wm.capacity_used_bytes(), 4 << 20);
            // Ceiling exhausted: a further request is refused (REAL aggregate accounting).
            assert!(wm.acquire_capacity(1).is_err(), "the ceiling is exhausted");
            let ws = wm
                .create_workspace("job-t2", 4 << 20, 0, 0, cap)
                .expect("create consumes the lease");
            wm.delete_workspace(ws)
                .expect("delete releases the capacity");
            assert_eq!(wm.capacity_used_bytes(), 0, "capacity fully returned");
            // Reusable: the freed ceiling admits a fresh lease.
            let again = wm
                .acquire_capacity(4 << 20)
                .expect("reuse the freed ceiling");
            again.release();
            let _ = std::fs::remove_dir_all(&base);
        }

        // ── Test 3: the REAL userns preparation/bind/workload/release transitions, deterministically. ──
        #[test]
        fn real_userns_preparation_bind_workload_release_transitions() {
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
            // Probe reusability WITHOUT poisoning: acquire then `release_unused` (never drop an
            // unreleased probe lease — that would emit a quarantine incident and poison the
            // allocator this test claims stays clean).
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

        // ── Test 4: the FULL capsule fake-Hop-B / fake-workload → real settle/delete. ──
        #[test]
        fn full_capsule_substituted_hopb_and_workload_then_real_settle() {
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
            // Step 8: durable state.
            let child_dirs = std::fs::read_dir(&workspace_base)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .count();
            assert_eq!(child_dirs, 0, "the workspace leaf was deleted by settle");
            assert_eq!(wm.capacity_used_bytes(), 0, "capacity zero after settle");
            assert!(wm.is_healthy(), "the workspace manager stays healthy");
            assert!(alloc.is_healthy(), "the userns allocator stays healthy");
            // Probe reusability WITHOUT poisoning (release the probe lease, never drop it unreleased).
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

        // ── Test 5: wrong backend / wrong base / wrong inode / symlink substitution refuse w/o delete. ──
        #[test]
        fn cross_backend_base_inode_and_symlink_substitutions_refuse_without_deleting() {
            // (a) A Btrfs-identity capability handed to the directory backend → BackendMismatch.
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

            // (b) A directory capability from base A refused by a backend over base B → WrongStorage.
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

            // (c) Inode substitution: replace the leaf with a fresh dir (new inode) → absence unproven.
            let (mut backend_c, _) = open_directory_backend("t5c");
            let ws_c = backend_c.create_workspace("job", 1 << 20, 0, 0).unwrap();
            let leaf_c = ws_c.host_path().to_path_buf();
            std::fs::remove_dir_all(&leaf_c).unwrap();
            std::fs::create_dir(&leaf_c).unwrap(); // a DIFFERENT inode at the same name.
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

            // (d) Symlink substitution: replace the leaf with a symlink → absence unproven, not followed.
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
                "the symlink was never followed — the decoy target is intact"
            );
        }

        // ── Test 6: an injected delete failure retains capacity, poisons the manager, and the
        //           absence-unproven outcome is what leaves a paired userns lease unreleased. ──
        #[test]
        fn injected_delete_failure_retains_capacity_and_poisons_the_manager() {
            let base = temp_root("t6").join("workspace");
            std::fs::create_dir_all(&base).unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 1 << 30).unwrap();
            let cap = wm.acquire_capacity(1 << 20).unwrap();
            let ws = wm.create_workspace("job-t6", 1 << 20, 0, 0, cap).unwrap();
            let host = ws.host_path().to_path_buf();
            // Inject the failure: swap the leaf for a different-inode dir so the verified delete
            // cannot prove absence.
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
            // The SAME absence-unproven storage error is what the settle path classifies as
            // NotProvenAbsent → the paired userns lease is left unreleased (never reissued).
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

        // ── Test 7: boot orphan reconciliation deletes stray child dirs, refuses non-dir entries. ──
        #[test]
        fn boot_orphan_reconciliation_deletes_orphans_and_refuses_malformed_entries() {
            // (a) Pre-seed orphan child dirs; construction reconciles them away and admits Healthy.
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

            // (b) A stray FILE (not a directory) refuses construction LOUDLY.
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

        // ── Test 8: ordinary-build + production-composition-root pins — the mode/seams are unreachable. ──
        #[test]
        fn ordinary_build_and_production_root_pins() {
            const WORKSPACE_MANAGER_SOURCE: &str = include_str!("../workspace_manager.rs");
            const WORKSPACE_STORAGE_SOURCE: &str = include_str!("../workspace_storage.rs");
            const USER_NAMESPACE_SOURCE: &str = include_str!("../user_namespace.rs");

            // Production composition in gvisor.rs NEVER names the dormant mode (the GvisorWorkspaceConfig
            // -> EphemeralDisk mapping is the only production selector).
            assert_eq!(
                crate::gvisor::source_pins::production_source()
                    .matches("DeterministicDirectoryForTests")
                    .count(),
                0,
                "no production gvisor path constructs the deterministic-directory mode"
            );

            // CT-007 5b.3-6e.2: the no-op test-support authority + the substituted-execution mode are
            // named ONLY by the test-support substrate (below the top-level test module) — production
            // source names neither. This keeps ruling-(A)'s substituted workload leg unreachable from
            // every production composition root.
            assert_eq!(
                crate::gvisor::source_pins::production_source()
                    .matches("NoOpTestSupportAuthority")
                    .count(),
                0,
                "no production gvisor path constructs the no-op test-support attempt authority"
            );
            assert_eq!(
                crate::gvisor::source_pins::production_source()
                    .matches("SubstitutedEvidenceMode")
                    .count(),
                0,
                "no production gvisor path names the substituted-evidence mode"
            );

            // CT-007 5b.3-6e.2 Stage A: the git-wire test-support substrate + the orchestrator-driving
            // seam are named ONLY by test/test-support code (the module below the top-level test module,
            // and the `#[cfg(feature = "test-support")]` runsc-driver file this scan never reads).
            // Production source names none of them — the whole composed active path is unreachable from
            // every production composition root until Stage B.
            assert_eq!(
                crate::gvisor::source_pins::production_source()
                    .matches("checkout_transport_test_support")
                    .count(),
                0,
                "no production gvisor path names the git-wire test-support module"
            );
            assert_eq!(
                crate::gvisor::source_pins::production_source()
                    .matches("drive_checkout_cycle_with_substituted_runsc_given")
                    .count(),
                0,
                "no production gvisor path names the orchestrator-driving runsc seam"
            );
            // CT-007 5b.3-6e.2 Stage A: the §4 prep-terminal/prep-retry tests inject a Hop-B disposition
            // via the new test-support driver + selector. Both live in the `#[cfg(feature =
            // "test-support")]` `runsc_driver` file (which `production_source` never reads) and NO
            // production composition root names them.
            assert_eq!(
                crate::gvisor::source_pins::production_source()
                    .matches("drive_checkout_cycle_with_injected_hop_b")
                    .count(),
                0,
                "no production gvisor path names the Hop-B-injecting runsc seam"
            );
            assert_eq!(
                crate::gvisor::source_pins::production_source()
                    .matches("InjectedHopBOutcome")
                    .count(),
                0,
                "no production gvisor path names the injected Hop-B outcome selector"
            );
            assert_eq!(
                crate::gvisor::source_pins::production_source()
                    .matches("deterministic_enabled_backend_for_tests")
                    .count(),
                0,
                "no production gvisor path builds the deterministic Enabled test backend"
            );
            // CT-007 slice 5b.3-6e.2 Stage A: the two OTHER helpers the §4 tests pub-name (the checkout
            // spec factory + the bare-repo stager) are likewise named ONLY by test/test-support code —
            // making them `pub` for cross-crate reach must not make any production path name them.
            assert_eq!(
                crate::gvisor::source_pins::production_source()
                    .matches("checkout_spec_for_backend")
                    .count(),
                0,
                "no production gvisor path builds the deterministic checkout spec"
            );
            assert_eq!(
                crate::gvisor::source_pins::production_source()
                    .matches("stage_checkout_repo_root")
                    .count(),
                0,
                "no production gvisor path stages the deterministic bare-repo root"
            );

            // The mode variant is cfg-gated (absent from ordinary builds).
            assert!(
                WORKSPACE_MANAGER_SOURCE.contains(
                    "#[cfg(any(test, feature = \"test-support\"))]\n    DeterministicDirectoryForTests {"
                ),
                "the DeterministicDirectoryForTests mode variant is test/test-support gated"
            );
            // The whole directory backend + typed identity + checked quota is cfg-gated.
            assert!(
                WORKSPACE_STORAGE_SOURCE.contains(
                    "#[cfg(any(test, feature = \"test-support\"))]\n#[derive(Debug)]\npub(crate) struct DirectoryWorkspaceStorage"
                ),
                "the directory backend struct is test/test-support gated"
            );
            // The userns fixture constructor was widened to test-support (NOT relaxed for production).
            assert!(
                USER_NAMESPACE_SOURCE.contains(
                    "#[cfg(any(test, feature = \"test-support\"))]\n    pub(crate) fn try_new_for_tests("
                ),
                "try_new_for_tests is test/test-support gated, never a production constructor"
            );
            // The production userns constructor is fixed to /etc/subuid — never an arbitrary path.
            assert!(
                USER_NAMESPACE_SOURCE.contains("pub fn try_new(")
                    && USER_NAMESPACE_SOURCE.contains("Path::new(\"/etc/subuid\")"),
                "the production allocator constructor stays pinned to /etc/subuid"
            );
        }

        // ── Test 9 (Sol blocker 2): an injected create failure (an untracked pre-existing leaf)
        //   is an UnrecoverableLeak → capacity RETAINED + manager poisoned. A residual directory can
        //   NEVER coexist with healthy admission + released capacity. ──
        #[test]
        fn injected_create_failure_retains_capacity_and_poisons_without_a_healthy_residual() {
            let base = temp_root("t9").join("workspace");
            std::fs::create_dir_all(&base).unwrap();
            let wm = deterministic_workspace_manager_for_tests(base.clone(), 1 << 30).unwrap();
            // Inject the failure: plant an untracked residual leaf at the job key AFTER boot
            // reconciliation (the manager canonicalizes its base, so match that).
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
                "capacity is RETAINED — never released while a residual directory exists"
            );
            assert!(
                canonical.join("job-t9").exists(),
                "the residual leaf is still present — surfaced via poison, not silently released"
            );
            let _ = std::fs::remove_dir_all(&base);
        }

        // ── Test 10 (Sol blocker 3): a REAL paired userns lease driven through the ACTUAL
        //   settlement branch with an injected workspace-delete failure — the lease is NOT released
        //   (quarantined), so the pool-1 slot cannot be reissued. ──
        #[test]
        fn injected_delete_failure_quarantines_the_paired_userns_lease() {
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
                &spec,
                &profile,
                container_id,
                PathBuf::from("/abs/staged-rootfs"),
                &wm,
                &alloc,
                None,
            )
            .expect("acquire a real paired workspace + userns lease");
            // Durably bind the lease (Allocated -> Bound) and record the Bound state settle validates.
            let (root_id, cgroup_id) = ((5_u64, 6_u64), (7_u64, 8_u64));
            ctx.lease
                .bind(container_id.to_string(), root_id, cgroup_id)
                .expect("bind the lease to a workload runtime");
            ctx.bind_state = LeaseBindState::Bound {
                container_id: container_id.to_string(),
                runsc_root_identity: root_id,
                cgroup_identity: cgroup_id,
            };
            // Inject the delete failure: swap the workspace leaf for a different-inode dir.
            let host = ctx.workspace.host_path().to_path_buf();
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
            // The paired userns lease was NEVER released — quarantined. The pool-1 slot is gone.
            assert!(
                alloc.lease().is_err(),
                "the quarantined userns slot CANNOT be reissued after an unproven delete"
            );
            let _ = std::fs::remove_dir_all(&root);
        }

        // ── Test 11 (CT-007 5b.3-6e.2, the negative provenance pair): the substituted workload
        //   builds runtime-quiescence evidence with a DELIBERATELY WRONG runsc_root_identity (diverging
        //   from the durable bind's OWN recorded output). The REAL `settle_enabled_finalization` tail
        //   MUST reject it AT SETTLE (its evidence-vs-recorded-binding provenance check), fail closed,
        //   and — per the real contract — leave the workspace UNDELETED (capacity retained, manager
        //   poisoned) and the userns slot unreissued. This is what proves the positive test's clean
        //   settle is NOT vacuous: flip only the derived identity and settlement refuses. ──
        #[test]
        fn substituted_workload_mismatched_evidence_is_rejected_at_settle() {
            let root = temp_root("t11");
            let (obs, wm, workspace_base, alloc, userns_base) =
                run_substituted_checkout_mismatched_evidence(
                    &root,
                    "checkout.sentinel",
                    b"shared-provenance",
                );
            // Hop B + the OCI-mount round-trip still succeeded (the divergence is ONLY in the workload
            // evidence's runsc-root identity) — isolating the failure to the settle provenance check.
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
            // The settle tail REFUSED — the whole point.
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
            // Fail-closed durable state: the workspace was NEVER deleted (settle refused before the
            // delete), so its leaf survives, capacity is retained, and the manager is poisoned.
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
            // The paired userns lease was NEVER released — the pool-1 slot cannot be reissued.
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
