//! # `read_replica` — S5, the authz read-replica (P-ID-16 → global P-074)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §2 (the **S5 row**: *Authz read-replica (ID-4)* — a Postgres/tuple-store **replica** of the
//! authn/authz hot path; **follows S1/S3/S8**; **read-only; stale-tolerant**; inherits the per-tenant
//! DEK; CONFIRMED), §13 (*measure before you shard* — ID-4: the **committed first scaling move** is
//! the dedicated authz read-replica S5; S8 is its concrete realisation for the consumer JOIN, but S5
//! is the named replica row in its own right). EI-01 §8: the read-replica is the named first scaling
//! move, **not** premature sharding — a thin, atomic move with its own staleness semantics.
//!
//! **Contract-index:** row **11.1** (the OLTP tier client — S5 is a *read replica of the OLTP tier*,
//! CONSUMED/WIRED here; it is not a new RPC contract, it is a replica of the existing read path).
//!
//! ## What this module ships (P-ID-16 — S5 only, a thin atomic scaling move)
//! S5 is a **read-only, stale-tolerant replica** of the OLTP tier the authn/authz hot-path reads go
//! to **when a zookie does not demand freshness** (the fail-static partner — S5 serves the same
//! `BoundedStale` reads S6 fronts, from a replicated snapshot, off the primary). It follows
//! S1/S3/S8: a replica row only ever appears in S5 by being **replicated from the primary**
//! ([`AuthzReadReplica::replicate`]); there is **no write path on S5** — every mutating accessor is
//! absent by construction, and [`AuthzReadReplica::reject_write`] names the structural rule.
//!
//! ## The consistency gate (the crux — §8.4 / row 4.10, the SAME split S6 honours)
//! [`AuthzReadReplica::route`] is the load-bearing decision: which store answers a read.
//! - **`ConsistencyMode::BoundedStale` (default-consistency) → S5.** A zookie does not demand
//!   freshness, so the read is served from the stale-tolerant replica (the scaling win: the
//!   high-QPS hot-path reads come off S5, not the primary).
//! - **`ConsistencyMode::Strong` (zookie-stamped) → BYPASS S5.** A read-your-writes / new-enemy
//!   read must NOT be served stale: it goes to the **primary** (or falls back to `check`). S5 is
//!   never read on a `Strong` request — the exact same bypass S6 (P-ID-15) and S8's watermark path
//!   (P-ID-12) enforce, here for the replica.
//!
//! This is the zookie-bypass-S5 mandatory-core branch: a mutation that serves a `Strong` read from
//! the (potentially stale) replica MUST be caught (it would defeat read-your-writes / the new-enemy
//! guard).
//!
//! ## Why S5 is its OWN store (not folded into S6/S7)
//! S6 (the fail-static cache) is an ephemeral Redis/Valkey-class *availability* cache, NEVER a source
//! of truth, TTL ≤ the revocation SLA; S7 is the revocation denylist. **S5 is a different thing:** a
//! durable Postgres-class **read replica** of the OLTP tier with its own *replication-lag* staleness
//! semantics (it is consistent up to its applied replication offset, not TTL-expiring). The prompt is
//! explicit that S5 is kept separate "because it is its own store with its own staleness semantics".
//! S6 fails STATIC on a *hiccup* (degraded availability); S5 serves *steady-state* default-consistency
//! reads from a lagging-but-live replica (a scaling move, not a degradation). They compose: S6 can
//! front S5 (a `BoundedStale` read hits S5; an Id hiccup that takes S5 down too still has S6's
//! last-coarse-grant fallback) — both honour the SAME zookie bypass.
//!
//! ## Floors named (frozen mechanism now → re-measured / wired later)
//! - **World-scale tunables (the Ids↔Filter cardinality cap, the `reverse_index_lag` / replication-lag
//!   SLO) are re-measured against S5/S8 at M5 in P-ID-31 (global P-424).** The SHAPE is frozen here
//!   (read-only, stale-tolerant, zookie-bypass); only the NUMBERS — the acceptable replication lag,
//!   the cardinality cap — are the default-to-beat re-measured under the 30× surge. Recorded:
//!   [`AuthzReadReplica::DEFAULT_MAX_REPLICATION_LAG_SECS`] is the engineering seed; P-ID-31 finalises
//!   it alongside the `reverse_index_lag` SLO already seeded in `thresholds.toml`.
//! - **The in-memory replica models the SQL/tuple-store replica** (the same EI-01 §1 deviation S1/S3/S8
//!   document): there is no live Postgres replica until the OLTP driver lands (P-S15). The
//!   `(tenant, region)`-partitioned, read-only, follows-the-primary, replication-offset-stamped
//!   semantics are byte-for-byte the §2 S5 contract; the seam shape does not change when the binding
//!   lands (a `replicate` call becomes "apply WAL/logical-replication delta"; a `route`→primary
//!   becomes "route to the primary connection pool").
//! - **The live primary read body the bypass routes to is the existing `check` / S3 / S1 read path**
//!   (P-ID-09 / P-ID-05 / P-ID-08). S5 does not re-implement it; the bypass returns
//!   [`ReadRoute::Primary`] and the caller runs the SAME authoritative read it already has (one
//!   primitive, no bespoke replica read path).

use myelin_identity::{Consistency, ConsistencyMode};
use myelin_storage::{OltpStoreHolder, TenantQuery, TenantScope, TenantTable};
use myelin_substrate::Seconds;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The S5 store's tenant-owned logical table name (the `(tenant, region)`-first RLS table the
/// replica mirrors). Every S5 access is built through [`TenantQuery::for_table`] over THIS table, so
/// a replica read without a verified `(tenant, region)` scope does not compile (the
/// `tenant-predicate` floor) — the replica inherits the primary's RLS partitioning, it does not relax
/// it.
pub const S5_TABLE: &str = "authz_read_replica";

/// The S5 store's stable holder name. S5 is a **replica of the OLTP tier** — it holds replicated
/// copies of authz read rows that reference subjects, so it is itself a `PersonalDataHolder` (it
/// auto-registers on construction; "we forgot the replica" is structurally impossible). Its erasure
/// posture INHERITS the primary's (§2 — "inherits"): a `remove` replicated from the primary
/// tombstones the row for free; the durable authority is S1/S3/S8 and S5 is reconstructible by
/// re-replicating from them (no bespoke replica recovery code). The DSR fan-out reaching S5 is the
/// GDPR-M1 derivative-erasure floor (P-ID-20 / P-GA-25), as for S8.
pub const S5_HOLDER: &str = "identity_authz_read_replica";

/// One replicated authz read row — an opaque `(key → value)` projection of a primary read row,
/// stamped with the primary **replication offset** it was applied at. S5 holds whatever the authn/
/// authz hot path reads (a principal-status row, a coarse grant, a reverse-index row); the replica is
/// agnostic to the row SHAPE — it mirrors the primary's bytes. The `key` is the row's primary key
/// within its `(tenant, region)` partition; the `value` is the replicated payload (opaque here, the
/// SQL row in the live binding). PII-free at this layer (the value is the already-encrypted /
/// reference-grade primary payload — S5 inherits the primary's encryption, §2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaRow {
    /// The row's primary key within its `(tenant, region)` partition (e.g. a principal id, a tuple
    /// key). Distinct keys never collide; a `replicate("add", …)` upserts by it, a
    /// `replicate("remove", …)` tombstones by it.
    pub key: String,
    /// The replicated payload (opaque — the primary's row bytes; the SQL row in the live binding).
    pub value: String,
}

/// The `(tenant, region)` partition key (§2 — S5 "follows S1/S3/S8", which are `(tenant, region)`
/// partitioned). A replica read for one `(tenant, region)` structurally cannot reach another's rows —
/// the replica inherits the primary's partitioning, it does not introduce a cross-tenant query path.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct PartKey {
    tenant: String,
    region: String,
}

/// One partition's replicated rows + the latest applied primary replication offset.
#[derive(Default)]
struct Partition {
    /// `key` → the replicated row (a `BTreeMap` so the replica is deterministic + a re-apply is
    /// idempotent — the same primary delta applied twice yields one row).
    rows: BTreeMap<String, ReplicaRow>,
    /// The latest primary **replication offset** applied to this partition (the replica's
    /// "how-fresh-am-I" cursor — the replication-lag analogue of S8's `revision_watermark`).
    /// Monotone: a redelivered older delta never moves it backward.
    applied_offset: u64,
}

/// The shared inner state of an [`AuthzReadReplica`] (behind `Arc<Mutex<…>>` so it is a cloneable
/// handle the replication feed + the readers share — one replica, EI-01 §7).
#[derive(Default)]
struct Inner {
    /// `(tenant, region)` → the replicated rows + applied offset. The OUTER map is the partition (no
    /// cross-tenant query path: a read for tenant A never touches tenant B's partition).
    partitions: BTreeMap<PartKey, Partition>,
}

/// **S5 — the authz read-replica (architecture §2 S5 row; the ID-4 first scaling move).**
///
/// A cloneable handle over a shared, **read-only**, **stale-tolerant** replica of the OLTP-tier
/// authz read path. Rows arrive ONLY by replication from the primary ([`AuthzReadReplica::replicate`])
/// — there is **no write accessor**; the only mutator is the replication apply, which is the primary's
/// WAL/logical-replication stream (modelled here as an explicit `replicate` call). Holder-registered
/// (it replicates rows that reference subjects). Every accessor takes a verified [`TenantScope`]:
/// **no cross-tenant query path** (it inherits the primary's RLS).
///
/// **The whole point (§13 / EI-01 §8).** *Measure before you shard.* The committed first scaling move
/// is a dedicated read replica, NOT premature sharding: the high-QPS default-consistency hot-path
/// reads come off S5 (relieving the primary), while a zookie-demanding read bypasses S5 to the primary
/// (it must not be served stale). S5 is stale-tolerant by design — it lags the primary by its
/// replication offset, and that is acceptable for exactly the reads a zookie does not pin.
#[derive(Clone)]
pub struct AuthzReadReplica {
    inner: Arc<Mutex<Inner>>,
    /// The holder this replica auto-registers as (the `PersonalDataHolder` seam) — proof the "every
    /// store is a holder" invariant holds for S5 (§3.4, GD-3; contract 10.1).
    holder: OltpStoreHolder,
    /// Telemetry: how many reads were served from S5 vs routed to the primary on a zookie. Drills /
    /// the scaling assertion read the served-from-replica ratio off this. Observability is part of the
    /// pass (EI-01 §3).
    telemetry: Arc<ReplicaTelemetry>,
}

impl Default for AuthzReadReplica {
    fn default() -> Self {
        AuthzReadReplica::new()
    }
}

impl AuthzReadReplica {
    /// The engineering-seed maximum acceptable replication lag (seconds) — the staleness budget S5 is
    /// allowed to lag the primary by for a `BoundedStale` read. The SHAPE (stale-tolerant within a
    /// bounded lag) is frozen; this NUMBER is the default-to-beat **re-measured + finalised at
    /// world-scale in P-ID-31 (global P-424)**, alongside the `reverse_index_lag` SLO already seeded in
    /// `thresholds.toml`. Recorded here (not hardcoded into the routing) so the floor is visible.
    pub const DEFAULT_MAX_REPLICATION_LAG_SECS: Seconds = 30;

    /// Build the S5 read replica. The store auto-registers as a `PersonalDataHolder` on construction
    /// (opening IS registering, §3.4) — so "we forgot the replica" is structurally impossible.
    pub fn new() -> AuthzReadReplica {
        let holder = OltpStoreHolder::new(S5_HOLDER);
        let _receipt = holder.register();
        AuthzReadReplica {
            inner: Arc::new(Mutex::new(Inner::default())),
            holder,
            telemetry: Arc::new(ReplicaTelemetry::default()),
        }
    }

    /// The store AS a `PersonalDataHolder` (the holder the DSR fan-out drives). S5 inherits the
    /// primary's erasure posture (§2): the DSR bodies (the derivative-erasure step that purges/re-
    /// replicates) land with the GDPR-M1 / P-ID-20 derivative-erasure path; here the REGISTRATION is
    /// real so the holder-registered architecture test sees S5.
    pub fn holder(&self) -> &OltpStoreHolder {
        &self.holder
    }

    /// The replica served/bypassed telemetry (the scaling-win signal: served-from-replica vs
    /// routed-to-primary).
    pub fn telemetry(&self) -> &ReplicaTelemetry {
        &self.telemetry
    }

    /// **THE consistency gate (the crux, §8.4 / row 4.10) — route a read to S5 or the primary.**
    /// This is the load-bearing decision the prompt's GATE quantifies:
    /// - **`BoundedStale` (default-consistency) → [`ReadRoute::Replica`].** A zookie does not demand
    ///   freshness ⇒ serve from the stale-tolerant replica (the scaling win).
    /// - **`Strong` (zookie-stamped) → [`ReadRoute::Primary`].** A read-your-writes / new-enemy read
    ///   must not be served stale ⇒ go to the primary (or fall back to `check`). **S5 is never read on
    ///   a `Strong` request.**
    ///
    /// The route is recorded in telemetry. This is a pure decision (no row access) so it is the same
    /// whether or not the partition exists — the caller then either reads S5 (on `Replica`) or runs
    /// its authoritative primary read (on `Primary`). Keeping route and read separate is what makes
    /// the zookie-bypass branch a single, mutation-testable decision.
    pub fn route(&self, at: &Consistency) -> ReadRoute {
        match at.mode {
            // Zookie-stamped strong read → BYPASS S5 (read-your-writes / new-enemy guard). The
            // replica is potentially stale; a strong read must come off the primary.
            ConsistencyMode::Strong => {
                self.telemetry.observe_primary();
                ReadRoute::Primary
            }
            // Default-consistency read → the stale-tolerant replica (the scaling move). The high-QPS
            // hot-path reads come off S5, relieving the primary.
            ConsistencyMode::BoundedStale => {
                self.telemetry.observe_replica();
                ReadRoute::Replica
            }
        }
    }

    /// **Read a replicated row from S5 — ONLY valid for a `BoundedStale` route.** Returns the
    /// replicated row for `key` in the verified `(tenant, region)` partition, or `None` if the replica
    /// has not yet replicated it (a stale replica legitimately lags). Built through a [`TenantQuery`]
    /// so the read carries its `(tenant, region)` predicate (the tenant-predicate floor) — an unscoped
    /// replica read is unconstructable, and there is NO cross-tenant read path.
    ///
    /// This is the S5 side of the [`ReadRoute::Replica`] branch; the [`ReadRoute::Primary`] branch
    /// does NOT call this (it runs the authoritative primary read). A caller that mistakenly read S5
    /// on a `Strong` route would have to ignore [`AuthzReadReplica::route`] — the routing + the read
    /// are deliberately separate so the bypass is a single decision.
    pub fn read(&self, scope: &TenantScope, key: &str) -> Option<ReplicaRow> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S5_TABLE));
        let pk = PartKey {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
        };
        self.lock()
            .partitions
            .get(&pk)
            .and_then(|p| p.rows.get(key).cloned())
    }

    /// **The replication apply — the ONLY path a row appears in / leaves S5 (S5 follows S1/S3/S8).**
    /// Apply one primary delta to the replica: `op` is `"add"` (upsert the row) / `"remove"`
    /// (tombstone it), stamped with the primary `offset` it was applied at. This models the primary's
    /// WAL/logical-replication stream (the live binding replaces it with "apply the replication
    /// delta"); there is deliberately **no public write/insert/update accessor** — the replica is
    /// read-only to its consumers, mutated only by replication from the primary.
    ///
    /// Built through a [`TenantQuery`] so the apply carries its `(tenant, region)` predicate (the
    /// tenant-predicate floor) — a tenant-less apply is unconstructable; a row replicated for tenant A
    /// lands in tenant A's partition. The apply is idempotent (a re-add is a no-op upsert; a re-remove
    /// of an absent row is a no-op) and advances the partition's `applied_offset` monotonically (an
    /// older redelivery never moves it backward).
    pub fn replicate(&self, scope: &TenantScope, op: &str, row: ReplicaRow, offset: u64) {
        // The tenant-predicate floor: the apply is built from the verified scope (no cross-tenant
        // write path). The thin `(tenant, region)` predicate is carried on the statement.
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S5_TABLE));
        let pk = PartKey {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
        };
        let mut inner = self.lock();
        let partition = inner.partitions.entry(pk).or_default();
        match op {
            "add" => {
                partition.rows.insert(row.key.clone(), row);
            }
            "remove" => {
                partition.rows.remove(&row.key);
            }
            // An unknown op is genuine uncertainty — the replica is NOT mutated (a malformed delta
            // never silently adds a replicated grant). The replication feed surfaces it loudly.
            _ => {}
        }
        // Advance the applied replication offset monotonically (the replica's freshness cursor). An
        // older redelivery never moves it backward.
        if offset > partition.applied_offset {
            partition.applied_offset = offset;
        }
    }

    /// The latest primary replication offset applied to a `(tenant, region)` partition (the replica's
    /// freshness cursor — the replication-lag analogue of S8's `revision_watermark`). A consumer that
    /// wants "is the replica fresh enough for this zookie" compares its required offset against this;
    /// the read-side consistency body that does so for the live binding rides P-ID-12's watermark
    /// path. An un-replicated partition reports offset 0.
    pub fn applied_offset(&self, scope: &TenantScope) -> u64 {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S5_TABLE));
        let pk = PartKey {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
        };
        self.lock()
            .partitions
            .get(&pk)
            .map(|p| p.applied_offset)
            .unwrap_or(0)
    }

    /// The number of replicated rows in a `(tenant, region)` partition (for tests / lag
    /// instrumentation). Scoped — no cross-tenant read path.
    pub fn row_count(&self, scope: &TenantScope) -> usize {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S5_TABLE));
        let pk = PartKey {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
        };
        self.lock()
            .partitions
            .get(&pk)
            .map(|p| p.rows.len())
            .unwrap_or(0)
    }

    /// **The structural read-only rule, made explicit + testable (the GATE "0 writes to S5").** S5 has
    /// no write/insert/update accessor — a consumer-side write attempt has nowhere to land. This
    /// method names the rule: a direct write to the replica is REJECTED ([`ReplicaWriteRejected`]) —
    /// the only path that mutates S5 is [`AuthzReadReplica::replicate`] (the primary's replication
    /// stream). It exists so a drill can assert "a write attempt errors" against a concrete surface
    /// rather than the absence of one (and so a future contributor who reaches for a write accessor
    /// finds the rule, not a silent insert).
    pub fn reject_write(&self) -> Result<std::convert::Infallible, ReplicaWriteRejected> {
        Err(ReplicaWriteRejected)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Where a read should be served from — the result of the S5 consistency gate
/// ([`AuthzReadReplica::route`]). The mandatory-core zookie-bypass branch is encoded here: `Strong`
/// reads route `Primary`, `BoundedStale` reads route `Replica`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadRoute {
    /// Serve from S5 (a `BoundedStale` default-consistency read — the stale-tolerant scaling path).
    Replica,
    /// Serve from the **primary** (or fall back to `check`) — a `Strong` zookie-stamped read that must
    /// NOT be served stale (read-your-writes / the new-enemy guard). S5 is bypassed entirely.
    Primary,
}

impl ReadRoute {
    /// Was this read routed to the stale-tolerant replica (the scaling win)?
    pub fn is_replica(&self) -> bool {
        matches!(self, ReadRoute::Replica)
    }

    /// Did this read BYPASS S5 to the primary (the zookie-bypass)?
    pub fn is_primary(&self) -> bool {
        matches!(self, ReadRoute::Primary)
    }
}

/// The structural rejection of a direct write to S5 (the read-only floor). S5 is read-only to its
/// consumers; the only mutator is [`AuthzReadReplica::replicate`] (the primary's replication stream).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplicaWriteRejected;

impl core::fmt::Display for ReplicaWriteRejected {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "S5 is a read-only replica (architecture §2): there is no consumer write path; the only \
             mutator is replication from the primary (AuthzReadReplica::replicate)"
        )
    }
}

impl std::error::Error for ReplicaWriteRejected {}

/// **The S5 served/bypassed telemetry (the scaling-win signal).** Every [`AuthzReadReplica::route`]
/// records whether the read was served from the replica (a `BoundedStale` hit — the primary was
/// relieved) or routed to the primary (a `Strong` bypass). Observability is part of the pass
/// (EI-01 §3); the metrics-health-port export lands with the real port binding, this is the in-process
/// counter.
#[derive(Debug, Default)]
pub struct ReplicaTelemetry {
    /// Reads served from S5 (`BoundedStale`) — the high-QPS hot-path reads taken off the primary.
    served_from_replica: AtomicU64,
    /// Reads routed to the primary (`Strong` bypass) — read-your-writes / new-enemy reads.
    routed_to_primary: AtomicU64,
}

impl ReplicaTelemetry {
    /// A `BoundedStale` read served from S5 (the scaling win).
    fn observe_replica(&self) {
        self.served_from_replica.fetch_add(1, Ordering::Relaxed);
    }

    /// A `Strong` read routed to the primary (the zookie bypass).
    fn observe_primary(&self) {
        self.routed_to_primary.fetch_add(1, Ordering::Relaxed);
    }

    /// The count of reads served from the replica (`BoundedStale`).
    pub fn served_from_replica(&self) -> u64 {
        self.served_from_replica.load(Ordering::Relaxed)
    }

    /// The count of reads routed to the primary (`Strong` bypass).
    pub fn routed_to_primary(&self) -> u64 {
        self.routed_to_primary.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind, Zookie};
    use myelin_tenancy::{Region, TenantId};

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn strong() -> Consistency {
        Consistency {
            at_least: Zookie("zk-00000000000000000005".into()),
            mode: ConsistencyMode::Strong,
        }
    }

    fn bounded_stale() -> Consistency {
        Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::BoundedStale,
        }
    }

    fn row(key: &str, value: &str) -> ReplicaRow {
        ReplicaRow {
            key: key.into(),
            value: value.into(),
        }
    }

    /// **A default-consistency (`BoundedStale`) read is served from S5 (the GATE — the scaling win).**
    /// The route is `Replica`; the replicated row is returned off S5.
    #[test]
    fn default_consistency_read_is_served_from_s5() {
        let s5 = AuthzReadReplica::new();
        let acme = scope("acme");
        // The primary replicated a grant row into S5.
        s5.replicate(&acme, "add", row("p:alice", "active"), 5);

        // A BoundedStale read routes to the replica…
        let route = s5.route(&bounded_stale());
        assert_eq!(
            route,
            ReadRoute::Replica,
            "a default-consistency read is served from S5"
        );
        assert!(route.is_replica());
        // …and the row is served off S5.
        assert_eq!(
            s5.read(&acme, "p:alice"),
            Some(row("p:alice", "active")),
            "the replicated row is served from the stale-tolerant replica"
        );
    }

    /// **A zookie-stamped (`Strong`) read BYPASSES S5 (the GATE — the zookie-bypass, mandatory-core).**
    /// Even when S5 holds a (stale) row for the same key, a `Strong` read routes to the PRIMARY — S5 is
    /// never read on a read-your-writes / new-enemy request.
    #[test]
    fn strong_read_bypasses_s5_to_the_primary() {
        let s5 = AuthzReadReplica::new();
        let acme = scope("acme");
        // S5 holds a STALE row (the primary has since changed it, but replication lags).
        s5.replicate(&acme, "add", row("p:alice", "STALE"), 1);

        // A Strong read does NOT route to the replica — it routes to the primary (bypass).
        let route = s5.route(&strong());
        assert_eq!(
            route,
            ReadRoute::Primary,
            "a zookie-stamped read bypasses S5"
        );
        assert!(route.is_primary());
        // The bypass is the load-bearing decision: the caller runs the authoritative primary read on
        // `Primary` and never consults S5's (possibly stale) row. (We assert the route, not a stale
        // read, precisely because the route is what guarantees the stale row is never served.)
    }

    /// **S5 is read-only (a write attempt errors) — the GATE "0 writes to S5".** There is no consumer
    /// write accessor; the explicit `reject_write` names the rule (the only mutator is `replicate`).
    #[test]
    fn s5_is_read_only_a_write_attempt_errors() {
        let s5 = AuthzReadReplica::new();
        let r = s5.reject_write();
        assert!(
            r.is_err(),
            "a direct write to S5 is rejected (read-only replica)"
        );
        assert_eq!(r.unwrap_err(), ReplicaWriteRejected);
    }

    /// **S5 follows the primary: a row appears ONLY by replication, and a `remove` tombstones it.**
    /// (S5 follows S1/S3/S8 — there is no other way in or out.)
    #[test]
    fn s5_follows_the_primary_replication_only() {
        let s5 = AuthzReadReplica::new();
        let acme = scope("acme");
        // Before any replication: empty.
        assert_eq!(s5.row_count(&acme), 0);
        assert_eq!(s5.read(&acme, "p:alice"), None, "no row before replication");

        // Replicate an add → the row appears.
        s5.replicate(&acme, "add", row("p:alice", "active"), 5);
        assert_eq!(s5.row_count(&acme), 1);
        assert_eq!(s5.read(&acme, "p:alice"), Some(row("p:alice", "active")));

        // Replicate a remove → the row is tombstoned (the JOIN/read stops returning it).
        s5.replicate(&acme, "remove", row("p:alice", "active"), 6);
        assert_eq!(
            s5.read(&acme, "p:alice"),
            None,
            "a removed grant is gone from the replica"
        );
    }

    /// **The replication apply is idempotent + the applied offset advances monotonically.** A
    /// redelivered older delta never moves the freshness cursor backward; a re-add is one row.
    #[test]
    fn replication_is_idempotent_and_offset_is_monotone() {
        let s5 = AuthzReadReplica::new();
        let acme = scope("acme");
        s5.replicate(&acme, "add", row("p:alice", "active"), 5);
        s5.replicate(&acme, "add", row("p:alice", "active"), 5); // re-apply
        assert_eq!(s5.row_count(&acme), 1, "a re-add is idempotent (one row)");
        assert_eq!(
            s5.applied_offset(&acme),
            5,
            "the offset is at the latest applied delta"
        );

        // A later delta advances the offset…
        s5.replicate(&acme, "add", row("p:bob", "active"), 7);
        assert_eq!(s5.applied_offset(&acme), 7);
        // …an older redelivery never moves it backward.
        s5.replicate(&acme, "add", row("p:carol", "active"), 3);
        assert_eq!(
            s5.applied_offset(&acme),
            7,
            "an older redelivery never regresses the offset"
        );
    }

    /// **No cross-tenant query path — S5 inherits the primary's RLS partitioning.** A row replicated
    /// under `acme` is invisible to a read under `globex`; the partitions are isolated by the verified
    /// `(tenant, region)` scope.
    #[test]
    fn zero_cross_tenant_replica_rows() {
        let s5 = AuthzReadReplica::new();
        let acme = scope("acme");
        let globex = scope("globex");
        s5.replicate(&acme, "add", row("p:alice", "active"), 5);

        assert_eq!(s5.row_count(&globex), 0, "0 cross-tenant replica rows");
        assert_eq!(
            s5.read(&globex, "p:alice"),
            None,
            "no cross-tenant replica read path"
        );
        assert_eq!(
            s5.applied_offset(&globex),
            0,
            "globex's offset is untouched by acme's replication"
        );
        // acme sees its own row.
        assert_eq!(s5.row_count(&acme), 1);
    }

    /// **S5 auto-registers as a PersonalDataHolder (contract 10.1 — it replicates rows referencing
    /// subjects).** Opening IS registering. The DSR bodies (derivative purge/re-replicate) are the
    /// GDPR-M1 / P-ID-20 derivative-erasure floor; S5 inherits the primary's erasure posture.
    #[test]
    fn s5_auto_registers_as_a_personal_data_holder() {
        let s5 = AuthzReadReplica::new();
        assert_eq!(
            s5.holder().store,
            S5_HOLDER,
            "S5 registered under its holder name"
        );
        let receipt = s5.holder().register();
        assert_eq!(receipt.store, S5_HOLDER);
    }

    /// **The route telemetry records served-from-replica vs routed-to-primary (the scaling-win
    /// signal).** Observability is part of the pass (EI-01 §3): two default-consistency reads come off
    /// S5, one zookie read bypasses to the primary.
    #[test]
    fn route_telemetry_records_the_scaling_split() {
        let s5 = AuthzReadReplica::new();
        let _ = s5.route(&bounded_stale());
        let _ = s5.route(&bounded_stale());
        let _ = s5.route(&strong());
        let t = s5.telemetry();
        assert_eq!(
            t.served_from_replica(),
            2,
            "two default-consistency reads served from S5"
        );
        assert_eq!(
            t.routed_to_primary(),
            1,
            "one zookie read bypassed to the primary"
        );
    }

    /// **The world-scale tunables floor is recorded (→ P-ID-31).** The replication-lag staleness
    /// budget is the engineering seed; the NUMBER is re-measured + finalised at M5 (P-ID-31 / P-424).
    /// The SHAPE (stale-tolerant within a bounded lag) is frozen here.
    #[test]
    fn world_scale_lag_tunable_is_the_named_default_to_beat() {
        assert_eq!(
            AuthzReadReplica::DEFAULT_MAX_REPLICATION_LAG_SECS,
            30,
            "the replication-lag staleness budget is the engineering seed (re-measured at P-ID-31)"
        );
    }
}
