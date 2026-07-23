use super::MergeCommandResult;
use crate::check_status::{CheckProvider, CheckState, CheckStatusRow, TrustTier};
use crate::pr_store::{MergeAttempt, PrRecord};
use crate::receive_pack::{Oid as PushOid, RefName, RejectReason};

impl MergeCommandResult {
    pub(super) fn into_attempt(self) -> MergeAttempt {
        match self {
            Self::Merged {
                base_ref,
                new_oid,
                update_seq,
            } => MergeAttempt::Merged {
                base_ref,
                new_oid,
                update_seq,
            },
            Self::Blocked { evaluation } => MergeAttempt::Blocked(evaluation),
            Self::InvalidHead { reason } => MergeAttempt::InvalidHead(reason),
            Self::RefRefused {
                base_ref,
                expected,
                actual,
            } => MergeAttempt::RefRefused(RejectReason::NonFastForward {
                ref_name: RefName::new(base_ref),
                expected: PushOid::new(expected),
                actual: PushOid::new(actual),
            }),
        }
    }
}

/// Replace compatibility-era PR check arrays with the durable projection snapshot used at the
/// merge-intent boundary. Only settled successes can become gate-green.
pub(super) fn overlay_projected_checks(
    record: &mut PrRecord,
    rows: impl IntoIterator<Item = CheckStatusRow>,
) {
    record.green_contexts.clear();
    record.fork_unendorsed_contexts.clear();
    for row in rows {
        if row.state != CheckState::Success || !row.cost_settled {
            continue;
        }
        let context = match row.context.provider {
            CheckProvider::Ci => row.context.name,
            CheckProvider::External => format!("external/{}", row.context.name),
        };
        match row.trust_tier {
            TrustTier::Trusted => record.green_contexts.push(context),
            TrustTier::UntrustedFork => record.fork_unendorsed_contexts.push(context),
        }
    }
    record.green_contexts.sort();
    record.green_contexts.dedup();
    record.fork_unendorsed_contexts.sort();
    record.fork_unendorsed_contexts.dedup();
}
