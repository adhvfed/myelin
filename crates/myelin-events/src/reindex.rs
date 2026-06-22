//! # `reindex` — the reindex-from-source seam + the `*.snapshot` event schema (EB-22 / P-142)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/event-bus.md`
//! §4.9 (reindex-from-source re-emit), §5.6 (the `events::reindex(scope)` surface). Contract-index
//! row **2.6** (`reindex-from-source` — owned seam + every subsystem's `replay`; sub-artifact-
//! granular). Doctrine: `external-insights/04-hard-problems.md` §5 (reindex-from-source as a
//! first-class resilience primitive — the index never reads owner DBs, so steady-state and recovery
//! use ONE code path and cannot drift, EI-04 §5.3).
//!
//! ## What this seam is
//! Every derived store (Search, Refs, OLAP, Notif read-models) is reconstructible by asking each
//! owner to **re-emit through the live consumer path** — never by a bespoke "read the index from
//! Postgres" backdoor. This is the §4.9 protocol:
//!
//! ```text
//! reindex(scope) → for each owning subsystem in scope:
//!    subsystem.replay(scope, since=<cursor>) → emits *.snapshot via the SAME outbox→bus path
//!    → the same live consumers (Search/Refs/OLAP/Notif) ingest idempotently (event_id dedup)
//! ```
//!
//! It is **four paths in one**: the recovery path (wipe a derived store → rebuild), the
//! schema-upcaster backfill path, the new-consumer bootstrap path, **and** the `resync_required`
//! fallback target for the firehose resume-cursor protocol ([`crate::firehose`], EB-21 / P-141 —
//! an out-of-window `last_seq` raises `resync_required`; the client falls back to a `*.snapshot`
//! replay, which this seam produces).
//!
//! ## The `*.snapshot` schema — idempotent on a deterministic `event_id`
//! A `*.snapshot` event carries the **same envelope** as a live event (so a derived consumer's
//! `handle` does not branch on cold-vs-live — that is what makes cold == live), but its `event_id`
//! is **deterministic from `(aggregate, version)`** ([`snapshot_event_id`]), NOT a fresh ULID. So:
//! - **re-running a reindex is safe** — the outbox `UNIQUE(event_id)` makes the second emit of the
//!   same `(aggregate, version)` an `ON CONFLICT DO NOTHING` no-op ([`reindex`] filters the dup
//!   before staging, modeling that), and the consumer's `consumer_dedup` ledger
//!   ([`crate::DedupLedger`]) makes a redelivered snapshot a handler no-op. Belt **and** braces.
//! - **a snapshot of the same aggregate@version always lands at the same id**, so a partial reindex
//!   that is retried converges (idempotency-by-construction).
//!
//! ## Sub-artifact granularity (contract 2.6, CONFIRM)
//! [`SnapshotScope`] is sub-artifact-granular: **CI one-run scope** (`ci:run:<id>`), **KN
//! page-subtree at block granularity** (`knowledge:page:<id>`) — so Search re-indexes / Refs
//! re-derives at sub-artifact granularity, the `#sub` resolution ladder (contract 5.7) degrades over
//! the same granularity. The scope is a **PII-free** opaque selector (references-not-payloads).
//!
//! ## Per-owner `replay` bodies (EI-01 §1) — M3 owners FILLED in EB-26, M4 owners are EB-27
//! This module ships **the SEAM + the `*.snapshot` schema + a small reference consumer** to prove
//! `cold == live` (BUS-D5). Each OWNING subsystem implements its real [`ReindexSource::replay`] body
//! with THAT subsystem (it reads its own source of truth, never the derived index — EI-04 §5.3):
//! - **M3 owners — FILLED (EB-26 / P-246):** Git's per-repo / per-blob / per-PR replay
//!   (`myelin_git::replay::GitReindexSource`) + Knowledge's page-subtree-at-BLOCK-granularity replay
//!   (`myelin_content::replay::KnowledgeReindexSource`) — both proven cold == live + idempotent in
//!   their own crates' tests against THIS seam.
//! - **FLOOR (M4 owners — EB-27):** CI's one-run replay, Issues/Chat replay, Refs' per-blob replay,
//!   Search's full reindex land with those subsystems' M4 prompts (`coverage-matrix` rows
//!   2.6/4.x/5.x).
//!
//! The [`ReferenceReindexSource`] here is the reference owner the BUS-D5 drill runs against; it is
//! NOT a stand-in for a real owner's replay (the real M3 owners are the two named above).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::outbox::{EmitContextBase, IdMinter, OutboxStore, Ulid};
use crate::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    Visibility,
};

/// The `*.snapshot` event-name token (Bus §6.4 cross-cutting). An owner's snapshot event type is
/// `<subsystem>.<artifact_type>.snapshot` (e.g. `knowledge.page.snapshot`, `ci.run.snapshot`) — the
/// same dotted grammar [`crate::validate_event_type`] admits (`snapshot` is a registered §6.4
/// noun-form name). A `*.snapshot` is NOT a new event KIND for the consumer: it carries the SAME
/// payload shape the live event of that aggregate carries (that is the cold == live invariant).
pub const SNAPSHOT_EVENT_NAME: &str = "snapshot";

/// Compute the **deterministic** `event_id` for a `*.snapshot` of `(aggregate, version)` (§4.9). The
/// id is a pure function of the aggregate key + its version, so:
/// - re-running a reindex re-emits the SAME id → the outbox `UNIQUE(event_id)` + the consumer's
///   `consumer_dedup` ledger both no-op the duplicate (idempotent, never a double effect);
/// - two reindex runs of the same store at the same versions converge byte-identically.
///
/// The id is prefixed `snap-` so it is distinguishable from a live ULID in logs/audit (a snapshot is
/// a re-emit, never a new fact). The hash is a small, dependency-free FNV-1a over
/// `"<aggregate>@<version>"` rendered hex — deterministic across processes and platforms (it must
/// match in a CI rerun and in a real OLTP binding alike). It is an idempotency key, not a security
/// primitive, so a fast non-cryptographic hash is correct here (collisions across distinct
/// `(aggregate, version)` would mis-dedup, but FNV-1a over the disjoint `aggregate@version` keyspace
/// of one tenant's append-only log does not collide in practice; a real binding may swap in BLAKE3
/// behind this same signature — the id STRING shape is the frozen surface).
pub fn snapshot_event_id(aggregate: &AggregateKey, version: u64) -> EventId {
    let keyed = format!("{}@{}", aggregate.0, version);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a offset basis
    for b in keyed.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3); // FNV-1a prime
    }
    EventId(format!("snap-{hash:016x}"))
}

/// A **sub-artifact-granular, PII-free** reindex selector (contract 2.6 — CI one-run, KN page-subtree
/// at block granularity). It names the OWNING subsystem + an opaque sub-artifact id; the owner's
/// [`ReindexSource::replay`] interprets it. It is references-not-payloads (no PII — a routing
/// selector, the same residency-safe discipline as [`crate::FirehoseScope`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SnapshotScope {
    /// The owning subsystem token (the §6.2 leading token: `ci`, `knowledge`, `refs`, `search`, …).
    pub owner: String,
    /// The opaque sub-artifact selector the owner interprets (`run:<id>`, `page:<id>`, `repo:<id>`,
    /// …). PII-free; references-not-payloads.
    pub selector: String,
}

impl SnapshotScope {
    /// Build a scope for `owner` + `selector`. Both are PII-free opaque tokens.
    pub fn new(owner: impl Into<String>, selector: impl Into<String>) -> SnapshotScope {
        SnapshotScope {
            owner: owner.into(),
            selector: selector.into(),
        }
    }

    /// The wire form `<owner>:<selector>` (the telemetry key; PII-free).
    pub fn as_key(&self) -> String {
        format!("{}:{}", self.owner, self.selector)
    }
}

/// One `*.snapshot` an owner's `replay` yields: the aggregate@version to re-emit, plus the SAME
/// envelope shape the live event carries (so the consumer cannot tell cold from live). The `type_`
/// is the owner's `<subsystem>.<artifact>.snapshot`; `subject`/`payload`/`data_role`/`visibility`
/// are the live event's (references-not-payloads).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotDraft {
    /// The aggregate this snapshot re-emits (the per-aggregate ordering key + half the deterministic
    /// id).
    pub aggregate: AggregateKey,
    /// The aggregate's version (the OTHER half of the deterministic id — a later version of the same
    /// aggregate is a DISTINCT snapshot, so an updated aggregate re-snapshots correctly).
    pub version: u64,
    /// The `<subsystem>.<artifact>.snapshot` event type.
    pub type_: EventType,
    /// The live subject ref (references-not-payloads).
    pub subject: ArtifactRef,
    /// The same payload the live event carries (refs/ids, never PII bodies).
    pub payload: serde_json::Value,
    /// The live event's data role.
    pub data_role: DataRole,
    /// The live event's visibility.
    pub visibility: Visibility,
}

impl SnapshotDraft {
    /// The deterministic `event_id` this snapshot will emit under (from `(aggregate, version)`).
    pub fn event_id(&self) -> EventId {
        snapshot_event_id(&self.aggregate, self.version)
    }

    /// Lower into the outbox [`EventDraft`] (the same emit shape a live event uses; the deterministic
    /// id is supplied separately to the emit, since [`EventDraft`] carries no id — the id is the
    /// transaction's, §2.2).
    fn to_event_draft(&self) -> EventDraft {
        EventDraft {
            type_: self.type_.clone(),
            subject: self.subject.clone(),
            aggregate: self.aggregate.clone(),
            payload: self.payload.clone(),
            data_role: self.data_role,
            visibility: self.visibility,
            // A `*.snapshot` re-emit carries no NEW inline PII — it re-emits an already-published
            // fact whose PII (if any) lives behind the owner's per-subject key, references-not-
            // payloads (the snapshot payload is refs/ids). An erased aggregate is NOT re-snapshotted
            // (the owner's replay skips tombstoned aggregates — the erasure stays erased across a
            // reindex, X-7).
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }
}

/// An **owning subsystem's** reindex-from-source side: replay `(scope, since)` → the `*.snapshot`
/// drafts to re-emit through the live path. Each owner (CI, Knowledge, Refs, Search, …) implements
/// this with ITS store in **EB-26 (M3/M4)** — the FLOOR. `replay` reads the owner's OWN source of
/// truth (its rows / its content store), NEVER the derived index, so steady-state (live events) and
/// recovery (snapshots) are one code path and cannot drift (EI-04 §5.3).
pub trait ReindexSource {
    /// The §6.2 subsystem token this source owns (`ci`, `knowledge`, …). [`reindex`] dispatches a
    /// scope to the source whose `owner_token()` matches `scope.owner`.
    fn owner_token(&self) -> &str;

    /// Replay every aggregate in `scope` whose version is `> since` (the cursor) → the `*.snapshot`
    /// drafts, in a DETERMINISTIC order (ascending aggregate, then version) so a rebuild is
    /// byte-reproducible. `since = None` replays the whole scope (the full rebuild); `since =
    /// Some(v)` is the incremental backfill (the schema-upcaster / new-consumer-bootstrap path).
    /// An erased/tombstoned aggregate is SKIPPED (the erasure stays erased across a reindex, X-7).
    fn replay(&self, scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft>;
}

/// A deterministic [`IdMinter`] that yields a PRESET sequence of ids (the snapshot ids), in order.
/// The outbox's `emit` mints via the injected [`IdMinter`]; a `*.snapshot` must land at its
/// DETERMINISTIC id ([`snapshot_event_id`]), not a fresh ULID — so [`reindex`] drives the emit with
/// this minter seeded with the precomputed snapshot ids. (A real OLTP binding emits the snapshot row
/// with an explicit `event_id = $1 ON CONFLICT DO NOTHING`; this is the in-memory model of that.)
struct PresetMinter {
    ids: std::sync::Mutex<std::collections::VecDeque<Ulid>>,
}

impl PresetMinter {
    fn new(ids: impl IntoIterator<Item = Ulid>) -> PresetMinter {
        PresetMinter {
            ids: std::sync::Mutex::new(ids.into_iter().collect()),
        }
    }
}

impl IdMinter for PresetMinter {
    fn mint(&self) -> Ulid {
        self.ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .expect("reindex preset minter underflow — one id per snapshot draft")
    }
}

/// The receipt a [`reindex`] run returns (the BUS-D5 artifact): how many snapshots were emitted vs
/// skipped-as-duplicate, per owner, so a re-run can be PROVEN idempotent (a second run emits 0 new).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ReindexReceipt {
    /// `*.snapshot` rows newly emitted (NOT already present at their deterministic id).
    pub snapshots_emitted: usize,
    /// `*.snapshot` rows skipped because their deterministic id was already in the outbox (the
    /// `ON CONFLICT DO NOTHING` idempotency no-op — a re-run reports these instead of emitting).
    pub snapshots_skipped_duplicate: usize,
    /// The owners replayed (the §6.2 tokens), in scope order.
    pub owners_replayed: Vec<String>,
}

/// An error from the reindex seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReindexError {
    /// No registered [`ReindexSource`] owns `scope.owner` — a reindex of an unknown owner is a LOUD
    /// error (never a silent empty rebuild — that would mask a wiring bug, EI-02 §4).
    NoSourceForOwner(String),
    /// The outbox emit/commit failed (the snapshots are NOT durable — emit-iff-committed).
    OutboxFailed(String),
}

impl std::fmt::Display for ReindexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReindexError::NoSourceForOwner(o) => {
                write!(f, "reindex: no registered owner for scope owner `{o}`")
            }
            ReindexError::OutboxFailed(e) => write!(f, "reindex: outbox emit failed: {e}"),
        }
    }
}

impl std::error::Error for ReindexError {}

/// **`events::reindex(scope, …)` (§5.6) — the seam.** Ask the OWNER of `scope` to `replay(scope,
/// since)`, then emit each `*.snapshot` draft through the **outbox** (the SAME outbox→bus→live-
/// consumer path BUS-2 mandates — no backdoor). Each snapshot lands at its DETERMINISTIC `event_id`
/// ([`snapshot_event_id`]); a draft whose id is ALREADY in the outbox is SKIPPED (the `ON CONFLICT
/// DO NOTHING` idempotency — so re-running a reindex emits 0 new rows). Returns the
/// [`ReindexReceipt`].
///
/// `since` is the cursor: `None` = full rebuild (the recovery / resync_required path); `Some(v)` =
/// incremental backfill (the upcaster / new-consumer-bootstrap path).
///
/// The snapshots co-commit (emit-iff-committed, BUS-D4): if the commit fails, NO snapshot is durable
/// (a half-rebuilt store is never observable). The relay then drains them to the live consumers; the
/// consumer's `consumer_dedup` ledger makes a redelivered snapshot a handler no-op.
pub fn reindex(
    scope: &SnapshotScope,
    since: Option<u64>,
    sources: &[&dyn ReindexSource],
    outbox: &mut OutboxStore,
    ctx_base: EmitContextBase,
) -> Result<ReindexReceipt, ReindexError> {
    // Dispatch to the owner of this scope (a LOUD error if none — never a silent empty rebuild).
    let source = sources
        .iter()
        .find(|s| s.owner_token() == scope.owner)
        .ok_or_else(|| ReindexError::NoSourceForOwner(scope.owner.clone()))?;

    let drafts = source.replay(scope, since);

    // Split into NEW (not yet in the outbox at their deterministic id) vs DUPLICATE (already
    // present → ON CONFLICT DO NOTHING). The deterministic id is what makes a re-run a no-op.
    let mut to_emit: Vec<SnapshotDraft> = Vec::new();
    let mut skipped_duplicate = 0usize;
    for draft in drafts {
        let id = draft.event_id();
        if outbox.row(&id).is_some() {
            skipped_duplicate += 1;
        } else {
            to_emit.push(draft);
        }
    }

    let snapshots_emitted = to_emit.len();
    if !to_emit.is_empty() {
        // Seed the preset minter with the snapshots' deterministic ids, in emit order, so the
        // outbox row lands at `snapshot_event_id(aggregate, version)` (NOT a fresh ULID).
        let ids: Vec<Ulid> = to_emit.iter().map(|d| Ulid(d.event_id().0)).collect();
        let minter: Arc<dyn IdMinter> = Arc::new(PresetMinter::new(ids));
        let mut tx = outbox.begin(minter, ctx_base);
        for draft in &to_emit {
            // A `*.snapshot` is a ROOT re-emit (cause = None): it re-emits an already-established
            // fact, it is not caused by another event (so its causal depth resets — a reindex is
            // not a runaway, the loop guards do not see it as a deepening chain).
            tx.emit(draft.to_event_draft(), None)
                .map_err(|e| ReindexError::OutboxFailed(e.0))?;
        }
        tx.stage_state_change(format!(
            "reindex owner={} scope={} emitted={snapshots_emitted}",
            scope.owner,
            scope.as_key()
        ));
        tx.commit().map_err(|e| ReindexError::OutboxFailed(e.0))?;
    }

    Ok(ReindexReceipt {
        snapshots_emitted,
        snapshots_skipped_duplicate: skipped_duplicate,
        owners_replayed: vec![scope.owner.clone()],
    })
}

// =================================================================================================
// The REFERENCE derived store + the reference reindex source — the small consumer the BUS-D5 drill
// runs `cold == live` against. NOT a real owner's replay (those are EB-26 / per-owner M3/M4); this
// is the seam's reference implementation, the one BUS-D5 proves byte-parity over.
// =================================================================================================

/// A tiny **derived store** (the shape Search/Refs/OLAP/Notif read-models are): a projection built
/// ONLY by ingesting events (live OR `*.snapshot` — the SAME `ingest` for both, that is cold ==
/// live). It NEVER reads an owner DB. The projection is a `BTreeMap<aggregate → payload>` (a
/// last-writer-wins materialization keyed by the aggregate), so two ingestion orders that see the
/// same final versions converge to the same bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DerivedStore {
    /// aggregate → (version, payload) — the materialized projection. A higher version wins (LWW), so
    /// a snapshot of an OLDER version never clobbers a live update.
    projection: BTreeMap<String, (u64, serde_json::Value)>,
    /// `event_id`s already applied (the in-store dedup — the same effectively-once the
    /// `consumer_dedup` ledger gives; modeled here so the reference consumer is self-contained).
    applied: std::collections::BTreeSet<String>,
}

impl DerivedStore {
    /// A fresh empty derived store.
    pub fn new() -> DerivedStore {
        DerivedStore::default()
    }

    /// **The ONE ingest path — live AND snapshot use it (cold == live).** Apply an envelope to the
    /// projection, idempotently on `event_id` (a redelivered live event OR a re-emitted snapshot is
    /// a no-op). The version is read from the payload's `version` field (the owner stamps it; a
    /// snapshot carries the live version — that is what makes a snapshot of version `v` indistinct
    /// from the live event of version `v`). Returns `true` iff the projection changed.
    pub fn ingest(&mut self, env: &EventEnvelope) -> bool {
        if !self.applied.insert(env.event_id.0.clone()) {
            return false; // already applied (effectively-once) — no double effect.
        }
        let version = env
            .payload
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let agg = env.aggregate.0.clone();
        match self.projection.get(&agg) {
            // LWW: only apply if this is a NEWER (or equal-first) version of the aggregate, so a
            // late snapshot of an older version cannot resurrect stale bytes.
            Some((existing_v, _)) if *existing_v >= version => false,
            _ => {
                self.projection.insert(agg, (version, env.payload.clone()));
                true
            }
        }
    }

    /// The materialized projection as canonical bytes (the byte-parity comparison BUS-D5 reads:
    /// cold-rebuilt == live MUST be byte-identical). Deterministic: the `BTreeMap` iterates in key
    /// order and `serde_json` serializes deterministically over that.
    pub fn parity_bytes(&self) -> Vec<u8> {
        let view: BTreeMap<&String, &serde_json::Value> =
            self.projection.iter().map(|(k, (_, v))| (k, v)).collect();
        serde_json::to_vec(&view).expect("projection serializes")
    }

    /// The number of aggregates materialized.
    pub fn len(&self) -> usize {
        self.projection.len()
    }

    /// `true` iff the projection is empty (a wiped store).
    pub fn is_empty(&self) -> bool {
        self.projection.is_empty()
    }
}

/// A **reference** [`ReindexSource`] — the in-test owner the BUS-D5 drill replays. It owns a
/// deterministic set of `(aggregate, version, payload)` triples (the owner's source of truth) and
/// replays them as `*.snapshot` drafts. A real owner's `replay` reads ITS store; this reads its
/// in-memory truth — the SAME shape (EB-26 fills the real bodies, the named floor).
pub struct ReferenceReindexSource {
    owner: String,
    artifact: String,
    /// The owner's source of truth: aggregate → (version, payload). A `BTreeMap` so the replay order
    /// is deterministic (ascending aggregate) — a rebuild is byte-reproducible.
    truth: BTreeMap<String, (u64, serde_json::Value)>,
}

impl ReferenceReindexSource {
    /// A reference source for `owner`/`artifact` (e.g. `ci`/`run`, `knowledge`/`page`).
    pub fn new(owner: impl Into<String>, artifact: impl Into<String>) -> ReferenceReindexSource {
        ReferenceReindexSource {
            owner: owner.into(),
            artifact: artifact.into(),
            truth: BTreeMap::new(),
        }
    }

    /// Record/update the owner's truth for `aggregate` at `version` with `payload` (the owner's live
    /// write). The `payload` gets a `version` field stamped in (so the derived store reads it).
    pub fn upsert(&mut self, aggregate: &str, version: u64, payload: serde_json::Value) {
        let mut payload = payload;
        if let serde_json::Value::Object(map) = &mut payload {
            map.insert("version".into(), serde_json::json!(version));
        }
        self.truth.insert(aggregate.to_string(), (version, payload));
    }

    /// The `*.snapshot` event type for this owner (`<owner>.<artifact>.snapshot`).
    pub fn snapshot_type(&self) -> EventType {
        EventType(format!(
            "{}.{}.{}",
            self.owner, self.artifact, SNAPSHOT_EVENT_NAME
        ))
    }
}

impl ReindexSource for ReferenceReindexSource {
    fn owner_token(&self) -> &str {
        &self.owner
    }

    fn replay(&self, _scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        // Deterministic ascending-aggregate replay; skip aggregates at/below the `since` cursor.
        self.truth
            .iter()
            .filter(|(_, (v, _))| since.is_none_or(|s| *v > s))
            .map(|(agg, (v, payload))| SnapshotDraft {
                aggregate: AggregateKey(agg.clone()),
                version: *v,
                type_: self.snapshot_type(),
                subject: ArtifactRef(format!("myelin://t/{}/{}/{agg}", self.owner, self.artifact)),
                payload: payload.clone(),
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Actor, CorrelationId, Region, TenantId, Timestamp};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            caused_by: None,
        }
    }

    /// Hand-build the envelope a relay would deliver from an outbox row (the live consumer's input).
    fn envelope_of(row: &crate::OutboxRow) -> EventEnvelope {
        row.envelope.clone()
    }

    /// The deterministic id is a pure function of `(aggregate, version)` — same inputs → same id;
    /// a different version → a different id; a different aggregate → a different id.
    #[test]
    fn snapshot_event_id_is_deterministic_from_aggregate_and_version() {
        let a = AggregateKey("ci.run:42".into());
        let b = AggregateKey("ci.run:43".into());
        assert_eq!(
            snapshot_event_id(&a, 1),
            snapshot_event_id(&a, 1),
            "same inputs → same id"
        );
        assert_ne!(
            snapshot_event_id(&a, 1),
            snapshot_event_id(&a, 2),
            "version bumps the id"
        );
        assert_ne!(
            snapshot_event_id(&a, 1),
            snapshot_event_id(&b, 1),
            "aggregate bumps the id"
        );
        assert!(
            snapshot_event_id(&a, 1).0.starts_with("snap-"),
            "snapshot ids are prefixed"
        );
    }

    /// **Unit: `*.snapshot` idempotency on the deterministic id — a replayed snapshot produces ONE
    /// effect.** Reindex the same scope TWICE: the first run emits the snapshots; the second run
    /// emits 0 NEW (every draft's deterministic id is already in the outbox → `ON CONFLICT DO
    /// NOTHING`), reporting them as skipped-duplicate.
    #[test]
    fn reindex_is_idempotent_a_rerun_emits_zero_new_snapshots() {
        let mut source = ReferenceReindexSource::new("ci", "run");
        source.upsert("ci.run:1", 1, serde_json::json!({ "status": "success" }));
        source.upsert("ci.run:2", 1, serde_json::json!({ "status": "failure" }));
        let sources: &[&dyn ReindexSource] = &[&source];
        let scope = SnapshotScope::new("ci", "run:all");

        let mut outbox = OutboxStore::new();
        let r1 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("first reindex");
        assert_eq!(r1.snapshots_emitted, 2, "first run emits both snapshots");
        assert_eq!(r1.snapshots_skipped_duplicate, 0);
        assert_eq!(outbox.committed_count(), 2);

        // Re-run: every snapshot's deterministic id is already present → 0 new, both skipped.
        let r2 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("second reindex");
        assert_eq!(r2.snapshots_emitted, 0, "a re-run emits 0 NEW (idempotent)");
        assert_eq!(r2.snapshots_skipped_duplicate, 2);
        assert_eq!(
            outbox.committed_count(),
            2,
            "still only 2 rows — no duplicate effect"
        );
    }

    /// The snapshots land at their DETERMINISTIC ids in the outbox (not fresh ULIDs) — so a relay's
    /// broker-side dedup + a consumer's `consumer_dedup` both key off the stable id.
    #[test]
    fn emitted_snapshots_carry_the_deterministic_event_id() {
        let mut source = ReferenceReindexSource::new("knowledge", "page");
        source.upsert(
            "knowledge.page:home",
            3,
            serde_json::json!({ "title_ref": "r1" }),
        );
        let sources: &[&dyn ReindexSource] = &[&source];
        let scope = SnapshotScope::new("knowledge", "page:home");

        let mut outbox = OutboxStore::new();
        reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");

        let expected = snapshot_event_id(&AggregateKey("knowledge.page:home".into()), 3);
        let row = outbox
            .row(&expected)
            .expect("snapshot lands at its deterministic id");
        assert_eq!(row.envelope.type_.0, "knowledge.page.snapshot");
        assert_eq!(
            row.envelope.depth, 0,
            "a snapshot is a ROOT re-emit (depth resets)"
        );
    }

    /// **Unit: the reference consumer rebuilds BYTE-IDENTICALLY from a `*.snapshot` replay (cold ==
    /// live).** Build a LIVE projection by ingesting the live events; wipe a second store and rebuild
    /// it from the snapshot replay; assert the two projections are byte-identical.
    #[test]
    fn reference_consumer_rebuilds_byte_identically_cold_equals_live() {
        // The owner's truth.
        let mut source = ReferenceReindexSource::new("ci", "run");
        source.upsert("ci.run:1", 1, serde_json::json!({ "status": "success" }));
        source.upsert("ci.run:2", 2, serde_json::json!({ "status": "failure" }));
        source.upsert("ci.run:3", 1, serde_json::json!({ "status": "running" }));

        // LIVE store: ingest the live events (modeled as the same snapshot drafts the owner would
        // have emitted live — the cold==live invariant is precisely that these are the same shape).
        let mut live = DerivedStore::new();
        let scope = SnapshotScope::new("ci", "run:all");
        for draft in source.replay(&scope, None) {
            let env = snapshot_envelope(&draft);
            live.ingest(&env);
        }

        // COLD store: wiped, rebuilt ONLY from the reindex snapshot replay through the outbox→relay
        // path. Reindex → read the emitted rows → ingest into the wiped store.
        let mut cold = DerivedStore::new();
        let sources: &[&dyn ReindexSource] = &[&source];
        let mut outbox = OutboxStore::new();
        reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
        for draft in source.replay(&scope, None) {
            let row = outbox.row(&draft.event_id()).expect("snapshot row present");
            cold.ingest(&envelope_of(&row));
        }

        assert_eq!(live.len(), 3);
        assert_eq!(cold.len(), 3);
        assert_eq!(
            cold.parity_bytes(),
            live.parity_bytes(),
            "cold == live (byte-identical)"
        );
    }

    /// A reindex of an UNKNOWN owner is a LOUD error (never a silent empty rebuild that would mask a
    /// wiring bug).
    #[test]
    fn reindex_of_unknown_owner_is_a_loud_error() {
        let source = ReferenceReindexSource::new("ci", "run");
        let sources: &[&dyn ReindexSource] = &[&source];
        let scope = SnapshotScope::new("refs", "edge:all");
        let mut outbox = OutboxStore::new();
        let err = reindex(&scope, None, sources, &mut outbox, ctx_base()).unwrap_err();
        assert_eq!(err, ReindexError::NoSourceForOwner("refs".into()));
    }

    /// `since` is the incremental cursor: only aggregates at a version ABOVE the cursor replay (the
    /// upcaster-backfill / new-consumer-bootstrap path).
    #[test]
    fn reindex_since_cursor_replays_only_newer_versions() {
        let mut source = ReferenceReindexSource::new("ci", "run");
        source.upsert("ci.run:1", 1, serde_json::json!({ "status": "old" }));
        source.upsert("ci.run:2", 5, serde_json::json!({ "status": "new" }));
        let sources: &[&dyn ReindexSource] = &[&source];
        let scope = SnapshotScope::new("ci", "run:all");

        let mut outbox = OutboxStore::new();
        let r = reindex(&scope, Some(3), sources, &mut outbox, ctx_base()).expect("incremental");
        assert_eq!(
            r.snapshots_emitted, 1,
            "only the version-5 aggregate replays past since=3"
        );
    }

    /// The derived store ingests idempotently on `event_id` (a redelivered snapshot is a no-op) AND
    /// LWW on version (a late snapshot of an OLDER version never clobbers a newer live one).
    #[test]
    fn derived_store_ingest_is_idempotent_and_lww() {
        let mut store = DerivedStore::new();
        let draft_v2 = SnapshotDraft {
            aggregate: AggregateKey("a:1".into()),
            version: 2,
            type_: EventType("x.a.snapshot".into()),
            subject: ArtifactRef("myelin://t/x/a/1".into()),
            payload: serde_json::json!({ "version": 2, "v": "new" }),
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
        };
        let env_v2 = snapshot_envelope(&draft_v2);
        assert!(store.ingest(&env_v2), "first ingest applies");
        assert!(
            !store.ingest(&env_v2),
            "redelivery is a no-op (idempotent on event_id)"
        );

        // An OLDER-version snapshot of the same aggregate (distinct id) does NOT clobber the newer.
        let draft_v1 = SnapshotDraft {
            version: 1,
            payload: serde_json::json!({ "version": 1, "v": "old" }),
            ..draft_v2.clone()
        };
        let env_v1 = snapshot_envelope(&draft_v1);
        assert!(
            !store.ingest(&env_v1),
            "an older-version snapshot is LWW-rejected (no resurrection)"
        );
        let bytes = store.parity_bytes();
        assert!(
            String::from_utf8_lossy(&bytes).contains("new"),
            "the newer version wins (LWW)"
        );
    }

    /// Build the `*.snapshot` envelope a relay would deliver for a draft (the consumer's input shape).
    fn snapshot_envelope(draft: &SnapshotDraft) -> EventEnvelope {
        EventEnvelope {
            event_id: draft.event_id(),
            type_: draft.type_.clone(),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            subject: draft.subject.clone(),
            aggregate: draft.aggregate.clone(),
            causation_id: None,
            correlation_id: CorrelationId(draft.event_id().0),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: draft.data_role,
            visibility: draft.visibility,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            payload: draft.payload.clone(),
        }
    }
}
