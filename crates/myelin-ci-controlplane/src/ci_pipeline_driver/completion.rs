use myelin_ci_sandbox::{PreparationTerminalDisposition, ResourceUsage, TerminalReport};
use myelin_flow::RunId;
use myelin_refs::ArtifactRef;
use myelin_tenancy::TenantId;

use crate::job_accounting_store::{disposition_receipt_v4, CiJobTerminalDisposition};
use crate::job_spec_store::ClaimedDispatchIdentity;

#[derive(Debug, PartialEq, Eq)]
pub enum ClaimRefusal {
    TenantMismatch { reporter: String, claimed: String },
    NoDispatchRecord { job_id: String },
    RunMismatch { durable: String, claimed: String },
    IdemMismatch { durable: String, claimed: String },
}

impl std::fmt::Display for ClaimRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimRefusal::TenantMismatch { reporter, claimed } => write!(
                f,
                "claimed tenant `{claimed}` is not this reporter's tenant `{reporter}`"
            ),
            ClaimRefusal::NoDispatchRecord { job_id } => write!(
                f,
                "no durable ci_job_spec dispatch record for job `{job_id}` (unclaimed/forged completion)"
            ),
            ClaimRefusal::RunMismatch { durable, claimed } => write!(
                f,
                "durable dispatch run_id `{durable}` does not match the claimed run `{claimed}`"
            ),
            ClaimRefusal::IdemMismatch { durable, claimed } => write!(
                f,
                "durable dispatch idem_token `{durable}` does not match the claimed `{claimed}`"
            ),
        }
    }
}

pub(super) fn verify_claimed_identity(
    reporter_tenant: &TenantId,
    claimed_tenant: &TenantId,
    presented_run: &str,
    presented_job_id: &str,
    presented_idem_token: &str,
    durable: Option<ClaimedDispatchIdentity>,
) -> Result<String, ClaimRefusal> {
    if claimed_tenant != reporter_tenant {
        return Err(ClaimRefusal::TenantMismatch {
            reporter: reporter_tenant.0.clone(),
            claimed: claimed_tenant.0.clone(),
        });
    }
    let Some(identity) = durable else {
        return Err(ClaimRefusal::NoDispatchRecord {
            job_id: presented_job_id.to_string(),
        });
    };
    if identity.run_id != presented_run {
        return Err(ClaimRefusal::RunMismatch {
            durable: identity.run_id,
            claimed: presented_run.to_string(),
        });
    }
    if identity.idem_token != presented_idem_token {
        return Err(ClaimRefusal::IdemMismatch {
            durable: identity.idem_token,
            claimed: presented_idem_token.to_string(),
        });
    }
    Ok(identity.stage)
}

#[derive(Clone, Copy)]
pub(super) struct CompletionReceiptInput<'a> {
    pub(super) tenant: &'a TenantId,
    pub(super) region: &'a str,
    pub(super) run: &'a RunId,
    pub(super) job_id: &'a str,
    pub(super) idem_token: &'a str,
    pub(super) stage: &'a str,
    pub(super) passed: bool,
    pub(super) timed_out: bool,
    pub(super) usage: ResourceUsage,
    pub(super) result_refs: &'a [ArtifactRef],
    pub(super) lease_owner: &'a str,
    pub(super) lease_epoch: i64,
    pub(super) claim_nonce: &'a str,
}

pub(super) fn completion_receipt(input: CompletionReceiptInput<'_>) -> String {
    let key = blake3::derive_key(
        "myelin.ci.completion-receipt.v3",
        input.claim_nonce.as_bytes(),
    );
    let mut hasher = blake3::Hasher::new_keyed(&key);
    for frame in [
        input.tenant.0.as_bytes(),
        input.region.as_bytes(),
        input.run.0.as_bytes(),
        input.job_id.as_bytes(),
        input.idem_token.as_bytes(),
        input.stage.as_bytes(),
        &[input.passed as u8],
        &[input.timed_out as u8],
        &input.usage.cpu_seconds.to_be_bytes(),
        &input.usage.mem_byte_seconds.to_be_bytes(),
        input.lease_owner.as_bytes(),
        &input.lease_epoch.to_be_bytes(),
        input.claim_nonce.as_bytes(),
    ] {
        hasher.update(&(frame.len() as u64).to_be_bytes());
        hasher.update(frame);
    }
    hasher.update(&(input.result_refs.len() as u64).to_be_bytes());
    for result_ref in input.result_refs {
        hasher.update(&(result_ref.0.len() as u64).to_be_bytes());
        hasher.update(result_ref.0.as_bytes());
    }
    format!("v3:{}", hasher.finalize().to_hex())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CompletionReceipts {
    pub(super) current_v4: String,
    pub(super) legacy_v3: String,
}

pub(super) fn completion_receipts_v4(
    input: CompletionReceiptInput<'_>,
    disposition: CiJobTerminalDisposition,
) -> CompletionReceipts {
    let legacy_v3 = completion_receipt(input);
    CompletionReceipts {
        current_v4: disposition_receipt_v4(&legacy_v3, disposition),
        legacy_v3,
    }
}

#[derive(Clone, Copy)]
pub(super) struct PreparationCompletionReceiptInput<'a> {
    pub(super) tenant: &'a TenantId,
    pub(super) region: &'a str,
    pub(super) wf_run_id: &'a str,
    pub(super) ci_run_id: &'a str,
    pub(super) job_id: &'a str,
    pub(super) idem_token: &'a str,
    pub(super) stage: &'a str,
    pub(super) reserve_handle: &'a str,
    pub(super) usage: ResourceUsage,
    pub(super) lease_owner: &'a str,
    pub(super) lease_epoch: i64,
    pub(super) claim_nonce: &'a str,
    pub(super) claim_started_at_epoch_secs: i64,
    pub(super) claim_expires_at_epoch_secs: i64,
}

pub(super) fn preparation_completion_receipts(
    input: PreparationCompletionReceiptInput<'_>,
    disposition: PreparationTerminalDisposition,
) -> CompletionReceipts {
    let key = blake3::derive_key(
        "myelin.ci.preparation-completion-receipt.v3",
        input.claim_nonce.as_bytes(),
    );
    let mut hasher = blake3::Hasher::new_keyed(&key);
    for frame in [
        input.tenant.as_str().as_bytes(),
        input.region.as_bytes(),
        input.wf_run_id.as_bytes(),
        input.ci_run_id.as_bytes(),
        input.job_id.as_bytes(),
        input.idem_token.as_bytes(),
        input.stage.as_bytes(),
        input.reserve_handle.as_bytes(),
        &input.usage.cpu_seconds.to_be_bytes(),
        &input.usage.mem_byte_seconds.to_be_bytes(),
        input.lease_owner.as_bytes(),
        &input.lease_epoch.to_be_bytes(),
        input.claim_nonce.as_bytes(),
        &input.claim_started_at_epoch_secs.to_be_bytes(),
        &input.claim_expires_at_epoch_secs.to_be_bytes(),
    ] {
        hasher.update(&(frame.len() as u64).to_be_bytes());
        hasher.update(frame);
    }
    let legacy_v3 = format!("v3:{}", hasher.finalize().to_hex());
    let disposition = CiJobTerminalDisposition::Preparation(disposition);
    CompletionReceipts {
        current_v4: disposition_receipt_v4(&legacy_v3, disposition),
        legacy_v3,
    }
}

pub(super) fn workload_disposition(report: &TerminalReport) -> CiJobTerminalDisposition {
    if report.timed_out {
        CiJobTerminalDisposition::WorkloadTimedOut
    } else if report.passed {
        CiJobTerminalDisposition::WorkloadPassed
    } else {
        CiJobTerminalDisposition::WorkloadFailed
    }
}
