use std::fmt;

use crate::backup::WalOffset;
use crate::{DurableAgentTraceStore, DurablePostPitLedger, PostPitErasureScope};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostRestoreAgentDataReport {
    pub restored_to_offset: WalOffset,
    pub selected_subjects: u64,
    pub newly_re_erased_subjects: u64,
    pub already_erased_subjects: u64,
    pub records_erased: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostRestoreAgentDataError {
    LedgerUnavailable,
    HolderUnavailable,
    IncompleteHolderProof,
    CountOverflow,
}

impl fmt::Display for PostRestoreAgentDataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::LedgerUnavailable => "the live post-restore erasure ledger is unavailable",
            Self::HolderUnavailable => "the restored agent-data holder is unavailable",
            Self::IncompleteHolderProof => {
                "the restored agent-data holder did not return a complete erasure proof"
            }
            Self::CountOverflow => "the post-restore agent-data report exceeded its count range",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for PostRestoreAgentDataError {}

#[derive(Clone)]
pub struct PostRestoreAgentDataReEraser {
    live_ledger: DurablePostPitLedger,
    restored_holder: DurableAgentTraceStore,
}

impl PostRestoreAgentDataReEraser {
    pub fn new(live_ledger: DurablePostPitLedger, restored_holder: DurableAgentTraceStore) -> Self {
        Self {
            live_ledger,
            restored_holder,
        }
    }

    pub async fn run(
        &self,
        restored_to_offset: WalOffset,
    ) -> Result<PostRestoreAgentDataReport, PostRestoreAgentDataError> {
        let records = self
            .live_ledger
            .completed_after(PostPitErasureScope::AgentData, restored_to_offset)
            .await
            .map_err(|_| PostRestoreAgentDataError::LedgerUnavailable)?;
        let selected_subjects =
            u64::try_from(records.len()).map_err(|_| PostRestoreAgentDataError::CountOverflow)?;
        let mut newly_re_erased_subjects = 0_u64;
        let mut already_erased_subjects = 0_u64;
        let mut records_erased = 0_u64;

        for record in records {
            let receipt = self
                .restored_holder
                .erase_for_subject(&record.tenant.0, &record.subject.0)
                .await
                .map_err(|_| PostRestoreAgentDataError::HolderUnavailable)?;
            if receipt.already_erased {
                already_erased_subjects = checked_increment(already_erased_subjects)?;
            } else {
                newly_re_erased_subjects = checked_increment(newly_re_erased_subjects)?;
            }

            let proof = self
                .restored_holder
                .erasure_proof_for_subject(&record.tenant.0, &record.subject.0)
                .await
                .map_err(|_| PostRestoreAgentDataError::HolderUnavailable)?
                .filter(|proof| proof.key_unrecoverable)
                .ok_or(PostRestoreAgentDataError::IncompleteHolderProof)?;
            let subject_records = proof
                .traces_erased
                .checked_add(proof.model_steps_erased)
                .and_then(|total| total.checked_add(proof.tool_effects_erased))
                .ok_or(PostRestoreAgentDataError::CountOverflow)?;
            records_erased = records_erased
                .checked_add(subject_records)
                .ok_or(PostRestoreAgentDataError::CountOverflow)?;
        }

        Ok(PostRestoreAgentDataReport {
            restored_to_offset,
            selected_subjects,
            newly_re_erased_subjects,
            already_erased_subjects,
            records_erased,
        })
    }
}

fn checked_increment(value: u64) -> Result<u64, PostRestoreAgentDataError> {
    value
        .checked_add(1)
        .ok_or(PostRestoreAgentDataError::CountOverflow)
}
