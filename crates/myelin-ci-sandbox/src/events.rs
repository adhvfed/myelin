use myelin_events::validate_event_type;
use myelin_events::AggregateKey;

/// The ONE canonical CI run ordering partition: `run:<run-id>`, in the
/// `type:id` aggregate form the outbox publisher requires. Accepts either a
/// bare run id or any `.../ci/run/<id>`-shaped reference and keeps only the
/// trailing id segment.
pub fn run_aggregate(run: &str) -> AggregateKey {
    let id = run.rsplit('/').next().unwrap_or(run);
    AggregateKey(format!("run:{id}"))
}

pub const CI_CHECK_UPDATED: &str = myelin_events::taxonomy::new_tokens::CI_CHECK_UPDATED;

pub const CI_RESULT: &str = myelin_events::taxonomy::new_tokens::CI_RESULT;

pub const CI_RUN_STARTED: &str = "ci.run.started";
pub const CI_RUN_SUCCEEDED: &str = "ci.run.succeeded";
pub const CI_RUN_FAILED: &str = "ci.run.failed";
pub const CI_RUN_CANCELLED: &str = "ci.run.cancelled";
pub const CI_RUN_TIMED_OUT: &str = "ci.run.timed_out";
pub const CI_RUN_REAPED: &str = "ci.run.reaped";

pub const CI_JOB_STARTED: &str = "ci.job.started";
pub const CI_JOB_SUCCEEDED: &str = "ci.job.succeeded";
pub const CI_JOB_FAILED: &str = "ci.job.failed";
pub const CI_JOB_CANCELLED: &str = "ci.job.cancelled";

pub const CI_LOG_AVAILABLE: &str = "ci.log.available";
pub const CI_ARTIFACT_PUBLISHED: &str = "ci.artifact.published";
pub const CI_COST_METERED: &str = "ci.cost.metered";

pub const CI_DEPLOYMENT_REQUESTED: &str = "ci.deployment.requested";
pub const CI_DEPLOYMENT_APPROVAL_REQUIRED: &str = "ci.deployment.approval_required";
pub const CI_DEPLOYMENT_APPROVED: &str = "ci.deployment.approved";
pub const CI_DEPLOYMENT_REJECTED: &str = "ci.deployment.rejected";
pub const CI_DEPLOYMENT_STARTED: &str = "ci.deployment.started";
pub const CI_DEPLOYMENT_SUCCEEDED: &str = "ci.deployment.succeeded";
pub const CI_DEPLOYMENT_FAILED: &str = "ci.deployment.failed";
pub const CI_DEPLOYMENT_ROLLED_BACK: &str = "ci.deployment.rolled_back";

pub const CI_RUNNER_REGISTERED: &str = "ci.runner.registered";
pub const CI_RUNNER_ATTESTED: &str = "ci.runner.attested";
pub const CI_RUNNER_DEGRADED: &str = "ci.runner.degraded";
pub const CI_RUNNER_OFFLINE: &str = "ci.runner.offline";

pub const CI_PIPELINE_CREATED: &str = "ci.pipeline.created";
pub const CI_PIPELINE_UPDATED: &str = "ci.pipeline.updated";
pub const CI_PIPELINE_VALIDATED: &str = "ci.pipeline.validated";

pub const CI_RUN_ERASED: &str = "ci.run.erased";
pub const CI_DEPLOYMENT_ERASED: &str = "ci.deployment.erased";
pub const CI_RUNNER_ERASED: &str = "ci.runner.erased";

pub const CI_RUN_SNAPSHOT: &str = "ci.run.snapshot";
pub const CI_DEPLOYMENT_SNAPSHOT: &str = "ci.deployment.snapshot";
pub const CI_PIPELINE_SNAPSHOT: &str = "ci.pipeline.snapshot";

pub const CI_LOG_APPENDED: &str = "ci.log.appended";

pub const CI_DURABLE_TOKENS: &[&str] = &[
    CI_CHECK_UPDATED,
    CI_RESULT,
    CI_RUN_STARTED,
    CI_RUN_SUCCEEDED,
    CI_RUN_FAILED,
    CI_RUN_CANCELLED,
    CI_RUN_TIMED_OUT,
    CI_RUN_REAPED,
    CI_JOB_STARTED,
    CI_JOB_SUCCEEDED,
    CI_JOB_FAILED,
    CI_JOB_CANCELLED,
    CI_LOG_AVAILABLE,
    CI_ARTIFACT_PUBLISHED,
    CI_COST_METERED,
    CI_DEPLOYMENT_REQUESTED,
    CI_DEPLOYMENT_APPROVAL_REQUIRED,
    CI_DEPLOYMENT_APPROVED,
    CI_DEPLOYMENT_REJECTED,
    CI_DEPLOYMENT_STARTED,
    CI_DEPLOYMENT_SUCCEEDED,
    CI_DEPLOYMENT_FAILED,
    CI_DEPLOYMENT_ROLLED_BACK,
    CI_RUNNER_REGISTERED,
    CI_RUNNER_ATTESTED,
    CI_RUNNER_DEGRADED,
    CI_RUNNER_OFFLINE,
    CI_PIPELINE_CREATED,
    CI_PIPELINE_UPDATED,
    CI_PIPELINE_VALIDATED,
    CI_RUN_ERASED,
    CI_DEPLOYMENT_ERASED,
    CI_RUNNER_ERASED,
    CI_RUN_SNAPSHOT,
    CI_DEPLOYMENT_SNAPSHOT,
    CI_PIPELINE_SNAPSHOT,
];

pub const CI_FIREHOSE_TOKENS: &[&str] = &[CI_LOG_APPENDED];

pub fn ci_event_tokens() -> impl Iterator<Item = &'static str> {
    CI_DURABLE_TOKENS
        .iter()
        .chain(CI_FIREHOSE_TOKENS.iter())
        .copied()
}

pub fn register_ci_tokens() -> Result<(), (&'static str, myelin_events::TaxonomyError)> {
    for tok in ci_event_tokens() {
        validate_event_type(tok).map_err(|e| (tok, e))?;
    }
    Ok(())
}

pub fn is_durable(token: &str) -> bool {
    CI_DURABLE_TOKENS.contains(&token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_ci_token_parses_the_bus_grammar() {
        for tok in ci_event_tokens() {
            assert!(
                validate_event_type(tok).is_ok(),
                "registered ci token `{tok}` is UNGRAMMATICAL: {:?}",
                validate_event_type(tok)
            );
        }
        assert!(
            register_ci_tokens().is_ok(),
            "register_ci_tokens() must succeed: {:?}",
            register_ci_tokens()
        );
    }

    #[test]
    fn every_ci_token_carries_the_ci_subsystem_prefix() {
        for tok in ci_event_tokens() {
            let head = tok.split('.').next().expect("non-empty token");
            assert_eq!(
                head, "ci",
                "token `{tok}` must carry the `ci` subsystem prefix"
            );
        }
    }

    #[test]
    fn durable_and_firehose_are_disjoint() {
        for f in CI_FIREHOSE_TOKENS {
            assert!(
                !CI_DURABLE_TOKENS.contains(f),
                "firehose token `{f}` must NOT be in the durable set"
            );
        }
    }

    #[test]
    fn no_duplicate_tokens() {
        let mut seen = HashSet::new();
        for tok in ci_event_tokens() {
            assert!(seen.insert(tok), "token `{tok}` appears more than once");
        }
        assert_eq!(
            seen.len(),
            CI_DURABLE_TOKENS.len() + CI_FIREHOSE_TOKENS.len()
        );
    }

    #[test]
    fn the_frozen_x1_seam_tokens_are_registered() {
        assert!(CI_DURABLE_TOKENS.contains(&CI_CHECK_UPDATED));
        assert!(CI_DURABLE_TOKENS.contains(&CI_RESULT));
        assert_eq!(CI_CHECK_UPDATED, "ci.check.updated");
        assert_eq!(CI_RESULT, "ci.result");
    }

    #[test]
    fn the_superseded_legacy_tokens_are_absent() {
        for tok in ci_event_tokens() {
            assert_ne!(
                tok, "ci.status.updated",
                "ci.status.updated is superseded by ci.check.updated (Δ1)"
            );
            assert_ne!(
                tok, "ci.run.passed",
                "ci.run.passed is superseded by ci.check.updated (Δ1)"
            );
        }
    }

    #[test]
    fn is_durable_distinguishes_the_split() {
        assert!(is_durable(CI_RUN_STARTED));
        assert!(is_durable(CI_LOG_AVAILABLE), "the log pointer is durable");
        assert!(
            !is_durable(CI_LOG_APPENDED),
            "the log frame is firehose-only, NOT durable"
        );
    }
}
