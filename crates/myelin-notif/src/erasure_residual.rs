use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{PiiKeyRef, Timestamp};
use myelin_tenancy::TenantId;

use crate::eu_provider::{EuProviderError, EuSovereignAdapter, ProviderErasureOutcome};
use crate::holder::RestrictSet;

pub const ERASURE_RESIDUAL_PROMPT: &str = "NOTIF-P27";

pub trait InlineDeliveryShredder {
    fn destroy_key(&self, key_ref: &PiiKeyRef) -> Result<(), DeliveryShredError>;

    fn is_live(&self, key_ref: &PiiKeyRef) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryShredError {
    KmsUnavailable(PiiKeyRef),
}

impl std::fmt::Display for DeliveryShredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeliveryShredError::KmsUnavailable(k) => write!(
                f,
                "crypto-shred: KMS unavailable for inline-PII delivery DEK {} - erase INCOMPLETE, retry",
                k.0
            ),
        }
    }
}

impl std::error::Error for DeliveryShredError {}

#[derive(Clone, Default)]
pub struct InMemoryDeliveryShredder {
    live: Arc<Mutex<std::collections::BTreeSet<String>>>,
    unreachable: Arc<Mutex<std::collections::BTreeSet<String>>>,
}

impl InMemoryDeliveryShredder {
    pub fn new() -> InMemoryDeliveryShredder {
        InMemoryDeliveryShredder::default()
    }

    pub fn seal(&self, key_ref: &PiiKeyRef) {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key_ref.0.clone());
    }

    pub fn make_unreachable(&self, key_ref: &PiiKeyRef) {
        self.unreachable
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key_ref.0.clone());
    }
}

impl InlineDeliveryShredder for InMemoryDeliveryShredder {
    fn destroy_key(&self, key_ref: &PiiKeyRef) -> Result<(), DeliveryShredError> {
        if self
            .unreachable
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&key_ref.0)
        {
            return Err(DeliveryShredError::KmsUnavailable(key_ref.clone()));
        }
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key_ref.0);
        Ok(())
    }

    fn is_live(&self, key_ref: &PiiKeyRef) -> bool {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&key_ref.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasedNotifSubject {
    pub subject_id: String,
    pub shredded_keys: Vec<PiiKeyRef>,
    pub provider_erasures_requested: Vec<String>,
    pub erased_at: Timestamp,
}

#[derive(Clone, Default)]
pub struct NotifErasureLedger {
    entries: Arc<Mutex<BTreeMap<String, ErasedNotifSubject>>>,
}

impl NotifErasureLedger {
    pub fn new() -> NotifErasureLedger {
        NotifErasureLedger::default()
    }

    pub fn record(
        &self,
        subject_id: &str,
        shredded_keys: &[PiiKeyRef],
        provider_erasures: &[String],
        erased_at: Timestamp,
    ) {
        let mut g = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = g
            .entry(subject_id.to_string())
            .or_insert_with(|| ErasedNotifSubject {
                subject_id: subject_id.to_string(),
                shredded_keys: Vec::new(),
                provider_erasures_requested: Vec::new(),
                erased_at: erased_at.clone(),
            });
        for k in shredded_keys {
            if !entry.shredded_keys.contains(k) {
                entry.shredded_keys.push(k.clone());
            }
        }
        for p in provider_erasures {
            if !entry.provider_erasures_requested.contains(p) {
                entry.provider_erasures_requested.push(p.clone());
            }
        }
    }

    pub fn is_erased(&self, subject_id: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(subject_id)
    }

    pub fn entry(&self, subject_id: &str) -> Option<ErasedNotifSubject> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_id)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffCellResidual {
    pub idem_key: String,
    pub inline_pii_key: Option<PiiKeyRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidualEraseReceipt {
    pub subject_id: String,
    pub tenant: TenantId,
    pub restrict_applied: bool,
    pub shredded_keys: Vec<PiiKeyRef>,
    pub provider_erasures_requested: Vec<String>,
    pub recoverable_remaining: usize,
}

impl ResidualEraseReceipt {
    pub fn is_green(&self) -> bool {
        self.recoverable_remaining == 0 && self.restrict_applied
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResidualEraseError {
    Shred(DeliveryShredError),
    ProviderErasure(EuProviderError),
}

impl std::fmt::Display for ResidualEraseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResidualEraseError::Shred(e) => write!(f, "erase residual incomplete: {e}"),
            ResidualEraseError::ProviderErasure(e) => {
                write!(
                    f,
                    "erase residual incomplete: provider-side erasure failed: {e}"
                )
            }
        }
    }
}

impl std::error::Error for ResidualEraseError {}

impl From<DeliveryShredError> for ResidualEraseError {
    fn from(e: DeliveryShredError) -> Self {
        ResidualEraseError::Shred(e)
    }
}

impl From<EuProviderError> for ResidualEraseError {
    fn from(e: EuProviderError) -> Self {
        ResidualEraseError::ProviderErasure(e)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn erase_residual<S: InlineDeliveryShredder>(
    subject_id: &str,
    tenant: &TenantId,
    residuals: &[OffCellResidual],
    shredder: &S,
    restrict: &RestrictSet,
    provider: &EuSovereignAdapter,
    ledger: &NotifErasureLedger,
    erased_at: Timestamp,
) -> Result<ResidualEraseReceipt, ResidualEraseError> {
    restrict.set(subject_id, true);

    let mut shredded_keys: Vec<PiiKeyRef> = Vec::new();
    for r in residuals {
        if let Some(key) = &r.inline_pii_key {
            shredder.destroy_key(key)?;
            if !shredded_keys.contains(key) {
                shredded_keys.push(key.clone());
            }
        }
    }

    let mut provider_erasures_requested: Vec<String> = Vec::new();
    for r in residuals {
        match provider.request_provider_erasure(&r.idem_key)? {
            ProviderErasureOutcome::Requested { provider_ref } => {
                if !provider_erasures_requested.contains(&provider_ref) {
                    provider_erasures_requested.push(provider_ref);
                }
            }
            ProviderErasureOutcome::NothingToErase => {}
        }
    }

    let recoverable_remaining = shredded_keys.iter().filter(|k| shredder.is_live(k)).count();

    ledger.record(
        subject_id,
        &shredded_keys,
        &provider_erasures_requested,
        erased_at,
    );

    Ok(ResidualEraseReceipt {
        subject_id: subject_id.to_string(),
        tenant: tenant.clone(),
        restrict_applied: restrict.is_restricted(subject_id),
        shredded_keys,
        provider_erasures_requested,
        recoverable_remaining,
    })
}

#[cfg(test)]
mod tests;
