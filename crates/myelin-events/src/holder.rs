use crate::outbox::{EmitContextBase, IdMinter, OutboxStore};
use crate::{
    Actor, AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventType,
    HandleOutcome, OutboxTx, PiiKeyRef, Region, TenantId, Timestamp, Visibility,
};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const ERASED_EVENT_NAME: &str = "erased";

pub const BUS_ERASED_TYPE: &str = "bus.event.erased";

pub trait InlinePiiShredder {
    fn destroy_key(&self, key_ref: &PiiKeyRef) -> Result<(), ShredError>;

    fn is_live(&self, key_ref: &PiiKeyRef) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShredError {
    KmsUnavailable(PiiKeyRef),
}

impl std::fmt::Display for ShredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShredError::KmsUnavailable(k) => {
                write!(
                    f,
                    "crypto-shred: KMS unavailable for {} - erase INCOMPLETE, retry",
                    k.0
                )
            }
        }
    }
}

impl std::error::Error for ShredError {}

#[derive(Clone, Default)]
pub struct InMemoryShredder {
    live: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    unreachable: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}

impl InMemoryShredder {
    pub fn new() -> Self {
        InMemoryShredder::default()
    }

    pub fn seal(&self, key_ref: &PiiKeyRef) {
        self.live
            .lock()
            .expect("shredder live poisoned")
            .insert(key_ref.0.clone());
    }

    pub fn make_unreachable(&self, key_ref: &PiiKeyRef) {
        self.unreachable
            .lock()
            .expect("shredder unreachable poisoned")
            .insert(key_ref.0.clone());
    }
}

impl InlinePiiShredder for InMemoryShredder {
    fn destroy_key(&self, key_ref: &PiiKeyRef) -> Result<(), ShredError> {
        if self
            .unreachable
            .lock()
            .expect("shredder unreachable poisoned")
            .contains(&key_ref.0)
        {
            return Err(ShredError::KmsUnavailable(key_ref.clone()));
        }
        self.live
            .lock()
            .expect("shredder live poisoned")
            .remove(&key_ref.0);
        Ok(())
    }

    fn is_live(&self, key_ref: &PiiKeyRef) -> bool {
        self.live
            .lock()
            .expect("shredder live poisoned")
            .contains(&key_ref.0)
    }
}

#[derive(Default)]
pub struct BusEventLog {
    events: Vec<EventEnvelope>,
    tombstoned: std::collections::BTreeSet<String>,
}

impl BusEventLog {
    pub fn new() -> Self {
        BusEventLog::default()
    }

    pub fn append(&mut self, env: EventEnvelope) {
        self.events.push(env);
    }

    pub fn events(&self) -> &[EventEnvelope] {
        &self.events
    }

    pub fn is_tombstoned(&self, event_id: &str) -> bool {
        self.tombstoned.contains(event_id)
    }

    fn mark_tombstoned(&mut self, event_id: &str) {
        self.tombstoned.insert(event_id.to_string());
    }
}

fn subject_of_key_ref(key_ref: &PiiKeyRef) -> Option<String> {
    let class = key_ref.0.rsplit('/').next()?;
    class.strip_prefix("subject:").map(|s| s.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocateReport {
    pub subject: String,
    pub inline_pii_events: Vec<LocatedEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocatedEvent {
    pub event_id: String,
    pub type_: String,
    pub pii_key_ref: PiiKeyRef,
    pub tombstoned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EraseReceipt {
    pub subject: String,
    pub tenant: TenantId,
    pub keys_shredded: usize,
    pub tombstones_emitted: usize,
    pub recoverable_remaining: usize,
}

pub struct BusHolder<S: InlinePiiShredder> {
    tenant: TenantId,
    region: Region,
    pub(crate) shredder: S,
}

impl<S: InlinePiiShredder> BusHolder<S> {
    pub fn new(tenant: TenantId, region: Region, shredder: S) -> Self {
        BusHolder {
            tenant,
            region,
            shredder,
        }
    }

    pub fn locate(&self, subject: &str, log: &BusEventLog) -> LocateReport {
        let mut inline_pii_events = Vec::new();
        for env in log.events() {
            if !env.contains_personal_data {
                continue;
            }
            let Some(key_ref) = env.pii_key_ref.as_ref() else {
                continue;
            };
            if subject_of_key_ref(key_ref).as_deref() != Some(subject) {
                continue;
            }
            inline_pii_events.push(LocatedEvent {
                event_id: env.event_id.0.clone(),
                type_: env.type_.0.clone(),
                pii_key_ref: key_ref.clone(),
                tombstoned: log.is_tombstoned(&env.event_id.0),
            });
        }
        LocateReport {
            subject: subject.to_string(),
            inline_pii_events,
        }
    }

    pub fn erase(
        &self,
        subject: &str,
        log: &mut BusEventLog,
        tx: &mut OutboxStore,
        minter: Arc<dyn IdMinter>,
    ) -> Result<EraseReceipt, ShredError> {
        let report = self.locate(subject, log);

        let mut distinct_keys: BTreeMap<String, PiiKeyRef> = BTreeMap::new();
        for ev in &report.inline_pii_events {
            distinct_keys
                .entry(ev.pii_key_ref.0.clone())
                .or_insert_with(|| ev.pii_key_ref.clone());
        }
        for key_ref in distinct_keys.values() {
            self.shredder.destroy_key(key_ref)?;
        }

        let mut tombstones_emitted = 0usize;
        let mut otx = tx.begin(minter, self.emit_ctx_base());
        for ev in &report.inline_pii_events {
            log.mark_tombstoned(&ev.event_id);
            let draft = self.erased_tombstone_draft(subject, &ev.event_id);
            otx.emit(draft, None)
                .map_err(|_| ShredError::KmsUnavailable(ev.pii_key_ref.clone()))?;
            tombstones_emitted += 1;
        }
        otx.stage_state_change(format!(
            "bus.erase subject={subject} keys={}",
            distinct_keys.len()
        ));
        otx.commit().map_err(|_| {
            ShredError::KmsUnavailable(
                report
                    .inline_pii_events
                    .first()
                    .map(|e| e.pii_key_ref.clone())
                    .unwrap_or_else(|| PiiKeyRef("kms://?/?/?".into())),
            )
        })?;

        let recoverable_remaining = report
            .inline_pii_events
            .iter()
            .filter(|ev| self.shredder.is_live(&ev.pii_key_ref))
            .count();

        Ok(EraseReceipt {
            subject: subject.to_string(),
            tenant: self.tenant.clone(),
            keys_shredded: distinct_keys.len(),
            tombstones_emitted,
            recoverable_remaining,
        })
    }

    pub fn export(&self, subject: &str, log: &BusEventLog) -> Vec<ExportedEvent> {
        let mut out = Vec::new();
        for env in log.events() {
            let is_subject_actor = env.actor.0.principal_id.0 == subject;
            let is_subject_pii = env
                .pii_key_ref
                .as_ref()
                .and_then(subject_of_key_ref)
                .as_deref()
                == Some(subject);
            if !is_subject_actor && !is_subject_pii {
                continue;
            }
            let tombstoned = log.is_tombstoned(&env.event_id.0);
            out.push(ExportedEvent {
                event_id: env.event_id.0.clone(),
                type_: env.type_.0.clone(),
                subject_ref: env.subject.0.clone(),
                payload: if tombstoned {
                    serde_json::json!({ "status": "erased" })
                } else {
                    env.payload.clone()
                },
            });
        }
        out
    }

    fn emit_ctx_base(&self) -> EmitContextBase {
        EmitContextBase {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            actor: platform_actor(&self.tenant),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
            caused_by: None,
        }
    }

    fn erased_tombstone_draft(&self, subject: &str, erased_event_id: &str) -> EventDraft {
        EventDraft {
            type_: EventType(BUS_ERASED_TYPE.into()),
            subject: ArtifactRef(format!(
                "myelin://{}/bus/event/{erased_event_id}",
                self.tenant.0
            )),
            aggregate: AggregateKey(format!("bus.event:{erased_event_id}")),
            payload: serde_json::json!({
                "erased_event_id": erased_event_id,
                "subject": subject,
                "reason": "crypto_shred",
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }
}

fn platform_actor(tenant: &TenantId) -> Actor {
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    Actor(Principal::stub(
        PrincipalId("bus:platform".into()),
        PrincipalKind::Service,
        tenant.clone(),
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportedEvent {
    pub event_id: String,
    pub type_: String,
    pub subject_ref: String,
    pub payload: serde_json::Value,
}

pub fn degrade_on_tombstone(env: &EventEnvelope) -> HandleOutcome {
    debug_assert_eq!(
        env.type_.0, BUS_ERASED_TYPE,
        "degrade_on_tombstone is for the *.erased tombstone only"
    );
    HandleOutcome::Done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive_envelope;
    use crate::envelope::EmitContext;
    use crate::outbox::MonotonicMinter;
    use crate::{CausedBy, EventId};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    fn actor_for(id: &str) -> Actor {
        Actor(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    fn retained(
        event_id: &str,
        type_: &str,
        actor_id: &str,
        pii_subject: Option<&str>,
    ) -> EventEnvelope {
        let (contains, key) = match pii_subject {
            Some(s) => (true, Some(PiiKeyRef(format!("kms://acme/0/subject:{s}")))),
            None => (false, None),
        };
        let draft = EventDraft {
            type_: EventType(type_.into()),
            subject: ArtifactRef(format!("myelin://acme/chat/message/{event_id}")),
            aggregate: AggregateKey(format!("chat.message:{event_id}")),
            payload: serde_json::json!({ "ref": format!("myelin://acme/chat/message/{event_id}") }),
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            contains_personal_data: contains,
            pii_key_ref: key,
        };
        let ctx = EmitContext {
            event_id: EventId(event_id.into()),
            tenant: tenant(),
            region: region(),
            actor: actor_for(actor_id),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
            caused_by: Some(CausedBy("human:h".into())),
        };
        derive_envelope(draft, ctx, None)
    }

    fn seeded_log_and_shredder() -> (BusEventLog, InMemoryShredder) {
        let mut log = BusEventLog::new();
        let shredder = InMemoryShredder::new();
        let e1 = retained("01J-1", "chat.message.created", "u42", Some("u42"));
        let e2 = retained("01J-2", "chat.message.created", "u99", Some("u99"));
        let e3 = retained("01J-3", "issue.issue.created", "u42", None);
        let e4 = retained("01J-4", "git.pr.opened", "u7", None);
        for e in [&e1, &e2, &e3, &e4] {
            if let Some(k) = &e.pii_key_ref {
                shredder.seal(k);
            }
        }
        log.append(e1);
        log.append(e2);
        log.append(e3);
        log.append(e4);
        (log, shredder)
    }

    #[test]
    fn locate_finds_only_the_subjects_inline_pii_events() {
        let (log, shredder) = seeded_log_and_shredder();
        let holder = BusHolder::new(tenant(), region(), shredder);
        let report = holder.locate("u42", &log);
        assert_eq!(
            report.inline_pii_events.len(),
            1,
            "only the one inline-PII event for u42"
        );
        assert_eq!(report.inline_pii_events[0].event_id, "01J-1");
        assert!(!report.inline_pii_events[0].tombstoned);
        assert!(report
            .inline_pii_events
            .iter()
            .all(|e| e.event_id != "01J-2"));
    }

    #[test]
    fn erase_destroys_dek_emits_tombstones_zero_recoverable() {
        let (mut log, shredder) = seeded_log_and_shredder();
        let key_u42 = PiiKeyRef("kms://acme/0/subject:u42".into());
        assert!(shredder.is_live(&key_u42), "u42's DEK starts live");

        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let mut outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

        let receipt = holder
            .erase("u42", &mut log, &mut outbox, minter)
            .expect("erase succeeds");

        assert!(!shredder.is_live(&key_u42), "u42's DEK is crypto-shredded");
        assert_eq!(
            receipt.recoverable_remaining, 0,
            "0 recoverable inline-PII after erase"
        );
        assert_eq!(receipt.keys_shredded, 1);
        assert_eq!(receipt.tombstones_emitted, 1, "one *.erased tombstone");
        assert_eq!(
            outbox.committed_count(),
            1,
            "the tombstone committed through the outbox"
        );
        assert!(log.is_tombstoned("01J-1"));
        assert!(shredder.is_live(&PiiKeyRef("kms://acme/0/subject:u99".into())));
    }

    #[test]
    fn consumer_degrades_gracefully_on_tombstone() {
        let (mut log, shredder) = seeded_log_and_shredder();
        let holder = BusHolder::new(tenant(), region(), shredder);
        let mut outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        holder
            .erase("u42", &mut log, &mut outbox, minter)
            .expect("erase");

        let tombstone = retained_tombstone();
        assert_eq!(degrade_on_tombstone(&tombstone), HandleOutcome::Done);
    }

    fn retained_tombstone() -> EventEnvelope {
        let draft = EventDraft {
            type_: EventType(BUS_ERASED_TYPE.into()),
            subject: ArtifactRef("myelin://acme/bus/event/01J-1".into()),
            aggregate: AggregateKey("bus.event:01J-1".into()),
            payload: serde_json::json!({ "erased_event_id": "01J-1", "subject": "u42" }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        let ctx = EmitContext {
            event_id: EventId("01J-T".into()),
            tenant: tenant(),
            region: region(),
            actor: actor_for("bus:platform"),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
            caused_by: None,
        };
        derive_envelope(draft, ctx, None)
    }

    #[test]
    fn export_returns_subject_events_with_references_resolved() {
        let (mut log, shredder) = seeded_log_and_shredder();
        let holder = BusHolder::new(tenant(), region(), shredder);

        let before = holder.export("u42", &log);
        assert!(before.iter().any(|e| e.event_id == "01J-1"));
        assert!(before.iter().any(|e| e.event_id == "01J-3"));
        assert!(before
            .iter()
            .all(|e| e.payload != serde_json::json!({ "status": "erased" })));

        let mut outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        holder
            .erase("u42", &mut log, &mut outbox, minter)
            .expect("erase");
        let after = holder.export("u42", &log);
        let erased = after
            .iter()
            .find(|e| e.event_id == "01J-1")
            .expect("still present");
        assert_eq!(erased.payload, serde_json::json!({ "status": "erased" }));
    }

    #[test]
    fn erase_is_loud_on_kms_failure_never_assumes_erased() {
        let (mut log, shredder) = seeded_log_and_shredder();
        let key_u42 = PiiKeyRef("kms://acme/0/subject:u42".into());
        shredder.make_unreachable(&key_u42);
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let mut outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

        let err = holder
            .erase("u42", &mut log, &mut outbox, minter)
            .expect_err("must be loud");
        assert_eq!(err, ShredError::KmsUnavailable(key_u42.clone()));
        assert_eq!(
            outbox.committed_count(),
            0,
            "no tombstone on a failed erase"
        );
        assert!(
            !log.is_tombstoned("01J-1"),
            "the event is not tombstoned on a failed erase"
        );
        assert!(shredder.is_live(&key_u42));
    }

    #[test]
    fn re_erase_is_idempotent_key_stays_destroyed() {
        let (mut log, shredder) = seeded_log_and_shredder();
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let mut outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

        let first = holder
            .erase("u42", &mut log, &mut outbox, minter.clone())
            .expect("first erase");
        assert_eq!(first.recoverable_remaining, 0);

        let second = holder
            .erase("u42", &mut log, &mut outbox, minter)
            .expect("re-erase");
        assert_eq!(
            second.recoverable_remaining, 0,
            "key stays destroyed across a re-erase"
        );
    }

    #[test]
    fn cdc_2_7_crypto_shred_tombstone_on_the_log() {
        let (mut log, shredder) = seeded_log_and_shredder();
        let holder = BusHolder::new(tenant(), region(), shredder.clone());
        let mut outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

        let receipt = holder
            .erase("u42", &mut log, &mut outbox, minter)
            .expect("erase");
        assert_eq!(receipt.recoverable_remaining, 0);
        assert!(receipt.keys_shredded >= 1);
        assert!(receipt.tombstones_emitted >= 1);
        let row = outbox
            .dead_letters()
            .into_iter()
            .chain(std::iter::empty())
            .next();
        let _ = row;
        assert_eq!(outbox.committed_count(), 1);
    }
}
