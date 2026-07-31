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
            bundle_dir: std::env::temp_dir()
                .join("myelin-runsc-driver-fake-bundle-does-not-exist"),
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

fn drive_checkout_cycle_with_substituted_runsc(root: &std::path::Path) {
    let sentinel = b"myelin-6e1b-provenance-sentinel";
    let (observation, workspace_manager, workspace_base, userns_allocator, userns_base) =
        super::run_substituted_checkout_success(root, "checkout.sentinel", sentinel);

    assert!(observation.hopb_write_ok, "the checked Hop B sentinel write must succeed");
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
    assert!(residual.is_empty(), "the workspace leaf must be gone after settle");
    assert_eq!(
        workspace_manager.capacity_used_bytes(),
        0,
        "capacity must be fully released after settle"
    );
    assert!(workspace_manager.is_healthy(), "the workspace manager stays healthy");
    assert!(userns_allocator.is_healthy(), "the userns allocator stays healthy");
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
