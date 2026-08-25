use std::sync::Arc;

use myelin_events::IdMinter;
use myelin_storage::{
    DekId, DurablePostPitLedger, KmsEngine, KmsError, PostPitErasureScope, PrivacyHolderReceipt,
    ProviderError, SubjectId,
};
use myelin_tenancy::TenantId;

use crate::chat_subject_key_class;
use crate::events::event_actor_pseudonym;
use crate::store::pg::{
    AuthoredMessageErasureState, MessageErasureAttempt, PgMessageStore,
    VerifiedMessageErasureAttempt,
};
use crate::store::StoreError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableChatMessageErasureProof {
    pub messages_erased: u64,
    pub erasure_events_co_committed: u64,
    pub key_destroyed_this_attempt: bool,
    pub destroyed_key_epoch: Option<u64>,
    pub key_unrecoverable: bool,
}

pub fn chat_message_holder_receipts(
    proof: &DurableChatMessageErasureProof,
) -> Result<Vec<PrivacyHolderReceipt>, &'static str> {
    if !proof.key_unrecoverable {
        return Err("Chat message erasure did not prove that its subject key is unrecoverable");
    }
    if proof.messages_erased != proof.erasure_events_co_committed {
        return Err("Chat message erasure did not co-commit one consequence per erased message");
    }
    PrivacyHolderReceipt::erasure("chat_messages", proof.messages_erased)
        .map(|receipt| vec![receipt])
}

#[derive(Debug)]
pub enum DurableChatMessageErasureError {
    Store(StoreError),
    Ledger(ProviderError),
    Key(KmsError),
    ClockBeforeUnixEpoch,
}

impl core::fmt::Display for DurableChatMessageErasureError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "durable Chat message erasure: {error}"),
            Self::Ledger(error) => write!(
                formatter,
                "durable Chat message erasure ledger is unavailable: {error}"
            ),
            Self::Key(error) => write!(formatter, "durable Chat message key erasure: {error}"),
            Self::ClockBeforeUnixEpoch => formatter.write_str(
                "system clock is before the Unix epoch; refusing an unorderable Chat erasure",
            ),
        }
    }
}

impl std::error::Error for DurableChatMessageErasureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Ledger(error) => Some(error),
            Self::Key(error) => Some(error),
            Self::ClockBeforeUnixEpoch => None,
        }
    }
}

#[derive(Clone)]
pub struct DurableChatMessageEraser {
    messages: PgMessageStore,
    kms: Arc<KmsEngine>,
    ledger: DurablePostPitLedger,
}

impl DurableChatMessageEraser {
    pub fn new(
        messages: PgMessageStore,
        kms: Arc<KmsEngine>,
        ledger: DurablePostPitLedger,
    ) -> Self {
        Self {
            messages,
            kms,
            ledger,
        }
    }

    pub async fn erase_subject_messages(
        &self,
        tenant: &str,
        subject: &str,
        event_ids: &dyn IdMinter,
        attempt: MessageErasureAttempt,
    ) -> Result<DurableChatMessageErasureProof, DurableChatMessageErasureError> {
        let author = event_actor_pseudonym(tenant, subject);
        let operation_id = attempt.operation_id().to_string();
        let preparation = self
            .messages
            .prepare_author_erasure(tenant, &author, &operation_id)
            .await
            .map_err(DurableChatMessageErasureError::Store)?;
        if let AuthoredMessageErasureState::Completed(receipt) = preparation {
            return Ok(proof_from_receipt(receipt, false, None));
        }

        self.messages
            .verify_author_erasure_ready(tenant, &author, &operation_id)
            .await
            .map_err(DurableChatMessageErasureError::Store)?;

        let tenant_id = TenantId::from_token(tenant);
        self.ledger
            .record(
                PostPitErasureScope::Chat,
                &tenant_id,
                &SubjectId::new(subject),
                unix_seconds(std::time::SystemTime::now())?,
            )
            .await
            .map_err(DurableChatMessageErasureError::Ledger)?;

        let key_id = DekId::new(tenant_id, chat_subject_key_class(&author));
        let destroyed_key_epoch = self
            .kms
            .export_dek(&key_id)
            .map_err(DurableChatMessageErasureError::Key)?
            .map(|(_, epoch)| epoch);
        let key_destroyed_this_attempt = self
            .kms
            .try_destroy_dek(&key_id)
            .map_err(DurableChatMessageErasureError::Key)?;
        if self
            .kms
            .export_dek(&key_id)
            .map_err(DurableChatMessageErasureError::Key)?
            .is_some()
        {
            return Err(DurableChatMessageErasureError::Key(
                KmsError::StateUnavailable("Chat subject key still resolves after destruction"),
            ));
        }

        let receipt = self
            .messages
            .tombstone_author_co_commit(
                tenant,
                event_ids,
                VerifiedMessageErasureAttempt::after_key_destruction(author, attempt),
            )
            .await
            .map_err(DurableChatMessageErasureError::Store)?;
        Ok(proof_from_receipt(
            receipt,
            key_destroyed_this_attempt,
            destroyed_key_epoch,
        ))
    }
}

fn proof_from_receipt(
    receipt: crate::store::pg::AuthoredMessageEraseReceipt,
    key_destroyed_this_attempt: bool,
    destroyed_key_epoch: Option<u64>,
) -> DurableChatMessageErasureProof {
    DurableChatMessageErasureProof {
        messages_erased: receipt.messages_tombstoned,
        erasure_events_co_committed: receipt.erasure_events_co_committed,
        key_destroyed_this_attempt,
        destroyed_key_epoch,
        key_unrecoverable: true,
    }
}

fn unix_seconds(time: std::time::SystemTime) -> Result<u64, DurableChatMessageErasureError> {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| DurableChatMessageErasureError::ClockBeforeUnixEpoch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_certificate_requires_key_and_event_proof() {
        let complete = DurableChatMessageErasureProof {
            messages_erased: 3,
            erasure_events_co_committed: 3,
            key_destroyed_this_attempt: true,
            destroyed_key_epoch: Some(4),
            key_unrecoverable: true,
        };
        let receipts = chat_message_holder_receipts(&complete).unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].holder, "chat_messages");
        assert_eq!(receipts[0].records_erased, 3);

        let mut incomplete = complete;
        incomplete.erasure_events_co_committed = 2;
        assert!(chat_message_holder_receipts(&incomplete).is_err());
    }
}
