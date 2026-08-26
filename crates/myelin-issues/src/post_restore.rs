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

use crate::{
    issue_title_holder_receipts, DurableIssueTitleEraser, DurableIssueTitleErasureError,
    IssueAuthorizer, IssuePermission, IssueTitleErasureAttempt, PgIssueStore, VisibleIssues,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostRestoreIssueTitleReport {
    pub restored_to_offset: WalOffset,
    pub selected_subjects: u64,
    pub newly_re_erased_subjects: u64,
    pub already_erased_subjects: u64,
    pub titles_erased: u64,
    pub erasure_events_co_committed: u64,
}

#[derive(Debug)]
pub enum PostRestoreIssueTitleError {
    Ledger(ProviderError),
    Erasure(DurableIssueTitleErasureError),
    IncompleteHolderProof,
    CountOverflow,
}

impl fmt::Display for PostRestoreIssueTitleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ledger(error) => {
                write!(
                    formatter,
                    "the live Issue erasure ledger is unavailable: {error}"
                )
            }
            Self::Erasure(error) => write!(formatter, "restored Issue re-erasure failed: {error}"),
            Self::IncompleteHolderProof => formatter
                .write_str("the restored Issue holder did not return a complete erasure proof"),
            Self::CountOverflow => {
                formatter.write_str("the post-restore Issue report exceeded its count range")
            }
        }
    }
}

impl std::error::Error for PostRestoreIssueTitleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Ledger(error) => Some(error),
            Self::Erasure(error) => Some(error),
            Self::IncompleteHolderProof | Self::CountOverflow => None,
        }
    }
}

#[derive(Clone)]
pub struct PostRestoreIssueTitleReEraser {
    live_ledger: DurablePostPitLedger,
    restored_titles: DurableIssueTitleEraser<RestoreOnlyAuthorizer>,
    region: Region,
}

impl PostRestoreIssueTitleReEraser {
    pub fn new(
        live_ledger: DurablePostPitLedger,
        restored_provider: SubstrateProvider,
        restored_kms: Arc<KmsEngine>,
    ) -> Self {
        let region = Region(restored_provider.config().region.clone());
        let store = PgIssueStore::new(
            restored_provider,
            restored_kms.clone(),
            RestoreOnlyAuthorizer,
        );
        let restored_titles =
            DurableIssueTitleEraser::new(store, restored_kms, live_ledger.clone());
        Self {
            live_ledger,
            restored_titles,
            region,
        }
    }

    pub async fn run(
        &self,
        restored_to_offset: WalOffset,
        observed: ClockReading,
    ) -> Result<PostRestoreIssueTitleReport, PostRestoreIssueTitleError> {
        let records = self
            .live_ledger
            .completed_after(PostPitErasureScope::IssueTitles, restored_to_offset)
            .await
            .map_err(PostRestoreIssueTitleError::Ledger)?;
        let selected_subjects =
            u64::try_from(records.len()).map_err(|_| PostRestoreIssueTitleError::CountOverflow)?;
        let mut report = PostRestoreIssueTitleReport {
            restored_to_offset,
            selected_subjects,
            newly_re_erased_subjects: 0,
            already_erased_subjects: 0,
            titles_erased: 0,
            erasure_events_co_committed: 0,
        };

        for record in records {
            let attempt = IssueTitleErasureAttempt::new(
                restore_operation_id(restored_to_offset, &record.tenant, &record.subject.0),
                restore_actor(&record.tenant, &self.region),
                observed.clone(),
            )
            .map_err(DurableIssueTitleErasureError::Store)
            .map_err(PostRestoreIssueTitleError::Erasure)?;
            let proof = self
                .restored_titles
                .erase_subject_titles_already_recorded(&record.tenant.0, &record.subject.0, attempt)
                .await
                .map_err(PostRestoreIssueTitleError::Erasure)?;
            issue_title_holder_receipts(&proof)
                .map_err(|_| PostRestoreIssueTitleError::IncompleteHolderProof)?;
            let subjects = if proof.already_completed {
                &mut report.already_erased_subjects
            } else {
                &mut report.newly_re_erased_subjects
            };
            *subjects = checked_add(*subjects, 1)?;
            report.titles_erased = checked_add(report.titles_erased, proof.titles_erased)?;
            report.erasure_events_co_committed = checked_add(
                report.erasure_events_co_committed,
                proof.erasure_events_co_committed,
            )?;
        }
        Ok(report)
    }
}

#[derive(Clone)]
struct RestoreOnlyAuthorizer;

impl IssueAuthorizer for RestoreOnlyAuthorizer {
    fn may_create(&self, _principal: &Principal, _project_id: &str) -> bool {
        false
    }

    fn may_access(
        &self,
        _principal: &Principal,
        _issue_id: &str,
        _permission: IssuePermission,
    ) -> bool {
        false
    }

    fn visible_issues(&self, _principal: &Principal) -> Result<VisibleIssues, String> {
        Ok(VisibleIssues::None)
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
    let mut digest =
        blake3::Hasher::new_derive_key("myelin.issues.post-restore-reerase.operation.v1");
    for field in [
        &restored_to_offset.to_be_bytes()[..],
        tenant.0.as_bytes(),
        subject.as_bytes(),
    ] {
        digest.update(&(field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    format!("post-restore-issues:{}", &digest.finalize().to_hex()[..32])
}

fn checked_add(left: u64, right: u64) -> Result<u64, PostRestoreIssueTitleError> {
    left.checked_add(right)
        .ok_or(PostRestoreIssueTitleError::CountOverflow)
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
