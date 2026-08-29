use std::fmt;
use std::sync::Arc;

use myelin_events::clock::ClockReading;
use myelin_events::Actor;
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{
    DurablePostPitLedger, KmsEngine, PostPitErasureScope, ProviderError, SubstrateProvider,
    WalOffset,
};
use myelin_tenancy::{Region, TenantId};

use crate::durable_erase::{
    pr_text_holder_receipts, DurablePrTextEraser, DurablePrTextErasureError,
};
use crate::pg_pr_store::{PgPrStore, PrTextErasureAttempt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostRestorePrTextReport {
    pub restored_to_offset: WalOffset,
    pub selected_subjects: u64,
    pub newly_re_erased_subjects: u64,
    pub already_erased_subjects: u64,
    pub pull_requests_erased: u64,
    pub erasure_events_co_committed: u64,
}

#[derive(Debug)]
pub enum PostRestorePrTextError {
    Ledger(ProviderError),
    Erasure(DurablePrTextErasureError),
    Store(crate::durable::DurableError),
    IncompleteHolderProof,
    CountOverflow,
}

impl fmt::Display for PostRestorePrTextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ledger(error) => {
                write!(
                    formatter,
                    "the live Git erasure ledger is unavailable: {error}"
                )
            }
            Self::Erasure(error) => write!(formatter, "restored Git re-erasure failed: {error}"),
            Self::Store(error) => write!(formatter, "restored Git PR store unavailable: {error}"),
            Self::IncompleteHolderProof => formatter
                .write_str("the restored Git holder did not return a complete erasure proof"),
            Self::CountOverflow => {
                formatter.write_str("the post-restore Git report exceeded its count range")
            }
        }
    }
}

impl std::error::Error for PostRestorePrTextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Ledger(error) => Some(error),
            Self::Erasure(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::IncompleteHolderProof | Self::CountOverflow => None,
        }
    }
}

#[derive(Clone)]
pub struct PostRestorePrTextReEraser {
    live_ledger: DurablePostPitLedger,
    restored_pull_requests: DurablePrTextEraser,
    region: Region,
}

impl PostRestorePrTextReEraser {
    pub fn new(
        live_ledger: DurablePostPitLedger,
        restored_provider: SubstrateProvider,
        restored_kms: Arc<KmsEngine>,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self, PostRestorePrTextError> {
        let region = Region(restored_provider.config().region.clone());
        let store = PgPrStore::new(restored_provider, restored_kms.clone(), runtime)
            .map_err(PostRestorePrTextError::Store)?;
        let restored_pull_requests =
            DurablePrTextEraser::new(store, restored_kms, live_ledger.clone());
        Ok(Self {
            live_ledger,
            restored_pull_requests,
            region,
        })
    }

    pub async fn run(
        &self,
        restored_to_offset: WalOffset,
        observed: ClockReading,
    ) -> Result<PostRestorePrTextReport, PostRestorePrTextError> {
        let records = self
            .live_ledger
            .completed_after(PostPitErasureScope::GitPrText, restored_to_offset)
            .await
            .map_err(PostRestorePrTextError::Ledger)?;
        let selected_subjects =
            u64::try_from(records.len()).map_err(|_| PostRestorePrTextError::CountOverflow)?;
        let mut report = PostRestorePrTextReport {
            restored_to_offset,
            selected_subjects,
            newly_re_erased_subjects: 0,
            already_erased_subjects: 0,
            pull_requests_erased: 0,
            erasure_events_co_committed: 0,
        };

        for record in records {
            let attempt = PrTextErasureAttempt::new(
                restore_operation_id(restored_to_offset, &record.tenant, &record.subject.0),
                restore_actor(&record.tenant, &self.region),
                observed.clone(),
            )
            .map_err(DurablePrTextErasureError::Store)
            .map_err(PostRestorePrTextError::Erasure)?;
            let proof = self
                .restored_pull_requests
                .erase_subject_pr_text_already_recorded(
                    &record.tenant.0,
                    &record.subject.0,
                    attempt,
                )
                .map_err(PostRestorePrTextError::Erasure)?;
            pr_text_holder_receipts(&proof)
                .map_err(|_| PostRestorePrTextError::IncompleteHolderProof)?;
            let subjects = if proof.already_completed {
                &mut report.already_erased_subjects
            } else {
                &mut report.newly_re_erased_subjects
            };
            *subjects = checked_add(*subjects, 1)?;
            report.pull_requests_erased =
                checked_add(report.pull_requests_erased, proof.pull_requests_erased)?;
            report.erasure_events_co_committed = checked_add(
                report.erasure_events_co_committed,
                proof.erasure_events_co_committed,
            )?;
        }
        Ok(report)
    }
}

fn restore_actor(tenant: &TenantId, region: &Region) -> Actor {
    Actor(Principal::new(
        tenant.clone(),
        region.clone(),
        PrincipalId("privacy-reerase".into()),
        PrincipalKind::Service,
        DataRole::Processor,
        PrincipalStatus::Active,
    ))
}

fn restore_operation_id(restored_to_offset: WalOffset, tenant: &TenantId, subject: &str) -> String {
    let mut digest = blake3::Hasher::new_derive_key("myelin.git.post-restore-reerase.operation.v1");
    for field in [
        &restored_to_offset.to_be_bytes()[..],
        tenant.0.as_bytes(),
        subject.as_bytes(),
    ] {
        digest.update(&(field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    format!("post-restore-git:{}", &digest.finalize().to_hex()[..32])
}

fn checked_add(left: u64, right: u64) -> Result<u64, PostRestorePrTextError> {
    left.checked_add(right)
        .ok_or(PostRestorePrTextError::CountOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_operation_identity_is_stable_and_discloses_no_subject_material() {
        let tenant = TenantId("acme".into());
        let first = restore_operation_id(42, &tenant, "person-private");
        assert_eq!(first, restore_operation_id(42, &tenant, "person-private"));
        assert_ne!(first, restore_operation_id(43, &tenant, "person-private"));
        assert!(!first.contains("acme") && !first.contains("person-private"));
    }
}
