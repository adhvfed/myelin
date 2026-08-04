use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, SubjectPattern};
use myelin_identity::iam_events::{signals, IDENTITY_TUPLE_WRITTEN};
use myelin_identity::{ObjectId, ObjectType, PrincipalId, RelName, Zookie};
use myelin_storage::{OltpStoreHolder, TenantQuery, TenantScope, TenantTable};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const S8_TABLE: &str = "authz_visible";

pub const S8_HOLDER: &str = "identity_authz_reverse_index";

pub const S8_CONSUMER: &str = "s8_reverse_index";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseRow {
    pub subject: PrincipalId,
    pub relation: RelName,
    pub object_id: ObjectId,
}

impl ReverseRow {
    fn key(&self) -> (String, String, String) {
        (
            self.subject.0.clone(),
            self.relation.0.clone(),
            self.object_id.0.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct PartKey {
    tenant: String,
    region: String,
    object_type: String,
}

#[derive(Default)]
struct Partition {
    rows: BTreeMap<(String, String, String), ReverseRow>,
}

#[derive(Default)]
struct Inner {
    partitions: BTreeMap<PartKey, Partition>,
    watermarks: BTreeMap<(String, String), Zookie>,
}

#[derive(Clone)]
pub struct ReverseIndex {
    inner: Arc<Mutex<Inner>>,
    holder: OltpStoreHolder,
}

impl Default for ReverseIndex {
    fn default() -> Self {
        ReverseIndex::new()
    }
}

impl ReverseIndex {
    pub fn new() -> ReverseIndex {
        let holder = OltpStoreHolder::new(S8_HOLDER);
        let _receipt = holder.register();
        ReverseIndex {
            inner: Arc::new(Mutex::new(Inner::default())),
            holder,
        }
    }

    pub fn holder(&self) -> &OltpStoreHolder {
        &self.holder
    }

    pub fn dek_class(&self, scope: &TenantScope) -> String {
        format!("kms://{}/tenant", scope.tenant().0)
    }

    pub fn apply_delta(
        &self,
        scope: &TenantScope,
        op: &str,
        object_type: &ObjectType,
        row: ReverseRow,
        zookie: &Zookie,
    ) {
        if !matches!(op, "add" | "remove") {
            return;
        }
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S8_TABLE));
        let pk = PartKey {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
            object_type: object_type.0.clone(),
        };
        let mut inner = self.lock();
        let partition = inner.partitions.entry(pk).or_default();
        match op {
            "add" => {
                partition.rows.insert(row.key(), row);
            }
            "remove" => {
                partition.rows.remove(&row.key());
            }
            _ => unreachable!("operation was validated before locking the projection"),
        }
        let wm_key = (scope.tenant().0.clone(), scope.region().0.clone());
        let advance = inner
            .watermarks
            .get(&wm_key)
            .map(|cur| zookie.0 > cur.0)
            .unwrap_or(true);
        if advance {
            inner.watermarks.insert(wm_key, zookie.clone());
        }
    }

    pub fn objects_for(
        &self,
        scope: &TenantScope,
        object_type: &ObjectType,
        subject: &PrincipalId,
        relation: &RelName,
    ) -> Vec<ObjectId> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S8_TABLE));
        let pk = PartKey {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
            object_type: object_type.0.clone(),
        };
        let inner = self.lock();
        inner
            .partitions
            .get(&pk)
            .map(|p| {
                p.rows
                    .values()
                    .filter(|r| &r.subject == subject && &r.relation == relation)
                    .map(|r| r.object_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn subjects_for(
        &self,
        scope: &TenantScope,
        object_type: &ObjectType,
        object_id: &str,
        relation: &RelName,
    ) -> Vec<PrincipalId> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S8_TABLE));
        let pk = PartKey {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
            object_type: object_type.0.clone(),
        };
        let inner = self.lock();
        inner
            .partitions
            .get(&pk)
            .map(|p| {
                p.rows
                    .values()
                    .filter(|r| r.object_id.0 == object_id && &r.relation == relation)
                    .map(|r| r.subject.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn watermark(&self, scope: &TenantScope) -> Zookie {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S8_TABLE));
        let inner = self.lock();
        inner
            .watermarks
            .get(&(scope.tenant().0.clone(), scope.region().0.clone()))
            .cloned()
            .unwrap_or_else(|| Zookie(String::new()))
    }

    pub fn row_count(&self, scope: &TenantScope, object_type: &ObjectType) -> usize {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S8_TABLE));
        let pk = PartKey {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
            object_type: object_type.0.clone(),
        };
        self.lock()
            .partitions
            .get(&pk)
            .map(|p| p.rows.len())
            .unwrap_or(0)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

static S8_SUBJECTS: &[SubjectPattern] = &[SubjectPattern(String::new())];

pub struct ReverseIndexConsumer {
    index: ReverseIndex,
    lag: Arc<AtomicU64>,
}

impl ReverseIndexConsumer {
    pub fn new(index: ReverseIndex) -> ReverseIndexConsumer {
        ReverseIndexConsumer {
            index,
            lag: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn index(&self) -> &ReverseIndex {
        &self.index
    }

    pub fn lag(&self) -> u64 {
        self.lag.load(Ordering::SeqCst)
    }

    pub const LAG_SIGNAL: &'static str = signals::REVERSE_INDEX_LAG;

    pub fn project(&self, ev: &EventEnvelope) -> Result<(), String> {
        self.lag.fetch_add(1, Ordering::SeqCst);
        let result = self.project_inner(ev);
        self.lag.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn project_inner(&self, ev: &EventEnvelope) -> Result<(), String> {
        if ev.actor.0.tenant != ev.tenant {
            return Err(format!(
                "identity.tuple.written actor tenant {:?} disagrees with envelope tenant {:?}",
                ev.actor.0.tenant, ev.tenant
            ));
        }
        let scope = TenantScope::from_verified_token(&ev.actor.0, ev.region.clone());

        let zookie = match ev.payload.get("zookie").and_then(|z| z.as_str()) {
            Some(z) if canonical_zookie(z) => Zookie(z.to_string()),
            Some(_) => {
                return Err(
                    "identity.tuple.written event carries a non-canonical zookie (expected `zk-` plus 20 decimal digits)"
                        .into(),
                )
            }
            None => {
                return Err("identity.tuple.written event carries no zookie (the S8 watermark)".into())
            }
        };

        let deltas = ev
            .payload
            .get("deltas")
            .and_then(|d| d.as_array())
            .ok_or_else(|| "identity.tuple.written event carries no deltas array".to_string())?;

        struct ValidatedDelta<'a> {
            op: &'a str,
            object: &'a str,
            relation: &'a str,
            subject: &'a str,
        }

        fn required_delta_field<'a>(
            delta: &'a serde_json::Value,
            index: usize,
            field: &str,
        ) -> Result<&'a str, String> {
            delta
                .as_object()
                .and_then(|object| object.get(field))
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    format!("identity.tuple.written delta {index} has no non-empty `{field}` string")
                })
        }

        let validated = deltas
            .iter()
            .enumerate()
            .map(|(index, delta)| {
                let op = required_delta_field(delta, index, "op")?;
                if !matches!(op, "add" | "remove") {
                    return Err(format!(
                        "identity.tuple.written delta {index} has unknown operation `{op}`"
                    ));
                }
                Ok(ValidatedDelta {
                    op,
                    object: required_delta_field(delta, index, "object")?,
                    relation: required_delta_field(delta, index, "relation")?,
                    subject: required_delta_field(delta, index, "subject")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        let mut applied_any = false;
        for delta in validated {
            if delta.subject.contains('#') {
                continue;
            }
            let object_type = ObjectType(type_of_object_id(delta.object));
            self.index.apply_delta(
                &scope,
                delta.op,
                &object_type,
                ReverseRow {
                    subject: PrincipalId(delta.subject.to_string()),
                    relation: RelName(delta.relation.to_string()),
                    object_id: ObjectId(delta.object.to_string()),
                },
                &zookie,
            );
            applied_any = true;
        }

        if !applied_any {
            self.index.advance_watermark_only(&scope, &zookie);
        }
        Ok(())
    }
}

impl EventHandler for ReverseIndexConsumer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        S8_SUBJECTS
    }

    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        if ev.type_.0 != IDENTITY_TUPLE_WRITTEN {
            return HandleOutcome::Done;
        }
        match self.project(ev) {
            Ok(()) => HandleOutcome::Done,
            Err(reason) => HandleOutcome::NonRetryable(myelin_events::Reason(reason)),
        }
    }
}

impl ReverseIndex {
    pub fn advance_watermark_only(&self, scope: &TenantScope, zookie: &Zookie) {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S8_TABLE));
        let mut inner = self.lock();
        let wm_key = (scope.tenant().0.clone(), scope.region().0.clone());
        let advance = inner
            .watermarks
            .get(&wm_key)
            .map(|cur| zookie.0 > cur.0)
            .unwrap_or(true);
        if advance {
            inner.watermarks.insert(wm_key, zookie.clone());
        }
    }
}

fn type_of_object_id(object_id: &str) -> String {
    object_id
        .split_once(':')
        .map(|(ty, _)| ty.to_string())
        .unwrap_or_else(|| object_id.to_string())
}

fn canonical_zookie(value: &str) -> bool {
    value
        .strip_prefix("zk-")
        .is_some_and(|revision| {
            revision.len() == 20
                && revision.bytes().all(|byte| byte.is_ascii_digit())
                && revision.parse::<u64>().is_ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tuple_store::TupleStore;
    use myelin_events::{BusTransport, InProcessBus, OutboxStore, Relay, Timestamp};
    use myelin_identity::{Principal, PrincipalKind, RelationTuple, TupleDelta};
    use myelin_tenancy::{Region, TenantId};

    fn scope(tenant: &str) -> TenantScope {
        TenantScope::from_verified_token(&actor_in(tenant), Region("eu-west".into()))
    }

    fn actor_in(tenant: &str) -> Principal {
        Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )
    }

    fn tuple(object: &str, relation: &str, subject: &str) -> RelationTuple {
        RelationTuple {
            object: ObjectId(object.into()),
            relation: RelName(relation.into()),
            subject: PrincipalId(subject.into()),
            caveat: None,
        }
    }

    fn now() -> Timestamp {
        Timestamp("2026-06-19T00:00:00Z".into())
    }

    fn feed_write(
        store: &TupleStore,
        outbox: &OutboxStore,
        consumer: &ReverseIndexConsumer,
        scope: &TenantScope,
        deltas: &[TupleDelta],
    ) -> Zookie {
        let z = store
            .write_tuples(
                scope,
                &actor_in(&scope.tenant().0),
                deltas,
                None,
                None,
                now(),
            )
            .expect("write");
        let bus = InProcessBus::new();
        let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
        relay.drain_to_empty();
        for env in bus.consume("") {
            consumer.handle(&env, &mut myelin_events::HandlerTx::none());
        }
        z
    }

    #[test]
    fn s8_ingests_identity_tuple_written_and_advances_watermark() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());
        let s = scope("acme");

        assert_eq!(index.watermark(&s), Zookie(String::new()));

        let z = feed_write(
            &store,
            &outbox,
            &consumer,
            &s,
            &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
        );

        assert_eq!(
            index.watermark(&s),
            z,
            "the S8 watermark advances on each identity.tuple.written"
        );
        assert_eq!(
            index.objects_for(
                &s,
                &ObjectType("repo".into()),
                &PrincipalId("p:alice".into()),
                &RelName("reader".into())
            ),
            vec![ObjectId("repo:core".into())],
            "S8 projects the direct grant into the reverse index"
        );
        assert_eq!(
            consumer.lag(),
            0,
            "reverse_index_lag returns to 0 after projection"
        );
    }

    #[test]
    fn watermark_advances_monotonically_and_never_regresses() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());
        let s = scope("acme");

        let z0 = feed_write(
            &store,
            &outbox,
            &consumer,
            &s,
            &[TupleDelta::Add(tuple("repo:a", "reader", "p:alice"))],
        );
        let z1 = feed_write(
            &store,
            &outbox,
            &consumer,
            &s,
            &[TupleDelta::Add(tuple("repo:b", "reader", "p:bob"))],
        );
        assert!(z1.0 > z0.0, "the second write's zookie is newer");
        assert_eq!(
            index.watermark(&s),
            z1,
            "the watermark is at the latest write"
        );

        index.advance_watermark_only(&s, &z0);
        assert_eq!(
            index.watermark(&s),
            z1,
            "an older redelivery never moves the watermark backward"
        );
    }

    #[test]
    fn remove_delta_tombstones_the_reverse_row() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());
        let s = scope("acme");
        feed_write(
            &store,
            &outbox,
            &consumer,
            &s,
            &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
        );
        assert_eq!(index.row_count(&s, &ObjectType("repo".into())), 1);
        feed_write(
            &store,
            &outbox,
            &consumer,
            &s,
            &[TupleDelta::Remove(tuple("repo:core", "reader", "p:alice"))],
        );
        assert!(
            index
                .objects_for(
                    &s,
                    &ObjectType("repo".into()),
                    &PrincipalId("p:alice".into()),
                    &RelName("reader".into())
                )
                .is_empty(),
            "a removed grant is gone from the reverse index"
        );
    }

    #[test]
    fn projection_apply_is_idempotent() {
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());
        let s = scope("acme");
        let row = ReverseRow {
            subject: PrincipalId("p:alice".into()),
            relation: RelName("reader".into()),
            object_id: ObjectId("repo:core".into()),
        };
        index.apply_delta(
            &s,
            "add",
            &ObjectType("repo".into()),
            row.clone(),
            &Zookie("zk-00000000000000000001".into()),
        );
        index.apply_delta(
            &s,
            "add",
            &ObjectType("repo".into()),
            row,
            &Zookie("zk-00000000000000000001".into()),
        );
        assert_eq!(
            index.row_count(&s, &ObjectType("repo".into())),
            1,
            "a re-add is idempotent (one row)"
        );
        let _ = consumer;
    }

    #[test]
    fn zero_cross_tenant_s8_rows() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());
        let acme = scope("acme");
        let globex = scope("globex");
        feed_write(
            &store,
            &outbox,
            &consumer,
            &acme,
            &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
        );

        assert_eq!(
            index.row_count(&globex, &ObjectType("repo".into())),
            0,
            "0 cross-tenant S8 rows"
        );
        assert!(
            index
                .objects_for(
                    &globex,
                    &ObjectType("repo".into()),
                    &PrincipalId("p:alice".into()),
                    &RelName("reader".into())
                )
                .is_empty(),
            "no cross-tenant reverse lookup path"
        );
        assert_eq!(index.row_count(&acme, &ObjectType("repo".into())), 1);
        assert_eq!(index.watermark(&globex), Zookie(String::new()));
    }

    #[test]
    fn s8_auto_registers_as_a_personal_data_holder() {
        let index = ReverseIndex::new();
        assert_eq!(
            index.holder().store,
            S8_HOLDER,
            "S8 registered under its holder name"
        );
        let receipt = index.holder().register();
        assert_eq!(receipt.store, S8_HOLDER);
    }

    #[test]
    fn userset_subject_delta_is_not_a_direct_row_but_advances_watermark() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());
        let s = scope("acme");
        let z = feed_write(
            &store,
            &outbox,
            &consumer,
            &s,
            &[TupleDelta::Add(tuple(
                "repo:core",
                "reader",
                "org:acme#member",
            ))],
        );
        assert_eq!(
            index.row_count(&s, &ObjectType("repo".into())),
            0,
            "a userset subject is not a direct row"
        );
        assert_eq!(
            index.watermark(&s),
            z,
            "the watermark advances even for a userset-only write"
        );
    }

    #[test]
    fn malformed_event_zookie_is_non_retryable_poison() {
        use myelin_events::{
            Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Visibility,
        };
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());
        let mut ev = EventEnvelope {
            event_id: EventId("e1".into()),
            type_: EventType(IDENTITY_TUPLE_WRITTEN.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(actor_in("acme")),
            subject: myelin_events::ArtifactRef("myelin://acme/identity/tuple/repo:core".into()),
            aggregate: AggregateKey("identity:tuple:acme:repo:core".into()),
            causation_id: None,
            correlation_id: CorrelationId("c1".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: now(),
            recorded_at: now(),
            payload: serde_json::json!({ "deltas": [] }),
        };
        let outcome = consumer.handle(&ev, &mut myelin_events::HandlerTx::none());
        assert!(
            matches!(outcome, HandleOutcome::NonRetryable(_)),
            "a zookie-less identity.tuple.written is a non-retryable poison, never a silent corruption"
        );
        assert_eq!(index.watermark(&scope("acme")), Zookie(String::new()));

        ev.event_id = EventId("e2".into());
        ev.payload = serde_json::json!({
            "zookie": "zzzz-poisons-lexical-order",
            "deltas": []
        });
        let outcome = consumer.handle(&ev, &mut myelin_events::HandlerTx::none());
        assert!(
            matches!(outcome, HandleOutcome::NonRetryable(_)),
            "a non-canonical zookie is poison rather than a trusted watermark"
        );
        assert_eq!(
            index.watermark(&scope("acme")),
            Zookie(String::new()),
            "invalid lexical watermark text cannot pin S8 ahead forever"
        );
    }

    #[test]
    fn malformed_delta_rejects_the_whole_event_without_advancing_watermark() {
        use myelin_events::{
            Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Visibility,
        };
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());
        let s = scope("acme");
        let last_good = Zookie("zk-00000000000000000001".into());
        index.apply_delta(
            &s,
            "add",
            &ObjectType("repo".into()),
            ReverseRow {
                subject: PrincipalId("p:alice".into()),
                relation: RelName("reader".into()),
                object_id: ObjectId("repo:core".into()),
            },
            &last_good,
        );

        let ev = EventEnvelope {
            event_id: EventId("e-malformed-delta".into()),
            type_: EventType(IDENTITY_TUPLE_WRITTEN.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(actor_in("acme")),
            subject: myelin_events::ArtifactRef("myelin://acme/identity/tuple/repo:core".into()),
            aggregate: AggregateKey("identity:tuple:acme:repo:core".into()),
            causation_id: None,
            correlation_id: CorrelationId("c-malformed-delta".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: now(),
            recorded_at: now(),
            payload: serde_json::json!({
                "zookie": "zk-00000000000000000002",
                "deltas": [
                    {
                        "op": "add",
                        "object": "repo:other",
                        "relation": "reader",
                        "subject": "p:bob"
                    },
                    {
                        "op": "delete",
                        "object": "repo:core",
                        "relation": "reader",
                        "subject": "p:alice"
                    }
                ]
            }),
        };

        let outcome = consumer.handle(&ev, &mut myelin_events::HandlerTx::none());
        assert!(
            matches!(outcome, HandleOutcome::NonRetryable(_)),
            "an unknown delta operation is a non-retryable producer poison"
        );
        assert_eq!(
            index.watermark(&s),
            last_good,
            "a rejected event cannot claim its newer revision was projected"
        );
        assert_eq!(
            index.objects_for(
                &s,
                &ObjectType("repo".into()),
                &PrincipalId("p:alice".into()),
                &RelName("reader".into())
            ),
            vec![ObjectId("repo:core".into())],
            "the last known-good projection remains intact"
        );
        assert!(
            index
                .objects_for(
                    &s,
                    &ObjectType("repo".into()),
                    &PrincipalId("p:bob".into()),
                    &RelName("reader".into())
                )
                .is_empty(),
            "validation finishes before the first delta mutates the projection"
        );
    }
}
