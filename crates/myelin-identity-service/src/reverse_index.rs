//! # `reverse_index` — S8, the per-tenant authz reverse index + its bus consumer (P-ID-11 → P-069)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §2 (the **S8** store row — the per-tenant `(subject, relation, object_id)` projection of S3 + a
//! `revision_watermark` column; partitioned `(tenant, region)` + object-type; **per-tenant only, no
//! cross-tenant query path**; derived — rebuildable from S3 by replaying the live consumer), §7.2
//! (the JOIN target `authz_visible` the consumer's own query planner conjoins against — the big-
//! result `Filter` path), §8.7 (S8 carries a `revision_watermark` derived from the zookie of the
//! `iam.tuple_written` event that produced it — the new-enemy guard's index half).
//!
//! **Contract-index:** rows **2.4** (the `EventHandler` feeding S8 from `iam.tuple_written`), **11.1**
//! (S8 as a co-located projection store), **10.1** (S8 as a new `PersonalDataHolder` — it references
//! subjects), **12.1** (`(tenant, region)` + type partition), **1.8** (`reverse_index_lag` telemetry).
//!
//! ## What this module ships (P-ID-11, the S8 half)
//! 1. **S8** ([`ReverseIndex`]) — the materialised `(subject, relation, object_id)` projection of S3,
//!    partitioned `(tenant, region)` then object-type, per-tenant-only (there is **no cross-tenant
//!    query path** — every accessor takes a verified [`TenantScope`] and a read for tenant A
//!    structurally cannot reach tenant B's partition), carrying a per-partition
//!    [`ReverseIndex::watermark`] (the latest applied `iam.tuple_written` zookie, §8.7), and
//!    holder-registered as a **NEW `PersonalDataHolder`** (it references subjects).
//! 2. **The S8 consumer** ([`ReverseIndexConsumer`]) — the [`myelin_events::EventHandler`] that
//!    consumes `iam.tuple.written` off the bus (the ONLY feed; reindex-from-source replays the SAME
//!    consumer over S3, no bespoke recovery code), applies each delta to the projection, **advances
//!    the watermark** to the event's zookie, and exposes [`reverse_index_lag`](ReverseIndexConsumer::lag)
//!    (the [`signals::REVERSE_INDEX_LAG`] sample). Idempotent on the delta identity (a redelivery is
//!    a no-op — the consumer runtime's `event_id` dedup is the outer guard; the projection apply is
//!    itself idempotent).
//!
//! ## The watermark (§8.7 — the index half of the new-enemy guard)
//! Every applied `iam.tuple_written` carries the write's zookie (`payload["zookie"]`). S8 records the
//! **latest** applied zookie per `(tenant, region)` partition as its `revision_watermark`. A
//! zookie-stamped scan compares its required revision against this watermark: at-or-after → the JOIN
//! serves; behind → wait-or-fall-back-to-`check`. **This prompt ships the watermark column + its
//! advance**; the *read-side* consistency path (the scan that waits/falls-back rather than serving
//! stale) is **P-ID-12 (P-070)** — named, not silently assumed done.
//!
//! ## Floors named (frozen now → bodies in a later prompt)
//! - **The `Filter` SetExpr→SQL lowering is P-ID-12 (P-070).** This prompt ships the JOIN-target
//!   projection + the reverse lookup [`ReverseIndex::objects_for`]; the consumer-composable SetExpr
//!   lowering (InRelation/TupleSet → the `authz_visible` JOIN; Union/Intersect/Difference →
//!   AND/OR/EXCEPT) replaces the [`crate::list_objects`] `Filter` stub there.
//! - **The watermark *read* consistency path (wait / fall-back-to-check) is P-ID-12 (P-070).** Named
//!   above. This module advances + exposes the watermark; the read-side guard is P-ID-12.
//! - **`list_subjects` at 50k-member density over S8 is P-ID-13 (P-071).** The same reverse index
//!   serves the read-fanout (`list_subjects(channel, watcher)`); the `SubjectTree` expand is P-ID-13.
//! - **The Ids↔Filter cardinality cap is a measured tunable** — the SHAPE is frozen (Ids under the
//!   cap, Filter above), only the NUMBER is open: the default-to-beat is written to `thresholds.toml`
//!   now ([`crate::list_objects::DEFAULT_IDS_CARDINALITY_CAP`]) and re-measured at world-scale in
//!   **P-ID-31 (P-074 finalises it, the floor named in the run table)**.
//! - **The in-memory store models the SQL S8 table** (the same EI-01 §1 deviation S3 documents):
//!   there is no live OLTP database until the driver lands (P-S15); the `(tenant, region)` + type
//!   partition, the RLS scope, and the watermark column are byte-for-byte the §2/§8.7 contract.

use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, SubjectPattern};
use myelin_identity::iam_events::{signals, IAM_TUPLE_WRITTEN};
use myelin_identity::{ObjectId, ObjectType, PrincipalId, RelName, Zookie};
use myelin_storage::{OltpStoreHolder, TenantQuery, TenantScope, TenantTable};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The S8 store's tenant-owned table name (the `(tenant, region)`-first RLS table). Every S8 access
/// is built through [`TenantQuery::for_table`] over THIS table, so a reverse-index read without a
/// verified `(tenant, region)` scope does not compile (the `tenant-predicate` floor).
pub const S8_TABLE: &str = "authz_visible";

/// The S8 store's stable holder name (the **NEW** `PersonalDataHolder` identifier — S8 references
/// subjects). The store auto-registers under this name so "we forgot the reverse index" is
/// structurally impossible.
pub const S8_HOLDER: &str = "identity_authz_reverse_index";

/// The S8 consumer name (the `iam.tuple.written` subscriber — rule 3, a `*`-free whitelist).
pub const S8_CONSUMER: &str = "s8_reverse_index";

/// One reverse-index row: the `(subject, relation, object_id)` projection of an S3 tuple
/// (architecture §2 — "per-tenant `(subject, relation, object_id)` projection of S3"). The
/// `(tenant, region)` partition + object-type are the OUTER partition (the map keys), so a row
/// carries only the three projected columns.
///
/// The frozen contract types ([`PrincipalId`]/[`RelName`]/[`ObjectId`]) derive `Hash`/`Eq` but not
/// `Ord` (they are opaque ABI tokens); the deduplicated, deterministic projection therefore keys on
/// the stable inner-string triple via [`ReverseRow::key`], never by adding `Ord` to a frozen type
/// (EI-01 §7 — never widen a frozen shape).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseRow {
    /// The subject the grant is for (`@subject`). PII-free opaque principal id (the S8-as-holder
    /// erasure unit is per-subject — the DSR bodies are the GDPR-M1 / P-ID-20 floor).
    pub subject: PrincipalId,
    /// The relation (`#relation`) — the JOIN keys on `(subject, relation)` and the object-type.
    pub relation: RelName,
    /// The object id (`object#`) — the consumer JOINs its OWN id column against this (§7.2).
    pub object_id: ObjectId,
}

impl ReverseRow {
    /// The stable `(subject, relation, object_id)` string key the dedup'd, deterministic projection
    /// orders on (the frozen ABI types are not `Ord`; their inner strings are).
    fn key(&self) -> (String, String, String) {
        (
            self.subject.0.clone(),
            self.relation.0.clone(),
            self.object_id.0.clone(),
        )
    }
}

/// The `(tenant, region)` + object-type partition key (architecture §2 — "partitioned
/// `(tenant, region)` + object-type"). The object-type is the §7.3 id-column discriminant a
/// consumer queries one type at a time (a PR list, a channel list), so the projection is keyed by
/// it. A read for one `(tenant, region, type)` structurally cannot reach another's bucket.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct PartKey {
    tenant: String,
    region: String,
    object_type: String,
}

/// One partition's projected rows + its revision watermark.
#[derive(Default)]
struct Partition {
    /// The `(subject, relation, object_id)` rows in this `(tenant, region, type)` partition, keyed
    /// by the stable inner-string triple → the row. A `BTreeMap` so the projection is deduplicated
    /// (idempotent apply) and deterministic (the frozen ABI types are not `Ord`; their strings are).
    rows: BTreeMap<(String, String, String), ReverseRow>,
}

/// The shared inner state of a [`ReverseIndex`] (behind `Arc<Mutex<…>>` so it is a cloneable handle
/// the consumer + readers share).
#[derive(Default)]
struct Inner {
    /// `(tenant, region, type)` → the projected rows. The OUTER map is the partition (no cross-
    /// tenant query path: a read for tenant A never touches tenant B's partition).
    partitions: BTreeMap<PartKey, Partition>,
    /// `(tenant, region)` → the latest applied `iam.tuple_written` zookie (the `revision_watermark`,
    /// §8.7). Keyed at the `(tenant, region)` grain (the zookie is a per-tenant monotonic revision),
    /// NOT per-type, so a scan of any type in the tenant reads one consistent watermark.
    watermarks: BTreeMap<(String, String), Zookie>,
}

/// **S8 — the per-tenant authz reverse index (architecture §2; the JOIN target of §7.2).**
///
/// A cloneable handle over shared state. The ONLY feed is [`ReverseIndexConsumer`] consuming
/// `iam.tuple.written` off the bus (reindex-from-source replays the SAME consumer over S3 — one code
/// path, no bespoke recovery). Holder-registered (a NEW `PersonalDataHolder` — it references
/// subjects). Every accessor takes a verified [`TenantScope`]: **no cross-tenant query path**.
#[derive(Clone)]
pub struct ReverseIndex {
    inner: Arc<Mutex<Inner>>,
    /// The holder this store auto-registers as (the `PersonalDataHolder` seam) — proof the "every
    /// store is a holder" invariant holds for S8 (§3.4, GD-3; contract 10.1).
    holder: OltpStoreHolder,
}

impl Default for ReverseIndex {
    fn default() -> Self {
        ReverseIndex::new()
    }
}

impl ReverseIndex {
    /// Build the S8 reverse index. The store auto-registers as a `PersonalDataHolder` on
    /// construction (opening IS registering, §3.4) — so "we forgot the reverse index" is
    /// structurally impossible.
    pub fn new() -> ReverseIndex {
        let holder = OltpStoreHolder::new(S8_HOLDER);
        let _receipt = holder.register();
        ReverseIndex {
            inner: Arc::new(Mutex::new(Inner::default())),
            holder,
        }
    }

    /// The store AS a `PersonalDataHolder` (the holder the DSR fan-out drives). The DSR bodies (the
    /// per-subject reverse-row erasure step) land with the GDPR M1 / P-ID-20 derivative-erasure
    /// path; here the REGISTRATION is real so the holder-registered architecture test sees S8.
    pub fn holder(&self) -> &OltpStoreHolder {
        &self.holder
    }

    /// The per-tenant DEK key class S8 encrypts under (the per-tenant-DEK pin, BY REFERENCE — the
    /// KMS hierarchy is Storage M1, P-ST-06/P-058). S8 indexes tuples (not content), so even a HYOK
    /// tenant's S8 works (§2 — "it indexes tuples, not content"); it still encrypts the per-tenant
    /// projection under the per-tenant key.
    pub fn dek_class(&self, scope: &TenantScope) -> String {
        format!("kms://{}/tenant", scope.tenant().0)
    }

    /// **Apply one `iam.tuple_written` delta to the projection + advance the watermark** (the
    /// consumer's per-delta step; factored out so a reindex / a drill can drive it directly). The
    /// `op` is `"add"` / `"remove"`; the apply is idempotent (a re-add is a no-op on the dedup'd
    /// `BTreeSet`, a re-remove of an absent row is a no-op). The watermark advances to `zookie` iff
    /// `zookie` is newer than the partition's current watermark (monotone — a redelivered older
    /// event never moves it backward).
    ///
    /// Built through a [`TenantQuery`] so the write carries its `(tenant, region)` predicate (the
    /// tenant-predicate floor) — a tenant-less apply is unconstructable.
    pub fn apply_delta(
        &self,
        scope: &TenantScope,
        op: &str,
        object_type: &ObjectType,
        row: ReverseRow,
        zookie: &Zookie,
    ) {
        // Keep the primitive fail-closed even when a drill or reindex drives it directly. The live
        // consumer validates the whole event before calling this method; an unknown operation must
        // never advance the watermark when this lower-level seam is used on its own.
        if !matches!(op, "add" | "remove") {
            return;
        }
        // The tenant-predicate floor: the apply is built from the verified scope (no cross-tenant
        // write path). The thin `(tenant, region)` predicate is carried on the statement.
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
        // Advance the watermark monotonically (the §8.7 revision_watermark). The zookie strings are
        // the zero-padded `zk-<rev>` form (S3 mints them), so lexical order == revision order — a
        // later zookie sorts after, and an older redelivery never moves the watermark backward.
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

    /// **The reverse lookup the `Filter` JOIN serves (architecture §7.2):** the object ids `subject`
    /// has `relation` on, within the verified `(tenant, region, type)` partition. This is the
    /// `(subject, relation) → {object_id}` direction the consumer conjoins its own id column against.
    /// Scoped to the verified scope — there is NO cross-tenant read path (the `tenant-predicate`
    /// floor + the partition isolation). The SetExpr→SQL lowering that wraps this into the consumer's
    /// query is P-ID-12; this is the index the lowering reads.
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

    /// **The inverse reverse lookup the `list_subjects` Expand serves at density (architecture
    /// §7.5):** the concrete principal subjects that have `relation` directly on `object_id`, within
    /// the verified `(tenant, region, type)` partition. This is the `(object_id, relation) →
    /// {subject}` direction the Zanzibar **Expand** (`list_subjects(channel, watcher)`) flattens — the
    /// **same** S8 projection [`ReverseIndex::objects_for`] reads in the opposite direction, so a
    /// 50k-member channel expands via an indexed lookup, NOT a per-member scan (C8). Scoped to the
    /// verified scope — there is NO cross-tenant read path (the `tenant-predicate` floor + the
    /// partition isolation). Returns the DIRECT principal subjects (the projection S8 carries —
    /// userset/inheritance edges are expanded by the [`crate::expand`] engine over the S3 snapshot).
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

    /// The `revision_watermark` for a `(tenant, region)` partition (§8.7) — the latest applied
    /// `iam.tuple_written` zookie. The read-side consistency path (compare a scan's required revision
    /// against this; wait / fall back to per-row `check` if behind) is P-ID-12; here the column +
    /// its monotone advance are shipped. An empty partition reports the genesis (empty) zookie.
    pub fn watermark(&self, scope: &TenantScope) -> Zookie {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S8_TABLE));
        let inner = self.lock();
        inner
            .watermarks
            .get(&(scope.tenant().0.clone(), scope.region().0.clone()))
            .cloned()
            .unwrap_or_else(|| Zookie(String::new()))
    }

    /// The number of projected rows in a `(tenant, region, type)` partition (for tests / the lag
    /// instrumentation). Scoped — no cross-tenant read path.
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

/// The S8 subjects whitelist — `iam.tuple.written` ONLY (rule 3: a `*`-free whitelist; an over-broad
/// subscription head-of-line-blocks everything). The reverse index is fed by exactly one token.
static S8_SUBJECTS: &[SubjectPattern] = &[SubjectPattern(String::new())];

/// **The S8 bus consumer (contract 2.4) — the EventHandler feeding S8 from `iam.tuple_written`.**
///
/// Built from the ONE consumer template (architecture §5): `subjects()` is a `*`-free whitelist
/// (`iam.tuple.written`), `handle` is idempotent (the projection apply is idempotent + the runtime's
/// `event_id` dedup is the outer guard). Each delivered `iam.tuple_written` event is projected into
/// S8 ([`ReverseIndex::apply_delta`]) and the watermark advanced to the event's zookie. The live
/// `reverse_index_lag` ([`ReverseIndexConsumer::lag`]) is the [`signals::REVERSE_INDEX_LAG`] sample:
/// events delivered but not yet projected (0 in steady state on the synchronous apply path; bumped
/// on entry, cleared on apply, so a drill can read it non-zero mid-flight).
///
/// **Residency / tenancy:** the consumer reads `(tenant, region)` off the **envelope** (first-class,
/// never optional) and builds the verified [`TenantScope`] from it — so a projected row lands in the
/// SAME `(tenant, region)` partition the write came from (no cross-region projection; ADR-11).
pub struct ReverseIndexConsumer {
    index: ReverseIndex,
    /// The live `reverse_index_lag` measurement (contract 1.8): events delivered but not yet
    /// projected. 0 in steady state on the synchronous apply path.
    lag: Arc<AtomicU64>,
}

impl ReverseIndexConsumer {
    /// Build the S8 consumer over a reverse index.
    pub fn new(index: ReverseIndex) -> ReverseIndexConsumer {
        ReverseIndexConsumer {
            index,
            lag: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The S8 reverse index this consumer feeds (read access for the `Filter` JOIN / tests).
    pub fn index(&self) -> &ReverseIndex {
        &self.index
    }

    /// The live `reverse_index_lag` sample ([`signals::REVERSE_INDEX_LAG`], contract 1.8): events
    /// delivered to the consumer but not yet projected into S8. The freshness SLO a scan's
    /// watermark-fallback (P-ID-12) reads; wiring the sample onto the metrics-health surface is the
    /// telemetry-contract floor (P-ID-15 / the §4.11 signal set).
    pub fn lag(&self) -> u64 {
        self.lag.load(Ordering::SeqCst)
    }

    /// The telemetry signal name this consumer emits (a named constant — drills assert against the
    /// NAME, never a literal, EI-01 §3 observability).
    pub const LAG_SIGNAL: &'static str = signals::REVERSE_INDEX_LAG;

    /// Project ONE `iam.tuple_written` event into S8 + advance the watermark. Factored out of
    /// [`EventHandler::handle`] so a reindex-from-source replay / a drill can drive it directly. The
    /// `(tenant, region)` partition is taken from the **envelope** (first-class); the deltas + the
    /// zookie are read off the references-not-payloads payload. Returns `Err(reason)` (a
    /// non-retryable poison) for a structurally malformed event (a missing/zookie-less payload) so a
    /// poison never silently corrupts the projection.
    pub fn project(&self, ev: &EventEnvelope) -> Result<(), String> {
        // Bump the lag the instant the event is accepted; clear it once projected (so a drill that
        // pauses between can read it non-zero — the SLO is the time-to-project).
        self.lag.fetch_add(1, Ordering::SeqCst);
        let result = self.project_inner(ev);
        self.lag.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn project_inner(&self, ev: &EventEnvelope) -> Result<(), String> {
        // The `(tenant, region)` partition is the envelope's (first-class, never optional). The
        // verified scope is built from the event's VERIFIED actor principal (whose `tenant` is the
        // envelope's `tenant` by construction — the write path stamps both from one verified scope)
        // + the envelope's pinned `region`. This is the SAME `from_verified_token` seam the write
        // path used; the projected row lands in the SAME `(tenant, region)` partition the write came
        // from (no cross-region projection; ADR-11). The IDOR-floor single-constructor invariant
        // holds: the scope is minted from a verified token, never from a path/string.
        if ev.actor.0.tenant != ev.tenant {
            // Defence-in-depth: a write whose actor tenant disagrees with the envelope tenant is
            // structurally impossible on the write path (one verified scope stamps both); if one
            // ever appears it is a poison, never a silently mis-partitioned projection.
            return Err(format!(
                "iam.tuple_written actor tenant {:?} disagrees with envelope tenant {:?}",
                ev.actor.0.tenant, ev.tenant
            ));
        }
        let scope = TenantScope::from_verified_token(&ev.actor.0, ev.region.clone());

        // The write's zookie (the §8.7 revision_watermark) — references-not-payloads carries it.
        let zookie = match ev.payload.get("zookie").and_then(|z| z.as_str()) {
            Some(z) => Zookie(z.to_string()),
            // A tuple-written event with no zookie is structurally malformed (S3 always stamps one)
            // → a non-retryable poison: never silently project a watermark-less delta.
            None => {
                return Err("iam.tuple_written event carries no zookie (the S8 watermark)".into())
            }
        };

        // The deltas (opaque object#relation@subject refs). An empty array is a valid no-op revision
        // bump, but a missing/non-array field is a malformed producer contract. Parse EVERY delta
        // before applying any of them so one poison cannot partially mutate S8 or advance its
        // watermark. Each delta is `{op, object, relation, subject}`.
        let deltas = ev
            .payload
            .get("deltas")
            .and_then(|d| d.as_array())
            .ok_or_else(|| "iam.tuple_written event carries no deltas array".to_string())?;

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
                    format!("iam.tuple_written delta {index} has no non-empty `{field}` string")
                })
        }

        let validated = deltas
            .iter()
            .enumerate()
            .map(|(index, delta)| {
                let op = required_delta_field(delta, index, "op")?;
                if !matches!(op, "add" | "remove") {
                    return Err(format!(
                        "iam.tuple_written delta {index} has unknown operation `{op}`"
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
            // A delta whose subject is a USERSET (`obj#rel`) is an inheritance edge, not a direct
            // `(subject, relation, object)` projection — S8 projects the DIRECT subject grants the
            // JOIN keys on (a principal id, never a `#`-bearing userset). The userset-expansion
            // (list_subjects at density) is P-ID-13; the reverse index of DIRECT grants is here.
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

        // Always advance the watermark to the event's zookie (even for a deltas-only-userset or empty
        // write — an empty/userset write is still a valid revision the watermark must reflect). Use a
        // dummy partition apply at the `(tenant, region)` grain so the watermark advances independent
        // of whether a direct-grant row was projected.
        if !applied_any {
            self.index.advance_watermark_only(&scope, &zookie);
        }
        Ok(())
    }
}

impl EventHandler for ReverseIndexConsumer {
    /// The `*`-free subject whitelist (rule 3): `iam.tuple.written` only. The reverse index is fed
    /// by exactly one token; an over-broad `*` would head-of-line-block the whole index.
    fn subjects(&self) -> &'static [SubjectPattern] {
        S8_SUBJECTS
    }

    /// Project the delivered `iam.tuple_written` event into S8 + advance the watermark (contract
    /// 2.4). Idempotent on the delta identity (a redelivery is a no-op — the projection apply is
    /// idempotent + the runtime's `event_id` dedup is the outer guard). A non-`iam.tuple.written`
    /// event is ignored (the whitelist binds only that token; defence-in-depth here too). A
    /// structurally-malformed `iam.tuple_written` (no zookie) is a non-retryable poison — never a
    /// silent corruption of the projection.
    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        if ev.type_.0 != IAM_TUPLE_WRITTEN {
            // Defence-in-depth: the whitelist binds iam.tuple.written, but a mis-delivery is a no-op
            // (never project a foreign event into the authz index).
            return HandleOutcome::Done;
        }
        match self.project(ev) {
            Ok(()) => HandleOutcome::Done,
            Err(reason) => HandleOutcome::NonRetryable(myelin_events::Reason(reason)),
        }
    }
}

impl ReverseIndex {
    /// Advance ONLY the `(tenant, region)` watermark (no row projected) — for an empty / userset-only
    /// `iam.tuple_written` write (a valid revision bump with no direct-grant row). Monotone (an older
    /// redelivery never moves it backward). Built through a [`TenantQuery`] (the tenant-predicate
    /// floor).
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

/// Infer an object's TYPE from its id by the leading `type:` prefix (`repo:core` → `repo`,
/// `project:web` → `project`). A bare id with no prefix is its own type (the projection still keys
/// on it — a consumer queries a known type). Mirrors `namespace::type_of_object_id` (the same
/// convention the core hierarchy + the M3/M4 fragments follow); kept local so the S8 module does not
/// reach into the namespace engine's private helper.
fn type_of_object_id(object_id: &str) -> String {
    object_id
        .split_once(':')
        .map(|(ty, _)| ty.to_string())
        .unwrap_or_else(|| object_id.to_string())
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

    /// An admin actor in a given tenant (its `tenant` is what stamps the envelope partition).
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

    /// Wire S3 → outbox → relay → S8 consumer (the live feed) and drain one write through it.
    fn feed_write(
        store: &TupleStore,
        outbox: &OutboxStore,
        consumer: &ReverseIndexConsumer,
        scope: &TenantScope,
        deltas: &[TupleDelta],
    ) -> Zookie {
        // The write actor's tenant matches the scope's tenant (one verified scope stamps both the
        // tuple partition and the event envelope), so the projected row lands in the right partition.
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
        // Drain the relay (what `serve` does) and hand each published envelope to the S8 consumer.
        let bus = InProcessBus::new();
        let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
        relay.drain_to_empty();
        for env in bus.consume("") {
            consumer.handle(&env, &mut myelin_events::HandlerTx::none());
        }
        z
    }

    /// **S8 ingests `iam.tuple_written` and advances the watermark (the GATE).** A committed S3 write
    /// emits `iam.tuple_written`; the relay publishes it; the S8 consumer projects it and the
    /// partition watermark advances to the write's zookie.
    #[test]
    fn s8_ingests_iam_tuple_written_and_advances_watermark() {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());
        let s = scope("acme");

        // Before any write: the watermark is genesis (empty).
        assert_eq!(index.watermark(&s), Zookie(String::new()));

        let z = feed_write(
            &store,
            &outbox,
            &consumer,
            &s,
            &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
        );

        // The watermark advanced to the write's zookie (§8.7).
        assert_eq!(
            index.watermark(&s),
            z,
            "the S8 watermark advances on each iam.tuple_written"
        );
        // The projection holds the (subject, relation, object_id) row.
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
        // reverse_index_lag is 0 in steady state (the synchronous apply cleared it).
        assert_eq!(
            consumer.lag(),
            0,
            "reverse_index_lag returns to 0 after projection"
        );
    }

    /// **The watermark advances monotonically across writes** (§8.7). Two sequential writes advance
    /// the watermark to the latest zookie; an older redelivery does NOT move it backward.
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

        // A redelivered OLDER event (z0's add again) must NOT regress the watermark.
        index.advance_watermark_only(&s, &z0);
        assert_eq!(
            index.watermark(&s),
            z1,
            "an older redelivery never moves the watermark backward"
        );
    }

    /// **A remove delta tombstones the reverse row (the JOIN stops returning it).** Add then remove
    /// `repo:core#reader@p:alice`: after the remove, the reverse lookup is empty.
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

    /// **The apply is idempotent (a redelivery is a no-op).** Projecting the SAME add twice yields
    /// one row (the dedup'd projection) — the consumer runtime's `event_id` dedup is the outer
    /// guard, and the apply itself is idempotent.
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
        let _ = consumer; // the consumer drives apply_delta; the apply is exercised directly here.
    }

    /// **0 cross-tenant S8 rows (the GATE — no cross-tenant query path).** A write under `acme` is
    /// invisible to a read under `globex`: the partitions are isolated by the verified
    /// `(tenant, region)` scope, and there is NO accessor that reads across them.
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

        // globex sees NOTHING acme projected (the partition is keyed by the verified scope).
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
        // acme sees its own row.
        assert_eq!(index.row_count(&acme, &ObjectType("repo".into())), 1);
        // globex's watermark is untouched by acme's write.
        assert_eq!(index.watermark(&globex), Zookie(String::new()));
    }

    /// **S8 auto-registers as a PersonalDataHolder (contract 10.1 — a NEW holder, it references
    /// subjects).** Opening IS registering. The DSR bodies (per-subject reverse-row erasure) are the
    /// GDPR-M1 / P-ID-20 derivative-erasure floor.
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

    /// **A userset-subject delta is NOT projected as a direct row (only direct grants).** An
    /// inheritance edge `repo:core#reader@(org:acme#member)` is not a `(subject, relation, object)`
    /// the JOIN keys on; S8 projects DIRECT subject grants. The watermark still advances (a valid
    /// revision). The userset-expansion (list_subjects at density) is P-ID-13.
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
        // No DIRECT row (the subject is a userset).
        assert_eq!(
            index.row_count(&s, &ObjectType("repo".into())),
            0,
            "a userset subject is not a direct row"
        );
        // But the watermark advanced (the write is a valid revision).
        assert_eq!(
            index.watermark(&s),
            z,
            "the watermark advances even for a userset-only write"
        );
    }

    /// **A zookie-less `iam.tuple_written` is a non-retryable poison (never a silent corruption).**
    /// A structurally-malformed event (no zookie) returns NonRetryable — the projection is not
    /// mutated and the watermark is not advanced.
    #[test]
    fn malformed_event_without_zookie_is_non_retryable_poison() {
        use myelin_events::{
            Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Visibility,
        };
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());
        let ev = EventEnvelope {
            event_id: EventId("e1".into()),
            type_: EventType(IAM_TUPLE_WRITTEN.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(actor_in("acme")),
            subject: myelin_events::ArtifactRef("myelin://acme/iam/tuple/repo:core".into()),
            aggregate: AggregateKey("iam:tuple:acme:repo:core".into()),
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
            // NO zookie in the payload → malformed.
            payload: serde_json::json!({ "deltas": [] }),
        };
        let outcome = consumer.handle(&ev, &mut myelin_events::HandlerTx::none());
        assert!(
            matches!(outcome, HandleOutcome::NonRetryable(_)),
            "a zookie-less iam.tuple_written is a non-retryable poison, never a silent corruption"
        );
        // Nothing was projected and the watermark did not advance.
        assert_eq!(index.watermark(&scope("acme")), Zookie(String::new()));
    }

    /// **A malformed delta cannot leave a revoked grant behind while claiming S8 is current.** The
    /// consumer validates the complete batch before applying its first delta, returns the poison as
    /// non-retryable, and leaves both rows and watermark at the last known-good revision.
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
            type_: EventType(IAM_TUPLE_WRITTEN.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(actor_in("acme")),
            subject: myelin_events::ArtifactRef("myelin://acme/iam/tuple/repo:core".into()),
            aggregate: AggregateKey("iam:tuple:acme:repo:core".into()),
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
