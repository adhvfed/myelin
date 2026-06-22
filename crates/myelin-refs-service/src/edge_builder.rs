//! The **refs-edge-builder** consumer (REF-P6 / P-155; contract 5.4 consumer side).
//!
//! **Owning architecture doc:** `reference-graph.md` §4.1 (edge extraction → emit — the deterministic
//! `edge_id = hash(tenant, source, target, rel)` ⇒ idempotent rebuild), §4.3 (the inverse-index
//! build: two consumers off the substrate `EventHandler` template; **steady-state ingestion and cold
//! rebuild are the SAME code path** → they cannot drift, §7 D-4), §3.7 (R1: the `edge` projection is
//! derived/rebuildable). **External insight:** `04-hard-problems.md` §5.3 (reindex-from-source the
//! resilience primitive — the index never reads owner DBs, so steady-state and recovery use ONE code
//! path); `01-process-and-quality-doctrine.md` §3 (observability is part of the pass — `index_lag` is
//! emitted, no signal == failed drill). **VISION §1** (the reference graph as connective tissue).
//!
//! ## What REF-P6 (P-155) ships — the consumer side of 5.4
//! The [`RefsEdgeBuilder`] is an ordinary [`myelin_events::EventHandler`] (contract 2.4), driven by
//! the ONE sanctioned consumer runtime ([`myelin_events::Consumer`], the seven encoded rules) +
//! the per-consumer [`myelin_events::DedupLedger`] (contract 2.5, idempotent on `event_id`). It:
//!
//! - **whitelists** `refs.edge.>` plus the typed-lifecycle subjects `issue.relation.>` and
//!   `knowledge.page.>` — **NEVER `*`** (it is one of the explicitly reviewed firehose-class infra
//!   consumers, BUS-4; an over-broad subscription head-of-line-blocks everything, BUS-3);
//! - **upserts on `*.created`** (`ON CONFLICT DO NOTHING/UPDATE` — idempotent via the deterministic
//!   [`edge_id`]; the same edge replayed twice is one row);
//! - **soft-deletes on `*.removed`** (tombstones the edge — a removed edge is not seen by the
//!   `edge_inbound` `WHERE NOT tombstoned` index, but its row is retained for audit/provenance);
//! - **tombstones on `*.erased`** (the erasure path — §4.6 ladder; the REAL crypto-shred body is
//!   REF-P15, this is the tombstone the erasure consumer drives);
//! - writes **`source_root`/`target_root`** by [`myelin_refs::strip_sub`] (REF-P1 — backlinks roll
//!   up to the `#sub`-stripped root);
//! - **acks after apply** (the runtime's rule 2 — the cursor advances only on a terminal `Done`);
//! - is **idempotent on `event_id`** via `consumer_dedup` (rule 1) AND on the deterministic
//!   `edge_id` (the upsert) — belt and braces.
//!
//! ## Steady-state == cold-rebuild — ONE code path (REF-D4, no drift)
//! [`RefsEdgeBuilder::project`] is the single ingest step. A live `refs.edge.created` and a
//! reindex-from-source `*.snapshot` replay (contract 2.6) BOTH flow through it — the handler does NOT
//! branch on cold-vs-live, because a `*.snapshot` carries the SAME envelope shape as a live event
//! (only its `event_id` is deterministic from `(aggregate, version)` so a re-run converges,
//! [`myelin_events::snapshot_event_id`]). There is **NO** "load the edge table from an owner's DB"
//! backdoor — the only way a row lands here is the live consumer path (the no-cross-db floor). The
//! cold-rebuild parity drill (REF-D4) rests on this: replay the same log → byte-identical index.
//!
//! ## Telemetry — `index_lag` (contract 1.8; observability is part of the pass)
//! [`RefsEdgeBuilder::index_lag`] is the live `refs.index_lag` sample
//! ([`RefsEdgeBuilder::INDEX_LAG_SIGNAL`]): events delivered to the builder but not yet projected
//! into the edge index. Bumped on entry to [`project`], cleared on apply, so a drill that pauses
//! mid-flight reads it non-zero (the SLO is the time-to-project). No signal == failed drill.
//!
//! ## Floors named (VISION §3 / prompt DoD)
//! - **The builder INGESTS; it does NOT invalidate the projection cache.** The `*.updated`/`*.erased`
//!   cache invalidation it would drive is **REF-P7**'s refs-projection-invalidator (over the no-op
//!   cache shim); a LIVE cache is **REF-P12**. Named so ingestion is not mistaken for a live
//!   projection — nothing here busts a cache yet.
//! - **The edge projection store is MODELLED in-memory here** ([`EdgeProjection`]); it is the §3.2
//!   `edge` table's semantics (deterministic-`edge_id` PK, tenant-partitioned, upsert/tombstone)
//!   byte-for-byte. The REAL `INSERT … ON CONFLICT` against the per-tenant-DEK-encrypted Postgres
//!   `edge` table (the REF-P5 schema) — executed in the SAME transaction as the `consumer_dedup`
//!   mark (the atomicity that makes idempotency real, not best-effort) — lands when the OLTP edge
//!   store is wired into `serve` (the REF-P5 migration + the storage `PgStore` pool; named, not
//!   silently skipped). The seam shape (the `EventHandler`, the deterministic `edge_id`, the
//!   `source_root`/`target_root` derivation, the `index_lag` signal) does NOT change. The REF-D7
//!   ingest-half (the producer-crash / outbox emit-iff-committed) is proven against the live stack in
//!   `tests/integration_ref_p6_edge_builder.rs` (the `integration` feature).
//! - **Mutation floor (mandatory-core).** The edge-builder decision logic — the deterministic
//!   `edge_id` derivation, the created/removed/erased branch, the `source_root`/`target_root`
//!   strip — is the mutation-tested core. The floor is stated + met by the unit + chained tests
//!   below (every branch + every derived column is asserted; a mutant that flips a branch or drops a
//!   derivation is caught). The world-scale ingest-under-load drill (REF-D-load) is a later band.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_refs::{strip_sub, ArtifactRef};
use myelin_tenancy::{Region, TenantId};

/// The `*`-free subject whitelist (rule 3, BUS-3/BUS-4): the builder consumes exactly the edge log
/// (`refs.edge.>`) plus the two typed-lifecycle subjects it mirrors as `lifecycle`-class edges
/// (`issue.relation.>`, `knowledge.page.>`). NEVER `*` — an over-broad subscription head-of-line
/// blocks the whole consumer. These are subject PREFIXES (the `Subscription`/`Consumer` prefix
/// model); the dotted event TYPE on the envelope (`refs.edge.created`, …) is what `project` branches
/// on. (The builder is one of the explicitly reviewed firehose-class infra consumers — BUS-4.)
pub static EDGE_BUILDER_SUBJECTS: &[SubjectPattern] = &[];

/// The durable consumer name (rule 4: bind-by-name; re-bound identically on reconnect so the SAME
/// dedup ledger + cursor are re-used → 0 lost across reconnect). PII-free identifier.
pub const EDGE_BUILDER_CONSUMER: &str = "refs-edge-builder";

/// The subject-prefix whitelist the builder binds through [`myelin_events::ConsumerSpec`] (the ONE
/// sanctioned entry-point). Exactly the edge log + the two typed-lifecycle subjects; NEVER `*`.
/// `consume(...)` rejects a `*`/empty subject loudly at registration. Returned as `&str` prefixes so
/// the service `serve` wires the consumer through the sanctioned [`myelin_events::consume`] path.
pub const EDGE_BUILDER_SUBJECT_PREFIXES: &[&str] =
    &["refs.edge.", "issue.relation.", "knowledge.page."];

/// The `rel_class` of an edge (§3.2/§3.3): `reference` (Refs-authoritative — the structured content
/// nodes `mentions`/`links`/`embeds`) or `lifecycle` (the TE-7 typed-edge mirror — issue relations,
/// page parent/child). The builder stamps this from the source subject family (a `refs.edge.*` event
/// is `reference`; an `issue.relation.*` / `knowledge.page.*` event is `lifecycle`). PII-free token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelClass {
    /// A structured content-node reference (`mentions`/`links`/`embeds`) — Refs-authoritative (§4.1).
    Reference,
    /// A typed lifecycle relation mirrored from the owning subsystem's typed table (TE-7; §3.3).
    Lifecycle,
}

impl RelClass {
    /// The frozen `rel_class` column token (`'reference'` | `'lifecycle'`, §3.2 CHECK vocabulary).
    pub fn as_str(self) -> &'static str {
        match self {
            RelClass::Reference => "reference",
            RelClass::Lifecycle => "lifecycle",
        }
    }
}

/// **The deterministic `edge_id` (§4.1; the idempotent-rebuild anchor).**
/// `edge_id = hash(tenant, source, target, rel)` — replay/redelivery of the SAME logical edge
/// upserts the SAME row (`ON CONFLICT DO NOTHING/UPDATE`), so idempotent rebuild is free and
/// steady-state == cold-rebuild. The hash is over the FULL `source`/`target` URNs (the `#sub` anchor
/// is part of the edge identity — "this message embeds block b9 of page 7c2" is a distinct edge from
/// one embedding the whole page) + the tenant (so two tenants' identical refs never collide) + the
/// `rel` token. PII-free: a content-addressed hash of opaque refs, never personal data.
///
/// The hash is rendered as a stable lowercase-hex string (a 16-byte FNV-1a digest, deterministic +
/// allocation-light + no external crypto dep — the edge_id is an idempotency key, NOT a security
/// primitive, so a fast non-cryptographic hash is correct here). The SAME `(tenant, source, target,
/// rel)` always yields the SAME id across processes/replays (no salt, no randomness).
pub fn edge_id(tenant: &TenantId, source: &str, target: &str, rel: &str) -> String {
    // FNV-1a over the length-prefixed, NUL-joined tuple so no field boundary is ambiguous (a field
    // can never absorb the next one's bytes — `("a", "bc")` and `("ab", "c")` hash distinctly).
    let mut h: u128 = 0x6c62272e07bb014262b821756295c58d; // FNV-1a 128-bit offset basis.
    const PRIME: u128 = 0x0000000001000000000000000000013b; // FNV-1a 128-bit prime.
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u128;
            h = h.wrapping_mul(PRIME);
        }
        // a NUL separator after each field so concatenation is unambiguous (fields are NUL-free URNs).
        h ^= 0x00;
        h = h.wrapping_mul(PRIME);
    };
    feed(tenant.0.as_bytes());
    feed(source.as_bytes());
    feed(target.as_bytes());
    feed(rel.as_bytes());
    format!("{h:032x}")
}

/// A materialised `edge` row (the §3.2 projection shape, modelled in-memory). The deterministic
/// `edge_id` is the PK; `source_root`/`target_root` are the `#sub`-stripped roots (the index keys).
/// `tombstoned` is the soft-delete flag (removed/erased). References-not-payloads: every field is an
/// opaque ref/token; `origin_actor` is a PSEUDONYMOUS Principal ref (erasure-safe — Refs never holds
/// the name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeRow {
    /// The deterministic `edge_id = hash(tenant, source, target, rel)` (the PK).
    pub edge_id: String,
    /// The FULL `#sub` source URN (the referencing side).
    pub source: ArtifactRef,
    /// The `#sub`-stripped source root (the outbound index key + the C-4 `SetExpr` filter column).
    pub source_root: ArtifactRef,
    /// The FULL `#sub` target URN (the referenced side).
    pub target: ArtifactRef,
    /// The `#sub`-stripped target root (the hot inbound index key).
    pub target_root: ArtifactRef,
    /// The edge relation token (`mentions`/`links`/`embeds` | lifecycle rels, §3.3).
    pub rel: String,
    /// `reference` (Refs-authoritative) | `lifecycle` (TE-7 mirror).
    pub rel_class: RelClass,
    /// The provenance event id (audit) — which log event wrote this row.
    pub origin_event: String,
    /// The PSEUDONYMOUS Principal ref that authored the edge (erasure-safe; never the name).
    pub origin_actor: String,
    /// The consistency token at edge-write time (§4.4).
    pub zookie: Option<String>,
    /// The erasure/deletion soft-delete flag (§4.6) — a tombstoned edge is hidden from the live
    /// `edge_inbound WHERE NOT tombstoned` index but retained for audit/provenance.
    pub tombstoned: bool,
}

/// The `(tenant, region)`-partition key — every read/write is tenant-first (the tenant-predicate /
/// no-cross-tenant-query-path floor; §3 "no cross-tenant query path"). PII-free: opaque partition
/// tokens.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PartKey {
    tenant: TenantId,
    region: Region,
}

/// **The `edge` inverse-index projection (R1, §3.7) — modelled in-memory.** A cloneable handle over
/// shared, tenant-partitioned state keyed by the deterministic `edge_id`. The ONLY writer is the
/// [`RefsEdgeBuilder`] consuming the edge log (steady-state) / a reindex-from-source replay
/// (cold-rebuild) — ONE code path, no owner-DB backdoor. The REAL per-tenant-DEK-encrypted Postgres
/// `edge` table (REF-P5) replaces this in-memory model when the store is wired into `serve` (named
/// floor; the seam shape does not change).
#[derive(Clone, Default)]
pub struct EdgeProjection {
    inner: Arc<Mutex<HashMap<PartKey, HashMap<String, EdgeRow>>>>,
}

impl EdgeProjection {
    /// A fresh, empty edge projection.
    pub fn new() -> EdgeProjection {
        EdgeProjection::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PartKey, HashMap<String, EdgeRow>>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **Upsert one edge row (idempotent on the deterministic `edge_id` — the §4.1 `ON CONFLICT`).**
    /// A re-upsert of the SAME `(tenant, source, target, rel)` lands on the SAME `edge_id`, so a
    /// replay/redelivery is one row (not a duplicate). The upsert REVIVES a previously-tombstoned
    /// edge (a removed-then-recreated edge is live again) — the chained-mutation
    /// created→removed→created path asserts this. Built tenant-first (the tenant-predicate floor).
    pub fn upsert(&self, tenant: &TenantId, region: &Region, row: EdgeRow) {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut inner = self.lock();
        inner
            .entry(pk)
            .or_default()
            .insert(row.edge_id.clone(), row);
    }

    /// **Soft-delete (tombstone) the edge `edge_id` (the `*.removed`/`*.erased` path, §4.6).** Sets
    /// `tombstoned = true` on the existing row (retained for audit/provenance, hidden from the live
    /// inbound index). A tombstone of an absent edge is a no-op (the edge was never built / already
    /// gone — idempotent; a redelivered `*.removed` never errors). The `origin_event` is advanced to
    /// the removing event for provenance. Tenant-first.
    pub fn tombstone(&self, tenant: &TenantId, region: &Region, edge_id: &str, origin_event: &str) {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut inner = self.lock();
        if let Some(part) = inner.get_mut(&pk) {
            if let Some(row) = part.get_mut(edge_id) {
                row.tombstoned = true;
                row.origin_event = origin_event.to_string();
            }
        }
    }

    /// The (live, non-tombstoned) edge row for `edge_id` in the `(tenant, region)` partition, if any.
    /// Tenant-first (no cross-tenant read path).
    pub fn get(&self, tenant: &TenantId, region: &Region, edge_id: &str) -> Option<EdgeRow> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock().get(&pk).and_then(|p| p.get(edge_id).cloned())
    }

    /// The count of LIVE (non-tombstoned) edges in the `(tenant, region)` partition (the cold-rebuild
    /// parity check reads this — a replayed log rebuilds the SAME live-edge set).
    pub fn live_count(&self, tenant: &TenantId, region: &Region) -> usize {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock()
            .get(&pk)
            .map(|p| p.values().filter(|r| !r.tombstoned).count())
            .unwrap_or(0)
    }

    /// The TOTAL row count (live + tombstoned) in the `(tenant, region)` partition — the rebuild
    /// parity check reads this so a cold rebuild reproduces the SAME rows (incl. tombstones) the
    /// steady-state path held (REF-D4 byte-parity).
    pub fn total_count(&self, tenant: &TenantId, region: &Region) -> usize {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock().get(&pk).map(|p| p.len()).unwrap_or(0)
    }

    /// **The canonical byte-image of the `(tenant, region)` partition (the REF-D4 reindex-parity
    /// comparison reads this — §4.7).** Serialises EVERY row (live AND tombstoned) in a DETERMINISTIC
    /// order (ascending `edge_id`) into a stable byte string, so two projections built by different
    /// paths (steady-state ingestion vs a cold reindex-from-source rebuild) are byte-identical **iff**
    /// they hold the same rows. This is the §4.7 "the rebuilt index byte-matches the live index"
    /// equality the reindex drill asserts on. It is a PURE function of the projection state (no clock,
    /// no randomness), so a re-run reproduces the same bytes.
    ///
    /// **`origin_event` is DELIBERATELY EXCLUDED** from the parity image. It is the PROVENANCE id of
    /// the log event that wrote the row — and a `*.snapshot` re-emit carries a DIFFERENT (deterministic
    /// `snap-…`) `event_id` than the original live event by construction (§4.7: the snapshot's id is
    /// `snapshot_event_id(aggregate, version)`, NOT the live ULID). So including `origin_event` would
    /// make a cold rebuild NEVER byte-match a live index, which is wrong — the architecture's
    /// "byte-matches the live index" is over the EDGE CONTENT the index serves (the edge identity +
    /// endpoints + derived roots + rel + class + actor + zookie + tombstone), the part that MUST be
    /// reproduced identically, NOT the per-write provenance id that legitimately differs between a live
    /// event and its replayed snapshot. (DEVIATION from a naive "every column" reading of §4.7, per
    /// EI-01 §1: documented here + in the reindex module; the parity property is content-equality, the
    /// recovery guarantee callers actually depend on.) Tenant-first (no cross-tenant read path);
    /// PII-free (every field is an opaque ref/token/pseudonymous id).
    pub fn parity_bytes(&self, tenant: &TenantId, region: &Region) -> Vec<u8> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut rows: Vec<EdgeRow> = self
            .lock()
            .get(&pk)
            .map(|p| p.values().cloned().collect())
            .unwrap_or_default();
        rows.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
        // A stable, unambiguous canonical encoding: one NUL-joined record per row, the records
        // newline-joined. Every field is rendered so no field boundary is ambiguous (the fields are
        // NUL-free URNs/tokens). Deterministic over the `edge_id`-sorted row vector.
        let mut out = Vec::new();
        for r in &rows {
            let rec = format!(
                "{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
                r.edge_id,
                r.source.0,
                r.source_root.0,
                r.target.0,
                r.target_root.0,
                r.rel,
                r.rel_class.as_str(),
                r.origin_actor,
                r.zookie.as_deref().unwrap_or(""),
                r.tombstoned,
            );
            out.extend_from_slice(rec.as_bytes());
            out.push(b'\n');
        }
        out
    }

    /// A stable parity HASH of the `(tenant, region)` partition (the green-artifact the reindex drill
    /// emits, §4.7 — "the reindex-parity hash"). A BLAKE3 digest of [`parity_bytes`], hex-rendered —
    /// the SAME `blake3:<hex>` content-address convention the BlobStore + the GDPR receipt + the audit
    /// Merkle leaf use (ONE convention, not a second one). Two paths agree on the bytes ⇒ agree on the
    /// hash; a single differing row flips it. PII-free (a hash over opaque refs).
    pub fn parity_hash(&self, tenant: &TenantId, region: &Region) -> String {
        let bytes = self.parity_bytes(tenant, region);
        format!("blake3:{}", blake3::hash(&bytes).to_hex())
    }

    /// **Wipe every row in the `(tenant, region)` partition (the cold-rebuild precondition — §4.7).**
    /// The reindex-from-source drill WIPES the derived index, then rebuilds it ONLY from the owner's
    /// replayed `*.snapshot` log through the live consumer — there is NO "reload from an owner DB"
    /// backdoor. This models the `TRUNCATE`/drop of the per-tenant `edge` partition before a recovery
    /// rebuild. Tenant-first (a wipe NEVER touches another tenant's partition).
    pub fn wipe_partition(&self, tenant: &TenantId, region: &Region) {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock().remove(&pk);
    }

    /// **Locate every edge naming `subject_id` as its `origin_actor` in the `(tenant, region)`
    /// partition (the REF-P15 holder `locate` query — §4.6).** Refs holds the subject ONLY as the
    /// PSEUDONYMOUS `origin_actor` opaque id (never the name), so "locate the subject's Refs data" is
    /// "the edges this opaque actor authored". Tenant-first (no cross-tenant path). Returns live AND
    /// tombstoned rows (a `locate` reports everything the subject touches; the audit row is retained).
    /// Deterministic order by `edge_id`. The opaque id is matched, never a name — erasure-safe.
    pub fn edges_by_actor(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject_id: &str,
    ) -> Vec<EdgeRow> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut rows: Vec<EdgeRow> = self
            .lock()
            .get(&pk)
            .map(|p| {
                p.values()
                    .filter(|r| r.origin_actor == subject_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
        rows
    }

    /// **The total count of edges naming `subject_id` (the `locate` cardinality the receipt records).**
    /// Tenant-first. PII-free: a count over an opaque-actor match.
    pub fn count_by_actor(&self, tenant: &TenantId, region: &Region, subject_id: &str) -> usize {
        self.edges_by_actor(tenant, region, subject_id).len()
    }

    /// **The LIVE inbound edges to `target_root` in the `(tenant, region)` partition (the
    /// `edge_inbound WHERE NOT tombstoned` range scan — §3.2).** This is the row source the REF-P11
    /// permission-filtered backlink read scans ONCE: tenant-first (no cross-tenant path), live edges
    /// only, keyed on the §3.2 stored `target_root` column (so "all backlinks to this artifact AND its
    /// sub-artifacts" is one range scan, not a `LIKE` prefix). Ordering is deterministic by `edge_id`
    /// for the in-memory model (the real Postgres `edge` table uses `ORDER BY created_at DESC`; the
    /// in-memory model has no `created_at` column — documented). The SetExpr/pagination are applied by
    /// [`crate::backlinks`] over this set — this method does NOT filter by permission (the permission
    /// filter is the lowered SetExpr the backlink read conjoins; this is the unfiltered candidate
    /// range the conjoin runs over, never returned raw to a caller).
    pub fn inbound_live(
        &self,
        tenant: &TenantId,
        region: &Region,
        target_root: &ArtifactRef,
    ) -> Vec<EdgeRow> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut rows: Vec<EdgeRow> = self
            .lock()
            .get(&pk)
            .map(|p| {
                p.values()
                    .filter(|r| !r.tombstoned && &r.target_root == target_root)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        // Deterministic order (the real table orders by `created_at DESC`; the in-memory model has no
        // timestamp, so order by the deterministic `edge_id` for a stable, reproducible scan).
        rows.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
        rows
    }

    /// **The LIVE outbound edges from `source_root` in the `(tenant, region)` partition (the
    /// `edge_outbound WHERE NOT tombstoned` range scan — §3.2).** This is the adjacency-list step the
    /// REF-P13 recursive-CTE traverse takes at each hop: "the edges DEPARTING this node" (so the
    /// multi-hop walk follows `source_root → target_root`). Tenant-first (no cross-tenant path), live
    /// edges only, keyed on the §3.2 stored `source_root` column (the `edge_outbound` index, §3.4).
    /// Deterministic order by `edge_id` (the in-memory model has no `created_at`; the real Postgres
    /// `edge` table uses the index order — documented). This is the UNFILTERED adjacency step; the
    /// traverse applies the `rel`/`rel_class` filter + the ONE `list_objects` post-filter over the
    /// COLLECTED node set (NOT per-hop) — see [`crate::traverse`].
    pub fn outbound_live(
        &self,
        tenant: &TenantId,
        region: &Region,
        source_root: &ArtifactRef,
    ) -> Vec<EdgeRow> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut rows: Vec<EdgeRow> = self
            .lock()
            .get(&pk)
            .map(|p| {
                p.values()
                    .filter(|r| !r.tombstoned && &r.source_root == source_root)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
        rows
    }
}

/// Why a `refs.edge.*` / typed-lifecycle event could not be projected — a structurally-malformed
/// event (a missing `source`/`target`/`rel`) is a LOUD, non-retryable poison, NEVER a silent
/// corruption of the index (fail-closed; EI-01 §5). The reason names the exact field missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectError(pub String);

/// **The refs-edge-builder consumer (REF-P6; contract 5.4 consumer side).** An ordinary
/// [`EventHandler`] over the [`EdgeProjection`]. Cloneable handle (the projection is shared). The
/// `index_lag` (contract 1.8) is live + observable.
#[derive(Clone)]
pub struct RefsEdgeBuilder {
    projection: EdgeProjection,
    /// The live `refs.index_lag` measurement (contract 1.8): events delivered but not yet projected.
    /// 0 in steady state on the synchronous apply path; bumped on entry / cleared on apply.
    index_lag: Arc<AtomicU64>,
}

impl RefsEdgeBuilder {
    /// The telemetry signal name this builder emits (contract 1.8). A named constant — drills assert
    /// against the NAME, never a literal (EI-01 §3 observability).
    pub const INDEX_LAG_SIGNAL: &'static str = "refs.index_lag";

    /// Build the refs-edge-builder over `projection` (the edge inverse index it feeds).
    pub fn new(projection: EdgeProjection) -> RefsEdgeBuilder {
        RefsEdgeBuilder {
            projection,
            index_lag: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The edge projection this builder feeds (read access for the backlink read / tests / a reindex
    /// parity check).
    pub fn projection(&self) -> &EdgeProjection {
        &self.projection
    }

    /// The live `refs.index_lag` sample (contract 1.8): events delivered to the builder but not yet
    /// projected into the edge index. 0 in steady state (the synchronous apply cleared it); a drill
    /// that pauses mid-`project` reads it non-zero.
    pub fn index_lag(&self) -> u64 {
        self.index_lag.load(Ordering::SeqCst)
    }

    /// **Project ONE edge-log / typed-lifecycle event into the index (the ONE ingest step).**
    /// Factored out of [`EventHandler::handle`] so a reindex-from-source replay (contract 2.6) /
    /// a drill can drive it directly — this is what makes **steady-state == cold-rebuild a single
    /// code path** (REF-D4): a live `refs.edge.created` and a `*.snapshot` replay both flow through
    /// HERE, the handler never branches cold-vs-live. Bumps `index_lag` on entry, clears on apply.
    ///
    /// The dotted event TYPE drives the branch (`*.created`/`*.relation.created`/`*.page.*` → upsert;
    /// `*.removed` → tombstone; `*.erased` → tombstone). The `source`/`target`/`rel`/`zookie` ride the
    /// references-not-payloads payload; the `(tenant, region)` partition + `origin_actor` come off the
    /// envelope (first-class). A structurally-malformed event is a non-retryable poison
    /// ([`ProjectError`]) — never a silent corruption.
    pub fn project(&self, ev: &EventEnvelope) -> Result<(), ProjectError> {
        self.index_lag.fetch_add(1, Ordering::SeqCst);
        let result = self.project_inner(ev);
        self.index_lag.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn project_inner(&self, ev: &EventEnvelope) -> Result<(), ProjectError> {
        let type_ = ev.type_.0.as_str();
        let event_name = type_.rsplit('.').next().unwrap_or("");

        match event_name {
            // The erasure path (§4.6): tombstone the edge(s) the erased subject authored/targets.
            // Driven by `*.erased` through the SAME live consumer (no erasure backdoor). The REAL
            // crypto-shred body is REF-P15; here the tombstone is the structural soft-delete.
            "erased" => self.apply_erased(ev),
            // The removal path: soft-delete (tombstone) the named edge.
            "removed" => self.apply_removed(ev),
            // The build path: upsert on the deterministic edge_id. `refs.edge.created` →
            // reference-class; `issue.relation.*` / `knowledge.page.*` → lifecycle-class (TE-7).
            // A `*.snapshot` reindex event carries the SAME `created`-shaped payload, so it ingests
            // here identically (cold == live).
            "created" | "set" | "parent_set" | "updated" | "snapshot" => self.apply_created(ev),
            // Any other event name on the whitelisted subjects is a no-op (defence-in-depth: the
            // whitelist binds the subject families, but an unrecognised typed event is ignored, never
            // mis-projected). A `knowledge.page.created` with no edge payload (a page with no
            // structured ref) is a valid no-op write.
            _ => Ok(()),
        }
    }

    /// The reference/lifecycle class of an event by its subsystem family (the source subject):
    /// `refs.edge.*` → `Reference`; `issue.relation.*` / `knowledge.page.*` → `Lifecycle` (TE-7).
    fn rel_class_of(type_: &str) -> RelClass {
        if type_.starts_with("refs.edge.") {
            RelClass::Reference
        } else {
            RelClass::Lifecycle
        }
    }

    fn apply_created(&self, ev: &EventEnvelope) -> Result<(), ProjectError> {
        let p = &ev.payload;
        // An edge-bearing event MUST carry source/target/rel; a typed-lifecycle event that carries
        // none (e.g. a `knowledge.page.created` with no structured ref) is a valid no-op write — it
        // is NOT a poison (a page can have no edges). Distinguish: only the explicit edge subjects
        // (`refs.edge.*`) REQUIRE the triple; a lifecycle subject with no edge payload is skipped.
        let has_edge_payload = p.get("source").is_some() || p.get("target").is_some();
        let is_edge_subject = ev.type_.0.starts_with("refs.edge.");
        if !has_edge_payload {
            if is_edge_subject {
                return Err(ProjectError(format!(
                    "{} carries no edge payload (source/target/rel)",
                    ev.type_.0
                )));
            }
            return Ok(()); // a lifecycle event with no edge is a no-op, not a poison.
        }

        let source = str_field(p, "source").ok_or_else(|| {
            ProjectError(format!("{} edge payload is missing `source`", ev.type_.0))
        })?;
        let target = str_field(p, "target").ok_or_else(|| {
            ProjectError(format!("{} edge payload is missing `target`", ev.type_.0))
        })?;
        let rel = str_field(p, "rel")
            .ok_or_else(|| ProjectError(format!("{} edge payload is missing `rel`", ev.type_.0)))?;

        let source_ref = ArtifactRef(source.clone());
        let target_ref = ArtifactRef(target.clone());
        let rel_class = Self::rel_class_of(ev.type_.0.as_str());

        let row = EdgeRow {
            edge_id: edge_id(&ev.tenant, &source, &target, &rel),
            source_root: strip_sub(&source_ref),
            target_root: strip_sub(&target_ref),
            source: source_ref,
            target: target_ref,
            rel,
            rel_class,
            origin_event: ev.event_id.0.clone(),
            // PSEUDONYMOUS Principal ref — the opaque actor id, never the name (erasure-safe, §4.6).
            // PROVENANCE-PRESERVING across a reindex: a `*.snapshot` re-emit carries the ORIGINAL
            // author in the payload `origin_actor` (the reindex DRIVER's principal is the envelope
            // actor, NOT the edge's author). So prefer the payload's `origin_actor` if present (the
            // snapshot path), falling back to the envelope actor (the live `refs.edge.created` path,
            // where the emitting principal IS the author). This is what makes the rebuilt index
            // byte-match the live index on the authorship column — and keeps erasure-by-actor
            // (`edges_by_actor`) correct after a recovery rebuild (§4.6/§4.7).
            origin_actor: str_field(p, "origin_actor")
                .unwrap_or_else(|| ev.actor.0.principal_id.0.clone()),
            zookie: str_field(p, "zookie"),
            tombstoned: false,
        };
        self.projection.upsert(&ev.tenant, &ev.region, row);
        Ok(())
    }

    fn apply_removed(&self, ev: &EventEnvelope) -> Result<(), ProjectError> {
        let p = &ev.payload;
        // A `*.removed` names the edge to tombstone — either directly by `edge_id`, or by the
        // `(source, target, rel)` triple (the builder derives the SAME deterministic id).
        let id = if let Some(id) = str_field(p, "edge_id") {
            id
        } else {
            let source = str_field(p, "source").ok_or_else(|| {
                ProjectError(format!(
                    "{} removal is missing `edge_id`/`source`",
                    ev.type_.0
                ))
            })?;
            let target = str_field(p, "target").ok_or_else(|| {
                ProjectError(format!(
                    "{} removal is missing `edge_id`/`target`",
                    ev.type_.0
                ))
            })?;
            let rel = str_field(p, "rel").ok_or_else(|| {
                ProjectError(format!("{} removal is missing `edge_id`/`rel`", ev.type_.0))
            })?;
            edge_id(&ev.tenant, &source, &target, &rel)
        };
        self.projection
            .tombstone(&ev.tenant, &ev.region, &id, &ev.event_id.0);
        Ok(())
    }

    fn apply_erased(&self, ev: &EventEnvelope) -> Result<(), ProjectError> {
        // `*.erased` drives the §4.6 tombstone (NO erasure backdoor — through the SAME live path).
        // The erased event names the edge(s) to tombstone the same way a removal does; the REAL
        // crypto-shred over cache PII is REF-P15. Reuse the removal logic (tombstone-by-id-or-triple).
        self.apply_removed(ev)
    }
}

/// Read a string field from a references-not-payloads payload, or `None` if absent / non-string.
fn str_field(payload: &serde_json::Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

impl EventHandler for RefsEdgeBuilder {
    /// The `*`-free subject whitelist (rule 3): `refs.edge.>` + the typed-lifecycle subjects. The
    /// `'static` slice the trait requires; the service `serve` binds the runtime through the
    /// sanctioned [`myelin_events::consume`] with [`EDGE_BUILDER_SUBJECT_PREFIXES`] (which the runtime
    /// rejects if `*`). NEVER `*` (BUS-3/BUS-4).
    fn subjects(&self) -> &'static [SubjectPattern] {
        EDGE_BUILDER_SUBJECTS
    }

    /// Project the delivered edge-log / typed-lifecycle event into the index (contract 2.4).
    /// Idempotent on the deterministic `edge_id` (the upsert) AND on `event_id` (the runtime's
    /// `consumer_dedup` outer guard, rule 1) — belt and braces. A structurally-malformed event is a
    /// non-retryable poison ([`HandleOutcome::NonRetryable`]) — never a silent corruption.
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
        match self.project(ev) {
            Ok(()) => HandleOutcome::Done,
            Err(ProjectError(reason)) => HandleOutcome::NonRetryable(Reason(reason)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-1".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }

    /// An edge event with a references-not-payloads payload. `type_` drives the branch; the payload
    /// carries source/target/rel/zookie.
    fn edge_event(id: &str, type_: &str, source: &str, target: &str, rel: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
            subject: ArtifactRef(source.into()),
            aggregate: AggregateKey(format!("edge:{source}->{target}")),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: None,
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload: serde_json::json!({ "source": source, "target": target, "rel": rel, "zookie": "zk-1" }),
        }
    }

    // --- The deterministic edge_id (§4.1) ---

    /// **`edge_id = hash(tenant, source, target, rel)` is deterministic + collision-distinct.** The
    /// SAME tuple always yields the SAME id (idempotent rebuild); a change in ANY field changes it;
    /// field boundaries are unambiguous (`("ab","c")` ≠ `("a","bc")`).
    #[test]
    fn edge_id_is_deterministic_and_field_unambiguous() {
        let t = tenant();
        let a = edge_id(&t, "s", "t", "mentions");
        assert_eq!(
            a,
            edge_id(&t, "s", "t", "mentions"),
            "the same tuple → the same id (idempotent)"
        );
        assert_ne!(
            a,
            edge_id(&t, "s", "t", "embeds"),
            "a different rel → a different id"
        );
        assert_ne!(
            a,
            edge_id(&t, "s2", "t", "mentions"),
            "a different source → a different id"
        );
        assert_ne!(
            a,
            edge_id(&TenantId("other".into()), "s", "t", "mentions"),
            "tenant-scoped id"
        );
        // field boundary: ("ab","c",…) must not collide with ("a","bc",…)
        assert_ne!(
            edge_id(&t, "ab", "c", "mentions"),
            edge_id(&t, "a", "bc", "mentions"),
            "field boundaries are unambiguous (NUL-separated)"
        );
    }

    // --- Upsert idempotency + source_root/target_root derivation (the mutation core) ---

    /// **Upsert on `created` is idempotent on the deterministic `edge_id` (one row), and derives
    /// `source_root`/`target_root` by strip_sub.** Replaying the SAME `refs.edge.created` twice
    /// leaves ONE row; the `#sub`-stripped roots are the index keys.
    #[test]
    fn created_upserts_one_row_and_derives_roots() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let src = "myelin://acme/chat/message/m1#block-9";
        let tgt = "myelin://acme/knowledge/page/7c2#block-3";
        let ev = edge_event("01J-1", "refs.edge.created", src, tgt, "embeds");

        assert_eq!(b.handle(&ev), HandleOutcome::Done);
        // replay the SAME event → still one row (idempotent on edge_id).
        assert_eq!(b.handle(&ev), HandleOutcome::Done);
        assert_eq!(
            b.projection().live_count(&tenant(), &region()),
            1,
            "idempotent: one row"
        );

        let id = edge_id(&tenant(), src, tgt, "embeds");
        let row = b
            .projection()
            .get(&tenant(), &region(), &id)
            .expect("the edge row exists");
        // the roots are the #sub-stripped parents (strip_sub, REF-P1).
        assert_eq!(
            row.source_root.0, "myelin://acme/chat/message/m1",
            "source_root strips #sub"
        );
        assert_eq!(
            row.target_root.0, "myelin://acme/knowledge/page/7c2",
            "target_root strips #sub"
        );
        // the FULL #sub URNs are retained (the edge identity is sub-precise).
        assert_eq!(row.source.0, src);
        assert_eq!(row.target.0, tgt);
        assert_eq!(row.rel, "embeds");
        assert_eq!(
            row.rel_class,
            RelClass::Reference,
            "refs.edge.* is reference-class"
        );
        // origin_actor is the PSEUDONYMOUS opaque principal id, never a name (erasure-safe).
        assert_eq!(row.origin_actor, "p-opaque-1");
        assert_eq!(row.zookie.as_deref(), Some("zk-1"));
        assert!(!row.tombstoned);
    }

    /// **A typed-lifecycle event (`issue.relation.created`) projects a `lifecycle`-class edge
    /// (TE-7).** The builder whitelists the typed subjects and mirrors them as lifecycle edges.
    #[test]
    fn typed_lifecycle_event_projects_a_lifecycle_class_edge() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let src = "myelin://acme/issue/issue/ENG-1";
        let tgt = "myelin://acme/issue/issue/ENG-2";
        let ev = edge_event("01J-rel", "issue.relation.created", src, tgt, "blocks");
        assert_eq!(b.handle(&ev), HandleOutcome::Done);
        let id = edge_id(&tenant(), src, tgt, "blocks");
        let row = b
            .projection()
            .get(&tenant(), &region(), &id)
            .expect("lifecycle edge exists");
        assert_eq!(
            row.rel_class,
            RelClass::Lifecycle,
            "issue.relation.* is lifecycle-class (TE-7)"
        );
    }

    /// **A `knowledge.page.created` with NO edge payload is a valid no-op (not a poison).** A page
    /// with no structured ref produces no edge — the builder must not poison on it.
    #[test]
    fn lifecycle_event_with_no_edge_payload_is_a_noop_not_poison() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let mut ev = edge_event("01J-pg", "knowledge.page.created", "x", "y", "z");
        ev.payload = serde_json::json!({ "title_ref": "r1" }); // no source/target/rel.
        assert_eq!(
            b.handle(&ev),
            HandleOutcome::Done,
            "no edge payload → no-op, not poison"
        );
        assert_eq!(
            b.projection().total_count(&tenant(), &region()),
            0,
            "no edge projected"
        );
    }

    /// **A `refs.edge.created` with a missing `source` is a LOUD non-retryable poison** (fail-closed
    /// — never a silent corruption of the index).
    #[test]
    fn malformed_edge_event_is_a_nonretryable_poison() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let mut ev = edge_event("01J-bad", "refs.edge.created", "s", "t", "mentions");
        ev.payload = serde_json::json!({ "target": "t", "rel": "mentions" }); // no source.
        match b.handle(&ev) {
            HandleOutcome::NonRetryable(Reason(r)) => {
                assert!(r.contains("source"), "names the field: {r}")
            }
            other => panic!("a malformed edge event must be a non-retryable poison, got {other:?}"),
        }
        assert_eq!(b.projection().total_count(&tenant(), &region()), 0);
    }

    // --- removed/erased tombstone idempotency ---

    /// **`removed` tombstones the edge (soft-delete); a redelivered removal is idempotent.** The row
    /// is retained (audit) but hidden from the live inbound count.
    #[test]
    fn removed_tombstones_and_is_idempotent() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let src = "myelin://acme/chat/message/m1";
        let tgt = "myelin://acme/issue/issue/ENG-1";
        b.handle(&edge_event(
            "01J-c",
            "refs.edge.created",
            src,
            tgt,
            "mentions",
        ));
        assert_eq!(b.projection().live_count(&tenant(), &region()), 1);

        // remove by the (source, target, rel) triple → tombstone.
        let rm = edge_event("01J-r", "refs.edge.removed", src, tgt, "mentions");
        assert_eq!(b.handle(&rm), HandleOutcome::Done);
        assert_eq!(
            b.projection().live_count(&tenant(), &region()),
            0,
            "tombstoned → hidden from live"
        );
        assert_eq!(
            b.projection().total_count(&tenant(), &region()),
            1,
            "row retained for audit"
        );
        // redelivered removal is a no-op (idempotent).
        assert_eq!(b.handle(&rm), HandleOutcome::Done);
        assert_eq!(b.projection().total_count(&tenant(), &region()), 1);

        let id = edge_id(&tenant(), src, tgt, "mentions");
        assert!(
            b.projection()
                .get(&tenant(), &region(), &id)
                .unwrap()
                .tombstoned
        );
    }

    /// **A tombstone of an edge that was never built is a no-op (idempotent), not an error.** A
    /// `*.removed`/`*.erased` that arrives before/without its `*.created` never poisons.
    #[test]
    fn tombstone_of_absent_edge_is_a_noop() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let rm = edge_event("01J-r", "refs.edge.removed", "s", "t", "mentions");
        assert_eq!(
            b.handle(&rm),
            HandleOutcome::Done,
            "removal of absent edge is a no-op"
        );
        assert_eq!(b.projection().total_count(&tenant(), &region()), 0);
    }

    /// **`*.erased` drives the §4.6 tombstone through the SAME live path (no erasure backdoor).**
    #[test]
    fn erased_tombstones_the_edge() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let src = "myelin://acme/chat/message/m1";
        let tgt = "myelin://acme/issue/issue/ENG-1";
        b.handle(&edge_event(
            "01J-c",
            "refs.edge.created",
            src,
            tgt,
            "mentions",
        ));
        let er = edge_event("01J-e", "chat.message.erased", src, tgt, "mentions");
        assert_eq!(b.handle(&er), HandleOutcome::Done);
        assert_eq!(
            b.projection().live_count(&tenant(), &region()),
            0,
            "erased → tombstoned"
        );
    }

    // --- index_lag telemetry (contract 1.8) ---

    /// **`index_lag` returns to 0 in steady state (the synchronous apply cleared it).** The signal
    /// NAME is the named constant (drills assert against the name, never a literal).
    #[test]
    fn index_lag_is_zero_in_steady_state_and_named() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        assert_eq!(b.index_lag(), 0, "a fresh builder has no lag");
        b.handle(&edge_event(
            "01J-1",
            "refs.edge.created",
            "s",
            "t",
            "mentions",
        ));
        assert_eq!(b.index_lag(), 0, "index_lag returns to 0 after projection");
        assert_eq!(
            RefsEdgeBuilder::INDEX_LAG_SIGNAL,
            "refs.index_lag",
            "the contract-1.8 signal name"
        );
    }
}
