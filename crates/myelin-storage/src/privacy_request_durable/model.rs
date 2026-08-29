use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Uuid;

use crate::AgentTraceSubjectErasureProof;

pub const PRIVACY_REQUEST_DEADLINE_DAYS: i64 = 30;
pub const MAX_PRIVACY_HOLDER_RECEIPTS: usize = 64;
const ERASURE_RECEIPT_CONTEXT: &str = "myelin.privacy-holder-receipt.erasure.v1";
const LEGACY_AGENT_DATA_RECEIPT_CONTEXT: &str = "myelin.privacy-holder-receipt.agent-data.v1";

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
    GitPullRequestText,
    IssueTitles,
}

impl PrivacyRequestScope {
    pub const fn token(self) -> &'static str {
        match self {
            Self::AgentData => "agent_data",
            Self::ChatMessages => "chat_messages",
            Self::GitPullRequestText => "git_pull_request_text",
            Self::IssueTitles => "issue_titles",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "agent_data" => Some(Self::AgentData),
            "chat_messages" => Some(Self::ChatMessages),
            "git_pull_request_text" => Some(Self::GitPullRequestText),
            "issue_titles" => Some(Self::IssueTitles),
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

        let operation = PrivacyRequestKind::Erasure.token().to_string();
        let content_hash = holder_receipt_hash(
            ERASURE_RECEIPT_CONTEXT,
            &holder,
            &operation,
            records_erased,
            true,
        );
        Ok(Self {
            holder,
            operation,
            content_hash,
            records_erased,
            key_unrecoverable: true,
        })
    }

    fn verify(&self, kind: PrivacyRequestKind) -> Result<(), &'static str> {
        if self.holder.is_empty()
            || self.holder.len() > 128
            || self.holder.trim() != self.holder
            || self.holder.chars().any(char::is_control)
            || self.operation != kind.token()
            || !self.key_unrecoverable
        {
            return Err("a privacy certificate contains an invalid or incomplete holder receipt");
        }
        let expected = holder_receipt_hash(
            ERASURE_RECEIPT_CONTEXT,
            &self.holder,
            &self.operation,
            self.records_erased,
            self.key_unrecoverable,
        );
        let legacy_agent_data = matches!(
            self.holder.as_str(),
            "agent_traces" | "model_replay" | "tool_effects"
        ) && self.content_hash
            == holder_receipt_hash(
                LEGACY_AGENT_DATA_RECEIPT_CONTEXT,
                &self.holder,
                &self.operation,
                self.records_erased,
                self.key_unrecoverable,
            );
        if self.content_hash != expected && !legacy_agent_data {
            return Err("a privacy holder receipt failed content verification");
        }
        Ok(())
    }
}

fn holder_receipt_hash(
    context: &'static str,
    holder: &str,
    operation: &str,
    records_erased: u64,
    key_unrecoverable: bool,
) -> String {
    let mut digest = blake3::Hasher::new_derive_key(context);
    for field in [
        holder.as_bytes(),
        operation.as_bytes(),
        &records_erased.to_be_bytes(),
        &[u8::from(key_unrecoverable)],
    ] {
        digest.update(&(field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    format!("blake3:{}", digest.finalize().to_hex())
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
    .map(|(holder, records_erased)| {
        let mut receipt = PrivacyHolderReceipt::erasure(holder, records_erased)?;
        receipt.content_hash = holder_receipt_hash(
            LEGACY_AGENT_DATA_RECEIPT_CONTEXT,
            &receipt.holder,
            &receipt.operation,
            receipt.records_erased,
            receipt.key_unrecoverable,
        );
        Ok(receipt)
    })
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
        verify_holder_receipts(kind, &holder_receipts)?;
        let mut certificate = Self {
            request_id: request_id.to_string(),
            kind,
            scope,
            holder_receipts,
            content_hash: String::new(),
        };
        certificate.content_hash = certificate_hash(&certificate);
        Ok(certificate)
    }

    pub fn verify(&self) -> Result<(), &'static str> {
        let request_id = Uuid::parse_str(&self.request_id)
            .map_err(|_| "a privacy certificate request identity is not a UUID")?;
        if request_id.to_string() != self.request_id {
            return Err("a privacy certificate request identity is not canonical");
        }
        if self.holder_receipts.is_empty()
            || self.holder_receipts.len() > MAX_PRIVACY_HOLDER_RECEIPTS
        {
            return Err(
                "a privacy certificate must contain a bounded non-empty holder receipt set",
            );
        }
        if self
            .holder_receipts
            .windows(2)
            .any(|pair| pair[0].holder >= pair[1].holder)
        {
            return Err("privacy certificate holders are not in canonical unique order");
        }
        verify_holder_receipts(self.kind, &self.holder_receipts)?;
        if self.content_hash != certificate_hash(self) {
            return Err("a privacy certificate failed content verification");
        }
        Ok(())
    }
}

fn verify_holder_receipts(
    kind: PrivacyRequestKind,
    receipts: &[PrivacyHolderReceipt],
) -> Result<(), &'static str> {
    if receipts
        .windows(2)
        .any(|pair| pair[0].holder == pair[1].holder)
    {
        return Err("a privacy certificate cannot contain duplicate holders");
    }
    receipts.iter().try_for_each(|receipt| receipt.verify(kind))
}

fn certificate_hash(certificate: &PrivacyRequestCertificate) -> String {
    let mut digest = blake3::Hasher::new_derive_key("myelin.privacy-request-certificate.v1");
    for field in [
        certificate.request_id.as_bytes(),
        certificate.kind.token().as_bytes(),
        certificate.scope.token().as_bytes(),
    ] {
        digest.update(&(field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    for receipt in &certificate.holder_receipts {
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
    format!("blake3:{}", digest.finalize().to_hex())
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
        assert!(receipts.iter().all(|receipt| receipt.key_unrecoverable
            && receipt.verify(PrivacyRequestKind::Erasure).is_ok()));
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

    #[test]
    fn a_certificate_detects_tampered_holder_counts_and_its_own_digest() {
        let mut certificate = PrivacyRequestCertificate::build(
            Uuid::from_u128(17),
            PrivacyRequestKind::Erasure,
            PrivacyRequestScope::ChatMessages,
            vec![PrivacyHolderReceipt::erasure("chat_messages", 3).unwrap()],
        )
        .unwrap();
        assert!(certificate.verify().is_ok());

        certificate.holder_receipts[0].records_erased = 4;
        assert_eq!(
            certificate.verify().unwrap_err(),
            "a privacy holder receipt failed content verification",
        );
        certificate.holder_receipts[0] = PrivacyHolderReceipt::erasure("chat_messages", 3).unwrap();
        certificate.content_hash = format!("blake3:{}", "0".repeat(64));
        assert_eq!(
            certificate.verify().unwrap_err(),
            "a privacy certificate failed content verification",
        );
    }
}
