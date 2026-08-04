use std::path::Path;
use std::sync::Arc;

use super::{
    revalidated_explicit_userns_root_identity, run_production_container_streaming, ContainerRun,
    LeaseBindState, OciConfig, RunFailure, RuntimeBinding, RuntimeFinalization, RuntimePreparation,
};
use crate::checkout_orchestration::WorkloadCredentialCarrier;
use crate::user_namespace::{CheckoutPreparationSession, UserNamespaceLease};
use crate::{JobSpec, LaunchPermit, RunnerHooks, SandboxCancellation, SandboxOutputSink};

#[allow(dead_code)]
pub(crate) enum BoundWorkloadRefusal {
    PermitRefused(String),
    PrepModeMismatch(String),
}

#[allow(dead_code)]
pub(crate) struct WorkloadRotatedSpec {
    spec: JobSpec,
}

impl WorkloadRotatedSpec {
    #[allow(dead_code)]
    pub(crate) fn from_carrier(carrier: &WorkloadCredentialCarrier, base_spec: &JobSpec) -> Self {
        WorkloadRotatedSpec {
            spec: carrier.workload_local_spec(base_spec),
        }
    }

    #[allow(private_interfaces, private_bounds)]
    fn acquire_permit_and_prep<'a>(
        &self,
        hooks: &RunnerHooks,
        workload_cfg: &OciConfig,
        lease: &'a mut UserNamespaceLease,
        session: &'a mut CheckoutPreparationSession,
        bind_state: &'a mut LeaseBindState,
    ) -> Result<(LaunchPermit, RuntimePreparation<'a>), BoundWorkloadRefusal> {
        let permit = hooks
            .acquire_launch_permit(&self.spec)
            .map_err(|hook_error| BoundWorkloadRefusal::PermitRefused(hook_error.to_string()))?;
        let expected_root_identity = revalidated_explicit_userns_root_identity().map_err(|reason| {
            BoundWorkloadRefusal::PermitRefused(format!(
                "runsc-root identity revalidation failed: {reason}"
            ))
        })?;
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
