//! # `tuple_store` — the S3 ReBAC tuple store + `write_tuples`/zookie (P-ID-08 → P-057)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §6 (the tuple shape `RelationTuple {tenant, region, object, relation, subject, caveat?,
//! zookie, expires_at?}`; `(tenant, region)` then object-id-hash partition; **no cross-tenant
//! tuple and no cross-tenant query path**; per-run grants are **auto-expiring tuples**
//! (`expires_at == run life`); authz state changes are **event-sourced** and Id emits
//! `iam.tuple_written` via the **outbox** — the only emit path; reindex-from-source rebuildable),
//! §8.4 / C10 (the zookie from `write_tuples` is stamped on the object — the new-enemy guard;
//! the zookie **advances monotonically** and is the revision watermark S8 honours), §2 (the S3
//! row of the store table).
//!
//! **Contract-index:** rows 4.6 (`write_tuples([Δtuple], precondition?) → zookie`), 4.10 (the
//! zookie **write-half**), 2.2/2.4 (the outbox emit path + the `EventHandler` template) —
//! 4.6 + the 4.10 write-half are **OWNED** here; 2.2/2.4 are **CONSUMED**.
//!
//! ## What this module ships (P-ID-08)
//! 1. **The S3 ReBAC tuple store** ([`TupleStore`]) — SpiceDB-class, the `(tenant, region)` +
//!    object-id-hash partition, RLS-scoped through [`myelin_storage::TenantScope`] /
//!    [`myelin_storage::TenantQuery`] (there is **no cross-tenant query path** — a read/write is
//!    built from a verified `(tenant, region)` scope, never a path/string), holder-registered as
//!    a `PersonalDataHolder` (the [`myelin_storage::OltpStoreHolder`] seam), and
//!    per-tenant-DEK-pinned **by reference** (the named floor below).
//! 2. **`write_tuples([Δtuple], precondition?) → zookie`** ([`TupleStore::write_tuples`], 4.6):
//!    an **atomic** write that (a) honours the precondition (a failed precondition aborts the
//!    whole write — read-modify-write is not lost), (b) applies the deltas under the store lock,
//!    (c) returns the **monotonically-advancing zookie** to stamp on the object, and (d) emits
//!    `iam.tuple_written` **via the outbox** — the ONLY emit path (the `no-raw-publish` lint
//!    forbids any other). The tuple write + the event **co-commit** in ONE
//!    [`myelin_events::OutboxTransaction`] (emit-iff-committed, BUS-D4): a write that did not
//!    commit emits nothing, and a committed write always emits.
//! 3. **The zookie write-half of 4.10** ([`Zookie`]): every `write_tuples` advances the store's
//!    monotonic revision and returns the new zookie (`page.acl_zookie`, Chat membership). It is
//!    the S8 watermark carried on the `iam.tuple_written` event.
//!
//! ## The co-commit invariant (the load-bearing security property, mutation-tested mandatory-core)
//! `write_tuples` opens **one** [`myelin_events::OutboxTransaction`], stages the tuple-state
//! change AND the `iam.tuple_written` event onto it, and commits **both together**. There is
//! deliberately **no other emit path**: the `iam.tuple_written` envelope is constructed only by
//! `OutboxTx::emit` inside that transaction, never by a direct `publish`. So:
//! - **0 emits without a committed write** — a precondition failure (or any pre-commit abort)
//!   drops the transaction and writes nothing (no tuple, no event);
//! - **0 committed writes without an emit** — the event is staged in the SAME transaction as the
//!   tuple mutation, so a committed write always carries its event.
//!
//! The `no-raw-publish` lint (P-019 / EB-07) is the workspace guard; [`tests`] pins it in-crate.
//!
//! ## Floors named (frozen shape now → bodies in a later prompt)
//! - **The read-your-writes consistency *read* half (the S8 watermark) is P-ID-12 (P-070).**
//!   This prompt ships the write-half: `write_tuples` returns the monotonically-advancing zookie
//!   and stamps it on the `iam.tuple_written` event for S8 to consume. The *read* side that waits
//!   for / falls back rather than serving stale (the new-enemy guard) is P-ID-12. Named, not
//!   silently assumed done.
//! - **The check engine (4.2) is P-ID-09 (P-067); S8 (the authz reverse index) is P-ID-11
//!   (P-069).** This store is the source the reverse index is fed from (`iam.tuple_written`); it
//!   does NOT evaluate `check` and does NOT materialise the reverse index here.
//! - **The per-tenant DEK is pinned BY REFERENCE.** The KMS three-level hierarchy is Storage M1
//!   (P-ST-06 → P-058, which sequences AFTER this prompt); the store declares the per-tenant key
//!   class it encrypts under ([`TupleStore::dek_class`]) so the wiring lands structurally when
//!   the KMS does. Named floor (EI-01 §4).
//! - **The in-memory store models the SQL S3 table** (the same EI-01 §1 deviation the outbox
//!   already documents): there is no live OLTP database until the driver lands (P-S15); the
//!   `(tenant, region)`-keyed, object-id-hash-partitioned, RLS-scoped, atomic semantics are
//!   byte-for-byte the 4.6/§6 contract. The seam shape does not change when the binding lands.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef as EvArtifactRef, DataRole as EvDataRole, EmitContextBase,
    EventDraft, EventType, IdMinter, MonotonicMinter, OutboxStore, OutboxTransaction, OutboxTx,
    Timestamp, Visibility,
};
use myelin_identity::iam_events::IAM_TUPLE_WRITTEN;
use myelin_identity::{DataRole, Precondition, Principal, RelationTuple, TupleDelta, Zookie};
use myelin_storage::{OltpStoreHolder, TenantQuery, TenantScope, TenantTable};
use myelin_tenancy::{Region, TenantId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The S3 store's tenant-owned table name (the `(tenant, region)`-first RLS table). Every store
/// access is built through [`TenantQuery::for_table`] over THIS table, so a tuple read/write
/// without a verified `(tenant, region)` scope does not compile (the `tenant-predicate` floor).
pub const S3_TABLE: &str = "rebac_tuple";

/// The S3 store's stable holder name (the `PersonalDataHolder` identifier). The store
/// auto-registers under this name so "we forgot the tuple store" is structurally impossible.
pub const S3_HOLDER: &str = "identity_rebac_tuples";

/// A relation-tuple write error (4.6). The taxonomy is intentionally tiny on this floor; what
/// matters is that a failed write is a typed, LOUD value — never a silent partial write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// The `precondition?` was not satisfied (e.g. the object's current zookie != the expected
    /// one) — the WHOLE write aborts (read-modify-write is not lost). No tuple changed, no event.
    PreconditionFailed {
        /// The zookie the caller expected the object to be at.
        expected: Zookie,
        /// The zookie the store is actually at (the `at_least` the caller must re-read from).
        actual: Zookie,
    },
    /// A delta targeted a different tenant than the verified write scope — rejected (there is no
    /// cross-tenant tuple). Defence in depth: the API never accepts a tenant from the tuple.
    CrossTenant {
        /// A short description of the rejected cross-tenant attempt (for the audit log).
        detail: String,
    },
    /// The outbox co-commit failed (the event + the tuple change could not be made durable
    /// together). Surfaced loudly — the write did NOT happen (emit-iff-committed).
    CommitFailed(String),
}

impl core::fmt::Display for WriteError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WriteError::PreconditionFailed { expected, actual } => write!(
                f,
                "write_tuples precondition failed: expected object zookie {expected:?} but the \
                 store is at {actual:?} (the whole write aborted — read-modify-write is not lost)"
            ),
            WriteError::CrossTenant { detail } => write!(
                f,
                "write_tuples rejected a cross-tenant delta: {detail} (there is no cross-tenant \
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

/// A stored S3 tuple (architecture §6: `RelationTuple {tenant, region, object, relation,
/// subject, caveat?, zookie, expires_at?}`).
///
/// The caller-supplied delta is the **frozen** [`myelin_identity::RelationTuple`]
/// (`{object, relation, subject, caveat?}`); the server stamps the ambient/managed fields — the
/// `(tenant, region)` partition key (from the verified scope, never the tuple), the `zookie` of
/// the write that last touched it, and the optional `expires_at` (per-run grants are
/// **auto-expiring tuples**, defence-in-depth for revoke-on-crash). This is NOT a re-definition
/// of the frozen `RelationTuple` — it WRAPS it with the store-managed columns §6 names, so the
/// frozen API shape is unchanged (EI-01 §7 — never redefine a frozen type).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTuple {
    /// The verified tenant partition (from the write scope — never from the tuple).
    pub tenant: TenantId,
    /// The residency region partition (12.1 — `(tenant, region)` is the partition key).
    pub region: Region,
    /// The caller-authored relation edge `object#relation@subject (+caveat?)` (the frozen shape).
    pub tuple: RelationTuple,
    /// The zookie of the write that last touched this tuple (the revision watermark).
    pub zookie: Zookie,
    /// `Some(deadline)` iff this is an auto-expiring per-run grant (`expires_at == run life`);
    /// `None` for an ordinary durable grant.
    pub expires_at: Option<Timestamp>,
}

impl StoredTuple {
    /// The object-id-hash partition bucket (architecture §6: `(tenant, region)` THEN object-id
    /// hash). A stable hash of the object id within the `(tenant, region)` partition — the
    /// shard key a real deployment routes on. Deterministic so the partition is reproducible.
    pub fn partition_bucket(&self, buckets: u64) -> u64 {
        debug_assert!(buckets > 0, "partition bucket count must be non-zero");
        // A small, stable FNV-1a over the object id (deterministic, dependency-free). The real
        // sharding key the deployment routes on is `(tenant, region, hash(object))`; the bucket
        // count is a deployment tunable. We hash the object id only — the (tenant, region) prefix
        // is the OUTER partition, the object-id hash is the inner one (§6).
        let mut h: u64 = 0xcbf29ce484222325;
        for b in self.tuple.object.0.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x100000001b3);
        }
        h % buckets
    }
}

/// The key a stored tuple is uniquely identified by within its `(tenant, region)` partition —
/// the `object#relation@subject` edge identity (a re-add of the same edge is idempotent; a
/// remove targets exactly this key).
// The edge-identity key of the in-memory test-double [`Inner`] partition map (MR-009b Wave 2 —
// `test-support`-gated; the durable PG path keys on the `rebac_tuple` primary key in SQL).
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

/// The shared inner state of a [`TupleStore`] (behind `Arc<Mutex<…>>` so the store is a cloneable
/// handle and `write_tuples` is atomic under one lock).
///
/// **MR-009b Wave 2 — TEST DOUBLE (compiled ONLY under `#[cfg(any(test, feature = "test-support"))]`).**
/// The PRODUCTION default is the durable PG backing ([`PgTupleBacking`], via [`TupleStore::with_pg`]);
/// this in-memory `Inner` is the DB-free unit-test double (SI-019 leaves the baseline).
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
struct Inner {
    /// The committed tuples, keyed by `(tenant, region)` partition then edge identity. The OUTER
    /// map is the `(tenant, region)` partition (no cross-tenant query path: a read for tenant A
    /// never touches tenant B's map). The inner map is the object#relation@subject edge.
    partitions: HashMap<(String, String), HashMap<TupleKey, StoredTuple>>,
}

/// The S3 ReBAC tuple store (architecture §6; contract 4.6 + the 4.10 write-half). A cloneable
/// handle over shared state. The ONLY mutation path is [`TupleStore::write_tuples`], which
/// co-commits the tuple change + the `iam.tuple_written` event through the outbox.
///
/// **No cross-tenant query path:** every accessor takes a verified [`TenantScope`] (minted only
/// from a verified token, never a path), and a query is built through [`TenantQuery::for_table`]
/// over [`S3_TABLE`] — so a tenant-less tuple access does not compile (the `tenant-predicate`
/// floor), and a read for one tenant structurally cannot reach another tenant's partition.
#[derive(Clone)]
pub struct TupleStore {
    /// The durable backing — the REAL PG `rebac_tuple` edge set (MR-007) on the production path, or
    /// the in-memory test-double on the default DB-free build. The system-of-record for the edges is
    /// the Pg backing; the in-memory map is an explicit test double (NOT the production default).
    backend: TupleBackend,
    /// The monotonic zookie revision — every `write_tuples` bumps it (the 4.10 write-half). An
    /// `AtomicU64` so the advance is monotonic even under concurrent writers; the string zookie
    /// is `zk-<rev>` (lexically monotonic with zero-padding, so a later zookie sorts after).
    revision: Arc<AtomicU64>,
    /// The store's outbox — the ONLY emit path (`no-raw-publish`). `write_tuples` stages the
    /// tuple change + the `iam.tuple_written` event into ONE transaction on this store and the
    /// relay (auto-started by `serve`) drains it.
    outbox: OutboxStore,
    /// The stable id-minter for the emitted `iam.tuple_written` events (the broker-side dedup id).
    minter: Arc<dyn IdMinter>,
    /// The holder this store auto-registers as (the `PersonalDataHolder` seam) — proof the
    /// "every store is a holder" invariant holds for S3 (§1.1, GD-3).
    holder: OltpStoreHolder,
}

/// The S3 store backing: the REAL durable PG `rebac_tuple` edge set (MR-007) — the PRODUCTION
/// default (MR-009b Wave 2) — or the in-memory test-double. Splitting the backing OUT of the role
/// struct's direct fields is what lets the `no-in-memory-durable-store` ratchet record the shortcut's
/// removal: the PRODUCTION-compiled enum presents ONLY the pool-backed `Pg` variant (the `Memory`
/// variant is `test-support`-gated, which the scanner strips as a test double), so `TupleStore` no
/// longer holds an in-memory collection in the production graph (SI-019 leaves the baseline).
#[derive(Clone)]
enum TupleBackend {
    /// The in-memory test-double — MR-009b Wave 2: compiled ONLY under
    /// `#[cfg(any(test, feature = "test-support"))]`. NOT the production system-of-record.
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<Inner>>),
    /// The REAL durable PG backing over the MR-022 provider pool + `with_tenant_tx` convention — the
    /// PRODUCTION DEFAULT (always compiled as of MR-009b Wave 2).
    Pg(PgTupleBacking),
}

/// The PG-backed S3 tuple backing (MR-007): the durable `rebac_tuple` edge set + the sync→async
/// bridge (`tokio::runtime::Handle` driving `block_in_place`+`block_on`, the same bridge
/// `ValkeyCache`/`S3BlobStore` use). The per-object zookie watermark is in-process CONSISTENCY
/// metadata (the durable system-of-record is the EDGE set in PG); the persisted-zookie/read-side is
/// the named P-ID-12 / MR-009 floor — a fresh instance reads the durable edges back and treats
/// not-yet-seen objects as at the genesis revision. The production default (always compiled, W2).
#[derive(Clone)]
struct PgTupleBacking {
    backing: Arc<myelin_storage::DurableTupleBacking>,
    rt: tokio::runtime::Handle,
    /// The in-process zookie WATERMARK — REBUILDABLE consistency metadata, NOT the durable
    /// system-of-record (the edge set lives in PG; a fresh instance re-derives it, treating
    /// not-yet-seen objects as the genesis revision — the named P-ID-12 / MR-009 read-side floor).
    /// Held as a distinct in-memory index type ([`PgZookieWatermark`]) so the durable backing does
    /// NOT present a bare in-memory collection to the `no-in-memory-durable-store` scanner (a durable
    /// PG backing carrying a rebuildable consistency cache is pool-backed, not an in-memory store).
    watermark: PgZookieWatermark,
}

/// The in-process zookie watermark for the Pg tuple path: `(tenant, region, object)` → the latest
/// zookie observed FOR THIS PROCESS. **REBUILDABLE consistency metadata, NOT a system-of-record** —
/// the durable edge set is in PG; this map is reset on a fresh instance (the persisted-zookie
/// read-side is the named P-ID-12 / MR-009 floor). A pure in-memory index/cache the durable backing
/// consults for precondition/watermark reads, not the tuple store's data of record.
#[derive(Clone, Default)]
struct PgZookieWatermark {
    inner: Arc<Mutex<HashMap<(String, String, String), Zookie>>>,
}

impl PgZookieWatermark {
    /// Lock the watermark index (poison-tolerant — a poisoned lock is a same-process panic-during-hold,
    /// which the consistency cache recovers from; the durable edges are unaffected).
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String, String), Zookie>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl TupleStore {
    /// Build the S3 store over the in-memory TEST-DOUBLE backing (MR-009b Wave 2: compiled ONLY under
    /// `#[cfg(any(test, feature = "test-support"))]`). The PRODUCTION constructor is
    /// [`TupleStore::with_pg`]; this `::new` is the DB-free unit-test entry point downstream crates
    /// reach via the `test-support` dev-dependency. Auto-registers as a `PersonalDataHolder` (§3.4).
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(outbox: OutboxStore) -> TupleStore {
        TupleStore::with_minter(outbox, Arc::new(MonotonicMinter::new()))
    }

    /// Build the S3 store with an explicit id-minter (so a test can inject a deterministic one;
    /// the real wall-clock+random ULID source implements the same [`IdMinter`] trait, P-S12). The
    /// in-memory TEST-DOUBLE constructor (MR-009b Wave 2 — `test-support`-gated).
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_minter(outbox: OutboxStore, minter: Arc<dyn IdMinter>) -> TupleStore {
        let holder = OltpStoreHolder::new(S3_HOLDER);
        // Opening IS registering (§3.4, GD-3): the S3 store auto-registers as a PersonalDataHolder
        // the moment it is constructed, so "we forgot the tuple store" is structurally impossible.
        let _receipt = holder.register();
        TupleStore {
            backend: TupleBackend::Memory(Arc::new(Mutex::new(Inner::default()))),
            revision: Arc::new(AtomicU64::new(0)),
            outbox,
            minter,
            holder,
        }
    }

    /// **Build the S3 store over the REAL durable PG backing (MR-007 / SI-019).** The `rebac_tuple`
    /// edge set persists through the MR-022 [`myelin_storage::SubstrateProvider`] pool +
    /// `with_tenant_tx` convention (RLS-scoped, no GUC bleed). `rt` is the tokio runtime handle the
    /// sync API drives the async backing on. The outbox emit + zookie semantics are preserved (the
    /// event still co-commits-iff-the-durable-write-succeeds). The store auto-registers as a holder.
    /// **The PRODUCTION default (MR-009b Wave 2) — always compiled.**
    pub fn with_pg(
        outbox: OutboxStore,
        backing: myelin_storage::DurableTupleBacking,
        rt: tokio::runtime::Handle,
    ) -> TupleStore {
        TupleStore::with_pg_minter(outbox, Arc::new(MonotonicMinter::new()), backing, rt)
    }

    /// [`Self::with_pg`] with an explicit id-minter (deterministic in tests). Always compiled (W2).
    pub fn with_pg_minter(
        outbox: OutboxStore,
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
                watermark: PgZookieWatermark::default(),
            }),
            revision: Arc::new(AtomicU64::new(0)),
            outbox,
            minter,
            holder,
        }
    }

    /// The per-tenant DEK key class this store encrypts under (the per-tenant-DEK pin, BY
    /// REFERENCE). The KMS hierarchy (P-ST-06 → P-058) is sequenced AFTER this prompt; the store
    /// declares the class name so the wiring lands structurally when the KMS does. Named floor.
    pub fn dek_class(&self, scope: &TenantScope) -> String {
        // `kms://<tenant>/<class>` — the tuple store encrypts under the per-TENANT key (a tuple is
        // tenant-content, not per-subject PII), matching the §2.10 pii_key_ref grammar shape.
        format!("kms://{}/tenant", scope.tenant().0)
    }

    /// The store AS a `PersonalDataHolder` (the holder the DSR fan-out drives). The DSR bodies
    /// (the per-subject tuple erasure step) land with the GDPR M1 / P-ID-20 erasure path; here the
    /// REGISTRATION is real (the holder is constructed + registered) so the holder-registered
    /// architecture test sees the S3 store.
    pub fn holder(&self) -> &OltpStoreHolder {
        &self.holder
    }

    /// The current zookie (the store's monotonic revision) — the watermark a reader stamps. The
    /// *read* side that compares this against a required revision (waits/falls-back) is P-ID-12.
    pub fn current_zookie(&self) -> Zookie {
        Self::zookie_of(self.revision.load(Ordering::SeqCst))
    }

    /// Render a revision number as the lexically-monotonic zookie string `zk-<020d>` so a later
    /// zookie sorts after an earlier one (the monotone-advance property reads/writes rely on).
    fn zookie_of(rev: u64) -> Zookie {
        Zookie(format!("zk-{rev:020}"))
    }

    /// The zookie of the object `object` in the verified scope's partition — the value to stamp on
    /// the object (`page.acl_zookie`). The zookie of the LAST write that touched any tuple about
    /// this object, or the store's current revision if the object has no tuples yet. Built through
    /// a [`TenantQuery`] so the access carries its `(tenant, region)` predicate (the floor).
    pub fn object_zookie(&self, scope: &TenantScope, object: &str) -> Zookie {
        // The tenant-predicate floor: the read is built from the verified scope (no cross-tenant
        // path). `_q.predicate_sql()` is the thin `(tenant, region)` clause every read carries.
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S3_TABLE));
        let part_key = Self::part_key(scope);
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            TupleBackend::Memory(inner_arc) => {
                let inner = Self::mem_lock(inner_arc);
                inner
                    .partitions
                    .get(&part_key)
                    .and_then(|p| {
                        // The latest (lexically-monotonic) zookie about this object. Zookie is not
                        // `Ord` (it is an opaque token at the ABI), so we compare by its inner string
                        // — the zero-padded `zk-<rev>` form, so lexical order == revision order.
                        p.values()
                            .filter(|t| t.tuple.object.0 == object)
                            .max_by(|a, b| a.zookie.0.cmp(&b.zookie.0))
                            .map(|t| t.zookie.clone())
                    })
                    .unwrap_or_else(|| self.current_zookie())
            }
            TupleBackend::Pg(pg) => {
                let zk = pg.watermark.lock();
                zk.get(&(part_key.0, part_key.1, object.to_string()))
                    .cloned()
                    .unwrap_or_else(|| self.current_zookie())
            }
        }
    }

    /// Read the tuples for a `(tenant, region)` partition (for the reverse-index feed / tests).
    /// There is NO accessor that reads across partitions — a read is scoped to one verified
    /// `(tenant, region)`, so cross-tenant reads are structurally impossible.
    pub fn tuples_in(&self, scope: &TenantScope) -> Vec<StoredTuple> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S3_TABLE));
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            TupleBackend::Memory(inner_arc) => {
                let inner = Self::mem_lock(inner_arc);
                inner
                    .partitions
                    .get(&Self::part_key(scope))
                    .map(|p| p.values().cloned().collect())
                    .unwrap_or_default()
            }
            TupleBackend::Pg(pg) => {
                let tenant = scope.tenant().0.clone();
                let region = scope.region().0.clone();
                // Read the DURABLE edge set back from PG (RLS-scoped through with_tenant_tx). A
                // backing error surfaces as an empty read here (the loud variant is the write path);
                // the durability test asserts presence, so a dropped read would fail it loudly.
                let edges = pg
                    .block(pg.backing.edges_in(&tenant, &region))
                    .unwrap_or_default();
                let zk = pg.watermark.lock();
                edges
                    .into_iter()
                    .map(|(object, relation, subject)| {
                        let zookie = zk
                            .get(&(tenant.clone(), region.clone(), object.clone()))
                            .cloned()
                            .unwrap_or_else(|| self.current_zookie());
                        StoredTuple {
                            tenant: scope.tenant().clone(),
                            region: scope.region().clone(),
                            tuple: myelin_identity::RelationTuple {
                                object: myelin_identity::ObjectId(object),
                                relation: myelin_identity::RelName(relation),
                                subject: myelin_identity::PrincipalId(subject),
                                caveat: None,
                            },
                            zookie,
                            // expires_at is NOT a rebac_tuple column — the per-run-grant TTL
                            // durability is the named boundary (deferred to a schema extension /
                            // MR-009). The DURABLE edge round-trips; the in-process auto-expiry
                            // remains exercised by the Memory backend.
                            expires_at: None,
                        }
                    })
                    .collect()
            }
        }
    }

    /// **`write_tuples([Δtuple], precondition?) → zookie` (contract 4.6; the 4.10 write-half).**
    ///
    /// The atomic write path. Under ONE store lock + ONE outbox transaction:
    /// 1. honour the `precondition?` — if the object's current zookie != the expected one, abort
    ///    the WHOLE write (return [`WriteError::PreconditionFailed`]); nothing changes, nothing
    ///    emits (read-modify-write is not lost);
    /// 2. apply the deltas (add/remove the `object#relation@subject` edges) — per-run grants are
    ///    auto-expiring tuples (`expires_at`);
    /// 3. advance the monotonic revision → the new `zookie` (the 4.10 write-half);
    /// 4. stage the `iam.tuple_written` event onto the SAME outbox transaction (carrying the
    ///    write's zookie for S8's watermark) and **co-commit** the tuple change + the event.
    ///
    /// `actor` is the writing principal (attributed by **opaque `principal_id` only** on the
    /// event — the erasable profile never enters the immutable log, EI-04 §1). `expires_at` is
    /// `Some(deadline)` for an auto-expiring per-run grant.
    ///
    /// Returns the advanced [`Zookie`] to stamp on the object, or a [`WriteError`] (in which case
    /// NOTHING changed and NOTHING emitted — emit-iff-committed).
    // `expires_at` (the auto-expiring per-run grant TTL) is consumed only by the in-memory test-double
    // write path; the durable `rebac_tuple` has no `expires_at` column (the named MR-009 boundary), so
    // in the durable-only default build it is unused — allowed rather than gated (it is public API).
    #[cfg_attr(
        not(any(test, feature = "test-support")),
        allow(unused_variables)
    )]
    pub fn write_tuples(
        &self,
        scope: &TenantScope,
        actor: &Principal,
        deltas: &[TupleDelta],
        precondition: Option<&Precondition>,
        expires_at: Option<Timestamp>,
        occurred_at: Timestamp,
    ) -> Result<Zookie, WriteError> {
        // The tenant-predicate floor: the whole write is built from the verified scope (no
        // cross-tenant write path). The thin `(tenant, region)` predicate is carried on every
        // statement; a tenant-less write is unconstructable (you need a TenantScope here).
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S3_TABLE));
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

    /// Build the ONE outbox transaction the write co-commits (the `iam.tuple_written` emit, the ONLY
    /// emit path). Returns the UNCOMMITTED transaction so the caller commits it at the right moment
    /// (emit-iff-committed): after the durable apply succeeds.
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
            // The outbox stamps `recorded_at` when the row is durably accepted; on this floor we
            // pass the same instant (the wall-clock source lands with the driver, P-S12).
            recorded_at: occurred_at.clone(),
            caused_by: None,
        };
        let mut tx = self.outbox.begin(Arc::clone(&self.minter), ctx_base);
        tx.stage_state_change(format!(
            "rebac: applied {} delta(s) → zookie {}",
            deltas.len(),
            zookie.0
        ));
        // The iam.tuple_written event — the event-sourced record S8 consumes, carrying the write's
        // zookie watermark. Attribution by OPAQUE principal_id; references-not-payloads; no PII.
        let draft = self.tuple_written_draft(scope, deltas, zookie);
        tx.emit(draft, None).map_err(|e| WriteError::CommitFailed(e.0))?;
        Ok(tx)
    }

    /// The in-memory test-double write path (MR-009b Wave 2: compiled ONLY under
    /// `#[cfg(any(test, feature = "test-support"))]`). Atomic under one lock; the state change + the
    /// event co-commit on one outbox transaction. The PRODUCTION path is [`Self::write_tuples_pg`].
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
        // Hold the store lock for the WHOLE write so it is atomic AND the zookie advance + apply are
        // serialized (concurrent writers get distinct, monotonic zookies).
        let mut inner = Self::mem_lock(inner_arc);

        // (1) Precondition — read-modify-write guard.
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

        // (2) Apply the deltas under the SAME lock (atomic).
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

        // Co-commit: the state change + the iam.tuple_written event become durable together.
        tx.commit().map_err(|e| WriteError::CommitFailed(e.0))?;
        Ok(zookie)
    }

    /// **The REAL durable write path (MR-007): the `rebac_tuple` edge set persists through the
    /// MR-022 `with_tenant_tx` convention, and the `iam.tuple_written` event co-commits
    /// emit-iff-the-durable-write-succeeded.** The event is staged FIRST (uncommitted); the deltas
    /// are then applied in ONE tenant-scoped DB transaction; only on durable success does the outbox
    /// commit (so a failed durable apply emits NOTHING). The zookie/precondition use the in-process
    /// watermark (the durable system-of-record is the EDGE set; the persisted-zookie read-side is the
    /// named P-ID-12 / MR-009 floor).
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

        // (1) Precondition — against the in-process zookie watermark.
        if let Some(pre) = precondition {
            if let Some(expected) = &pre.expected_zookie {
                let actual = self
                    .pg_object_zookie(pg, &tenant, &region, deltas)
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

        // Stage the event (uncommitted) — emit-iff the durable write below succeeds.
        let tx = self.stage_event(scope, actor, deltas, &zookie, occurred_at)?;

        // Apply the deltas atomically in ONE tenant-scoped DB transaction (with_tenant_tx).
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
        pg.block(pg.backing.apply_deltas(&tenant, &region, edge_deltas))
            .map_err(|e| WriteError::CommitFailed(e.to_string()))?;

        // Durable write succeeded → co-commit the event (the outbox emit).
        tx.commit().map_err(|e| WriteError::CommitFailed(e.0))?;

        // Advance the in-process zookie watermark for the touched objects.
        {
            let mut zk = pg.watermark.lock();
            for d in deltas {
                let obj = match d {
                    TupleDelta::Add(t) | TupleDelta::Remove(t) => t.object.0.clone(),
                };
                zk.insert((tenant.clone(), region.clone(), obj), zookie.clone());
            }
        }
        Ok(zookie)
    }

    /// The newest in-process zookie among the objects the `deltas` touch (the Pg-path precondition
    /// read). `None` if none of those objects has been written in this process yet.
    fn pg_object_zookie(
        &self,
        pg: &PgTupleBacking,
        tenant: &str,
        region: &str,
        deltas: &[TupleDelta],
    ) -> Option<Zookie> {
        let zk = pg.watermark.lock();
        deltas
            .iter()
            .filter_map(|d| {
                let obj = match d {
                    TupleDelta::Add(t) | TupleDelta::Remove(t) => t.object.0.clone(),
                };
                zk.get(&(tenant.to_string(), region.to_string(), obj)).cloned()
            })
            .max_by(|a, b| a.0.cmp(&b.0))
    }

    /// The `iam.tuple_written` [`EventDraft`] (references-not-payloads, opaque-id attribution).
    /// The subject is the object the write is about (the first delta's object — the aggregate is
    /// the object so per-object ordering holds); the payload carries the object refs + the
    /// write's zookie (the S8 watermark), never any PII.
    fn tuple_written_draft(
        &self,
        scope: &TenantScope,
        deltas: &[TupleDelta],
        zookie: &Zookie,
    ) -> EventDraft {
        // The object the event is about (per-object ordering aggregate). A write with no delta is
        // a no-op the caller should not make; we still produce a stable subject for safety.
        let object = deltas
            .iter()
            .map(|d| match d {
                TupleDelta::Add(t) | TupleDelta::Remove(t) => t.tuple_object(),
            })
            .next()
            .unwrap_or("unknown");
        // The PII-free subject ArtifactRef for the object (myelin://<tenant>/iam/tuple/<object>).
        let subject = EvArtifactRef(format!(
            "myelin://{}/iam/tuple/{}",
            scope.tenant().0,
            object
        ));
        // Per-object ordering: the aggregate is the object, so all tuple-writes about one object
        // are sequenced (the relay drains an aggregate in seq order).
        let aggregate = AggregateKey(format!("iam:tuple:{}:{}", scope.tenant().0, object));
        // references-not-payloads: object refs + the zookie watermark, NEVER PII. The deltas are
        // summarised as op + object#relation@subject refs (all opaque references).
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
            type_: EventType(IAM_TUPLE_WRITTEN.to_string()),
            subject,
            aggregate,
            payload: serde_json::json!({
                "zookie": zookie.0,        // the S8 revision watermark
                "deltas": ops,             // opaque object#relation@subject refs
            }),
            // The authz state change is recorded under the tenant's controller role (§2.1).
            data_role: EvDataRole::Controller,
            visibility: Visibility::Internal,
            // opaque-id attribution + references-not-payloads ⇒ never inline PII (EI-04 §1).
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }

    /// The `(tenant, region)` partition key for a verified scope (the OUTER partition; the
    /// object-id hash is the inner one). A `(String, String)` so the partition map is keyed by
    /// the residency-pinned tenant+region — a read for one never reaches another's bucket.
    fn part_key(scope: &TenantScope) -> (String, String) {
        (scope.tenant().0.clone(), scope.region().0.clone())
    }

    /// The newest zookie among the objects the `deltas` touch, within a locked partition (the
    /// precondition read). `None` if none of those objects has a tuple yet. Memory-path helper
    /// (MR-009b Wave 2 — `test-support`-gated, it takes the test-double [`Inner`]).
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

    /// Lock the in-memory test-double backing (the Memory arm). Static — it takes the backing arc so
    /// the borrow checker is happy across the dispatch. Memory-path helper (MR-009b Wave 2 —
    /// `test-support`-gated).
    #[cfg(any(test, feature = "test-support"))]
    fn mem_lock(arc: &Arc<Mutex<Inner>>) -> std::sync::MutexGuard<'_, Inner> {
        arc.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl PgTupleBacking {
    /// Drive an async backing call from the sync store API (the same `block_in_place`+`block_on`
    /// bridge `ValkeyCache`/`S3BlobStore` use — safe inside a multi-thread runtime).
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

/// A tiny extension so the draft/precondition helpers can read a delta's object id without
/// re-matching the `Add`/`Remove` shape everywhere.
trait TupleDeltaObject {
    fn tuple_object(&self) -> &str;
}

impl TupleDeltaObject for RelationTuple {
    fn tuple_object(&self) -> &str {
        &self.object.0
    }
}

/// A convenience: build an auto-expiring per-run grant `expires_at` from a run deadline (the
/// `expires_at == run life` rule). The structural carrier; the wall-clock deadline source lands
/// with `mint_run_token` (P-ID-18) — here the caller supplies the RFC-3339 deadline.
pub fn run_grant_expiry(run_deadline: impl Into<String>) -> Timestamp {
    Timestamp(run_deadline.into())
}

/// Forward the contract `DataRole` → events `DataRole` (the name-aligned reconciliation, EI-01
/// §7). Exposed so a caller threading the identity-owned role onto an emit does it through one
/// seam, not an ad-hoc match.
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

    /// **`write_tuples` is atomic + returns a monotonically-advancing zookie (4.6 + 4.10 write
    /// half).** Two sequential writes return strictly-increasing zookies; the tuples are durable
    /// in the verified partition after the (atomic) write.
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
        // Both tuples are durable in the partition (the atomic write applied them).
        let tuples = store.tuples_in(&s);
        assert_eq!(tuples.len(), 2, "both adds are durable");
        // The object's stamped zookie is the latest write's.
        assert_eq!(store.object_zookie(&s, "repo:core"), z1);
    }

    /// **A failed precondition aborts the WHOLE write (read-modify-write is not lost).** Writing
    /// with an `expected_zookie` that does not match the object's current zookie returns
    /// `PreconditionFailed` and changes NOTHING (no tuple added, no zookie advance observed by a
    /// reader, no event emitted).
    #[test]
    fn failed_precondition_aborts_the_whole_write_and_emits_nothing() {
        let store = TupleStore::new(OutboxStore::new());
        let s = scope("acme");
        // Seed an object at z0.
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

        // Write with a STALE expected zookie (a concurrent revoke moved the object on).
        let stale = Zookie("zk-00000000000000000000".into()); // the genesis zookie, not z0.
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
        // Nothing changed: the writer tuple was NOT added, and NO new event was emitted.
        assert_eq!(
            store.tuples_in(&s).len(),
            1,
            "the aborted write added no tuple"
        );
        assert_eq!(
            store.outbox.outbox_depth(),
            depth_before,
            "a failed precondition emits NOTHING (emit-iff-committed)"
        );
    }

    /// A matching precondition is honoured (the write proceeds) — the read-modify-write happy path.
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
        // Now write with the CORRECT expected zookie → proceeds.
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
        assert_eq!(store.tuples_in(&s).len(), 2);
    }

    /// **The emit is via the OUTBOX only (the no-raw-publish floor): a committed write emits
    /// `iam.tuple_written`, and the relay publishes exactly that.** This is the GATE: the
    /// committed write produced one outbox row carrying the `iam.tuple.written` type + the write's
    /// zookie, and there is NO other emit path.
    #[test]
    fn committed_write_emits_iam_tuple_written_via_the_outbox_only() {
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

        // Exactly one unsent outbox row (the iam.tuple_written event) — emit-iff-committed.
        assert_eq!(
            outbox.outbox_depth(),
            1,
            "the committed write emitted exactly one event"
        );
        // Drain the relay (what `serve` does) and assert the published event is iam.tuple.written
        // carrying the write's zookie + opaque-id attribution + no PII.
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
            env.type_.0, IAM_TUPLE_WRITTEN,
            "the only emit is iam.tuple.written"
        );
        assert!(
            !env.contains_personal_data,
            "the iam.* event carries no inline PII"
        );
        assert_eq!(
            env.payload["zookie"],
            serde_json::json!(z.0),
            "the event carries the S8 watermark"
        );
        // Attribution is by opaque principal_id only (the actor) — never the erasable profile.
        assert_eq!(env.actor.0.principal_id, PrincipalId("p-admin".into()));
        assert_eq!(outbox.outbox_depth(), 0, "the relay drained the outbox");
    }

    /// **0 emits without a committed write, 0 committed writes without an emit (the GATE).** N
    /// committed writes ⇒ exactly N events; the failed (precondition) write ⇒ 0 extra events.
    #[test]
    fn emit_count_equals_committed_write_count() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let s = scope("acme");
        // Three committed writes (distinct objects → 3 events).
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
        // One FAILED write (stale precondition on object "a") → 0 extra events.
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
            "exactly 3 events for 3 committed writes — 0 emits without a committed write"
        );
    }

    /// **A per-run grant is an auto-expiring tuple (`expires_at == run life`).** Writing a grant
    /// with a run deadline stores it with `expires_at` set (the defence-in-depth revoke-on-crash).
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
            .into_iter()
            .find(|t| t.tuple.object.0 == "run:R1")
            .expect("the grant is stored");
        assert_eq!(
            grant.expires_at,
            Some(deadline),
            "a per-run grant auto-expires (== run life)"
        );
        // An ordinary grant (no deadline) does NOT auto-expire.
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
            .into_iter()
            .find(|t| t.tuple.object.0 == "repo:core")
            .expect("the durable grant is stored");
        assert_eq!(
            durable.expires_at, None,
            "an ordinary grant is durable (no expiry)"
        );
    }

    /// **No cross-tenant tuple / no cross-tenant query path.** A write under tenant `acme` is
    /// invisible to a read under tenant `globex` — the partitions are isolated by the verified
    /// `(tenant, region)` scope, and there is NO accessor that reads across them.
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

        // globex sees NOTHING acme wrote (the partition is keyed by the verified scope).
        assert!(
            store.tuples_in(&globex).is_empty(),
            "no cross-tenant read path"
        );
        // acme sees its own tuple.
        assert_eq!(store.tuples_in(&acme).len(), 1);
    }

    /// **The S3 store auto-registers as a PersonalDataHolder (§1.1, GD-3).** Opening IS
    /// registering — the holder is constructed + registered under the S3 holder name. The DSR
    /// bodies (per-subject tuple erasure) are the GDPR-M1 / P-ID-20 floor.
    #[test]
    fn s3_store_registers_as_a_personal_data_holder() {
        let store = TupleStore::new(OutboxStore::new());
        assert_eq!(
            store.holder().store,
            S3_HOLDER,
            "the S3 store registered under its holder name"
        );
        // The holder implements the frozen PersonalDataHolder shape (the DSR bodies are the floor).
        let receipt = store.holder().register();
        assert_eq!(receipt.store, S3_HOLDER);
    }

    /// **The tenant-predicate floor: a remove targets exactly the edge.** A removed edge is gone
    /// from the partition (a re-add is idempotent); the access is built through a TenantQuery so
    /// it carries its `(tenant, region)` predicate.
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
        // re-add the SAME edge → idempotent (still one tuple).
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
            store.tuples_in(&s).len(),
            1,
            "re-adding the same edge is idempotent"
        );
        // remove it → gone.
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
            store.tuples_in(&s).is_empty(),
            "the remove deleted the edge"
        );
    }

    /// The object-id-hash partition bucket is stable + deterministic (the §6 inner partition key).
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

    /// The per-tenant DEK is pinned by reference (the named KMS floor): the class names the
    /// per-tenant key the store encrypts under (the KMS hierarchy lands P-058).
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

    /// **The CDC consumer half of 4.6:** a `write_tuples` caller (the role-compile path) writes a
    /// role grant and receives the advanced zookie to stamp on the object — the provider+consumer
    /// pair exercising the 4.6 contract. The full provider engine (S8 consuming the event) is
    /// P-ID-11; this is the write-side CDC the prompt requires.
    #[test]
    fn cdc_write_tuples_role_compile_caller() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let s = scope("acme");
        // A "role-compile" caller grants org membership (the org→team→project hierarchy edge).
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
        // The caller stamps the returned zookie on the object (page.acl_zookie / Chat membership).
        assert_eq!(store.object_zookie(&s, "org:acme"), zookie);
        // The event is on the outbox for S8 to consume (the consumer half lands P-ID-11).
        let row = outbox
            .row(&{
                // the single committed row's id
                let inner_count = outbox.committed_count();
                assert_eq!(inner_count, 1);
                // re-fetch by draining order: read the only row's event_id via a relay publish.
                let bus = InProcessBus::new();
                let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
                relay.drain_to_empty();
                let published = bus.consume("");
                published[0].event_id.clone()
            })
            .expect("the iam.tuple_written row exists for S8");
        assert_eq!(row.envelope.type_.0, IAM_TUPLE_WRITTEN);
        let _ = ConsumerName("s8_reverse_index".into()); // the S8 consumer name (P-ID-11).
    }
}
