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
fn substitute_checkout_spec() -> crate::JobSpec {
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
struct NoOpTestSupportAuthority;

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
