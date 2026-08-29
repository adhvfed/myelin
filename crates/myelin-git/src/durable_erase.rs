use std::sync::Arc;

use myelin_storage::{
    DekId, DurablePostPitLedger, KmsEngine, KmsError, PostPitErasureScope, PrivacyHolderReceipt,
    ProviderError, SubjectId,
};
use myelin_tenancy::TenantId;

use crate::dek::git_subject_key_class;
use crate::durable::DurableError;
use crate::pg_pr_store::{
    AuthoredPrTextEraseReceipt, AuthoredPrTextErasureState, PgPrStore, PrTextErasureAttempt,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurablePrTextErasureProof {
    pub pull_requests_erased: u64,
    pub erasure_events_co_committed: u64,
    pub already_completed: bool,
    pub key_destroyed_this_attempt: bool,
    pub destroyed_key_epoch: Option<u64>,
    pub key_unrecoverable: bool,
}

pub fn pr_text_holder_receipts(
    proof: &DurablePrTextErasureProof,
) -> Result<Vec<PrivacyHolderReceipt>, &'static str> {
    if !proof.key_unrecoverable {
        return Err("Git PR text erasure did not prove that its subject key is unrecoverable");
    }
    if proof.pull_requests_erased != proof.erasure_events_co_committed {
        return Err("Git PR text erasure did not co-commit one consequence per pull request");
    }
    PrivacyHolderReceipt::erasure("git_pull_request_text", proof.pull_requests_erased)
        .map(|receipt| vec![receipt])
}

#[derive(Debug)]
pub enum DurablePrTextErasureError {
    Store(DurableError),
    Ledger(ProviderError),
    Key(KmsError),
}

impl core::fmt::Display for DurablePrTextErasureError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "durable Git PR text erasure: {error}"),
            Self::Ledger(error) => write!(
                formatter,
                "durable Git PR text erasure ledger is unavailable: {error}"
            ),
            Self::Key(error) => write!(formatter, "durable Git PR text key erasure: {error}"),
        }
    }
}

impl std::error::Error for DurablePrTextErasureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Ledger(error) => Some(error),
            Self::Key(error) => Some(error),
        }
    }
}

#[derive(Clone)]
pub struct DurablePrTextEraser {
    pull_requests: PgPrStore,
    kms: Arc<KmsEngine>,
    ledger: DurablePostPitLedger,
}

enum PreparedPrTextErasure {
    Completed(AuthoredPrTextEraseReceipt),
    Pending,
}

impl DurablePrTextEraser {
    pub fn new(
        pull_requests: PgPrStore,
        kms: Arc<KmsEngine>,
        ledger: DurablePostPitLedger,
    ) -> Self {
        Self {
            pull_requests,
            kms,
            ledger,
        }
    }

    pub async fn erase_subject_pr_text(
        &self,
        tenant: &str,
        subject: &str,
        attempt: PrTextErasureAttempt,
    ) -> Result<DurablePrTextErasureProof, DurablePrTextErasureError> {
        match self.prepare(tenant, subject, &attempt)? {
            PreparedPrTextErasure::Completed(receipt) => {
                return Ok(proof_from_receipt(receipt, true, false, None));
            }
            PreparedPrTextErasure::Pending => {}
        }

        self.ledger
            .record(
                PostPitErasureScope::GitPrText,
                &TenantId::from_token(tenant),
                &SubjectId::new(subject),
                attempt
                    .completed_at_offset()
                    .map_err(DurablePrTextErasureError::Store)?,
            )
            .await
            .map_err(DurablePrTextErasureError::Ledger)?;

        self.erase_prepared(tenant, subject, attempt)
    }

    pub(crate) fn erase_subject_pr_text_already_recorded(
        &self,
        tenant: &str,
        subject: &str,
        attempt: PrTextErasureAttempt,
    ) -> Result<DurablePrTextErasureProof, DurablePrTextErasureError> {
        match self.prepare(tenant, subject, &attempt)? {
            PreparedPrTextErasure::Completed(receipt) => {
                return Ok(proof_from_receipt(receipt, true, false, None));
            }
            PreparedPrTextErasure::Pending => {}
        }
        self.erase_prepared(tenant, subject, attempt)
    }

    fn prepare(
        &self,
        tenant: &str,
        subject: &str,
        attempt: &PrTextErasureAttempt,
    ) -> Result<PreparedPrTextErasure, DurablePrTextErasureError> {
        let state = self
            .pull_requests
            .prepare_pr_text_erasure(tenant, subject, attempt.operation_id())
            .map_err(DurablePrTextErasureError::Store)?;
        if let AuthoredPrTextErasureState::Completed(receipt) = state {
            return Ok(PreparedPrTextErasure::Completed(receipt));
        }
        self.pull_requests
            .verify_pr_text_erasure_ready(tenant, subject, attempt.operation_id())
            .map_err(DurablePrTextErasureError::Store)?;
        Ok(PreparedPrTextErasure::Pending)
    }

    fn erase_prepared(
        &self,
        tenant: &str,
        subject: &str,
        attempt: PrTextErasureAttempt,
    ) -> Result<DurablePrTextErasureProof, DurablePrTextErasureError> {
        let key_id = DekId::new(TenantId::from_token(tenant), git_subject_key_class(subject));
        let destroyed_key_epoch = self
            .kms
            .export_dek(&key_id)
            .map_err(DurablePrTextErasureError::Key)?
            .map(|(_, epoch)| epoch);
        let key_destroyed_this_attempt = self
            .kms
            .try_destroy_dek(&key_id)
            .map_err(DurablePrTextErasureError::Key)?;
        if self
            .kms
            .export_dek(&key_id)
            .map_err(DurablePrTextErasureError::Key)?
            .is_some()
        {
            return Err(DurablePrTextErasureError::Key(KmsError::StateUnavailable(
                "Git PR text subject key still resolves",
            )));
        }

        let receipt = self
            .pull_requests
            .tombstone_pr_text_co_commit(
                tenant,
                crate::pg_pr_store::pr_text_erasure::VerifiedPrTextErasureAttempt::after_key_destruction(
                    subject,
                    attempt,
                ),
            )
            .map_err(DurablePrTextErasureError::Store)?;
        Ok(proof_from_receipt(
            receipt,
            false,
            key_destroyed_this_attempt,
            destroyed_key_epoch,
        ))
    }
}

fn proof_from_receipt(
    receipt: AuthoredPrTextEraseReceipt,
    already_completed: bool,
    key_destroyed_this_attempt: bool,
    destroyed_key_epoch: Option<u64>,
) -> DurablePrTextErasureProof {
    DurablePrTextErasureProof {
        pull_requests_erased: receipt.pull_requests_tombstoned,
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
    fn a_holder_receipt_requires_key_and_event_proof() {
        let complete = DurablePrTextErasureProof {
            pull_requests_erased: 2,
            erasure_events_co_committed: 2,
            already_completed: false,
            key_destroyed_this_attempt: true,
            destroyed_key_epoch: Some(3),
            key_unrecoverable: true,
        };
        let receipts = pr_text_holder_receipts(&complete).unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].holder, "git_pull_request_text");
        assert_eq!(receipts[0].records_erased, 2);

        let mut incomplete = complete;
        incomplete.erasure_events_co_committed = 1;
        assert!(pr_text_holder_receipts(&incomplete).is_err());
    }
}
