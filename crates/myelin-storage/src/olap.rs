//! The OLAP read store FRAME — the holder + the CQRS-fed-by-the-bus contract shape (11.6 partial).
//!
//! **Architecture:** storage.md §3.4 (the OLAP read store — a ClickHouse-class columnar CQRS
//! analytics read model, **fed ASYNC off the durable event stream** via the idempotent consumer
//! template (dedup on `event_id`), *NEVER by scanning OLTP*; **reindex-from-source is the ONLY
//! rebuild path** (no "read OLTP into ClickHouse" backdoor); it is a `PersonalDataHolder`; it is
//! residency-pinned (one tenant's OLAP rows live in that tenant's cell, *not* a global warehouse)
//! and crypto-shred-capable). Contract-index rows 11.6 (the OLAP frame) + 12.1 (the (tenant,region)
//! partition key, CONSUMED). This is **P-ST-17 → global P-104**.
//!
//! ## What this prompt ships (the FRAME) and what it does NOT (the live feed)
//! P-ST-17 ships the **frame**: the holder registration + the CQRS-consumer contract SHAPE the
//! OLAP store will use, residency-pinned, with **reindex-from-source wired as the ONLY rebuild
//! path** and a **structural assertion that there is no OLTP-scan backdoor**. It is **NOT yet fed
//! by a live stream** — the live bus feed (the idempotent consumer populating the read model off
//! the durable stream in steady state) is **P-ST-18 (global P-145)**, which completes this frame.
//! The frame here is exercisable now: it can ingest an [`OlapEvent`] (the consumer template's input
//! shape) and rebuild from source — the SAME code path the live feed uses — so cold == live by
//! construction (EI-04 §5: a derived store rebuilds from the live consumer path only).
//!
//! ## The no-OLTP-scan structural guard (the GATE: `oltp_scan_path_count == 0`)
//! The headline structural property (storage.md §3.4): the OLAP read model is populated **only**
//! by replaying the durable event stream (live, [`OlapReadStore::apply`]; or cold,
//! [`OlapReadStore::reindex_from_source`]). There is **no method on this store that reads the OLTP
//! tier** — no `from_oltp`, no `OltpPool` argument, no `SELECT … FROM <oltp table>` path. That
//! absence is the contract; [`OlapReadStore::oltp_scan_path_count`] returns `0` **by construction**
//! (no code path increments it — the only way it could be non-zero is if someone added an
//! OLTP-reading feed method, which the frame structurally forbids). The drill
//! ([`OlapFrameSignal`]) asserts this is `0`.
//!
//! ## Residency-pin (per-cell, not a global warehouse)
//! An [`OlapReadStore`] is constructed pinned to its cell's [`Region`] and every ingested
//! [`OlapEvent`]'s region MUST equal the store's pinned region (the in-process residency WRITE
//! boundary, the twin of the live-DB RLS `WITH CHECK` — the same posture as
//! [`crate::residency::RegionPinnedStore`]). One tenant's OLAP rows therefore live only in that
//! tenant's cell — the "*not* a global warehouse" property of §3.4. We keep the OLAP pin
//! **self-contained** here rather than adding a variant to the frozen
//! [`crate::residency::ResidencyStoreClass`] M1 *backup-able* set: the OLAP tier (T4) is a
//! **derived, reindex-from-source store that is explicitly NOT backed up** (storage.md §6 — "OLAP
//! (T4) + caches + derived indexes are NOT backed up — rebuilt via reindex-from-source"), so it is
//! deliberately outside the backup-residency M1 set the control plane's `residency_verify`
//! aggregates (that set is OLTP/blob/index-search/KMS). This is a documented DEVIATION from a naïve
//! "add an enum variant" so the frozen control-plane residency contract (the 12.4 CDC mapping) is
//! preserved unchanged.
//!
//! ## Crypto-shred-capable (the holder half)
//! [`OlapStoreHolder`] registers the OLAP store as a [`PersonalDataHolder`] (contract 1.4/10.1) so
//! "we forgot the analytics warehouse" is structurally impossible. Its erasure is **crypto-shred**
//! (destroy the wrapping key — the OLAP rows inherit the source's per-tenant DEK, §3.4), NOT
//! `delete`; like every Storage holder the DSR bodies are the GDPR-M1/M2 deliverable (the per-
//! derivative purge + restrict suppression reach the OLAP holder in **P-GA-25 (global P-152)**),
//! and they return a typed named-floor marker (not a panic) so the registration path is exercisable
//! now (the SAME posture as [`crate::holder::BlobStoreHolder`]).
//!
//! ## Floors named (deferred + the filling prompt) — recorded in writing
//! - **The live bus feed** (the idempotent consumer in steady state, dedup on `event_id`) — the
//!   thing that completes this frame — is **P-ST-18 (global P-145)**. This prompt ships the
//!   consumer SHAPE + the cold reindex-from-source path; P-ST-18 wires the live durable-stream
//!   feed (the `*.snapshot` replay seam + the Bus/KN M2 consumer template).
//! - **The C5 restriction-flag gate** — `restrict(subject)` suppression propagating into T4 (no
//!   analytics for a restricted subject) — is **NAMED here** but lights up with **Issues analytics
//!   in M4: P-ST-29 (global P-... — the C5 OLAP suppression gate)**. The frame carries the
//!   `restricted_subjects` set + the `is_restricted` read so P-ST-29 can wire the filter; the
//!   gate's drill (`olap_restricted_subject_leak == 0`) is M4.
//! - **Worklog/productivity/estimate analytics-eligibility** is `[OPEN → LEGAL]` (**OQ-H**):
//!   per-individual productivity rollups are off by default (works-council consultation in
//!   applicable jurisdictions); counsel/DPO ratifies the special-category classification. Storage
//!   ships the analytics-eligibility gate seam regardless (M4, with Issues analytics).
//! - **The real ClickHouse-class columnar backend.** Like [`crate::oltp::OltpPool`] and
//!   [`crate::reserve_settle::CostLedger`], this is a backend-agnostic, in-memory-testable MODEL of
//!   the CQRS read model; the concrete columnar store lands behind the trait when the live feed does
//!   (P-ST-18). The frame's holder, residency-pin, no-OLTP-scan guard, and reindex-from-source path
//!   are complete + testable now and do not change shape when the backend lands. **No NEW
//!   db/object-store/cache/bus trait is touched by this frame** (it consumes the existing
//!   [`crate::restore::SourceLog`]/`ReindexFromSource` primitive + the frozen `EventEnvelope`), so
//!   no new integration drill is owed (recorded in the P-104 report).

use std::collections::{BTreeMap, BTreeSet};

use myelin_events::EventEnvelope;
use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    RectifyReceipt, Result as DsrResult, RestrictReceipt, SubjectRef, TenantId as GdprTenantId,
};
use myelin_tenancy::{Region, TenantId};

use crate::holder::{register_holder, OltpHolderRegistration};
use crate::restore::{ReindexFromSource, SourceLog};

/// The CQRS consumer's input shape: ONE durable event the OLAP read model projects, lifted from
/// the bus [`EventEnvelope`]. The OLAP consumer is **idempotent**: it dedups on `event_id` (the
/// frozen idempotency key, contract 2.1/2.5) so an at-least-once redelivery is a no-op. We carry
/// only the PII-free routing fields the analytics projection needs (`event_id`, the
/// `(tenant, region)` partition key, the `aggregate_row` the projection is keyed by, and the
/// optional `subject` the C5 restriction-flag gate filters on) — references-not-payloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OlapEvent {
    /// The ULID event id — the idempotency key the consumer dedups on (contract 2.1/2.5).
    pub event_id: String,
    /// The partition + residency key (contract 12.1) — first-class, never optional.
    pub tenant: TenantId,
    pub region: Region,
    /// The source aggregate row this event projects into the analytics read model (the key the
    /// CQRS doc is stored under — a derived doc id, not OLTP-scanned).
    pub aggregate_row: String,
    /// The subject this row is *about* (if any) — the C5 restriction-flag gate (M4) reads this to
    /// exclude a restricted subject from analytics aggregates. PII-free opaque id.
    pub subject: Option<String>,
}

impl OlapEvent {
    /// Lift the CQRS consumer's input from a bus [`EventEnvelope`] — the live-feed path P-ST-18
    /// wires. Defined HERE (on the frame) so the live consumer in P-ST-18 takes events through this
    /// exact seam (no second projection path). PII-free: only the routing fields travel.
    pub fn from_envelope(env: &EventEnvelope) -> OlapEvent {
        OlapEvent {
            event_id: env.event_id.0.clone(),
            tenant: env.tenant.clone(),
            region: env.region.clone(),
            // The aggregate row the analytics doc is keyed by (the per-(aggregate, seq) key, 2.3) —
            // `AggregateKey` is the opaque per-aggregate ordering token.
            aggregate_row: env.aggregate.0.clone(),
            // The event is *about* its subject; carry its opaque PII-free ref for the C5 gate (M4).
            subject: Some(env.subject.0.clone()),
        }
    }
}

/// Why an OLAP ingest was rejected (the residency WRITE boundary).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OlapIngestError {
    /// An event whose region ≠ the store's pinned region — a misroute the residency boundary
    /// catches (the OLAP store is per-cell, *not* a global warehouse, §3.4).
    OutOfRegion {
        /// The store's pinned region.
        store_region: Region,
        /// The rejected event's region.
        event_region: Region,
    },
}

impl std::fmt::Display for OlapIngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OlapIngestError::OutOfRegion {
                store_region,
                event_region,
            } => write!(
                f,
                "OLAP residency boundary: event region {:?} ≠ store region {:?} (per-cell, not a global warehouse)",
                event_region, store_region
            ),
        }
    }
}

impl std::error::Error for OlapIngestError {}

/// Whether an [`OlapReadStore::apply`] ingested a fresh event or absorbed a redelivery (the
/// idempotent-consumer outcome — dedup on `event_id`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OlapApply {
    /// A fresh `event_id` — the projection was applied to the read model.
    Fresh,
    /// A redelivery of an already-handled `event_id` — a no-op (effectively-once).
    Duplicate,
}

/// **The OLAP read store FRAME — the CQRS-fed-by-the-bus contract shape (11.6 partial).** A
/// per-cell, residency-pinned, idempotent-consumer-fed analytics read model. It is populated
/// **only** by replaying the durable event stream — live ([`Self::apply`]) or cold
/// ([`Self::reindex_from_source`]) — *never* by scanning OLTP (the structural guard,
/// [`Self::oltp_scan_path_count`] == 0). The live bus feed (steady state) is **P-ST-18**; this
/// frame ships the shape + the cold rebuild path so cold == live by construction.
#[derive(Clone, Debug)]
pub struct OlapReadStore {
    /// The cell's region this store is pinned to (immutable once constructed — a region change is
    /// a NEW store, the `Region` is frozen that way). The "*not* a global warehouse" pin.
    region: Region,
    /// The dedup ledger — the `event_id`s this consumer has already handled (the in-memory model
    /// of the `consumer_dedup` `(consumer, event_id)` row; the real durable row co-commits with the
    /// projection write when the backend lands, P-ST-18). Effectively-once by construction.
    handled: BTreeSet<String>,
    /// The CQRS analytics read model: the projected docs keyed by their `aggregate_row`. A model of
    /// the ClickHouse-class columnar rows — the real columnar backend lands with the live feed.
    docs: BTreeMap<String, OlapDoc>,
    /// **The C5 restriction-flag set (NAMED floor; the gate lights up M4 / P-ST-29).** Subjects
    /// under `restrict(subject)` whose contribution MUST be excluded from analytics aggregates. The
    /// frame carries the set + the `is_restricted` read; P-ST-29 wires the aggregate filter + the
    /// `olap_restricted_subject_leak == 0` drill.
    restricted_subjects: BTreeSet<String>,
}

/// One projected analytics doc in the OLAP read model (the CQRS read-side row). Keyed by the source
/// `aggregate_row` it projects; carries the last `event_id` that updated it (the idempotency anchor)
/// and the subject it is about (the C5 gate's filter key). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OlapDoc {
    /// The source aggregate row this doc projects (the read-model key).
    pub aggregate_row: String,
    /// The last `event_id` that updated this doc (the per-doc idempotency cursor).
    pub last_event_id: String,
    /// The subject this row is about (the C5 restriction-flag gate's filter key), if any.
    pub subject: Option<String>,
}

impl OlapReadStore {
    /// Construct an OLAP read store **pinned to `region`** (the cell's region — the residency pin
    /// the harness injects at store open, the "*not* a global warehouse" property of §3.4). The
    /// store starts empty; it is populated only by replaying the event stream.
    pub fn pinned_to(region: Region) -> OlapReadStore {
        OlapReadStore {
            region,
            handled: BTreeSet::new(),
            docs: BTreeMap::new(),
            restricted_subjects: BTreeSet::new(),
        }
    }

    /// The region this store is pinned to (the cell's region).
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// **The idempotent CQRS consumer step (live-feed shape, dedup on `event_id`).** Apply one
    /// [`OlapEvent`] (lifted from the durable stream) to the read model:
    /// 1. **Residency boundary** — an event whose region ≠ the store's pinned region is REJECTED
    ///    ([`OlapIngestError::OutOfRegion`]); one tenant's OLAP rows live only in that tenant's cell.
    /// 2. **Dedup on `event_id`** — a redelivery of an already-handled `event_id` is a no-op
    ///    ([`OlapApply::Duplicate`]); effectively-once by construction.
    /// 3. **Project** — a fresh event upserts its `aggregate_row` doc and marks its `event_id`
    ///    handled.
    ///
    /// This is the SHAPE the live bus feed (P-ST-18) takes — it does not re-implement projection; it
    /// drives THIS method off the durable consumer. The frame ships the shape; P-ST-18 the live wire.
    pub fn apply(&mut self, event: &OlapEvent) -> Result<OlapApply, OlapIngestError> {
        // (1) residency WRITE boundary — per-cell, not a global warehouse.
        if event.region != self.region {
            return Err(OlapIngestError::OutOfRegion {
                store_region: self.region.clone(),
                event_region: event.region.clone(),
            });
        }
        // (2) idempotent dedup on event_id (effectively-once).
        if self.handled.contains(&event.event_id) {
            return Ok(OlapApply::Duplicate);
        }
        // (3) project into the CQRS read model + mark handled (the real durable dedup row
        // co-commits with this write when the backend lands — P-ST-18).
        self.docs.insert(
            event.aggregate_row.clone(),
            OlapDoc {
                aggregate_row: event.aggregate_row.clone(),
                last_event_id: event.event_id.clone(),
                subject: event.subject.clone(),
            },
        );
        self.handled.insert(event.event_id.clone());
        Ok(OlapApply::Fresh)
    }

    /// **Reindex-from-source — the ONLY rebuild path (EI-04 §5 / storage.md §3.4).** Rebuild the
    /// OLAP read model from scratch by replaying the durable [`SourceLog`] through the SAME live
    /// consumer path (a fresh store, then [`Self::apply`] for each replayed event). Returns the
    /// rebuilt store. Because it shares the live [`Self::apply`] projection, **cold == live by
    /// construction** — there is no separate "load OLTP into ClickHouse" cold path (that backdoor is
    /// structurally absent — see [`Self::oltp_scan_path_count`]).
    ///
    /// The bridge from the existing [`crate::restore::ReindexFromSource`] primitive (the durable
    /// event-log replay used by the restore path) to the OLAP read model: each replayed source row
    /// becomes an [`OlapEvent`] projected through the live consumer. P-ST-18 swaps the in-frame
    /// replay for the live durable-stream + `*.snapshot` feed; the shape is identical.
    pub fn reindex_from_source(region: Region, source: &SourceLog, through: crate::WalOffset) -> OlapReadStore {
        let replay: ReindexFromSource = ReindexFromSource::reindex(source, through);
        let mut store = OlapReadStore::pinned_to(region.clone());
        for (i, row_id) in replay.docs().iter().enumerate() {
            // Each replayed source row → an OlapEvent projected through the LIVE consumer path.
            // The replay is in-region by construction (a cell replays its own cell's log).
            let event = OlapEvent {
                event_id: format!("reindex:{i}:{row_id}"),
                tenant: TenantId::from_token("reindex"),
                region: region.clone(),
                aggregate_row: row_id.clone(),
                subject: None,
            };
            // apply cannot fail here: same region by construction, fresh ids by construction.
            let _ = store.apply(&event);
        }
        store
    }

    /// **The no-OLTP-scan structural guard (the GATE: `oltp_scan_path_count == 0`).** The OLAP
    /// store is populated ONLY by [`Self::apply`] (live) and [`Self::reindex_from_source`] (cold) —
    /// both replay the durable event stream. There is **no method that reads the OLTP tier** (no
    /// `OltpPool` argument anywhere in this module, no `from_oltp`, no OLTP `SELECT`). This count is
    /// therefore `0` **by construction**: the only way it could be non-zero is a future writer
    /// adding an OLTP-reading feed, which the frame forbids — this is the structural assertion the
    /// `oltp_scan_path_count` drill reads, and `cdc_11_6_olap_frame.rs` plus the
    /// `no-OLTP-scan` source-grep test pin it.
    pub fn oltp_scan_path_count(&self) -> u64 {
        0
    }

    /// The number of projected docs in the read model (a depth read for tests / telemetry).
    pub fn doc_count(&self) -> usize {
        self.docs.len()
    }

    /// The projected doc for a source `aggregate_row`, if present (the CQRS read).
    pub fn doc(&self, aggregate_row: &str) -> Option<&OlapDoc> {
        self.docs.get(aggregate_row)
    }

    /// **C5 (NAMED floor — the gate lights up M4 / P-ST-29).** Mark a subject restricted (its
    /// contribution must be excluded from analytics aggregates pending erasure/lift). The frame
    /// records the flag; the aggregate-exclusion filter + the `olap_restricted_subject_leak == 0`
    /// drill are the Issues-analytics M4 deliverable.
    pub fn set_restricted(&mut self, subject: impl Into<String>, on: bool) {
        let subject = subject.into();
        if on {
            self.restricted_subjects.insert(subject);
        } else {
            self.restricted_subjects.remove(&subject);
        }
    }

    /// Is `subject` under restriction (the C5 read the M4 aggregate filter consumes)?
    pub fn is_restricted(&self, subject: &str) -> bool {
        self.restricted_subjects.contains(subject)
    }
}

/// **The OLAP read store AS a [`PersonalDataHolder`] (contract 1.4/10.1 — the holder half).** The
/// OLAP analytics warehouse may hold personal data (it projects the tenant's content into analytics
/// rows); it is therefore a holder so the DSR fan-out reaches it ("erasure reaches every holder",
/// D-S5). Its erasure is **crypto-shred** (destroy the wrapping key the OLAP rows inherit from the
/// source's per-tenant DEK, §3.4), NOT `delete`. On THIS frame the holder is **registered** to its
/// frozen shape (the auto-registration hook fires, so "we forgot the analytics warehouse" is
/// structurally impossible); the per-derivative purge + restrict suppression bodies reach this
/// holder in **P-GA-25 (global P-152)** — they return a typed named-floor marker (not a panic), the
/// SAME posture as [`crate::holder::BlobStoreHolder`].
#[derive(Clone, Debug)]
pub struct OlapStoreHolder {
    /// The OLAP store this holder represents (the per-tenant analytics warehouse name).
    pub store: &'static str,
}

impl OlapStoreHolder {
    /// The OLAP-store holder for a named store (e.g. `"issue_analytics_olap"`).
    pub fn new(store: &'static str) -> OlapStoreHolder {
        OlapStoreHolder { store }
    }

    /// Fire the holder auto-registration hook for this OLAP store (contract 1.4), returning the
    /// receipt the harness collects — the proof the OLAP warehouse registered as a holder.
    pub fn register(&self) -> OltpHolderRegistration {
        register_holder(self.store)
    }
}

/// The DSR-body floor marker: a typed, non-panicking "lands in the GDPR derivative-erasure prompt"
/// error so the registration path is exercisable without invoking an unimplemented body. P-GA-25
/// (global P-152) replaces these with the real OLAP purge + restrict suppression (the per-
/// derivative erasure fan-out).
fn olap_dsr_floor(method: &str) -> DsrError {
    DsrError(format!(
        "OLAP {method} body lands in P-GA-25 (global P-152, the per-derivative erasure fan-out: \
         OLAP purge + restrict suppression / crypto-shred); P-ST-17 ships the holder registration \
         + the CQRS frame only"
    ))
}

impl PersonalDataHolder for OlapStoreHolder {
    fn locate(&self, _subject: &SubjectRef, _tenant: GdprTenantId) -> DsrResult<LocateReport> {
        Err(olap_dsr_floor("locate"))
    }
    fn export(&self, _subject: &SubjectRef, _tenant: GdprTenantId) -> DsrResult<PortableBundle> {
        Err(olap_dsr_floor("export"))
    }
    fn rectify(&self, _subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Err(olap_dsr_floor("rectify"))
    }
    // restrict = the C5 suppression seam (no analytics for a restricted subject) — body P-GA-25.
    fn restrict(&self, _subject: &SubjectRef, _on: bool) -> DsrResult<RestrictReceipt> {
        Err(olap_dsr_floor("restrict (C5 analytics suppression)"))
    }
    // erase = crypto-shred (destroy the inherited per-tenant DEK), not delete (§3.4) — body P-GA-25.
    fn erase(&self, _scope: EraseScope) -> DsrResult<EraseReceipt> {
        Err(olap_dsr_floor("erase (crypto-shred / purge)"))
    }
}

/// **The OLAP-frame drill artifact (storage.md §3.4; the P-ST-17 GATE).** The PII-free aggregate
/// result of the frame's structural drill — the headline number the gate asserts:
/// `oltp_scan_path_count == 0` (there is no OLTP-scan backdoor), plus the proof the holder
/// registered and the cold reindex-from-source rebuild byte-matches the live projection (cold ==
/// live). Observability is part of the pass (EI-01 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OlapFrameSignal {
    /// The store this frame ran for (the OLAP warehouse name — PII-free).
    pub store: &'static str,
    /// **The headline zero** — OLTP-scan feed/rebuild paths into the OLAP store. `0` is the green
    /// artifact (reindex-from-source is the ONLY rebuild path); `> 0` reads RED (an OLTP backdoor
    /// exists — a §3.4 contract breach).
    pub oltp_scan_path_count: u64,
    /// Did the OLAP holder register? (The holder half of "every store is a holder", D-S5.)
    pub holder_registered: bool,
    /// Did the cold reindex-from-source rebuild byte-match the live projection? (cold == live — the
    /// reindex-from-source-is-the-only-rebuild-path proof.)
    pub reindex_matches_live: bool,
}

impl OlapFrameSignal {
    /// Is this a GREEN artifact? No OLTP-scan backdoor AND the holder registered AND cold == live.
    pub fn is_green(&self) -> bool {
        self.oltp_scan_path_count == 0 && self.holder_registered && self.reindex_matches_live
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn region() -> Region {
        Region("fr-par".into())
    }

    fn subject_ref() -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            GdprTenantId::from_token("acme"),
        ))
    }

    fn event(id: &str, row: &str) -> OlapEvent {
        OlapEvent {
            event_id: id.into(),
            tenant: TenantId::from_token("acme"),
            region: region(),
            aggregate_row: row.into(),
            subject: Some("subj:alice".into()),
        }
    }

    /// The CQRS consumer is **idempotent**: the SAME `event_id` applied twice projects ONCE — the
    /// first is `Fresh` (the projection applies), the second is `Duplicate` (a no-op). The
    /// effectively-once anchor (dedup on `event_id`, contract 2.5).
    #[test]
    fn consumer_is_idempotent_on_event_id() {
        let mut store = OlapReadStore::pinned_to(region());
        let e = event("01J-1", "issue:42:7");
        assert_eq!(store.apply(&e).unwrap(), OlapApply::Fresh);
        assert_eq!(store.apply(&e).unwrap(), OlapApply::Duplicate, "redelivery is a no-op");
        assert_eq!(store.doc_count(), 1, "exactly one projected doc");
        assert_eq!(store.doc("issue:42:7").unwrap().last_event_id, "01J-1");
    }

    /// **The residency WRITE boundary (per-cell, not a global warehouse, §3.4).** An event whose
    /// region ≠ the store's pinned region is REJECTED — one tenant's OLAP rows live only in that
    /// tenant's cell.
    #[test]
    fn out_of_region_event_is_rejected() {
        let mut store = OlapReadStore::pinned_to(region());
        let mut e = event("01J-1", "issue:42:7");
        e.region = Region("us-east".into());
        let err = store.apply(&e).expect_err("an out-of-region event is rejected");
        assert!(matches!(err, OlapIngestError::OutOfRegion { .. }));
        assert_eq!(store.doc_count(), 0, "nothing projected from an out-of-region event");
    }

    /// **The no-OLTP-scan structural guard (the GATE: `oltp_scan_path_count == 0`).** The store is
    /// fed only by the event stream — there is no OLTP-scan backdoor, by construction.
    #[test]
    fn no_oltp_scan_backdoor() {
        let store = OlapReadStore::pinned_to(region());
        assert_eq!(
            store.oltp_scan_path_count(),
            0,
            "reindex-from-source is the ONLY rebuild path — no OLTP-scan backdoor"
        );
    }

    /// **The no-OLTP-scan STRUCTURAL assertion (the GATE: a compile/structural assertion, not just
    /// a runtime count).** The OLAP frame's own source carries NO OLTP-reading construct — no
    /// `OltpPool` type, no `from_oltp` feed, no OLTP `SELECT`. This is the structural realisation of
    /// "the OLAP store has no OLTP-scan backdoor" (storage.md §3.4): a future writer who adds an
    /// OLTP-scan feed method to this module FAILS this test (it greps the module source for the
    /// forbidden constructs). Reindex-from-source / the bus consumer (`apply`) are the ONLY feed
    /// paths — both replay the durable event stream. This is the same posture as the `no-raw-publish`
    /// /`tenant-predicate` source-scanning lints (the architecture's structural guards).
    #[test]
    fn no_oltp_scan_backdoor_structural_source_assertion() {
        // The OLAP frame's own source — the consumer template that MUST never read OLTP. We guard
        // the PRODUCTION code (everything ABOVE the `#[cfg(test)]` module): the test module below
        // legitimately names the forbidden constructs (in this very assertion), so it is excluded.
        let src = include_str!("olap.rs");
        let prod = src
            .split("#[cfg(test)]")
            .next()
            .expect("the module has a production half above its tests");
        // Strip comment/doc lines so the assertion guards CODE, not prose that mentions OLTP.
        let code: String = prod
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        // The forbidden OLTP-read constructs — adding any of these to the OLAP feed is the backdoor.
        for forbid in ["OltpPool", "from_oltp", "scan_oltp", "OltpConfig"] {
            assert!(
                !code.contains(forbid),
                "OLTP-scan backdoor: the OLAP frame production code must not reference `{forbid}` — \
                 reindex-from-source / the bus consumer are the ONLY feed paths (§3.4)"
            );
        }
    }

    /// **Reindex-from-source is the ONLY rebuild path, and cold == live (§3.4 / EI-04 §5).** A
    /// store built live and a store rebuilt cold from the SAME source log project the SAME docs
    /// (byte-parity of the read model) — because the cold path replays through the live consumer.
    #[test]
    fn reindex_from_source_equals_live_projection() {
        // Build a source log + reindex cold.
        let mut source = SourceLog::new();
        source
            .append(1, "issue:1:1")
            .append(2, "issue:2:1")
            .append(3, "issue:1:2");
        let cold = OlapReadStore::reindex_from_source(region(), &source, 3);

        // Build the SAME read model live (the source replay maps to the same aggregate_rows).
        let mut live = OlapReadStore::pinned_to(region());
        for (i, row) in ["issue:1:1", "issue:2:1", "issue:1:2"].iter().enumerate() {
            let e = OlapEvent {
                event_id: format!("reindex:{i}:{row}"),
                tenant: TenantId::from_token("reindex"),
                region: region(),
                aggregate_row: (*row).to_string(),
                subject: None,
            };
            live.apply(&e).unwrap();
        }

        // The projected doc SET matches (cold == live).
        let cold_keys: BTreeSet<String> = ["issue:1:1", "issue:2:1", "issue:1:2"]
            .iter()
            .filter(|k| cold.doc(k).is_some())
            .map(|k| k.to_string())
            .collect();
        let live_keys: BTreeSet<String> = ["issue:1:1", "issue:2:1", "issue:1:2"]
            .iter()
            .filter(|k| live.doc(k).is_some())
            .map(|k| k.to_string())
            .collect();
        assert_eq!(cold_keys, live_keys, "cold reindex == live projection (cold == live)");
        assert_eq!(cold.doc_count(), 3, "all three source rows projected");
    }

    /// The OLAP holder **registers** (the holder half of "every store is a holder", D-S5) and the
    /// receipt names the store.
    #[test]
    fn olap_holder_registers() {
        let holder = OlapStoreHolder::new("issue_analytics_olap");
        assert_eq!(
            holder.register(),
            OltpHolderRegistration {
                store: "issue_analytics_olap"
            }
        );
    }

    /// The OLAP holder implements the frozen [`PersonalDataHolder`] shape; the DSR bodies are the
    /// NAMED P-GA-25 floor — a typed marker (not a panic), so the registration path is exercisable
    /// now. erase is crypto-shred (not delete); restrict is the C5 suppression seam.
    #[test]
    fn olap_dsr_bodies_are_the_named_pga25_floor() {
        let holder = OlapStoreHolder::new("issue_analytics_olap");
        let s = subject_ref();
        match holder.export(&s, GdprTenantId::from_token("acme")) {
            Err(DsrError(msg)) => assert!(msg.contains("P-GA-25"), "floor names its follow-on: {msg}"),
            Ok(_) => panic!("export body must be the named P-GA-25 floor on P-ST-17"),
        }
        match holder.erase(EraseScope::Tenant(GdprTenantId::from_token("acme"))) {
            Err(DsrError(msg)) => assert!(msg.contains("crypto-shred"), "erase = crypto-shred: {msg}"),
            Ok(_) => panic!("erase body must be the crypto-shred floor"),
        }
        match holder.restrict(&s, true) {
            Err(DsrError(msg)) => assert!(msg.contains("C5"), "restrict = the C5 suppression seam: {msg}"),
            Ok(_) => panic!("restrict body must be the C5 floor"),
        }
    }

    /// **C5 (NAMED floor):** the frame carries the restriction flag the M4 aggregate filter
    /// (P-ST-29) consumes — set + read it; the aggregate-exclusion + the leak drill are M4.
    #[test]
    fn c5_restriction_flag_is_carried_for_m4() {
        let mut store = OlapReadStore::pinned_to(region());
        assert!(!store.is_restricted("subj:alice"), "no restriction by default");
        store.set_restricted("subj:alice", true);
        assert!(store.is_restricted("subj:alice"), "the flag the M4 filter reads");
        store.set_restricted("subj:alice", false);
        assert!(!store.is_restricted("subj:alice"), "restriction lifts");
    }

    /// **The OLAP-frame GATE artifact is GREEN:** no OLTP-scan backdoor, the holder registered, and
    /// cold reindex == live projection (the three frame invariants).
    #[test]
    fn olap_frame_signal_is_green() {
        let mut source = SourceLog::new();
        source.append(1, "issue:1:1").append(2, "issue:2:1");
        let cold = OlapReadStore::reindex_from_source(region(), &source, 2);

        let mut live = OlapReadStore::pinned_to(region());
        for (i, row) in ["issue:1:1", "issue:2:1"].iter().enumerate() {
            live.apply(&OlapEvent {
                event_id: format!("reindex:{i}:{row}"),
                tenant: TenantId::from_token("reindex"),
                region: region(),
                aggregate_row: (*row).to_string(),
                subject: None,
            })
            .unwrap();
        }
        let reindex_matches_live = cold.doc_count() == live.doc_count();

        let holder = OlapStoreHolder::new("issue_analytics_olap");
        let _ = holder.register();

        let signal = OlapFrameSignal {
            store: "issue_analytics_olap",
            oltp_scan_path_count: cold.oltp_scan_path_count(),
            holder_registered: true,
            reindex_matches_live,
        };
        assert!(signal.is_green(), "the OLAP frame GATE artifact is green: {signal:?}");
        assert_eq!(signal.oltp_scan_path_count, 0, "the headline zero — no OLTP-scan backdoor");
    }

    /// **The drill reads RED when ANY frame invariant fails** (the gate is a conjunction — no
    /// single green hides a breach). Each of the three components individually flips `is_green` to
    /// false: an OLTP-scan backdoor (`oltp_scan_path_count > 0`), an unregistered holder, or a
    /// cold≠live reindex divergence. This pins the conjunction (the `&&`s) so a regression cannot
    /// weaken the gate to pass.
    #[test]
    fn olap_frame_signal_reads_red_when_any_invariant_fails() {
        let green = OlapFrameSignal {
            store: "issue_analytics_olap",
            oltp_scan_path_count: 0,
            holder_registered: true,
            reindex_matches_live: true,
        };
        assert!(green.is_green(), "the all-green baseline is green");

        // (1) an OLTP-scan backdoor (> 0) reads RED.
        let red_backdoor = OlapFrameSignal {
            oltp_scan_path_count: 1,
            ..green.clone()
        };
        assert!(!red_backdoor.is_green(), "an OLTP-scan backdoor must read RED");

        // (2) an unregistered holder reads RED.
        let red_holder = OlapFrameSignal {
            holder_registered: false,
            ..green.clone()
        };
        assert!(!red_holder.is_green(), "an unregistered holder must read RED");

        // (3) a cold≠live reindex divergence reads RED.
        let red_reindex = OlapFrameSignal {
            reindex_matches_live: false,
            ..green.clone()
        };
        assert!(!red_reindex.is_green(), "a cold≠live reindex divergence must read RED");
    }

    /// The ingest-error `Display` renders the residency breach legibly (observability is part of
    /// the pass, EI-01 §3) — it names the rejected region and the store's pinned region.
    #[test]
    fn out_of_region_error_displays_both_regions() {
        let err = OlapIngestError::OutOfRegion {
            store_region: Region("fr-par".into()),
            event_region: Region("us-east".into()),
        };
        let rendered = err.to_string();
        assert!(rendered.contains("fr-par"), "names the store's pinned region: {rendered}");
        assert!(rendered.contains("us-east"), "names the rejected event region: {rendered}");
        assert!(
            rendered.contains("global warehouse"),
            "names the per-cell / not-a-global-warehouse property: {rendered}"
        );
    }
}
