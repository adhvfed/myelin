use crate::prefs::Channel;
use crate::{Receipt, RedactedMessage};
use myelin_tenancy::Region;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub const OPEN_LEGAL_PROVIDER_DPA: OpenLegalFlag = OpenLegalFlag {
    id: "NOTIF-P26-OPEN-LEGAL",
    subject: "concrete EU-sovereign delivery vendor + DPA / sub-processor posture",
    engineering_posture_ships: "DeliveryAdapter trait impl + EU-region guard + RedactedMessage \
        minimisation + provider-side-erasure-request hook",
    residual_for_counsel:
        "one ratified lawful-basis statement for the already-sent off-cell payload (10.9)",
    owner: "counsel / DPO",
    raised: "2026-06-25",
    resolved: false,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenLegalFlag {
    pub id: &'static str,
    pub subject: &'static str,
    pub engineering_posture_ships: &'static str,
    pub residual_for_counsel: &'static str,
    pub owner: &'static str,
    pub raised: &'static str,
    pub resolved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportReceipt {
    pub provider_ref: String,
    pub accepted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderErasureOutcome {
    Requested { provider_ref: String },
    NothingToErase,
}

pub trait EuTransport: Send + Sync {
    fn transport_id(&self) -> &str;

    fn submit(
        &self,
        message: &RedactedMessage,
        idem_key: &str,
        region: &Region,
    ) -> TransportReceipt;

    fn request_erasure(&self, provider_ref: &str) -> bool;
}

pub struct EuSovereignAdapter {
    channel: Channel,
    region: Region,
    transport: Arc<dyn EuTransport>,
    adapter_id: String,
    submitted: Arc<Mutex<BTreeMap<String, String>>>,
}

impl EuSovereignAdapter {
    pub fn new(
        channel: Channel,
        region: Region,
        transport: Arc<dyn EuTransport>,
    ) -> EuSovereignAdapter {
        let adapter_id = format!("eu:{}:{}", transport.transport_id(), channel.token());
        EuSovereignAdapter {
            channel,
            region,
            transport,
            adapter_id,
            submitted: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn adapter_id(&self) -> &str {
        &self.adapter_id
    }

    pub fn guard_region(&self) -> Result<(), EuProviderError> {
        if crate::delivery::is_eu_region(&self.region) {
            Ok(())
        } else {
            Err(EuProviderError::NonEuRegion(self.region.0.clone()))
        }
    }

    pub fn provider_ref_for(&self, idem_key: &str) -> Option<String> {
        self.submitted
            .lock()
            .expect("eu adapter submitted-map lock")
            .get(idem_key)
            .cloned()
    }

    pub fn try_send(
        &self,
        message: &RedactedMessage,
        idem_key: &str,
    ) -> Result<Receipt, EuProviderError> {
        self.guard_region()?;
        let receipt = self.transport.submit(message, idem_key, &self.region);
        if receipt.accepted {
            self.submitted
                .lock()
                .expect("eu adapter submitted-map lock")
                .insert(idem_key.to_string(), receipt.provider_ref.clone());
        }
        Ok(Receipt {
            idem_key: idem_key.to_string(),
            accepted: receipt.accepted,
        })
    }

    pub fn request_provider_erasure(
        &self,
        idem_key: &str,
    ) -> Result<ProviderErasureOutcome, EuProviderError> {
        let provider_ref = match self.provider_ref_for(idem_key) {
            Some(r) => r,
            None => return Ok(ProviderErasureOutcome::NothingToErase),
        };
        if self.transport.request_erasure(&provider_ref) {
            self.submitted
                .lock()
                .expect("eu adapter submitted-map lock")
                .remove(idem_key);
            Ok(ProviderErasureOutcome::Requested { provider_ref })
        } else {
            Err(EuProviderError::ErasureRejected(provider_ref))
        }
    }
}

impl crate::DeliveryAdapter for EuSovereignAdapter {
    fn channel(&self) -> &str {
        self.channel.token()
    }

    fn region(&self) -> &Region {
        &self.region
    }

    fn send(&self, message: &RedactedMessage, idem_key: &str) -> Receipt {
        match self.try_send(message, idem_key) {
            Ok(receipt) => receipt,
            Err(EuProviderError::NonEuRegion(_)) => Receipt {
                idem_key: idem_key.to_string(),
                accepted: false,
            },
            Err(_) => Receipt {
                idem_key: idem_key.to_string(),
                accepted: false,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EuProviderError {
    NonEuRegion(String),
    ErasureRejected(String),
}

impl std::fmt::Display for EuProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EuProviderError::NonEuRegion(r) => {
                write!(
                    f,
                    "EU-sovereign provider refuses to egress from non-EU region `{r}`"
                )
            }
            EuProviderError::ErasureRejected(p) => {
                write!(
                    f,
                    "sub-processor rejected the provider-side erasure request for `{p}`"
                )
            }
        }
    }
}

impl std::error::Error for EuProviderError {}

#[derive(Clone)]
pub struct RecordingEuTransport {
    transport_id: String,
    refs: Arc<Mutex<BTreeMap<String, String>>>,
    submitted: Arc<Mutex<Vec<String>>>,
    erased: Arc<Mutex<std::collections::BTreeSet<String>>>,
    bounce: Arc<Mutex<std::collections::BTreeSet<String>>>,
}

impl RecordingEuTransport {
    pub fn new(transport_id: &str) -> RecordingEuTransport {
        RecordingEuTransport {
            transport_id: transport_id.to_string(),
            refs: Arc::new(Mutex::new(BTreeMap::new())),
            submitted: Arc::new(Mutex::new(Vec::new())),
            erased: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
            bounce: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
        }
    }

    pub fn with_bounce(self, idem_key: &str) -> RecordingEuTransport {
        self.bounce
            .lock()
            .expect("bounce set lock")
            .insert(idem_key.to_string());
        self
    }

    pub fn submit_count(&self, idem_key: &str) -> usize {
        self.submitted
            .lock()
            .expect("submitted log lock")
            .iter()
            .filter(|k| *k == idem_key)
            .count()
    }

    pub fn was_erased(&self, provider_ref: &str) -> bool {
        self.erased
            .lock()
            .expect("erased set lock")
            .contains(provider_ref)
    }
}

impl EuTransport for RecordingEuTransport {
    fn transport_id(&self) -> &str {
        &self.transport_id
    }

    fn submit(
        &self,
        _message: &RedactedMessage,
        idem_key: &str,
        _region: &Region,
    ) -> TransportReceipt {
        let mut refs = self.refs.lock().expect("refs lock");
        if let Some(existing) = refs.get(idem_key) {
            return TransportReceipt {
                provider_ref: existing.clone(),
                accepted: true,
            };
        }
        self.submitted
            .lock()
            .expect("submitted log lock")
            .push(idem_key.to_string());
        let bounced = self
            .bounce
            .lock()
            .expect("bounce set lock")
            .contains(idem_key);
        if bounced {
            return TransportReceipt {
                provider_ref: String::new(),
                accepted: false,
            };
        }
        let provider_ref = format!("{}:{}", self.transport_id, idem_key);
        refs.insert(idem_key.to_string(), provider_ref.clone());
        TransportReceipt {
            provider_ref,
            accepted: true,
        }
    }

    fn request_erasure(&self, provider_ref: &str) -> bool {
        self.erased
            .lock()
            .expect("erased set lock")
            .insert(provider_ref.to_string());
        true
    }
}

#[cfg(test)]
mod tests;
