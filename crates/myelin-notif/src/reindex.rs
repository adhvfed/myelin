use std::collections::BTreeMap;

use myelin_events::reindex::{reindex as bus_reindex, ReindexError as BusReindexError};
use myelin_events::{
    AggregateKey, ArtifactRef, Consumer, DataRole, EmitContextBase, EventType, Message,
    OutboxStore, ReindexReceipt as BusReindexReceipt, ReindexSource, SnapshotDraft, SnapshotScope,
    Visibility,
};
use myelin_query::signals::Signal;
use myelin_tenancy::TenantId;

use crate::router::{InboxProjection, SignalRouter};

pub const NOTIF_OWNER_TOKEN: &str = "notif";

pub const NOTIF_SNAPSHOT_TYPE: &str = "notif.signal.snapshot";

pub const DEFAULT_RETENTION_DAYS: u32 = 90;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionWindow {
    pub days: u32,
}

impl Default for RetentionWindow {
    fn default() -> RetentionWindow {
        RetentionWindow {
            days: DEFAULT_RETENTION_DAYS,
        }
    }
}

impl RetentionWindow {
    pub fn new() -> RetentionWindow {
        RetentionWindow::default()
    }

    pub fn of_days(days: u32) -> RetentionWindow {
        RetentionWindow { days: days.max(1) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReindexError {
    Bus(String),
    MissingSnapshot(String),
}

impl std::fmt::Display for ReindexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReindexError::Bus(e) => write!(f, "notif reindex: bus re-emit failed: {e}"),
            ReindexError::MissingSnapshot(id) => {
                write!(f, "notif reindex: snapshot {id} not found in the outbox (re-emit did not stage it)")
            }
        }
    }
}

impl std::error::Error for ReindexError {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReindexReceipt {
    pub snapshots_emitted: usize,
    pub snapshots_skipped_duplicate: usize,
    pub signals_replayed: usize,
    pub signals_deduplicated: usize,
    pub owners_replayed: Vec<String>,
}

pub struct NotifReindexer<'a> {
    consumer: &'a Consumer<SignalRouter>,
}

impl<'a> NotifReindexer<'a> {
    pub fn new(consumer: &'a Consumer<SignalRouter>) -> NotifReindexer<'a> {
        NotifReindexer { consumer }
    }

    pub fn inbox(&self) -> &InboxProjection {
        self.consumer.handler().inbox()
    }

    pub fn reindex(
        &self,
        tenant: &TenantId,
        scope: &SnapshotScope,
        since: Option<u64>,
        sources: &[&dyn ReindexSource],
        outbox: &mut OutboxStore,
        ctx_base: EmitContextBase,
    ) -> Result<ReindexReceipt, ReindexError> {
        if since.is_none() {
            self.inbox().wipe_tenant(tenant);
        }

        let BusReindexReceipt {
            snapshots_emitted,
            snapshots_skipped_duplicate,
            owners_replayed,
        } = bus_reindex(scope, since, sources, outbox, ctx_base).map_err(map_bus_err)?;

        let mut receipt = ReindexReceipt {
            snapshots_emitted,
            snapshots_skipped_duplicate,
            owners_replayed,
            ..Default::default()
        };

        let full_rebuild = since.is_none();
        for source in sources {
            if source.owner_token() != scope.owner {
                continue;
            }
            for draft in source.replay(scope, since) {
                let event_id = draft.event_id(tenant);
                if full_rebuild {
                    self.consumer
                        .dedup()
                        .forget(self.consumer.name(), &event_id);
                }
                let row = outbox
                    .row(&event_id)
                    .ok_or_else(|| ReindexError::MissingSnapshot(event_id.0.clone()))?;
                let msg = Message {
                    subject: row.envelope.subject.0.clone(),
                    envelope: row.envelope.clone(),
                };
                match self.consumer.deliver(&msg) {
                    myelin_events::Delivered::Deduplicated => receipt.signals_deduplicated += 1,
                    _ => receipt.signals_replayed += 1,
                }
            }
        }

        Ok(receipt)
    }
}

fn map_bus_err(e: BusReindexError) -> ReindexError {
    ReindexError::Bus(e.to_string())
}

pub fn inbox_parity_hash(inbox: &InboxProjection, tenant: &TenantId) -> String {
    let mut rows = inbox.snapshot_for_tenant(tenant);
    rows.sort_by(|a, b| {
        (a.recipient.as_str(), a.dedup_key.as_str())
            .cmp(&(b.recipient.as_str(), b.dedup_key.as_str()))
    });
    let mut hasher = blake3::Hasher::new();
    for row in &rows {
        for field in [
            row.recipient.as_str(),
            row.dedup_key.as_str(),
            row.item_id.as_str(),
            row.subject.0.as_str(),
            row.state.as_str(),
            row.snooze_until.as_deref().unwrap_or(""),
        ] {
            hasher.update(field.as_bytes());
            hasher.update(&[0u8]);
        }
        hasher.update(format!("{:?}", row.reason).as_bytes());
        hasher.update(&[0u8]);
        hasher.update(format!("{:?}", row.class).as_bytes());
        hasher.update(&[0u8]);
        hasher.update(&row.coalesce_count.to_le_bytes());
        hasher.update(&[0u8]);
    }
    format!("blake3:{}", hasher.finalize().to_hex())
}

pub fn signal_snapshot_draft(signal: &Signal, version: u64) -> SnapshotDraft {
    let subject = signal_snapshot_subject(signal);
    SnapshotDraft {
        aggregate: AggregateKey(format!("signal:{}", signal.dedup_key.0)),
        version,
        type_: EventType(NOTIF_SNAPSHOT_TYPE.into()),
        subject: ArtifactRef(subject),
        payload: serde_json::to_value(signal).unwrap_or(serde_json::Value::Null),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
    }
}

pub fn signal_snapshot_subject(signal: &Signal) -> String {
    format!(
        "sig.{}.{}.{}",
        signal.tenant.0,
        signal.severity.token(),
        signal.rule_id.0
    )
}

#[derive(Default)]
pub struct SignalReindexSource {
    truth: BTreeMap<String, (u64, Signal)>,
}

impl SignalReindexSource {
    pub fn new() -> SignalReindexSource {
        SignalReindexSource::default()
    }

    pub fn upsert(&mut self, signal: Signal, version: u64) {
        let key = format!("signal:{}", signal.dedup_key.0);
        self.truth.insert(key, (version, signal));
    }

    pub fn len(&self) -> usize {
        self.truth.len()
    }

    pub fn is_empty(&self) -> bool {
        self.truth.is_empty()
    }
}

impl ReindexSource for SignalReindexSource {
    fn owner_token(&self) -> &str {
        NOTIF_OWNER_TOKEN
    }

    fn replay(&self, _scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        self.truth
            .values()
            .filter(|(v, _)| since.is_none_or(|s| *v > s))
            .map(|(v, signal)| signal_snapshot_draft(signal, *v))
            .collect()
    }
}

pub fn notif_scope(selector: impl Into<String>) -> SnapshotScope {
    SnapshotScope::new(NOTIF_OWNER_TOKEN, selector)
}

#[cfg(test)]
mod tests;
