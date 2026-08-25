use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Uuid;

use crate::AgentTraceSubjectErasureProof;

pub const PRIVACY_REQUEST_DEADLINE_DAYS: i64 = 30;
pub const MAX_PRIVACY_HOLDER_RECEIPTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyRequestKind {
    Erasure,
}

impl PrivacyRequestKind {
    pub const fn token(self) -> &'static str {
        match self {
            Self::Erasure => "erasure",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "erasure" => Some(Self::Erasure),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyRequestScope {
    AgentData,
    ChatMessages,
}

impl PrivacyRequestScope {
    pub const fn token(self) -> &'static str {
        match self {
            Self::AgentData => "agent_data",
            Self::ChatMessages => "chat_messages",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "agent_data" => Some(Self::AgentData),
            "chat_messages" => Some(Self::ChatMessages),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyRequestState {
    Pending,
    Processing,
    Completed,
}

impl PrivacyRequestState {
    pub const fn token(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Completed => "completed",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "processing" => Some(Self::Processing),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewPrivacyRequest {
    pub request_id: Uuid,
    pub owner_principal_id: String,
    pub client_nonce: String,
    pub kind: PrivacyRequestKind,
    pub scope: PrivacyRequestScope,
    pub submitted_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyHolderReceipt {
    pub holder: String,
    pub operation: String,
    pub content_hash: String,
    pub records_erased: u64,
    pub key_unrecoverable: bool,
}

impl PrivacyHolderReceipt {
    pub fn erasure(holder: impl Into<String>, records_erased: u64) -> Result<Self, &'static str> {
        let holder = holder.into();
        if holder.is_empty()
            || holder.len() > 128
            || holder.trim() != holder
            || holder.chars().any(char::is_control)
        {
            return Err("a privacy holder name must be a clean 1-128 byte value");
        }

        let operation = PrivacyRequestKind::Erasure.token();
        let mut digest = blake3::Hasher::new_derive_key("myelin.privacy-holder-receipt.erasure.v1");
        for field in [
            holder.as_bytes(),
            operation.as_bytes(),
            &records_erased.to_be_bytes(),
            &[1],
        ] {
            digest.update(&(field.len() as u64).to_be_bytes());
            digest.update(field);
        }
        Ok(Self {
            holder,
            operation: operation.into(),
            content_hash: format!("blake3:{}", digest.finalize().to_hex()),
            records_erased,
            key_unrecoverable: true,
        })
    }
}

pub fn agent_data_holder_receipts(
    proof: &AgentTraceSubjectErasureProof,
) -> Result<Vec<PrivacyHolderReceipt>, &'static str> {
    if !proof.key_unrecoverable {
        return Err("agent-data erasure did not prove that its subject key is unrecoverable");
    }

    [
        ("agent_traces", proof.traces_erased),
        ("model_replay", proof.model_steps_erased),
        ("tool_effects", proof.tool_effects_erased),
    ]
    .into_iter()
    .map(|(holder, records_erased)| PrivacyHolderReceipt::erasure(holder, records_erased))
    .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyRequestCertificate {
    pub request_id: String,
    pub kind: PrivacyRequestKind,
    pub scope: PrivacyRequestScope,
    pub holder_receipts: Vec<PrivacyHolderReceipt>,
    pub content_hash: String,
}

impl PrivacyRequestCertificate {
    pub fn build(
        request_id: Uuid,
        kind: PrivacyRequestKind,
        scope: PrivacyRequestScope,
        mut holder_receipts: Vec<PrivacyHolderReceipt>,
    ) -> Result<Self, &'static str> {
        if holder_receipts.is_empty() || holder_receipts.len() > MAX_PRIVACY_HOLDER_RECEIPTS {
            return Err(
                "a privacy certificate must contain a bounded non-empty holder receipt set",
            );
        }
        holder_receipts.sort_by(|left, right| left.holder.cmp(&right.holder));
        if holder_receipts
            .windows(2)
            .any(|pair| pair[0].holder == pair[1].holder)
        {
            return Err("a privacy certificate cannot contain duplicate holders");
        }
        if holder_receipts.iter().any(|receipt| {
            receipt.holder.is_empty()
                || receipt.holder.len() > 128
                || receipt.operation != kind.token()
                || !valid_blake3_digest(&receipt.content_hash)
                || !receipt.key_unrecoverable
        }) {
            return Err("a privacy certificate contains an invalid or incomplete holder receipt");
        }

        let mut digest = blake3::Hasher::new_derive_key("myelin.privacy-request-certificate.v1");
        for field in [
            request_id.as_bytes().as_slice(),
            kind.token().as_bytes(),
            scope.token().as_bytes(),
        ] {
            digest.update(&(field.len() as u64).to_be_bytes());
            digest.update(field);
        }
        for receipt in &holder_receipts {
            for field in [
                receipt.holder.as_bytes(),
                receipt.operation.as_bytes(),
                receipt.content_hash.as_bytes(),
                &receipt.records_erased.to_be_bytes(),
                &[u8::from(receipt.key_unrecoverable)],
            ] {
                digest.update(&(field.len() as u64).to_be_bytes());
                digest.update(field);
            }
        }

        Ok(Self {
            request_id: request_id.to_string(),
            kind,
            scope,
            holder_receipts,
            content_hash: format!("blake3:{}", digest.finalize().to_hex()),
        })
    }
}

fn valid_blake3_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("blake3:")
        && value[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurablePrivacyRequest {
    pub request_id: Uuid,
    pub owner_principal_id: String,
    pub kind: PrivacyRequestKind,
    pub scope: PrivacyRequestScope,
    pub state: PrivacyRequestState,
    pub attempt_count: u32,
    pub last_failure: Option<String>,
    pub submitted_at: DateTime<Utc>,
    pub deadline_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub certificate: Option<PrivacyRequestCertificate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreatePrivacyRequestOutcome {
    Created(DurablePrivacyRequest),
    Replayed(DurablePrivacyRequest),
    OwnerUnavailable,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivacyRequestLease {
    pub(crate) request: DurablePrivacyRequest,
    pub(crate) lease_owner: String,
    pub(crate) lease_epoch: i64,
}

impl PrivacyRequestLease {
    pub fn request(&self) -> &DurablePrivacyRequest {
        &self.request
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimPrivacyRequestOutcome {
    Claimed(PrivacyRequestLease),
    Busy(DurablePrivacyRequest),
    Completed(DurablePrivacyRequest),
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletePrivacyRequestOutcome {
    Completed(DurablePrivacyRequest),
    LeaseLost,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_real_erasure_becomes_three_independently_counted_holder_receipts() {
        let receipts = agent_data_holder_receipts(&AgentTraceSubjectErasureProof {
            traces_erased: 2,
            model_steps_erased: 3,
            tool_effects_erased: 5,
            key_unrecoverable: true,
        })
        .unwrap();

        assert_eq!(
            receipts
                .iter()
                .map(|receipt| (receipt.holder.as_str(), receipt.records_erased))
                .collect::<Vec<_>>(),
            [
                ("agent_traces", 2),
                ("model_replay", 3),
                ("tool_effects", 5)
            ]
        );
        assert!(
            receipts
                .iter()
                .all(|receipt| receipt.key_unrecoverable
                    && valid_blake3_digest(&receipt.content_hash))
        );
    }

    #[test]
    fn a_holder_receipt_cannot_certify_a_recoverable_key() {
        let result = agent_data_holder_receipts(&AgentTraceSubjectErasureProof {
            traces_erased: 0,
            model_steps_erased: 0,
            tool_effects_erased: 0,
            key_unrecoverable: false,
        });

        assert_eq!(
            result.unwrap_err(),
            "agent-data erasure did not prove that its subject key is unrecoverable"
        );
    }
}
