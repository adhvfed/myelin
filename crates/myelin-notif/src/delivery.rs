use crate::prefs::Channel;
use crate::{HumanisedString, Receipt, RedactedMessage};
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

impl Channel {
    pub fn is_in_cell(self) -> bool {
        matches!(self, Channel::InApp)
    }

    pub fn is_off_cell(self) -> bool {
        !self.is_in_cell()
    }
}

pub fn redact_for_offcell(summary: HumanisedString, class: crate::Class) -> RedactedMessage {
    RedactedMessage {
        rendered: summary,
        class,
    }
}

pub fn build_idem_key(item_id: &str, channel: Channel) -> String {
    format!("{item_id}:{}", channel.token())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryRecord {
    pub item_id: String,
    pub channel: Channel,
    pub idem_key: String,
    pub redacted: bool,
    pub accepted: bool,
    pub adapter: String,
}

#[derive(Clone, Default)]
pub struct DeliveryLedger {
    rows: Arc<Mutex<BTreeMap<(String, String), DeliveryRecord>>>,
}

impl DeliveryLedger {
    pub fn new() -> DeliveryLedger {
        DeliveryLedger::default()
    }

    pub fn record(&self, tenant: &TenantId, rec: DeliveryRecord) -> bool {
        let mut rows = self.rows.lock().expect("delivery ledger lock");
        let key = (tenant.0.clone(), rec.idem_key.clone());
        if rows.contains_key(&key) {
            return false;
        }
        rows.insert(key, rec);
        true
    }

    pub fn contains(&self, tenant: &TenantId, idem_key: &str) -> bool {
        self.rows
            .lock()
            .expect("delivery ledger lock")
            .contains_key(&(tenant.0.clone(), idem_key.to_string()))
    }

    pub fn get(&self, tenant: &TenantId, idem_key: &str) -> Option<DeliveryRecord> {
        self.rows
            .lock()
            .expect("delivery ledger lock")
            .get(&(tenant.0.clone(), idem_key.to_string()))
            .cloned()
    }

    pub fn effective_count(&self, tenant: &TenantId) -> usize {
        self.rows
            .lock()
            .expect("delivery ledger lock")
            .keys()
            .filter(|(t, _)| t == &tenant.0)
            .count()
    }
}

pub fn effective_delivery_count(
    ledger: &DeliveryLedger,
    tenant: &TenantId,
    item_id: &str,
    channel: Channel,
) -> usize {
    let idem = build_idem_key(item_id, channel);
    usize::from(ledger.contains(tenant, &idem))
}

#[derive(Clone)]
pub struct MockAdapter {
    channel: Channel,
    region: Region,
    sent: Arc<Mutex<Vec<String>>>,
    bounce: Arc<Mutex<std::collections::BTreeSet<String>>>,
}

impl MockAdapter {
    pub fn new(channel: Channel, region: Region) -> MockAdapter {
        MockAdapter {
            channel,
            region,
            sent: Arc::new(Mutex::new(Vec::new())),
            bounce: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
        }
    }

    pub fn with_bounce(self, idem_key: &str) -> MockAdapter {
        self.bounce
            .lock()
            .expect("bounce set lock")
            .insert(idem_key.to_string());
        self
    }

    pub fn sent_log(&self) -> Vec<String> {
        self.sent.lock().expect("sent log lock").clone()
    }

    pub fn send_count(&self, idem_key: &str) -> usize {
        self.sent
            .lock()
            .expect("sent log lock")
            .iter()
            .filter(|k| *k == idem_key)
            .count()
    }
}

impl crate::DeliveryAdapter for MockAdapter {
    fn channel(&self) -> &str {
        self.channel.token()
    }

    fn region(&self) -> &Region {
        &self.region
    }

    fn send(&self, _message: &RedactedMessage, idem_key: &str) -> Receipt {
        self.sent
            .lock()
            .expect("sent log lock")
            .push(idem_key.to_string());
        let bounced = self
            .bounce
            .lock()
            .expect("bounce set lock")
            .contains(idem_key);
        Receipt {
            idem_key: idem_key.to_string(),
            accepted: !bounced,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Delivered(Receipt),
    AlreadyDelivered { accepted: bool },
    Bounced(Receipt),
}

pub struct DeliveryFabric {
    adapters: BTreeMap<Channel, Arc<dyn crate::DeliveryAdapter + Send + Sync>>,
    ledger: DeliveryLedger,
}

impl DeliveryFabric {
    pub fn new(ledger: DeliveryLedger) -> DeliveryFabric {
        DeliveryFabric {
            adapters: BTreeMap::new(),
            ledger,
        }
    }

    pub fn with_mock(ledger: DeliveryLedger, region: Region) -> DeliveryFabric {
        let mut fabric = DeliveryFabric::new(ledger);
        for channel in [
            Channel::InApp,
            Channel::WebPush,
            Channel::MobilePush,
            Channel::Email,
            Channel::Desktop,
        ] {
            fabric = fabric.with_adapter(Arc::new(MockAdapter::new(channel, region.clone())));
        }
        fabric
    }

    pub fn with_adapter(
        mut self,
        adapter: Arc<dyn crate::DeliveryAdapter + Send + Sync>,
    ) -> DeliveryFabric {
        if let Some(channel) = channel_from_token(adapter.channel()) {
            self.adapters.insert(channel, adapter);
        }
        self
    }

    pub fn ledger(&self) -> &DeliveryLedger {
        &self.ledger
    }

    pub fn adapter(
        &self,
        channel: Channel,
    ) -> Option<&Arc<dyn crate::DeliveryAdapter + Send + Sync>> {
        self.adapters.get(&channel)
    }

    pub fn deliver(
        &self,
        tenant: &TenantId,
        item_id: &str,
        channel: Channel,
        message: &RedactedMessage,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let idem_key = build_idem_key(item_id, channel);

        if let Some(existing) = self.ledger.get(tenant, &idem_key) {
            return Ok(DeliveryOutcome::AlreadyDelivered {
                accepted: existing.accepted,
            });
        }

        let adapter = self
            .adapters
            .get(&channel)
            .ok_or_else(|| DeliveryError::NoAdapter(channel.token()))?;

        let receipt = adapter.send(message, &idem_key);
        let redacted = channel.is_off_cell();

        let rec = DeliveryRecord {
            item_id: item_id.to_string(),
            channel,
            idem_key: idem_key.clone(),
            redacted,
            accepted: receipt.accepted,
            adapter: adapter.region().0.clone() + ":" + adapter.channel(),
        };
        let first = self.ledger.record(tenant, rec);
        if !first {
            let existing = self
                .ledger
                .get(tenant, &idem_key)
                .expect("the winning record exists");
            return Ok(DeliveryOutcome::AlreadyDelivered {
                accepted: existing.accepted,
            });
        }

        if receipt.accepted {
            Ok(DeliveryOutcome::Delivered(receipt))
        } else {
            Ok(DeliveryOutcome::Bounced(receipt))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryError {
    NoAdapter(&'static str),
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeliveryError::NoAdapter(c) => {
                write!(f, "no delivery adapter registered for channel `{c}`")
            }
        }
    }
}

impl std::error::Error for DeliveryError {}

pub fn channel_from_token(token: &str) -> Option<Channel> {
    match token {
        "in_app" => Some(Channel::InApp),
        "web_push" => Some(Channel::WebPush),
        "mobile_push" => Some(Channel::MobilePush),
        "email" => Some(Channel::Email),
        "desktop" => Some(Channel::Desktop),
        _ => None,
    }
}

pub fn is_eu_region(region: &Region) -> bool {
    let code = region.as_str();
    const EU_PREFIXES: &[&str] = &[
        "fr-", "de-", "nl-", "pl-", "eu-", "eea-", "es-", "it-", "se-", "fi-", "ie-", "be-", "at-",
        "dk-", "pt-", "cz-", "ro-", "gr-", "hu-",
    ];
    EU_PREFIXES.iter().any(|p| code.starts_with(p))
}

#[cfg(test)]
mod tests;
