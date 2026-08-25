use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Uuid;

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
}

impl PrivacyRequestScope {
    pub const fn token(self) -> &'static str {
        match self {
            Self::AgentData => "agent_data",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "agent_data" => Some(Self::AgentData),
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
    pub already_erased: bool,
    pub key_unrecoverable: bool,
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
                &[u8::from(receipt.already_erased)],
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
