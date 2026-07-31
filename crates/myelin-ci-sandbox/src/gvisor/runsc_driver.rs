//! CT-007 slice 5b.3-6e.1 (DORMANT): the hardware-independent runsc-driver test seam.
//!
//! This module is gated `#[cfg(feature = "test-support")]`, so it is ABSENT from the ordinary
//! dependency graph and from the `gvisor.rs` production-source dormancy pins (which read
//! `include_str!("gvisor.rs")` and never see this file). Its whole purpose is to let the composed
//! active-path tests (5b.3-6e.2 / design §4) drive a full sandbox cycle WITHOUT a real host — no
//! Btrfs, no `/etc/subuid`, no KVM, no `runsc`.
//!
//! It substitutes ONLY the runtime EXECUTION — the workload `runsc` spawn — with a deterministic
//! canned result, while driving everything else FOR REAL:
//!
//! - the real [`GvisorBackend::compute_launch_preflight`](super::GvisorBackend) (isolation floor,
//!   hardening profile, registry rootfs resolution);
//! - the real parent-attempt reservation ([`RunnerHooks::reserve_parent_attempt`] → the caller's real
//!   hooks/authorities), so `Admitted` retains the durable parent row + reserve and `AttemptsExhausted`
//!   terminalizes without spawning;
//! - the real shared post-reservation compute body (settlement, guest registration, completion
//!   settlement);
//! - the real [`SandboxCycleOutcome`] routing the runner lane consumes.
//!
//! The COMPUTE cycle needs no checkout capsule, so it drives hardware-independently with a
//! `Disabled`-workspace backend. The checkout-capsule variant — substituting the advertise / fetch /
//! Hop-B executions while driving the SEALED `checkout_runtime` capsule through its own accessors —
//! is the extension 5b.3-6e.2's composed §4 tests build on this same seam; nothing here weakens the
//! 6a capsule inseparability (this module never names a capsule field).

use crate::{
    JobSpec, ResourceUsage, RunnerHooks, SandboxCycleOutcome, SandboxLaunchError, SandboxResult,
};

/// A deterministic stand-in for the spawned `runsc` child: it never touches a real runtime, and its
/// teardown (`kill`/`wait`) is a clean no-op success.
struct FakeRunscChild;

impl super::RunscChild for FakeRunscChild {
    fn kill(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn wait(&mut self) -> Result<i32, String> {
        Ok(0)
    }
}

/// Fabricate the already-`Finalized` envelope a successful workload run would hand back — a clean
/// exit-0 result carrying `workload_usage`, a fake child, a non-existent bundle dir (its teardown
/// removal is a harmless no-op), and canned `Rootless` quiescence evidence. This is the SUBSTITUTED
/// runtime execution; every other step of the cycle runs for real.
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
    /// **Drive one COMPUTE cycle with a SUBSTITUTED runsc execution (CT-007 slice 5b.3-6e.1 —
    /// DORMANT, test-support only).**
    ///
    /// Runs the REAL dormant [`launch_compute_orchestrated_with`](super::GvisorBackend) — hence the
    /// real preflight, the real parent-attempt reservation via `hooks` (the caller wires the real
    /// authorities), the real shared common body, and the real [`SandboxCycleOutcome`] routing — while
    /// the workload `runsc` spawn is replaced by a canned exit-0 finalization carrying
    /// `workload_usage`. On `AttemptsExhausted` the fake is never invoked (nothing spawns) and the
    /// outcome is a [`SandboxCycleOutcome::PreparationTerminal`]; on `Admitted` the fake runs and the
    /// outcome is a [`SandboxCycleOutcome::WorkloadLaunched`]. No Btrfs / `/etc/subuid` / KVM / runsc
    /// is touched.
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
                // Commit the launch permit exactly as the production run closure would, then hand back
                // the SUBSTITUTED finalization — the only step this seam fakes.
                permit
                    .commit_and_release()
                    .map_err(|error| super::RunFailure::uncommitted(error.to_string()))?;
                Ok(fake_workload_finalization(workload_usage))
            },
        )
    }
}

/// **CT-007 slice 5b.3-6e.1b: the checkout-capsule variant of the hardware-independent driver
/// (DORMANT, test-support only).**
///
/// Drives the deterministic substituted checkout SUCCESS cycle under `root` — a fresh
/// deterministic-directory workspace manager + a fixture-`subuid` userns allocator + a REAL capsule,
/// then the sealed [`AcquiredCheckoutRuntime::execute_substituted_checkout_for_test_support`](super::AcquiredCheckoutRuntime)
/// seam (real `Allocated → PreparationBound → Prepared → Bound` transitions; substituted Hop B /
/// workload; real settle). Asserts the sentinel round-tripped through the retained OCI mount and the
/// capsule settled cleanly, then confirms the durable step-8 state (workspace deleted, capacity zero,
/// userns slot reusable, managers healthy). This is the building block 5b.3-6e.2's composed §4 tests
/// extend; it is `pub` so an integration binary can call it, and it anchors the whole `test-support`
/// substrate for the ordinary (non-`cfg(test)`) `--features test-support` build. No Btrfs / subuid /
/// KVM / runsc is touched.
impl super::GvisorBackend {
    #[allow(clippy::unused_self)]
    pub fn drive_checkout_cycle_with_substituted_runsc(&self, root: &std::path::Path) {
        drive_checkout_cycle_with_substituted_runsc(root)
    }
}

/// **The Hop-B outcome a composed §4 active-path test injects (CT-007 slice 5b.3-6e.2 Stage A —
/// DORMANT, test-support only).** Every arm runs the audited `substituted_hop_b_for_test_support`,
/// consuming and committing the real materialization authorization and driving the real
/// `Allocated → PreparationBound → Prepared` transitions. The failure arms then inject a
/// `RejectedAfterQuiescence` carrying the exact disposition, so the REAL continuation routes it as
/// production would. This is a pure test seam: no production path names it
/// (pinned production-zero), and it changes NOTHING about steps 1–14, which stay single-sourced through
/// the production orchestrator.
#[derive(Clone, Copy, Debug)]
pub enum InjectedHopBOutcome {
    /// Hop B succeeds — the audited real preparation transitions run and the workload settles.
    Success,
    /// Hop B fails terminally (a proven-quiescent `Terminal(Failed)` disposition) — the continuation
    /// completes the materialization phase and produces `PreparationTerminal`.
    TerminalFailed,
    /// Hop B fails retryably (a proven-quiescent `RetryableInfrastructure` disposition) — the
    /// continuation routes requeue-or-exhausted, producing `PreparationRetryable` while the budget holds.
    RetryableInfrastructure,
}

impl super::GvisorBackend {
    /// **Drive one CHECKOUT cycle through the REAL orchestrator with a SUBSTITUTED runsc execution
    /// (CT-007 slice 5b.3-6e.2 Stage A — DORMANT, test-support only).**
    ///
    /// Unlike [`Self::drive_checkout_cycle_with_substituted_runsc`] (which drives the sealed capsule
    /// seams directly), this seam drives the REAL outer orchestrator
    /// [`launch_checkout_orchestrated_with_given`](super::GvisorBackend) — hence the REAL preflight
    /// (isolation floor, hardening profile, registry rootfs resolution, workspace/userns health), the
    /// REAL parent-attempt admission via `hooks.reserve_parent_attempt` (the caller wires the real V2
    /// hooks whose `Admitted` retains the real `DurableAttemptAuthority`), the REAL transport-phase
    /// begin/complete, the REAL advertise + fetch credential mint/authorize/renew, the REAL capsule
    /// acquisition, and the REAL step-15 continuation — while ONLY the hardware is substituted: Hop A
    /// runs as a scripted two-call permit-recording executor (advertise then fetch), and the
    /// continuation runs [`launch_checkout_continuation_given`](super::GvisorBackend) with the audited
    /// `substituted_hop_b_for_test_support` / `substituted_workload_for_test_support` seams. Steps 1–14
    /// are single-sourced through the production orchestrator.
    ///
    /// Returns the REAL `(outcome, recorded)` pair: the orchestrator's typed
    /// [`CheckoutContinuationOutcome`](crate::checkout_orchestration::CheckoutContinuationOutcome) and
    /// the executor's per-call `(run-token JTI, permit-committed)` record — so a composed test can prove
    /// exactly two executions, distinct advertise/fetch JTIs, both permits committed, and the outcome
    /// shape. No Btrfs / `/etc/subuid` / KVM / runsc is touched.
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

    /// **Drive one CHECKOUT cycle with a SUBSTITUTED runsc execution AND an INJECTED Hop-B outcome
    /// (CT-007 slice 5b.3-6e.2 Stage A — DORMANT, test-support only).** Identical to
    /// [`Self::drive_checkout_cycle_with_substituted_runsc_given`] (steps 1–14 single-sourced through the
    /// production orchestrator) except the composed test chooses the Hop-B disposition — success, a
    /// terminal failure, or a retryable-infrastructure failure — so the §4 tests can prove the active
    /// path routes a Hop-B `PreparationTerminal` / `PreparationRetryable` exactly as production does. No
    /// Btrfs / `/etc/subuid` / KVM / runsc is touched.
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

    /// The single-sourced body shared by both public checkout drivers. `hop_b_succeeds` runs the audited
    /// success seam; otherwise `injected_disposition` is emitted after the same real materialization
    /// authorization, launch-boundary commit, and durable preparation transitions.
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

        // The advertised oid MUST equal the commit the orchestrator derives from the spec's workspace —
        // read it back from the spec so this seam works for any caller-built checkout spec.
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
                    // Hop B: consume/commit the REAL materialization authorization and drive the REAL
                    // durable `Allocated → PreparationBound → Prepared` transitions in every arm. The
                    // success arm performs the checked sentinel write; a failure arm injects its chosen
                    // post-quiescence disposition only after those shared fences have run.
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
                    // Workload: the REAL `run_retained_workload_inner` (materialization completion,
                    // lease renewal, workload-credential mint/rotation, real launch-permit acquire +
                    // commit, and the real settle tail), faking only explicit-userns revalidation and
                    // the runsc spawn. Evidence is DERIVED from the real bind. Never reached on an
                    // injected Hop-B failure (the continuation returns before the workload).
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

    // Step 8: durable state. Workspace deleted (no child dirs), capacity fully released, userns slot
    // reusable, both managers healthy.
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
    // Probe reusability WITHOUT poisoning: acquire then `release_unused` (dropping an unreleased
    // probe lease would emit a quarantine incident and poison the allocator this asserts stays clean).
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
