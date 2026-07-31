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
