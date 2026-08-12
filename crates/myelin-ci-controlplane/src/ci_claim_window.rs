use myelin_ci_sandbox::{derive_checkout_authorization_scope, JobKind, WorkspaceSpec};

use crate::job_spec_store::MAX_JOB_TIMEOUT_SECS;
use crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS;

pub const CI_EXECUTION_LEASE_HEADROOM_SECS: u64 = 600;

pub const CI_CHECKOUT_PARENT_ATTEMPT_EXECUTIONS: u64 = 4;

pub const MAX_CI_JOB_CLAIM_WINDOW_SECS: u64 = CI_CHECKOUT_PARENT_ATTEMPT_EXECUTIONS
    * (MAX_JOB_TIMEOUT_SECS as u64 + CI_EXECUTION_LEASE_HEADROOM_SECS);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiClaimWindowError {
    MalformedWorkspace(String),
    TimeoutTooLong { requested: u32, ceiling: u32 },
}

impl core::fmt::Display for CiClaimWindowError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CiClaimWindowError::MalformedWorkspace(detail) => write!(
                f,
                "claim window is underivable: the spec's checkout intent is malformed: {detail}"
            ),
            CiClaimWindowError::TimeoutTooLong { requested, ceiling } => write!(
                f,
                "claim window is underivable: spec timeout_secs {requested} exceeds the {ceiling}s \
                 ceiling"
            ),
        }
    }
}

impl std::error::Error for CiClaimWindowError {}

pub fn is_checkout_bearing(
    kind: JobKind,
    workspace: &WorkspaceSpec,
) -> Result<bool, CiClaimWindowError> {
    derive_checkout_authorization_scope(kind, workspace)
        .map(|scope| scope.is_some())
        .map_err(CiClaimWindowError::MalformedWorkspace)
}

pub fn claim_window_secs(
    kind: JobKind,
    workspace: &WorkspaceSpec,
    timeout_secs: u32,
) -> Result<i64, CiClaimWindowError> {
    if timeout_secs > MAX_JOB_TIMEOUT_SECS {
        return Err(CiClaimWindowError::TimeoutTooLong {
            requested: timeout_secs,
            ceiling: MAX_JOB_TIMEOUT_SECS,
        });
    }
    if !is_checkout_bearing(kind, workspace)? {
        return Ok(CI_RUNNER_EXECUTION_LEASE_TTL_SECS);
    }
    let slot = u64::from(timeout_secs).saturating_add(CI_EXECUTION_LEASE_HEADROOM_SECS);
    let window = slot.saturating_mul(CI_CHECKOUT_PARENT_ATTEMPT_EXECUTIONS);
    Ok(window.min(MAX_CI_JOB_CLAIM_WINDOW_SECS) as i64)
}

pub fn claim_window_secs_for_template(
    spec: &myelin_ci_sandbox::JobSpecTemplate,
) -> Result<i64, CiClaimWindowError> {
    claim_window_secs(spec.kind, &spec.workspace, spec.limits.timeout_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout_workspace() -> WorkspaceSpec {
        WorkspaceSpec {
            repo_ref: Some("myelin://acme/git/repo/core".into()),
            commit: Some("a".repeat(40)),
        }
    }

    #[test]
    fn the_max_claim_window_is_pinned_to_the_topology_and_the_ceiling() {
        assert_eq!(MAX_CI_JOB_CLAIM_WINDOW_SECS, 88_800);
        assert_eq!(
            MAX_CI_JOB_CLAIM_WINDOW_SECS,
            4 * (MAX_JOB_TIMEOUT_SECS as u64 + 600)
        );
        assert_eq!(CI_RUNNER_EXECUTION_LEASE_TTL_SECS, 22_200);
    }

    #[test]
    fn a_non_checkout_job_retains_the_flat_window_at_every_timeout() {
        for timeout_secs in [1_u32, 2 * 60 * 60, MAX_JOB_TIMEOUT_SECS] {
            assert_eq!(
                claim_window_secs(JobKind::Ci, &WorkspaceSpec::default(), timeout_secs).unwrap(),
                CI_RUNNER_EXECUTION_LEASE_TTL_SECS,
                "non-checkout timeout {timeout_secs}s must keep the flat 22,200s claim window"
            );
            assert_eq!(
                claim_window_secs(JobKind::Agent, &WorkspaceSpec::default(), timeout_secs).unwrap(),
                CI_RUNNER_EXECUTION_LEASE_TTL_SECS
            );
        }
    }

    #[test]
    fn a_checkout_bearing_job_gets_four_execution_slots() {
        assert_eq!(
            claim_window_secs(JobKind::Ci, &checkout_workspace(), 1).unwrap(),
            4 * (1 + 600)
        );
        assert_eq!(
            claim_window_secs(JobKind::Ci, &checkout_workspace(), 2 * 60 * 60).unwrap(),
            4 * (7_200 + 600)
        );
        assert_eq!(
            claim_window_secs(JobKind::Ci, &checkout_workspace(), MAX_JOB_TIMEOUT_SECS).unwrap(),
            MAX_CI_JOB_CLAIM_WINDOW_SECS as i64,
            "a checkout job at the ceiling derives exactly the durable maximum"
        );
    }

    #[test]
    fn every_derived_window_is_within_the_durable_bound() {
        for timeout_secs in [1_u32, 30, 600, 7_200, MAX_JOB_TIMEOUT_SECS] {
            for workspace in [WorkspaceSpec::default(), checkout_workspace()] {
                let window = claim_window_secs(JobKind::Ci, &workspace, timeout_secs).unwrap();
                assert!(
                    (1..=MAX_CI_JOB_CLAIM_WINDOW_SECS as i64).contains(&window),
                    "derived window {window} must satisfy the durable CHECK (1..={MAX_CI_JOB_CLAIM_WINDOW_SECS})"
                );
            }
        }
    }

    #[test]
    fn an_underivable_window_is_a_typed_refusal() {
        assert_eq!(
            claim_window_secs(
                JobKind::Ci,
                &WorkspaceSpec::default(),
                MAX_JOB_TIMEOUT_SECS + 1
            ),
            Err(CiClaimWindowError::TimeoutTooLong {
                requested: MAX_JOB_TIMEOUT_SECS + 1,
                ceiling: MAX_JOB_TIMEOUT_SECS,
            })
        );
        let mixed = WorkspaceSpec {
            repo_ref: Some("myelin://acme/git/repo/core".into()),
            commit: None,
        };
        assert!(matches!(
            claim_window_secs(JobKind::Ci, &mixed, 60),
            Err(CiClaimWindowError::MalformedWorkspace(_))
        ));
        assert!(matches!(
            claim_window_secs(JobKind::Agent, &checkout_workspace(), 60),
            Err(CiClaimWindowError::MalformedWorkspace(_))
        ));
    }

    #[test]
    fn checkout_intent_is_read_through_the_one_sanctioned_facade() {
        assert!(!is_checkout_bearing(JobKind::Ci, &WorkspaceSpec::default()).unwrap());
        assert!(is_checkout_bearing(JobKind::Ci, &checkout_workspace()).unwrap());
    }
}
