use crate::holder::{BusEventLog, BusHolder, EraseReceipt, InlinePiiShredder, ShredError};
use crate::outbox::{IdMinter, OutboxStore};
use crate::{PiiKeyRef, Region, TenantId, Timestamp};
#[cfg(any(test, feature = "test-support"))]
use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasedSubject {
    pub subject: String,
    pub key_refs: Vec<PiiKeyRef>,
    pub erased_at: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErasureLedgerError {
    Unavailable,
}

impl std::fmt::Display for ErasureLedgerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("bus erasure ledger is unavailable"),
        }
    }
}

impl std::error::Error for ErasureLedgerError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BusErasureError {
    Shred(ShredError),
    Ledger(ErasureLedgerError),
}

impl std::fmt::Display for BusErasureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shred(error) => error.fmt(f),
            Self::Ledger(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for BusErasureError {}

impl From<ShredError> for BusErasureError {
    fn from(error: ShredError) -> Self {
        Self::Shred(error)
    }
}

impl From<ErasureLedgerError> for BusErasureError {
    fn from(error: ErasureLedgerError) -> Self {
        Self::Ledger(error)
    }
}

pub trait DurableBusErasure: Send + Sync {
    fn record(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
        key_refs: &[PiiKeyRef],
        erased_at: &Timestamp,
    ) -> Result<(), ErasureLedgerError>;
    fn is_erased(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
    ) -> Result<bool, ErasureLedgerError>;
    fn entries(
        &self,
        tenant: &TenantId,
        region: &Region,
    ) -> Result<Vec<ErasedSubject>, ErasureLedgerError>;
}

#[derive(Clone)]
pub struct BusErasureLedger {
    tenant: TenantId,
    region: Region,
    backend: BusErasureBackend,
}

#[derive(Clone)]
enum BusErasureBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<BTreeMap<String, ErasedSubject>>>),
    Durable(Arc<dyn DurableBusErasure>),
}

impl BusErasureLedger {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(tenant: TenantId, region: Region) -> Self {
        BusErasureLedger {
            tenant,
            region,
            backend: BusErasureBackend::Memory(Arc::new(Mutex::new(BTreeMap::new()))),
        }
    }

    pub fn durable(tenant: TenantId, region: Region, backing: Arc<dyn DurableBusErasure>) -> Self {
        BusErasureLedger {
            tenant,
            region,
            backend: BusErasureBackend::Durable(backing),
        }
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn record(
        &self,
        subject: &str,
        key_refs: &[PiiKeyRef],
        erased_at: Timestamp,
    ) -> Result<(), ErasureLedgerError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            BusErasureBackend::Memory(entries) => {
                let mut g = entries.lock().unwrap_or_else(|e| e.into_inner());
                let entry = g
                    .entry(subject.to_string())
                    .or_insert_with(|| ErasedSubject {
                        subject: subject.to_string(),
                        key_refs: Vec::new(),
                        erased_at: erased_at.clone(),
                    });
                for k in key_refs {
                    if !entry.key_refs.contains(k) {
                        entry.key_refs.push(k.clone());
                    }
                }
                entry.key_refs.sort_by(|a, b| a.0.cmp(&b.0));
                Ok(())
            }
            BusErasureBackend::Durable(d) => {
                d.record(&self.tenant, &self.region, subject, key_refs, &erased_at)
            }
        }
    }

    pub fn is_erased(&self, subject: &str) -> Result<bool, ErasureLedgerError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            BusErasureBackend::Memory(entries) => Ok(entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(subject)),
            BusErasureBackend::Durable(d) => d.is_erased(&self.tenant, &self.region, subject),
        }
    }

    pub fn entries(&self) -> Result<Vec<ErasedSubject>, ErasureLedgerError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            BusErasureBackend::Memory(entries) => Ok(entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .values()
                .cloned()
                .collect()),
            BusErasureBackend::Durable(d) => d.entries(&self.tenant, &self.region),
        }
    }

    pub fn len(&self) -> Result<usize, ErasureLedgerError> {
        self.entries().map(|entries| entries.len())
    }

    pub fn is_empty(&self) -> Result<bool, ErasureLedgerError> {
        self.entries().map(|entries| entries.is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReErasureReceipt {
    pub tenant: TenantId,
    pub region: Region,
    pub re_erased_subjects: usize,
    pub keys_resurrected_by_restore: usize,
    pub tombstones_re_emitted: usize,
    pub resurrected: usize,
    pub ran_at: Timestamp,
}

impl ReErasureReceipt {
    pub fn is_green(&self) -> bool {
        self.resurrected == 0
    }
}

impl<S: InlinePiiShredder> BusHolder<S> {
    pub fn erase_and_record(
        &self,
        subject: &str,
        log: &mut BusEventLog,
        tx: &mut OutboxStore,
        minter: Arc<dyn IdMinter>,
        ledger: &BusErasureLedger,
        now: Timestamp,
    ) -> Result<EraseReceipt, BusErasureError> {
        let report = self.locate(subject, log);
        let mut distinct: Vec<PiiKeyRef> = Vec::new();
        for ev in &report.inline_pii_events {
            if !distinct.contains(&ev.pii_key_ref) {
                distinct.push(ev.pii_key_ref.clone());
            }
        }

        let receipt = self.erase(subject, log, tx, minter)?;

        ledger.record(subject, &distinct, now)?;
        Ok(receipt)
    }

    pub fn re_erase_after_restore(
        &self,
        ledger: &BusErasureLedger,
        log: &mut BusEventLog,
        tx: &mut OutboxStore,
        minter: Arc<dyn IdMinter>,
        now: Timestamp,
    ) -> Result<ReErasureReceipt, BusErasureError> {
        let entries = ledger.entries()?;

        let mut keys_resurrected_by_restore = 0usize;
        for entry in &entries {
            for key in &entry.key_refs {
                if self.shredder.is_live(key) {
                    keys_resurrected_by_restore += 1;
                }
            }
        }

        let mut tombstones_re_emitted = 0usize;
        for entry in &entries {
            let receipt = self.erase(&entry.subject, log, tx, minter.clone())?;
            tombstones_re_emitted += receipt.tombstones_emitted;
        }

        let mut resurrected = 0usize;
        for entry in &entries {
            for key in &entry.key_refs {
                if self.shredder.is_live(key) {
                    resurrected += 1;
                }
            }
        }

        Ok(ReErasureReceipt {
            tenant: ledger.tenant().clone(),
            region: ledger.region().clone(),
            re_erased_subjects: entries.len(),
            keys_resurrected_by_restore,
            tombstones_re_emitted,
            resurrected,
            ran_at: now,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EmitContext;
    use crate::holder::InMemoryShredder;
    use crate::outbox::MonotonicMinter;
    use crate::{
        derive_envelope, Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EventDraft, EventId,
        EventType, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn available<T>(result: Result<T, ErasureLedgerError>) -> T {
        result.expect("in-memory erasure ledger is available")
    }

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn now() -> Timestamp {
        Timestamp("2026-06-19T00:00:00Z".into())
    }
    fn actor_for(id: &str) -> Actor {
        Actor(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    fn inline_pii(event_id: &str, subject: &str) -> crate::EventEnvelope {
        let draft = EventDraft {
            type_: EventType("chat.message.created".into()),
            subject: ArtifactRef(format!("myelin://acme/chat/message/{event_id}")),
            aggregate: AggregateKey(format!("chat.message:{event_id}")),
            payload: serde_json::json!({ "ref": format!("myelin://acme/chat/message/{event_id}") }),
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            contains_personal_data: true,
            pii_key_ref: Some(PiiKeyRef(format!("kms://acme/0/subject:{subject}"))),
        };
        let ctx = EmitContext {
            event_id: EventId(event_id.into()),
            tenant: tenant(),
            region: region(),
            actor: actor_for(subject),
            schema_ver: 1,
            occurred_at: now(),
            recorded_at: now(),
            caused_by: Some(CausedBy("human:h".into())),
        };
        derive_envelope(draft, ctx, None)
    }

    fn seeded(subjects: &[&str]) -> (BusEventLog, InMemoryShredder) {
        let mut log = BusEventLog::new();
        let shredder = InMemoryShredder::new();
        for (i, s) in subjects.iter().enumerate() {
            let ev = inline_pii(&format!("01J-{i}"), s);
            if let Some(k) = &ev.pii_key_ref {
                shredder.seal(k);
            }
            log.append(ev);
        }
        (log, shredder)
    }

    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }

    #[test]
    fn erase_and_record_writes_the_pii_free_ledger() {
        let (mut log, shredder) = seeded(&["u42"]);
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let ledger = BusErasureLedger::new(tenant(), region());
        let mut outbox = OutboxStore::new();

        holder
            .erase_and_record("u42", &mut log, &mut outbox, minter(), &ledger, now())
            .expect("erase+record");

        assert!(
            available(ledger.is_erased("u42")),
            "the ledger remembers u42 was erased"
        );
        assert_eq!(available(ledger.len()), 1);
        let entries = available(ledger.entries());
        let entry = &entries[0];
        assert_eq!(entry.subject, "u42");
        assert_eq!(
            entry.key_refs,
            vec![PiiKeyRef("kms://acme/0/subject:u42".into())]
        );
        assert!(
            !shredder.is_live(&PiiKeyRef("kms://acme/0/subject:u42".into())),
            "key shredded"
        );
    }

    #[test]
    fn re_erase_after_restore_re_destroys_resurrected_keys() {
        let (mut live_log, shredder) = seeded(&["u42"]);
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let ledger = BusErasureLedger::new(tenant(), region());
        let mut outbox = OutboxStore::new();
        holder
            .erase_and_record("u42", &mut live_log, &mut outbox, minter(), &ledger, now())
            .expect("erase+record");
        let key = PiiKeyRef("kms://acme/0/subject:u42".into());
        assert!(!shredder.is_live(&key), "key dead in the live cell");

        let (mut restored_log, _) = seeded(&["u42"]);
        shredder.seal(&key);
        assert!(shredder.is_live(&key), "the restore RESURRECTED u42's DEK");

        let mut reerase_outbox = OutboxStore::new();
        let receipt = holder
            .re_erase_after_restore(
                &ledger,
                &mut restored_log,
                &mut reerase_outbox,
                minter(),
                now(),
            )
            .expect("re-erase");

        assert!(
            !shredder.is_live(&key),
            "the key stays destroyed across the restore"
        );
        assert_eq!(receipt.re_erased_subjects, 1);
        assert_eq!(
            receipt.keys_resurrected_by_restore, 1,
            "the restore brought the key back"
        );
        assert!(
            receipt.tombstones_re_emitted >= 1,
            "re-tombstoned the restored row"
        );
        assert_eq!(receipt.resurrected, 0, "0 resurrected keys post-restore");
        assert!(receipt.is_green());
    }

    #[test]
    fn re_erase_is_idempotent_when_nothing_resurrected() {
        let (mut log, shredder) = seeded(&["u42"]);
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let ledger = BusErasureLedger::new(tenant(), region());
        let mut outbox = OutboxStore::new();
        holder
            .erase_and_record("u42", &mut log, &mut outbox, minter(), &ledger, now())
            .expect("erase+record");

        let (mut log2, _) = seeded(&["u42"]);
        let mut outbox2 = OutboxStore::new();
        let receipt = holder
            .re_erase_after_restore(&ledger, &mut log2, &mut outbox2, minter(), now())
            .expect("re-erase no-op");
        assert_eq!(
            receipt.keys_resurrected_by_restore, 0,
            "nothing was resurrected"
        );
        assert_eq!(receipt.resurrected, 0);
        assert!(receipt.is_green());
    }

    #[test]
    fn ledger_records_many_subjects_and_replays_all() {
        let (mut log, shredder) = seeded(&["u1", "u2", "u3"]);
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let ledger = BusErasureLedger::new(tenant(), region());
        let mut outbox = OutboxStore::new();
        let m = minter();
        for s in ["u1", "u2", "u3"] {
            holder
                .erase_and_record(s, &mut log, &mut outbox, m.clone(), &ledger, now())
                .expect("erase+record");
        }
        assert_eq!(available(ledger.len()), 3);

        let (mut restored, _) = seeded(&["u1", "u2", "u3"]);
        for s in ["u1", "u2", "u3"] {
            shredder.seal(&PiiKeyRef(format!("kms://acme/0/subject:{s}")));
        }
        let mut ro = OutboxStore::new();
        let receipt = holder
            .re_erase_after_restore(&ledger, &mut restored, &mut ro, minter(), now())
            .expect("re-erase all");
        assert_eq!(receipt.re_erased_subjects, 3);
        assert_eq!(receipt.keys_resurrected_by_restore, 3);
        assert_eq!(
            receipt.resurrected, 0,
            "all three stay destroyed across the restore"
        );
    }

    #[test]
    fn re_erase_is_loud_on_kms_failure() {
        let (mut log, shredder) = seeded(&["u42"]);
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let ledger = BusErasureLedger::new(tenant(), region());
        let mut outbox = OutboxStore::new();
        holder
            .erase_and_record("u42", &mut log, &mut outbox, minter(), &ledger, now())
            .expect("erase+record");

        let (mut restored, _) = seeded(&["u42"]);
        let key = PiiKeyRef("kms://acme/0/subject:u42".into());
        shredder.seal(&key);
        shredder.make_unreachable(&key);
        let mut ro = OutboxStore::new();
        let err = holder
            .re_erase_after_restore(&ledger, &mut restored, &mut ro, minter(), now())
            .expect_err("loud on KMS failure");
        assert!(matches!(
            err,
            BusErasureError::Shred(ShredError::KmsUnavailable(_))
        ));
    }
}
