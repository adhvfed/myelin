#[cfg(any(test, feature = "test-support"))]
use myelin_events::MonotonicMinter;
use myelin_events::{
    derive_envelope, Actor, AggregateKey, ArtifactRef as EvArtifactRef, DataRole as EvDataRole,
    EmitContext, EventDraft, EventEnvelope, EventId, EventType, IdMinter, Timestamp, Visibility,
};
#[cfg(any(test, feature = "test-support"))]
use myelin_events::{EmitContextBase, OutboxStore, OutboxTransaction, OutboxTx};
use myelin_identity::iam_events::IDENTITY_TUPLE_WRITTEN;
use myelin_identity::{DataRole, Precondition, Principal, RelationTuple, TupleDelta, Zookie};
use myelin_storage::{OltpStoreHolder, TenantQuery, TenantScope, TenantTable};
use myelin_tenancy::{Region, TenantId};
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

pub const S3_TABLE: &str = "rebac_tuple";

pub const S3_HOLDER: &str = "identity_rebac_tuples";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    EmptyWrite,
    PreconditionFailed { expected: Zookie, actual: Zookie },
    CrossTenant { detail: String },
    CommitFailed(String),
}

impl core::fmt::Display for WriteError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WriteError::EmptyWrite => write!(f, "write_tuples requires at least one relationship delta"),
            WriteError::PreconditionFailed { expected, actual } => write!(
                f,
                "write_tuples precondition failed: expected object zookie {expected:?} but the \
                 store is at {actual:?} (the whole write aborted - read-modify-write is not lost)"
            ),
            WriteError::CrossTenant { detail } => write!(
                f,
                "write_tuples rejected a cross-tenant relationship write: {detail} (there is no cross-tenant \
                 tuple and no cross-tenant query path, identity §6)"
            ),
            WriteError::CommitFailed(why) => {
                write!(
                    f,
                    "write_tuples outbox co-commit failed (the write did NOT happen): {why}"
                )
            }
        }
    }
}

impl std::error::Error for WriteError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTuple {
    pub tenant: TenantId,
    pub region: Region,
    pub tuple: RelationTuple,
    pub zookie: Zookie,
    pub expires_at: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TupleSnapshot {
    pub(crate) tuples: Vec<StoredTuple>,
    pub(crate) zookie: Zookie,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TupleReadError(String);

impl core::fmt::Display for TupleReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "relationship tuple snapshot unavailable: {}", self.0)
    }
}

impl std::error::Error for TupleReadError {}

impl StoredTuple {
    pub fn partition_bucket(&self, buckets: u64) -> u64 {
        debug_assert!(buckets > 0, "partition bucket count must be non-zero");
        let mut h: u64 = 0xcbf29ce484222325;
        for b in self.tuple.object.0.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x100000001b3);
        }
        h % buckets
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TupleKey {
    object: String,
    relation: String,
    subject: String,
}

#[cfg(any(test, feature = "test-support"))]
impl TupleKey {
    fn of(t: &RelationTuple) -> TupleKey {
        TupleKey {
            object: t.object.0.clone(),
            relation: t.relation.0.clone(),
            subject: t.subject.0.clone(),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
struct Inner {
    partitions: HashMap<(String, String), HashMap<TupleKey, StoredTuple>>,
}

#[derive(Clone)]
pub struct TupleStore {
    backend: TupleBackend,
    revision: Arc<AtomicU64>,
    #[cfg(test)]
    read_failure: Option<TupleReadError>,
    #[cfg(any(test, feature = "test-support"))]
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
    holder: OltpStoreHolder,
}

#[derive(Clone)]
enum TupleBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<Inner>>),
    Pg(PgTupleBacking),
}

#[derive(Clone)]
struct PgTupleBacking {
    backing: Arc<myelin_storage::DurableTupleBacking>,
    rt: tokio::runtime::Handle,
}

impl TupleStore {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(outbox: OutboxStore) -> TupleStore {
        TupleStore::with_minter(outbox, Arc::new(MonotonicMinter::new()))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_minter(outbox: OutboxStore, minter: Arc<dyn IdMinter>) -> TupleStore {
        let holder = OltpStoreHolder::new(S3_HOLDER);
        let _receipt = holder.register();
        TupleStore {
            backend: TupleBackend::Memory(Arc::new(Mutex::new(Inner::default()))),
            revision: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            read_failure: None,
            outbox,
            minter,
            holder,
        }
    }

    pub fn with_pg(
        backing: myelin_storage::DurableTupleBacking,
        rt: tokio::runtime::Handle,
    ) -> TupleStore {
        TupleStore::with_pg_minter(Arc::new(myelin_events::UlidMinter::new()), backing, rt)
    }

    pub fn with_pg_minter(
        minter: Arc<dyn IdMinter>,
        backing: myelin_storage::DurableTupleBacking,
        rt: tokio::runtime::Handle,
    ) -> TupleStore {
        let holder = OltpStoreHolder::new(S3_HOLDER);
        let _receipt = holder.register();
        TupleStore {
            backend: TupleBackend::Pg(PgTupleBacking {
                backing: Arc::new(backing),
                rt,
            }),
            revision: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            read_failure: None,
            #[cfg(any(test, feature = "test-support"))]
            outbox: OutboxStore::new(),
            minter,
            holder,
        }
    }

    pub fn dek_class(&self, scope: &TenantScope) -> String {
        format!("kms://{}/tenant", scope.tenant().0)
    }

    pub fn holder(&self) -> &OltpStoreHolder {
        &self.holder
    }

    pub fn current_zookie(&self) -> Zookie {
        Self::zookie_of(self.revision.load(Ordering::SeqCst))
    }

    #[cfg(test)]
    pub(crate) fn with_unavailable_reads(mut self, detail: impl Into<String>) -> TupleStore {
        self.read_failure = Some(TupleReadError(detail.into()));
        self
    }

    fn zookie_of(rev: u64) -> Zookie {
        Zookie(format!("zk-{rev:020}"))
    }

    pub fn object_zookie(
        &self,
        scope: &TenantScope,
        object: &str,
    ) -> Result<Zookie, TupleReadError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S3_TABLE));
        let part_key = Self::part_key(scope);
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            TupleBackend::Memory(inner_arc) => {
                let inner = Self::mem_lock(inner_arc);
                Ok(inner
                    .partitions
                    .get(&part_key)
                    .and_then(|p| {
                        p.values()
                            .filter(|t| t.tuple.object.0 == object)
                            .max_by(|a, b| a.zookie.0.cmp(&b.zookie.0))
                            .map(|t| t.zookie.clone())
                    })
                    .unwrap_or_else(|| self.current_zookie()))
            }
            TupleBackend::Pg(pg) => {
                let revision = pg
                    .block(pg.backing.object_revision(&part_key.0, &part_key.1, object))
                    .map_err(|error| TupleReadError(error.to_string()))?;
                self.revision.fetch_max(revision, Ordering::SeqCst);
                Ok(Self::zookie_of(revision))
            }
        }
    }

    pub(crate) fn snapshot_in(&self, scope: &TenantScope) -> Result<TupleSnapshot, TupleReadError> {
        #[cfg(test)]
        if let Some(error) = &self.read_failure {
            return Err(error.clone());
        }

        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S3_TABLE));
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            TupleBackend::Memory(inner_arc) => {
                let inner = Self::mem_lock(inner_arc);
                let tuples = inner
                    .partitions
                    .get(&Self::part_key(scope))
                    .map(|p| p.values().cloned().collect())
                    .unwrap_or_default();
                let zookie = self.current_zookie();
                Ok(TupleSnapshot { tuples, zookie })
            }
            TupleBackend::Pg(pg) => {
                let tenant = scope.tenant().0.clone();
                let region = scope.region().0.clone();
                let snapshot = pg
                    .block(pg.backing.snapshot_in(&tenant, &region))
                    .map_err(|error| TupleReadError(error.to_string()))?;
                self.revision.fetch_max(snapshot.revision, Ordering::SeqCst);
                let tuples = snapshot
                    .edges
                    .into_iter()
                    .map(|edge| StoredTuple {
                        tenant: scope.tenant().clone(),
                        region: scope.region().clone(),
                        tuple: myelin_identity::RelationTuple {
                            object: myelin_identity::ObjectId(edge.object),
                            relation: myelin_identity::RelName(edge.relation),
                            subject: myelin_identity::PrincipalId(edge.subject),
                            caveat: None,
                        },
                        zookie: Self::zookie_of(edge.revision),
                        expires_at: None,
                    })
                    .collect();
                Ok(TupleSnapshot {
                    tuples,
                    zookie: Self::zookie_of(snapshot.revision),
                })
            }
        }
    }

    pub fn tuples_in(&self, scope: &TenantScope) -> Result<Vec<StoredTuple>, TupleReadError> {
        self.snapshot_in(scope).map(|snapshot| snapshot.tuples)
    }

    #[cfg_attr(not(any(test, feature = "test-support")), allow(unused_variables))]
    pub fn write_tuples(
        &self,
        scope: &TenantScope,
        actor: &Principal,
        deltas: &[TupleDelta],
        precondition: Option<&Precondition>,
        expires_at: Option<Timestamp>,
        occurred_at: Timestamp,
    ) -> Result<Zookie, WriteError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S3_TABLE));
        self.validate_write_scope(scope, actor, deltas)?;
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            TupleBackend::Memory(inner_arc) => self.write_tuples_memory(
                inner_arc,
                scope,
                actor,
                deltas,
                precondition,
                expires_at,
                &occurred_at,
            ),
            TupleBackend::Pg(pg) => {
                self.write_tuples_pg(pg, scope, actor, deltas, precondition, &occurred_at)
            }
        }
    }

    fn validate_write_scope(
        &self,
        scope: &TenantScope,
        actor: &Principal,
        deltas: &[TupleDelta],
    ) -> Result<(), WriteError> {
        if deltas.is_empty() {
            return Err(WriteError::EmptyWrite);
        }
        if &actor.tenant != scope.tenant() {
            return Err(WriteError::CrossTenant {
                detail: "the attributed actor is outside the verified tenant scope".into(),
            });
        }
        for delta in deltas {
            let tuple = match delta {
                TupleDelta::Add(tuple) | TupleDelta::Remove(tuple) => tuple,
            };
            self.reject_foreign_object(scope, &tuple.object.0)?;
            if let Some((userset_object, _)) = tuple.subject.0.rsplit_once('#') {
                self.reject_foreign_object(scope, userset_object)?;
            }
        }
        Ok(())
    }

    fn reject_foreign_object(&self, scope: &TenantScope, object: &str) -> Result<(), WriteError> {
        let tenant =
            myelin_refs::object_key(&EvArtifactRef(object.to_string())).and_then(|key| key.tenant);
        if tenant
            .as_deref()
            .is_some_and(|tenant| tenant != scope.tenant().0)
        {
            return Err(WriteError::CrossTenant {
                detail: "a relationship object names a different tenant".into(),
            });
        }
        Ok(())
    }

    fn derive_tuple_event(
        &self,
        scope: &TenantScope,
        actor: &Principal,
        deltas: &[TupleDelta],
        zookie: &Zookie,
        occurred_at: &Timestamp,
    ) -> (AggregateKey, EventEnvelope) {
        let draft = self.tuple_written_draft(scope, deltas, zookie);
        let aggregate = draft.aggregate.clone();
        let event_id: EventId = self.minter.mint().into();
        let ctx = EmitContext {
            event_id,
            tenant: scope.tenant().clone(),
            region: scope.region().clone(),
            actor: Actor(actor.clone()),
            schema_ver: 1,
            occurred_at: occurred_at.clone(),
            recorded_at: occurred_at.clone(),
            caused_by: None,
        };
        let envelope = derive_envelope(draft, ctx, None);
        (aggregate, envelope)
    }

    #[cfg(any(test, feature = "test-support"))]
    fn stage_event(
        &self,
        scope: &TenantScope,
        actor: &Principal,
        deltas: &[TupleDelta],
        zookie: &Zookie,
        occurred_at: &Timestamp,
    ) -> Result<OutboxTransaction, WriteError> {
        let ctx_base = EmitContextBase {
            tenant: scope.tenant().clone(),
            region: scope.region().clone(),
            actor: Actor(actor.clone()),
            schema_ver: 1,
            occurred_at: occurred_at.clone(),
            recorded_at: occurred_at.clone(),
            caused_by: None,
        };
        let mut tx = self.outbox.begin(Arc::clone(&self.minter), ctx_base);
        tx.stage_state_change(format!(
            "rebac: applied {} delta(s) → zookie {}",
            deltas.len(),
            zookie.0
        ));
        let draft = self.tuple_written_draft(scope, deltas, zookie);
        tx.emit(draft, None)
            .map_err(|e| WriteError::CommitFailed(e.0))?;
        Ok(tx)
    }

    #[cfg(any(test, feature = "test-support"))]
    #[allow(clippy::too_many_arguments)]
    fn write_tuples_memory(
        &self,
        inner_arc: &Arc<Mutex<Inner>>,
        scope: &TenantScope,
        actor: &Principal,
        deltas: &[TupleDelta],
        precondition: Option<&Precondition>,
        expires_at: Option<Timestamp>,
        occurred_at: &Timestamp,
    ) -> Result<Zookie, WriteError> {
        let part_key = Self::part_key(scope);
        let mut inner = Self::mem_lock(inner_arc);

        if let Some(pre) = precondition {
            if let Some(expected) = &pre.expected_zookie {
                let actual = Self::object_zookie_locked(&inner, &part_key, deltas)
                    .unwrap_or_else(|| Self::zookie_of(self.revision.load(Ordering::SeqCst)));
                if &actual != expected {
                    return Err(WriteError::PreconditionFailed {
                        expected: expected.clone(),
                        actual,
                    });
                }
            }
        }

        let new_rev = self.revision.fetch_add(1, Ordering::SeqCst) + 1;
        let zookie = Self::zookie_of(new_rev);
        let tx = self.stage_event(scope, actor, deltas, &zookie, occurred_at)?;

        let partition = inner.partitions.entry(part_key).or_default();
        for delta in deltas {
            match delta {
                TupleDelta::Add(t) => {
                    partition.insert(
                        TupleKey::of(t),
                        StoredTuple {
                            tenant: scope.tenant().clone(),
                            region: scope.region().clone(),
                            tuple: t.clone(),
                            zookie: zookie.clone(),
                            expires_at: expires_at.clone(),
                        },
                    );
                }
                TupleDelta::Remove(t) => {
                    partition.remove(&TupleKey::of(t));
                }
            }
        }

        tx.commit().map_err(|e| WriteError::CommitFailed(e.0))?;
        Ok(zookie)
    }

    fn write_tuples_pg(
        &self,
        pg: &PgTupleBacking,
        scope: &TenantScope,
        actor: &Principal,
        deltas: &[TupleDelta],
        precondition: Option<&Precondition>,
        occurred_at: &Timestamp,
    ) -> Result<Zookie, WriteError> {
        let tenant = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let expected = precondition.and_then(|value| value.expected_zookie.clone());

        let edge_deltas: Vec<(myelin_storage::TupleEdgeOp, String, String, String)> = deltas
            .iter()
            .map(|d| match d {
                TupleDelta::Add(t) => (
                    myelin_storage::TupleEdgeOp::Add,
                    t.object.0.clone(),
                    t.relation.0.clone(),
                    t.subject.0.clone(),
                ),
                TupleDelta::Remove(t) => (
                    myelin_storage::TupleEdgeOp::Remove,
                    t.object.0.clone(),
                    t.relation.0.clone(),
                    t.subject.0.clone(),
                ),
            })
            .collect();
        let event_store = self.clone();
        let event_scope = scope.clone();
        let event_actor = actor.clone();
        let event_deltas = deltas.to_vec();
        let event_time = occurred_at.clone();
        let outcome = pg
            .block(pg.backing.apply_deltas_co_commit(
                &tenant,
                &region,
                edge_deltas,
                expected.as_ref().map(|zookie| zookie.0.clone()),
                move |revision| {
                    let (aggregate, envelope) = event_store.derive_tuple_event(
                        &event_scope,
                        &event_actor,
                        &event_deltas,
                        &Self::zookie_of(revision),
                        &event_time,
                    );
                    (aggregate.0, envelope)
                },
            ))
            .map_err(|error| WriteError::CommitFailed(error.to_string()))?;

        match outcome {
            myelin_storage::DurableTupleWriteOutcome::Committed { revision } => {
                self.revision.fetch_max(revision, Ordering::SeqCst);
                Ok(Self::zookie_of(revision))
            }
            myelin_storage::DurableTupleWriteOutcome::PreconditionFailed { actual_revision } => {
                Err(WriteError::PreconditionFailed {
                    expected: expected
                        .expect("the durable store only compares a supplied revision"),
                    actual: Self::zookie_of(actual_revision),
                })
            }
        }
    }

    fn tuple_written_draft(
        &self,
        scope: &TenantScope,
        deltas: &[TupleDelta],
        zookie: &Zookie,
    ) -> EventDraft {
        let object = deltas
            .iter()
            .map(|d| match d {
                TupleDelta::Add(t) | TupleDelta::Remove(t) => t.tuple_object(),
            })
            .next()
            .unwrap_or("unknown");
        let subject = EvArtifactRef(format!(
            "myelin://{}/identity/tuple/{}",
            scope.tenant().0,
            object
        ));
        let aggregate = AggregateKey(format!("identity:tuple:{}:{}", scope.tenant().0, object));
        let ops: Vec<serde_json::Value> = deltas
            .iter()
            .map(|d| match d {
                TupleDelta::Add(t) => serde_json::json!({
                    "op": "add",
                    "object": t.object.0,
                    "relation": t.relation.0,
                    "subject": t.subject.0,
                }),
                TupleDelta::Remove(t) => serde_json::json!({
                    "op": "remove",
                    "object": t.object.0,
                    "relation": t.relation.0,
                    "subject": t.subject.0,
                }),
            })
            .collect();
        EventDraft {
            type_: EventType(IDENTITY_TUPLE_WRITTEN.to_string()),
            subject,
            aggregate,
            payload: serde_json::json!({
                "zookie": zookie.0,
                "deltas": ops,
            }),
            data_role: EvDataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }

    fn part_key(scope: &TenantScope) -> (String, String) {
        (scope.tenant().0.clone(), scope.region().0.clone())
    }

    #[cfg(any(test, feature = "test-support"))]
    fn object_zookie_locked(
        inner: &Inner,
        part_key: &(String, String),
        deltas: &[TupleDelta],
    ) -> Option<Zookie> {
        let partition = inner.partitions.get(part_key)?;
        let objects: Vec<&str> = deltas
            .iter()
            .map(|d| match d {
                TupleDelta::Add(t) | TupleDelta::Remove(t) => t.tuple_object(),
            })
            .collect();
        partition
            .values()
            .filter(|t| objects.contains(&t.tuple.object.0.as_str()))
            .max_by(|a, b| a.zookie.0.cmp(&b.zookie.0))
            .map(|t| t.zookie.clone())
    }

    #[cfg(any(test, feature = "test-support"))]
    fn mem_lock(arc: &Arc<Mutex<Inner>>) -> std::sync::MutexGuard<'_, Inner> {
        arc.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl PgTupleBacking {
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

trait TupleDeltaObject {
    fn tuple_object(&self) -> &str;
}

impl TupleDeltaObject for RelationTuple {
    fn tuple_object(&self) -> &str {
        &self.object.0
    }
}

pub fn run_grant_expiry(run_deadline: impl Into<String>) -> Timestamp {
    Timestamp(run_deadline.into())
}

pub fn data_role_to_events(role: DataRole) -> EvDataRole {
    match role {
        DataRole::Controller => EvDataRole::Controller,
        DataRole::Processor => EvDataRole::Processor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{BusTransport, ConsumerName, InProcessBus, Relay};
    use myelin_identity::{ObjectId, PrincipalId, PrincipalKind, RelName};

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn actor() -> Principal {
        Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
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

    #[test]
    fn write_tuples_is_atomic_and_returns_monotonic_zookie() {
        let store = TupleStore::new(OutboxStore::new());
        let s = scope("acme");

        let z0 = store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
                None,
                None,
                now(),
            )
            .expect("first write");
        let z1 = store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Add(tuple("repo:core", "writer", "p:bob"))],
                None,
                None,
                now(),
            )
            .expect("second write");

        assert!(
            z1.0 > z0.0,
            "the zookie advances monotonically: {z1:?} must sort after {z0:?}"
        );
        let tuples = store.tuples_in(&s).expect("read both stored tuples");
        assert_eq!(tuples.len(), 2, "both adds are durable");
        assert_eq!(
            store
                .object_zookie(&s, "repo:core")
                .expect("read the object revision"),
            z1
        );
    }

    #[test]
    fn an_empty_relationship_write_is_not_a_revision_or_event() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let result = store.write_tuples(&scope("acme"), &actor(), &[], None, None, now());

        assert_eq!(result, Err(WriteError::EmptyWrite));
        assert_eq!(
            store.current_zookie(),
            Zookie("zk-00000000000000000000".into())
        );
        assert_eq!(outbox.outbox_depth(), 0);
    }

    #[test]
    fn failed_precondition_aborts_the_whole_write_and_emits_nothing() {
        let store = TupleStore::new(OutboxStore::new());
        let s = scope("acme");
        let z0 = store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
                None,
                None,
                now(),
            )
            .expect("seed");
        let depth_before = store.outbox.outbox_depth();

        let stale = Zookie("zk-00000000000000000000".into());
        let err = store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Add(tuple("repo:core", "writer", "p:bob"))],
                Some(&Precondition {
                    expected_zookie: Some(stale.clone()),
                }),
                None,
                now(),
            )
            .expect_err("a stale precondition must abort the write");
        match err {
            WriteError::PreconditionFailed { expected, actual } => {
                assert_eq!(expected, stale);
                assert_eq!(
                    actual, z0,
                    "the actual zookie is the object's current revision"
                );
            }
            other => panic!("expected PreconditionFailed, got {other:?}"),
        }
        assert_eq!(
            store
                .tuples_in(&s)
                .expect("read tuples after the rejected write")
                .len(),
            1,
            "the aborted write added no tuple"
        );
        assert_eq!(
            store.outbox.outbox_depth(),
            depth_before,
            "a failed precondition emits NOTHING (emit-iff-committed)"
        );
    }

    #[test]
    fn matching_precondition_proceeds() {
        let store = TupleStore::new(OutboxStore::new());
        let s = scope("acme");
        let z0 = store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
                None,
                None,
                now(),
            )
            .expect("seed");
        let z1 = store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Add(tuple("repo:core", "writer", "p:bob"))],
                Some(&Precondition {
                    expected_zookie: Some(z0.clone()),
                }),
                None,
                now(),
            )
            .expect("a matching precondition proceeds");
        assert!(z1.0 > z0.0);
        assert_eq!(
            store
                .tuples_in(&s)
                .expect("read tuples after the matching write")
                .len(),
            2
        );
    }

    #[test]
    fn committed_write_emits_identity_tuple_written_via_the_outbox_only() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let s = scope("acme");

        let z = store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
                None,
                None,
                now(),
            )
            .expect("write");

        assert_eq!(
            outbox.outbox_depth(),
            1,
            "the committed write emitted exactly one event"
        );
        let bus = InProcessBus::new();
        let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
        relay.drain_to_empty();
        let published = bus.consume("");
        assert_eq!(
            published.len(),
            1,
            "the relay published exactly the one event (no ghost)"
        );
        let env = &published[0];
        assert_eq!(
            env.type_.0, IDENTITY_TUPLE_WRITTEN,
            "the only emit is identity.tuple.written"
        );
        assert!(
            !env.contains_personal_data,
            "the identity.* event carries no inline PII"
        );
        assert_eq!(
            env.payload["zookie"],
            serde_json::json!(z.0),
            "the event carries the S8 watermark"
        );
        assert_eq!(env.actor.0.principal_id, PrincipalId("p-admin".into()));
        assert_eq!(outbox.outbox_depth(), 0, "the relay drained the outbox");
    }

    #[test]
    fn emit_count_equals_committed_write_count() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let s = scope("acme");
        for (i, obj) in ["a", "b", "c"].iter().enumerate() {
            store
                .write_tuples(
                    &s,
                    &actor(),
                    &[TupleDelta::Add(tuple(obj, "reader", &format!("p:{i}")))],
                    None,
                    None,
                    now(),
                )
                .expect("committed write");
        }
        let _ = store.write_tuples(
            &s,
            &actor(),
            &[TupleDelta::Add(tuple("a", "writer", "p:x"))],
            Some(&Precondition {
                expected_zookie: Some(Zookie("zk-nope".into())),
            }),
            None,
            now(),
        );
        assert_eq!(
            outbox.committed_count(),
            3,
            "exactly 3 events for 3 committed writes - 0 emits without a committed write"
        );
    }

    #[test]
    fn per_run_grant_is_an_auto_expiring_tuple() {
        let store = TupleStore::new(OutboxStore::new());
        let s = scope("acme");
        let deadline = run_grant_expiry("2026-06-19T01:00:00Z");
        store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Add(tuple("run:R1", "runner", "agent:A1"))],
                None,
                Some(deadline.clone()),
                now(),
            )
            .expect("per-run grant write");
        let grant = store
            .tuples_in(&s)
            .expect("read the per-run grant")
            .into_iter()
            .find(|t| t.tuple.object.0 == "run:R1")
            .expect("the grant is stored");
        assert_eq!(
            grant.expires_at,
            Some(deadline),
            "a per-run grant auto-expires (== run life)"
        );
        store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
                None,
                None,
                now(),
            )
            .expect("durable grant");
        let durable = store
            .tuples_in(&s)
            .expect("read the durable grant")
            .into_iter()
            .find(|t| t.tuple.object.0 == "repo:core")
            .expect("the durable grant is stored");
        assert_eq!(
            durable.expires_at, None,
            "an ordinary grant is durable (no expiry)"
        );
    }

    #[test]
    fn no_cross_tenant_tuple_or_query_path() {
        let store = TupleStore::new(OutboxStore::new());
        let acme = scope("acme");
        let globex = scope("globex");

        store
            .write_tuples(
                &acme,
                &actor(),
                &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
                None,
                None,
                now(),
            )
            .expect("acme write");

        assert!(
            store
                .tuples_in(&globex)
                .expect("read the other tenant's partition")
                .is_empty(),
            "no cross-tenant read path"
        );
        assert_eq!(
            store
                .tuples_in(&acme)
                .expect("read the owning tenant's partition")
                .len(),
            1
        );
    }

    #[test]
    fn a_relationship_write_cannot_attribute_or_smuggle_another_tenant() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let acme = scope("acme");
        let globex = scope("globex");

        let attempts = [
            store.write_tuples(
                &globex,
                &actor(),
                &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
                None,
                None,
                now(),
            ),
            store.write_tuples(
                &acme,
                &actor(),
                &[TupleDelta::Add(tuple(
                    "myelin://globex/git/repo/core",
                    "reader",
                    "p:alice",
                ))],
                None,
                None,
                now(),
            ),
            store.write_tuples(
                &acme,
                &actor(),
                &[TupleDelta::Add(tuple(
                    "repo:core",
                    "reader",
                    "myelin://globex/identity/team/reviewers#member",
                ))],
                None,
                None,
                now(),
            ),
        ];

        assert!(
            attempts
                .iter()
                .all(|result| matches!(result, Err(WriteError::CrossTenant { .. }))),
            "actor attribution, relationship objects, and usersets all stay in the verified tenant"
        );
        assert!(
            store
                .tuples_in(&acme)
                .expect("inspect the rejected write partition")
                .is_empty(),
            "no rejected relationship is stored"
        );
        assert_eq!(
            outbox.outbox_depth(),
            0,
            "a rejected relationship emits no poisoned projection event"
        );
    }

    #[test]
    fn s3_store_registers_as_a_personal_data_holder() {
        let store = TupleStore::new(OutboxStore::new());
        assert_eq!(
            store.holder().store,
            S3_HOLDER,
            "the S3 store registered under its holder name"
        );
        let receipt = store.holder().register();
        assert_eq!(receipt.store, S3_HOLDER);
    }

    #[test]
    fn remove_delta_deletes_the_edge_and_add_is_idempotent() {
        let store = TupleStore::new(OutboxStore::new());
        let s = scope("acme");
        store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
                None,
                None,
                now(),
            )
            .unwrap();
        store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
                None,
                None,
                now(),
            )
            .unwrap();
        assert_eq!(
            store
                .tuples_in(&s)
                .expect("read the idempotently added edge")
                .len(),
            1,
            "re-adding the same edge is idempotent"
        );
        store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Remove(tuple("repo:core", "reader", "p:alice"))],
                None,
                None,
                now(),
            )
            .unwrap();
        assert!(
            store
                .tuples_in(&s)
                .expect("read tuples after removing the edge")
                .is_empty(),
            "the remove deleted the edge"
        );
    }

    #[test]
    fn object_id_hash_partition_is_stable() {
        let t = StoredTuple {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            tuple: tuple("repo:core", "reader", "p:alice"),
            zookie: Zookie("zk-1".into()),
            expires_at: None,
        };
        let b1 = t.partition_bucket(256);
        let b2 = t.partition_bucket(256);
        assert_eq!(b1, b2, "the object-id-hash partition is deterministic");
        assert!(b1 < 256, "the bucket is within the partition count");
    }

    #[test]
    fn per_tenant_dek_class_is_pinned_by_reference() {
        let store = TupleStore::new(OutboxStore::new());
        let s = scope("acme");
        assert_eq!(
            store.dek_class(&s),
            "kms://acme/tenant",
            "the store pins the per-tenant DEK class"
        );
    }

    #[test]
    fn cdc_write_tuples_role_compile_caller() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let s = scope("acme");
        let zookie = store
            .write_tuples(
                &s,
                &actor(),
                &[TupleDelta::Add(tuple("org:acme", "member", "p:alice"))],
                None,
                None,
                now(),
            )
            .expect("the role-compile caller writes the grant");
        assert_eq!(
            store
                .object_zookie(&s, "org:acme")
                .expect("read the object revision"),
            zookie
        );
        let row = outbox
            .row(&{
                let inner_count = outbox.committed_count();
                assert_eq!(inner_count, 1);
                let bus = InProcessBus::new();
                let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
                relay.drain_to_empty();
                let published = bus.consume("");
                published[0].event_id.clone()
            })
            .expect("the identity.tuple.written row exists for S8");
        assert_eq!(row.envelope.type_.0, IDENTITY_TUPLE_WRITTEN);
        let _ = ConsumerName("s8_reverse_index".into());
    }
}
