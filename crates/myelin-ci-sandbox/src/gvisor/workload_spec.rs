//! CT-007 slice 5b.3-6c (Sol's r5 finding 2): the WORKLOAD-ROTATED spec wrapper, isolated in its OWN
//! module so that **the inner `JobSpec` never escapes** — the terminal, language-enforced form of "the
//! workload permit hook and executor receive ONLY the rotated spec, never a stale/substituted one".
//!
//! The prior fence exposed `as_job_spec() -> &JobSpec`; because `JobSpec` is `Clone` with public
//! credential fields, an outer helper could `let mut s = ws.as_job_spec().clone(); s.run_token = stale;
//! execute(&s)` — a real credential-substitution hole. This module closes it: [`WorkloadRotatedSpec`]
//! has a PRIVATE field, NO `as_job_spec`/no accessor returning `&JobSpec`, and NO `Clone`/`From` that
//! could hand out the inner spec. Its ONE run method — [`WorkloadRotatedSpec::acquire_permit_and_run`]
//! — calls `RunnerHooks::acquire_launch_permit(&self.spec)` and `execute(&self.spec, …)` with its OWN
//! private `&JobSpec`, so NO `&JobSpec` ever leaves this module for outer code to clone or substitute.
//! The compute-shared `acquire_launch_permit` + executor signatures stay `&JobSpec` (unchanged) — they
//! are merely CALLED from inside the wrapper. The module's exported surface is audited CLOSED-WORLD by
//! `the_workload_spec_module_shape_is_pinned` (a sibling test in `gvisor::tests`): the ONLY items are
//! this struct + the `BoundWorkloadRefusal` enum + the impl with EXACTLY `{from_carrier,
//! acquire_permit_and_run}`, and no method may return `&JobSpec`.

use std::path::Path;
use std::sync::Arc;

use super::{
    revalidated_explicit_userns_root_identity, run_production_container_streaming, ContainerRun,
    LeaseBindState, OciConfig, RunFailure, RuntimeBinding, RuntimeFinalization, RuntimePreparation,
};
use crate::checkout_orchestration::WorkloadCredentialCarrier;
use crate::user_namespace::{CheckoutPreparationSession, UserNamespaceLease};
use crate::{JobSpec, LaunchPermit, RunnerHooks, SandboxCancellation, SandboxOutputSink};

/// A pre-execution refusal from [`WorkloadRotatedSpec::acquire_permit_and_run`] — the workload never
/// launched.
#[allow(dead_code)]
pub(crate) enum BoundWorkloadRefusal {
    /// The launch permit was refused, or the runsc-root identity could not be revalidated — a pure
    /// pre-execution refusal.
    PermitRefused(String),
    /// The workload OCI layout did not match the prepared mode (an `Uncommitted`-phase failure).
    PrepModeMismatch(String),
}

/// **The workload-rotated `JobSpec`, sealed.** Constructible ONLY from a [`WorkloadCredentialCarrier`]
/// together with the base spec (so the rotation cannot be skipped), and consumable ONLY through
/// [`Self::acquire_permit_and_run`]. The inner spec is a PRIVATE field with no accessor returning
/// `&JobSpec` and no `Clone`/`From`, so it can never be extracted to be cloned/substituted.
#[allow(dead_code)]
pub(crate) struct WorkloadRotatedSpec {
    spec: JobSpec,
}

impl WorkloadRotatedSpec {
    /// The ONE constructor: rotate `base_spec` onto `carrier`'s workload generation. This is the ONLY
    /// place `base_spec` is consumed on the workload path; from here only this sealed wrapper travels.
    #[allow(dead_code)]
    pub(crate) fn from_carrier(carrier: &WorkloadCredentialCarrier, base_spec: &JobSpec) -> Self {
        WorkloadRotatedSpec {
            spec: carrier.workload_local_spec(base_spec),
        }
    }

    /// PRIVATE: acquire the workload launch permit under the ROTATED spec + revalidate the runsc-root
    /// identity + build the `EnabledPrepared` `RuntimePreparation` over the disjoint scoped borrows.
    /// Shared by the production and `#[cfg(test)]` run methods; never exposed. Returns the permit + prep;
    /// the spec used for the permit is the private `&self.spec` and is NOT returned.
    #[allow(private_interfaces, private_bounds)]
    fn acquire_permit_and_prep<'a>(
        &self,
        hooks: &RunnerHooks,
        workload_cfg: &OciConfig,
        lease: &'a mut UserNamespaceLease,
        session: &'a mut CheckoutPreparationSession,
        bind_state: &'a mut LeaseBindState,
    ) -> Result<(LaunchPermit, RuntimePreparation<'a>), BoundWorkloadRefusal> {
        // Step 22: acquire the workload launch permit under the ROTATED spec — the only spec here.
        let permit = hooks
            .acquire_launch_permit(&self.spec)
            .map_err(|hook_error| BoundWorkloadRefusal::PermitRefused(hook_error.to_string()))?;
        // Seed the runsc-root identity for the EnabledPrepared binding (re-revalidated live at the bind
        // boundary inside the workload runner).
        let expected_root_identity = revalidated_explicit_userns_root_identity().map_err(|reason| {
            BoundWorkloadRefusal::PermitRefused(format!(
                "runsc-root identity revalidation failed: {reason}"
            ))
        })?;
        // Steps 23–24: build the RuntimePreparation over DISJOINT scoped borrows of the capsule's own
        // lease/session/bind_state.
        let prep = RuntimePreparation::new(
            workload_cfg,
            RuntimeBinding::EnabledPrepared {
                expected_root_identity,
                lease,
                session,
                bind_state,
            },
        )
        .map_err(|reason| {
            BoundWorkloadRefusal::PrepModeMismatch(format!(
                "workload OCI layout did not match the prepared mode: {reason}"
            ))
        })?;
        Ok((permit, prep))
    }

    /// **PRODUCTION: acquire the permit AND run the bound workload via the FIXED real runner
    /// ITSELF.** (Sol's r6 finding 2.) There is NO caller-supplied `execute` closure receiving
    /// `&JobSpec`: this method calls [`run_production_container_streaming`](super::run_production_container_streaming)
    /// DIRECTLY with the private `&self.spec`, threading the workload `output`/`cancellation` as plain
    /// arguments. So no production code ever holds a `&JobSpec` it could clone/substitute — the
    /// substitution is not expressible. Injection lives ONLY in the `#[cfg(test)]`
    /// [`Self::acquire_permit_and_run_given`].
    // Names gvisor-module-private RuntimeFinalization / LeaseBindState — legitimate for a `pub(crate)`
    // seam inside a gvisor submodule (they never leave the crate).
    #[allow(
        clippy::too_many_arguments,
        dead_code,
        private_interfaces,
        private_bounds
    )]
    pub(crate) fn acquire_permit_and_run(
        &self,
        hooks: &RunnerHooks,
        workload_cfg: &OciConfig,
        workload_container_id: &str,
        rootfs: &Path,
        lease: &mut UserNamespaceLease,
        session: &mut CheckoutPreparationSession,
        bind_state: &mut LeaseBindState,
        output: Option<Arc<dyn SandboxOutputSink>>,
        cancellation: SandboxCancellation,
    ) -> Result<
        Result<RuntimeFinalization<Result<ContainerRun, RunFailure>>, RunFailure>,
        BoundWorkloadRefusal,
    > {
        let (permit, prep) = self.acquire_permit_and_prep(hooks, workload_cfg, lease, session, bind_state)?;
        Ok(run_production_container_streaming(
            &self.spec,
            workload_cfg,
            permit,
            rootfs,
            workload_container_id,
            output,
            cancellation,
            prep,
        ))
    }

    /// **`test`/`test-support` ONLY: acquire the workload launch permit against the ROTATED spec,
    /// without building the `RuntimePreparation`.** The hardware-independent workload-launch CONTROL-PLANE
    /// fence — for V2 this is `authorize_workload_v2_retained` + the queue→running launch CAS carried by
    /// the returned [`LaunchPermit`]. Split out of [`Self::acquire_permit_and_prep`] so the deterministic
    /// runsc-driver seam can drive the REAL permit fence while faking ONLY the hardware
    /// (`revalidated_explicit_userns_root_identity` + the runsc spawn). The `&self.spec` is used exactly
    /// as line 73's production acquisition does and is NEVER returned — the seal holds.
    #[cfg(any(test, feature = "test-support"))]
    #[allow(dead_code)]
    pub(crate) fn acquire_launch_permit_for_test_support(
        &self,
        hooks: &RunnerHooks,
    ) -> Result<LaunchPermit, BoundWorkloadRefusal> {
        hooks
            .acquire_launch_permit(&self.spec)
            .map_err(|hook_error| BoundWorkloadRefusal::PermitRefused(hook_error.to_string()))
    }

    /// **`#[cfg(test)]` ONLY: the injectable execution seam.** Lets a deterministic test drive the run
    /// with a FAKE spawn (no `runsc`) — the ONE place an `execute` closure receiving `&JobSpec` exists,
    /// and it is absent from every ordinary/`test-support` build. Production callers reach only the
    /// fixed-runner [`Self::acquire_permit_and_run`] above.
    #[cfg(test)]
    #[allow(
        clippy::too_many_arguments,
        dead_code,
        private_interfaces,
        private_bounds
    )]
    pub(crate) fn acquire_permit_and_run_given<F>(
        &self,
        hooks: &RunnerHooks,
        workload_cfg: &OciConfig,
        workload_container_id: &str,
        rootfs: &Path,
        lease: &mut UserNamespaceLease,
        session: &mut CheckoutPreparationSession,
        bind_state: &mut LeaseBindState,
        execute: F,
    ) -> Result<
        Result<RuntimeFinalization<Result<ContainerRun, RunFailure>>, RunFailure>,
        BoundWorkloadRefusal,
    >
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
        let (permit, prep) = self.acquire_permit_and_prep(hooks, workload_cfg, lease, session, bind_state)?;
        Ok(execute(
            &self.spec,
            workload_cfg,
            permit,
            rootfs,
            workload_container_id,
            prep,
        ))
    }
}
