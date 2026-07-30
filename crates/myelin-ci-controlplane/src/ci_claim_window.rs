//! # `ci_claim_window` — the immutable, topology-derived scheduler claim window
//!
//! **The problem this exists to solve.** `job_queue` carries TWO time surfaces (arch 02 §2.1):
//! `lease_expires`, the heartbeat-extendable EXECUTION lease, and `claim_started_at`/
//! `claim_expires_at`, the IMMUTABLE claim-generation facts every downstream authority binds
//! (parent-attempt admission, the launch CAS, `VERIFY_JOB_LAUNCH_LIVE_QUERY`,
//! `CONSUME_PREPARATION_CLAIM_QUERY`, token minting). Both were sized from ONE flat constant,
//! [`CI_RUNNER_EXECUTION_LEASE_TTL_SECS`](crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS) —
//! correct while a claim contains exactly one execution, but a checkout-bearing parent attempt
//! legally contains FOUR sequential full-timeout executions (Hop A advertise, Hop A fetch, Hop B
//! materialization, workload). Its claim would read expired at ~6h10m while the work is still
//! legitimate.
//!
//! **The split this module owns.** The execution lease keeps bounding ONE execution and is renewed
//! at every preparation phase boundary
//! ([`PreparationLeaseCheckpoint`](myelin_ci_sandbox::PreparationLeaseCheckpoint)); the claim window
//! becomes the per-generation HARD CEILING, derived once at dispatch from the job's own durable
//! limits — the same philosophy as the prelaunch journal's `seal_after` deadline ("sized from the
//! job's own durable limits, immutable thereafter"). It is never renewed: option B (extending
//! `claim_expires_at`) would require coordinated mutation of queue timestamps, parent-attempt rows,
//! token authority, and every exact-generation replay check.
//!
//! **Deliberately topology-aware for checkout-bearing jobs ONLY.** A non-checkout job keeps the
//! existing flat window byte-for-byte, so this slice makes no observable timing change to the
//! deployed fleet. Per-job tightening of the non-checkout window is a later, independently reviewed
//! policy change.

use myelin_ci_sandbox::{derive_checkout_authorization_scope, JobKind, WorkspaceSpec};

use crate::job_spec_store::MAX_JOB_TIMEOUT_SECS;
use crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS;

/// Per-execution host-overhead margin (sandbox setup/teardown, reaper cadence, clock skew). Held
/// PER EXECUTION rather than once per attempt: each `runsc` execution carries its own margin.
pub const CI_EXECUTION_LEASE_HEADROOM_SECS: u64 = 600;

/// Sequential executions one checkout-bearing parent attempt may legally contain: Hop A advertise,
/// Hop A fetch, Hop B materialization, workload. Each is separately bounded by the job's own
/// `timeout_secs`, so the attempt's hard ceiling is four slots, not one.
pub const CI_CHECKOUT_PARENT_ATTEMPT_EXECUTIONS: u64 = 4;

/// The largest claim window any dispatch may derive — the four-execution topology at the
/// [`MAX_JOB_TIMEOUT_SECS`] ceiling (88,800s / 24h40m). This is the single Rust authority the
/// `job_queue.claim_window_secs` CHECK constraint's literal upper bound must equal (pinned by
/// `migrations.rs`'s drift assertion) and the ceiling
/// [`CiJobTokenRequest::validate`](crate::ci_manifest_job_runner::CiJobTokenRequest::validate)
/// accepts a claim lifetime up to.
pub const MAX_CI_JOB_CLAIM_WINDOW_SECS: u64 = CI_CHECKOUT_PARENT_ATTEMPT_EXECUTIONS
    * (MAX_JOB_TIMEOUT_SECS as u64 + CI_EXECUTION_LEASE_HEADROOM_SECS);

/// A claim window could not be derived. Typed and fail-closed: a dispatch whose window is
/// underivable never becomes a claimable row, and a claimed row whose durable window disagrees with
/// its spec never mints a credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiClaimWindowError {
    /// The spec's `workspace` is not a well-formed checkout intent (mixed `Some`/`None`, an
    /// unparseable `repo_ref`, a malformed commit), so whether it is checkout-bearing is unknown.
    MalformedWorkspace(String),
    /// The spec's `timeout_secs` exceeds [`MAX_JOB_TIMEOUT_SECS`] — the derivation would exceed the
    /// durable CHECK bound, so it is refused here rather than at the constraint.
    TimeoutTooLong {
        /// The spec's requested wall-clock timeout.
        requested: u32,
        /// The enforced ceiling.
        ceiling: u32,
    },
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

/// Whether a dispatched job is checkout-bearing — i.e. whether its parent attempt contains the two
/// Hop A executions plus Hop B, or only the workload. Derived through the ONE sanctioned sandbox
/// facade so it can never disagree with what the authorization chain itself parses.
pub fn is_checkout_bearing(
    kind: JobKind,
    workspace: &WorkspaceSpec,
) -> Result<bool, CiClaimWindowError> {
    derive_checkout_authorization_scope(kind, workspace)
        .map(|scope| scope.is_some())
        .map_err(CiClaimWindowError::MalformedWorkspace)
}

/// **Derive the immutable claim window for one dispatch.**
///
/// - checkout-bearing → `4 * (timeout_secs + 600)`: four independently bounded executions, each with
///   its own host-overhead margin.
/// - non-checkout → [`CI_RUNNER_EXECUTION_LEASE_TTL_SECS`] UNCHANGED (22,200s at the 6h ceiling),
///   regardless of the job's own timeout. This slice deliberately changes nothing for the deployed
///   fleet; the window is topology-aware for checkout jobs only.
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
    // Both factors are bounded by the ceiling check above, so this cannot overflow i64; the
    // saturating form keeps that structural rather than relying on the reader's arithmetic.
    let slot = u64::from(timeout_secs).saturating_add(CI_EXECUTION_LEASE_HEADROOM_SECS);
    let window = slot.saturating_mul(CI_CHECKOUT_PARENT_ATTEMPT_EXECUTIONS);
    Ok(window.min(MAX_CI_JOB_CLAIM_WINDOW_SECS) as i64)
}

/// [`claim_window_secs`] over a durable launch template — the form dispatch persistence, the
/// dispatch replay check, and the token issuer's cross-check all use, so all three derive the window
/// from exactly the same bytes.
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

    /// **The constant is derived from ONE authority and equals the migration's CHECK bound.** A
    /// future timeout/headroom change fails HERE (and in `migrations.rs`'s literal-drift assertion)
    /// rather than silently diverging from the durable constraint.
    #[test]
    fn the_max_claim_window_is_pinned_to_the_topology_and_the_ceiling() {
        assert_eq!(MAX_CI_JOB_CLAIM_WINDOW_SECS, 88_800);
        assert_eq!(
            MAX_CI_JOB_CLAIM_WINDOW_SECS,
            4 * (MAX_JOB_TIMEOUT_SECS as u64 + 600)
        );
        assert_eq!(CI_RUNNER_EXECUTION_LEASE_TTL_SECS, 22_200);
    }

    /// **A non-checkout job keeps the existing flat window at EVERY timeout** — the no-behavior-
    /// change proof for the deployed fleet. A 1-second job, a 2-hour job, and a job at the 6-hour
    /// ceiling all derive 22,200 seconds, exactly what the flat constant produced before this slice.
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

    /// **A checkout-bearing job's window scales with its OWN durable timeout, four executions deep.**
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

    /// **Every derivable window sits inside the durable CHECK bound.**
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

    /// **An over-ceiling timeout and a malformed workspace are typed refusals, never a guess.**
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
        // An agent job may not be checkout-bearing at all — refused, never silently flattened.
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
