use std::fmt;
use std::sync::Arc;

use myelin_events::{Actor, IdMinter, Timestamp};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{
    DurablePostPitLedger, KmsEngine, PostPitErasureScope, ProviderError, WalOffset,
};
use myelin_tenancy::{Region, TenantId};

use crate::events::pseudonymized_event_principal;
use crate::store::pg::{MessageErasureAttempt, PgMessageStore};
use crate::{
    chat_message_holder_receipts, DurableChatMessageEraser, DurableChatMessageErasureError,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostRestoreChatMessageReport {
    pub restored_to_offset: WalOffset,
    pub selected_subjects: u64,
    pub newly_re_erased_subjects: u64,
    pub already_erased_subjects: u64,
    pub messages_erased: u64,
    pub erasure_events_co_committed: u64,
}

#[derive(Debug)]
pub enum PostRestoreChatMessageError {
    Ledger(ProviderError),
    Erasure(DurableChatMessageErasureError),
    IncompleteHolderProof,
    CountOverflow,
}

impl fmt::Display for PostRestoreChatMessageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ledger(error) => {
                write!(
                    formatter,
                    "the live Chat erasure ledger is unavailable: {error}"
                )
            }
            Self::Erasure(error) => write!(formatter, "restored Chat re-erasure failed: {error}"),
            Self::IncompleteHolderProof => formatter
                .write_str("the restored Chat holder did not return a complete erasure proof"),
            Self::CountOverflow => {
                formatter.write_str("the post-restore Chat report exceeded its count range")
            }
        }
    }
}

impl std::error::Error for PostRestoreChatMessageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Ledger(error) => Some(error),
            Self::Erasure(error) => Some(error),
            Self::IncompleteHolderProof | Self::CountOverflow => None,
        }
    }
}

#[derive(Clone)]
pub struct PostRestoreChatMessageReEraser {
    live_ledger: DurablePostPitLedger,
    restored_messages: DurableChatMessageEraser,
    region: Region,
}

impl PostRestoreChatMessageReEraser {
    pub fn new(
        live_ledger: DurablePostPitLedger,
        restored_store: PgMessageStore,
        restored_kms: Arc<KmsEngine>,
    ) -> Self {
        let region = Region(restored_store.region().to_string());
        let restored_messages =
            DurableChatMessageEraser::new(restored_store, restored_kms, live_ledger.clone());
        Self {
            live_ledger,
            restored_messages,
            region,
        }
    }

    pub async fn run(
        &self,
        restored_to_offset: WalOffset,
        event_ids: &dyn IdMinter,
        now: Timestamp,
    ) -> Result<PostRestoreChatMessageReport, PostRestoreChatMessageError> {
        let records = self
            .live_ledger
            .completed_after(PostPitErasureScope::Chat, restored_to_offset)
            .await
            .map_err(PostRestoreChatMessageError::Ledger)?;
        let selected_subjects =
            u64::try_from(records.len()).map_err(|_| PostRestoreChatMessageError::CountOverflow)?;
        let mut report = PostRestoreChatMessageReport {
            restored_to_offset,
            selected_subjects,
            newly_re_erased_subjects: 0,
            already_erased_subjects: 0,
            messages_erased: 0,
            erasure_events_co_committed: 0,
        };

        for record in records {
            let attempt = MessageErasureAttempt::new(
                restore_operation_id(restored_to_offset, &record.tenant, &record.subject.0),
                restore_actor(&record.tenant, &self.region),
                now.clone(),
                now.clone(),
            );
            let proof = self
                .restored_messages
                .erase_subject_messages_already_recorded(
                    &record.tenant.0,
                    &record.subject.0,
                    event_ids,
                    attempt,
                )
                .await
                .map_err(PostRestoreChatMessageError::Erasure)?;
            chat_message_holder_receipts(&proof)
                .map_err(|_| PostRestoreChatMessageError::IncompleteHolderProof)?;
            let subject_count = if proof.already_completed {
                &mut report.already_erased_subjects
            } else {
                &mut report.newly_re_erased_subjects
            };
            *subject_count = checked_add(*subject_count, 1)?;
            report.messages_erased = checked_add(report.messages_erased, proof.messages_erased)?;
            report.erasure_events_co_committed = checked_add(
                report.erasure_events_co_committed,
                proof.erasure_events_co_committed,
            )?;
        }
        Ok(report)
    }
}

fn restore_actor(tenant: &TenantId, region: &Region) -> Actor {
    let operator = Principal::new(
        tenant.clone(),
        region.clone(),
        PrincipalId("privacy-reerase".into()),
        PrincipalKind::Service,
        DataRole::Processor,
        PrincipalStatus::Active,
    );
    Actor(pseudonymized_event_principal(&tenant.0, &operator))
}

fn restore_operation_id(restored_to_offset: WalOffset, tenant: &TenantId, subject: &str) -> String {
    let mut digest =
        blake3::Hasher::new_derive_key("myelin.chat.post-restore-reerase.operation.v1");
    for field in [
        &restored_to_offset.to_be_bytes()[..],
        tenant.0.as_bytes(),
        subject.as_bytes(),
    ] {
        digest.update(&(field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    format!("post-restore-chat:{}", &digest.finalize().to_hex()[..32])
}

fn checked_add(left: u64, right: u64) -> Result<u64, PostRestoreChatMessageError> {
    left.checked_add(right)
        .ok_or(PostRestoreChatMessageError::CountOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_operation_identity_is_stable_and_carries_no_subject_material() {
        let tenant = TenantId("acme".into());
        let first = restore_operation_id(42, &tenant, "person-private");
        assert_eq!(first, restore_operation_id(42, &tenant, "person-private"));
        assert_ne!(first, restore_operation_id(43, &tenant, "person-private"));
        assert!(!first.contains("acme") && !first.contains("person-private"));
    }
}
