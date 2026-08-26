use std::sync::Arc;

use myelin_storage::{
    DekId, DurablePostPitLedger, KmsEngine, KmsError, PostPitErasureScope, PrivacyHolderReceipt,
    ProviderError, SubjectId,
};
use myelin_tenancy::TenantId;

use crate::dek::issue_subject_key_class;
use crate::pg_issue_store::{
    AuthoredIssueTitleEraseReceipt, AuthoredIssueTitleErasureState, IssueTitleErasureAttempt,
    VerifiedIssueTitleErasureAttempt,
};
use crate::{IssueAuthorizer, IssueStoreError, PgIssueStore};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableIssueTitleErasureProof {
    pub titles_erased: u64,
    pub erasure_events_co_committed: u64,
    pub already_completed: bool,
    pub key_destroyed_this_attempt: bool,
    pub destroyed_key_epoch: Option<u64>,
    pub key_unrecoverable: bool,
}

pub fn issue_title_holder_receipts(
    proof: &DurableIssueTitleErasureProof,
) -> Result<Vec<PrivacyHolderReceipt>, &'static str> {
    if !proof.key_unrecoverable {
        return Err("Issue-title erasure did not prove that its subject key is unrecoverable");
    }
    if proof.titles_erased != proof.erasure_events_co_committed {
        return Err("Issue-title erasure did not co-commit one consequence per erased title");
    }
    PrivacyHolderReceipt::erasure("issue_titles", proof.titles_erased).map(|receipt| vec![receipt])
}

#[derive(Debug)]
pub enum DurableIssueTitleErasureError {
    Store(IssueStoreError),
    Ledger(ProviderError),
    Key(KmsError),
}

impl core::fmt::Display for DurableIssueTitleErasureError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "durable Issue-title erasure: {error}"),
            Self::Ledger(error) => write!(
                formatter,
                "durable Issue-title erasure ledger is unavailable: {error}"
            ),
            Self::Key(error) => write!(formatter, "durable Issue-title key erasure: {error}"),
        }
    }
}

impl std::error::Error for DurableIssueTitleErasureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Ledger(error) => Some(error),
            Self::Key(error) => Some(error),
        }
    }
}

#[derive(Clone)]
pub struct DurableIssueTitleEraser<A: IssueAuthorizer + Clone> {
    issues: PgIssueStore<A>,
    kms: Arc<KmsEngine>,
    ledger: DurablePostPitLedger,
}

enum PreparedTitleErasure {
    Completed(AuthoredIssueTitleEraseReceipt),
    Pending,
}

impl<A: IssueAuthorizer + Clone> DurableIssueTitleEraser<A> {
    pub fn new(issues: PgIssueStore<A>, kms: Arc<KmsEngine>, ledger: DurablePostPitLedger) -> Self {
        Self {
            issues,
            kms,
            ledger,
        }
    }

    pub async fn erase_subject_titles(
        &self,
        tenant: &str,
        subject: &str,
        attempt: IssueTitleErasureAttempt,
    ) -> Result<DurableIssueTitleErasureProof, DurableIssueTitleErasureError> {
        match self.prepare(tenant, subject, &attempt).await? {
            PreparedTitleErasure::Completed(receipt) => {
                return Ok(proof_from_receipt(receipt, true, false, None));
            }
            PreparedTitleErasure::Pending => {}
        }

        self.ledger
            .record(
                PostPitErasureScope::IssueTitles,
                &TenantId::from_token(tenant),
                &SubjectId::new(subject),
                attempt
                    .completed_at_offset()
                    .map_err(DurableIssueTitleErasureError::Store)?,
            )
            .await
            .map_err(DurableIssueTitleErasureError::Ledger)?;

        self.erase_prepared(tenant, subject, attempt).await
    }

    pub(crate) async fn erase_subject_titles_already_recorded(
        &self,
        tenant: &str,
        subject: &str,
        attempt: IssueTitleErasureAttempt,
    ) -> Result<DurableIssueTitleErasureProof, DurableIssueTitleErasureError> {
        match self.prepare(tenant, subject, &attempt).await? {
            PreparedTitleErasure::Completed(receipt) => {
                return Ok(proof_from_receipt(receipt, true, false, None));
            }
            PreparedTitleErasure::Pending => {}
        }
        self.erase_prepared(tenant, subject, attempt).await
    }

    async fn prepare(
        &self,
        tenant: &str,
        subject: &str,
        attempt: &IssueTitleErasureAttempt,
    ) -> Result<PreparedTitleErasure, DurableIssueTitleErasureError> {
        let state = self
            .issues
            .prepare_title_erasure(tenant, subject, attempt.operation_id())
            .await
            .map_err(DurableIssueTitleErasureError::Store)?;
        if let AuthoredIssueTitleErasureState::Completed(receipt) = state {
            return Ok(PreparedTitleErasure::Completed(receipt));
        }
        self.issues
            .verify_title_erasure_ready(tenant, subject, attempt.operation_id())
            .await
            .map_err(DurableIssueTitleErasureError::Store)?;
        Ok(PreparedTitleErasure::Pending)
    }

    async fn erase_prepared(
        &self,
        tenant: &str,
        subject: &str,
        attempt: IssueTitleErasureAttempt,
    ) -> Result<DurableIssueTitleErasureProof, DurableIssueTitleErasureError> {
        let key_id = DekId::new(
            TenantId::from_token(tenant),
            issue_subject_key_class(subject),
        );
        let destroyed_key_epoch = self
            .kms
            .export_dek(&key_id)
            .map_err(DurableIssueTitleErasureError::Key)?
            .map(|(_, epoch)| epoch);
        let key_destroyed_this_attempt = self
            .kms
            .try_destroy_dek(&key_id)
            .map_err(DurableIssueTitleErasureError::Key)?;
        if self
            .kms
            .export_dek(&key_id)
            .map_err(DurableIssueTitleErasureError::Key)?
            .is_some()
        {
            return Err(DurableIssueTitleErasureError::Key(
                KmsError::StateUnavailable("Issue title subject key still resolves"),
            ));
        }

        let receipt = self
            .issues
            .tombstone_titles_co_commit(
                tenant,
                VerifiedIssueTitleErasureAttempt::after_key_destruction(subject, attempt),
            )
            .await
            .map_err(DurableIssueTitleErasureError::Store)?;
        Ok(proof_from_receipt(
            receipt,
            false,
            key_destroyed_this_attempt,
            destroyed_key_epoch,
        ))
    }
}

fn proof_from_receipt(
    receipt: AuthoredIssueTitleEraseReceipt,
    already_completed: bool,
    key_destroyed_this_attempt: bool,
    destroyed_key_epoch: Option<u64>,
) -> DurableIssueTitleErasureProof {
    DurableIssueTitleErasureProof {
        titles_erased: receipt.titles_tombstoned,
        erasure_events_co_committed: receipt.erasure_events_co_committed,
        already_completed,
        key_destroyed_this_attempt,
        destroyed_key_epoch,
        key_unrecoverable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_certificate_requires_both_key_and_consequence_proof() {
        let complete = DurableIssueTitleErasureProof {
            titles_erased: 2,
            erasure_events_co_committed: 2,
            already_completed: false,
            key_destroyed_this_attempt: true,
            destroyed_key_epoch: Some(3),
            key_unrecoverable: true,
        };
        let receipts = issue_title_holder_receipts(&complete).unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].holder, "issue_titles");
        assert_eq!(receipts[0].records_erased, 2);

        let mut incomplete = complete;
        incomplete.erasure_events_co_committed = 1;
        assert!(issue_title_holder_receipts(&incomplete).is_err());
    }
}
