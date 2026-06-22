//! # The OLAP read store fed by the bus — the LIVE feed that completes the frame (11.6).
//!
//! **Prompt:** P-ST-18 → global **P-145** (M2). **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.4 (the OLAP store fed ASYNC off
//! the durable event stream via the idempotent consumer, dedup on `event_id`, NEVER scanning OLTP;
//! **reindex-from-source the ONLY rebuild path**). Contract-index row **11.6 OWNED** (OLAP fed by
//! the bus — completing the frame). Consumed: **2.4/2.6** (the consumer template + the
//! reindex-from-source `*.snapshot` re-emit seam, `myelin_events::reindex`, EB-22 / P-142).
//! Doctrine: `external-insights/04-hard-problems.md` §5 (reindex-from-source — the derived store
//! rebuilds via the live consumer path ONLY, never a bespoke recovery reader);
//! `external-insights/01-process-and-quality-doctrine.md` §3 (prove-it; exercise the REAL thing —
//! reindex-from-cold on a real stream), §4.
//!
//! ## What this prompt ADDS to the P-ST-17 frame ([`crate::olap`]) — coherence, EI-01 §7
//! P-ST-17 (global P-104) shipped the **frame**: [`crate::olap::OlapReadStore`] (the idempotent
//! consumer `apply`, dedup on `event_id`), the residency boundary, the holder, the C5 flag, the
//! no-OLTP-scan structural guard, and a MODELED cold rebuild over [`crate::restore::SourceLog`]. It
//! was explicitly **NOT yet fed by a live stream**. This prompt is that live stream: it wires the
//! OLAP store as a **real bus consumer** that ingests `EventEnvelope`s drained off the REAL
//! outbox→relay→[`myelin_events::InProcessBus`] path (the consumer template, contract 2.4), and it
//! wires **`reindex(scope)`** through the REAL [`myelin_events::reindex`] `*.snapshot` re-emit seam
//! (contract 2.6) so a cold rebuild is BYTE-IDENTICAL to live (the F4 gate). It REUSES the frame's
//! [`crate::olap::OlapReadStore::apply`] as the ONE projection path (live AND cold drive the SAME
//! `apply` — that is what makes cold == live by construction); it does NOT re-implement projection,
//! re-define the read model, or fork a second OLAP store. The frame's `OlapEvent::from_envelope` is
//! the ONE lift from a bus envelope.
//!
//! ## The F4 gate (the storage face of BUS-D5): `reindex(scope)` byte-matches live
//! storage.md §3.4 / EI-04 §5: a derived store is rebuilt ONLY by re-emitting `*.snapshot` events
//! through the SAME outbox→bus→live-consumer path, never by reading an owner DB. The F4 OLAP
//! reindex-parity drill ([`tests::…`] + `tests/stor_f4_olap_reindex_parity_drill.rs`) WIPES the
//! OLAP store, runs [`myelin_events::reindex`] (which drives the OWNER's `replay` → `*.snapshot`
//! through the outbox), drains the relay to the bus, ingests the snapshots through the OLAP
//! consumer, and asserts the rebuilt [`crate::olap::OlapReadStore::parity_bytes`] is byte-identical
//! to the live projection. Telemetry: `reindex_parity_hash` matches (cold == live).
//!
//! ## No OLTP-scan backdoor — STRUCTURAL (the headline of §3.4)
//! Every population path here is an `EventEnvelope` ingest (live, [`OlapBusConsumer::ingest`]; or
//! cold, the same consumer fed the re-emitted `*.snapshot`s). There is NO `OltpPool` argument, no
//! `from_oltp`, no OLTP `SELECT` anywhere in this module — `reindex(scope)` asks the OWNER to
//! `replay` ITS source of truth through the bus, it does not read the OLTP tier into the warehouse.
//! [`crate::olap::OlapReadStore::oltp_scan_path_count`] stays `0` by construction.
//!
//! ## Floors named (EI-01 §1) — deferred + the filling prompt, recorded in writing
//! - **The C5 restriction-flag gate** (`restrict(subject)` suppression → no analytics for a
//!   restricted subject, `olap_restricted_subject_leak == 0`) remains NAMED for **M4: P-ST-29**
//!   (the C5 OLAP suppression gate, with Issues analytics). The frame carries the
//!   `restricted_subjects` set + the `is_restricted` read; P-ST-29 wires the aggregate filter.
//! - **The real ClickHouse-class columnar backend.** The OLAP read model is a backend-agnostic,
//!   in-memory-testable MODEL (the SAME posture as [`crate::oltp::OltpPool`] /
//!   [`crate::reserve_settle::CostLedger`]); the concrete columnar store lands behind the trait
//!   when the live ingest is wired to the real durable bus. The live-feed SHAPE here (the bus
//!   consumer, the reindex-from-source `*.snapshot` re-emit, the byte-parity) does not change shape
//!   when the columnar backend lands. **No NEW db/object-store/cache/bus trait is touched** — this
//!   feed REUSES the existing `myelin_events` outbox→relay→bus→consumer seam (contract 2.4/2.6) +
//!   the frozen `EventEnvelope`. So no new live-stack integration drill is OWED by this prompt; the
//!   real-bus integration is exercised by the Bus's own NATS JetStream integration feature
//!   (`myelin-events/integration`) the relay rides — recorded in the P-145 report.
//! - **The per-owner real `replay` body** (CI one-run, KN page-subtree, Refs per-blob) is the Bus's
//!   **EB-26 (P-246, M3)** floor (`myelin_events::ReindexSource::replay`); this prompt uses the
//!   [`OlapAnalyticsSource`] reference owner the F4 drill replays — NOT a stand-in for a real
//!   owner's replay (the SAME posture as the Bus's `ReferenceReindexSource` for BUS-D5).

use std::collections::BTreeMap;

use myelin_events::{
    reindex, ArtifactRef, BusTransport, DataRole, EmitContextBase, EventEnvelope, InProcessBus,
    OutboxStore, Region, ReindexError, ReindexReceipt, ReindexSource, Relay, SnapshotDraft,
    SnapshotScope, Visibility,
};

use crate::olap::{OlapApply, OlapEvent, OlapIngestError, OlapReadStore};

/// **The OLAP store's bus consumer (the live-feed half of 11.6, contract 2.4).** The idempotent CQRS
/// consumer that fronts the [`OlapReadStore`]: it lifts a durable bus [`EventEnvelope`] into the
/// frame's `OlapEvent` shape ([`OlapEvent::from_envelope`]) and drives the frame's
/// [`OlapReadStore::apply`] (dedup on `event_id`). This is the SAME consumer for LIVE events and for
/// re-emitted `*.snapshot`s — there is exactly ONE projection path, which is what makes cold == live
/// by construction (EI-04 §5.3). It does not re-implement the read model; it wraps the frame store.
#[derive(Clone, Debug)]
pub struct OlapBusConsumer {
    store: OlapReadStore,
}

impl OlapBusConsumer {
    /// Boot the OLAP consumer pinned to `region` (the cell's region — the residency pin). Starts
    /// with an empty read model; it is populated ONLY by ingesting the durable event stream.
    pub fn boot(region: Region) -> OlapBusConsumer {
        OlapBusConsumer {
            store: OlapReadStore::pinned_to(region),
        }
    }

    /// **Ingest one durable bus event into the OLAP read model (the idempotent consumer step).**
    /// Lifts the envelope via the frame's [`OlapEvent::from_envelope`] and drives
    /// [`OlapReadStore::apply`]: an out-of-region event is REJECTED (the per-cell residency
    /// boundary), a redelivered `event_id` is a no-op ([`OlapApply::Duplicate`]). This is the ONE
    /// path live events AND `*.snapshot`s take — cold == live by construction.
    pub fn ingest(&mut self, env: &EventEnvelope) -> Result<OlapApply, OlapIngestError> {
        self.store.apply(&OlapEvent::from_envelope(env))
    }

    /// Ingest a BATCH of durable bus events (the drain from the relay/bus), returning the count of
    /// FRESH (newly-projected) events. Out-of-region events are surfaced as the first error (the
    /// residency boundary is a hard fail, never a silent drop).
    pub fn ingest_batch(&mut self, envs: &[EventEnvelope]) -> Result<usize, OlapIngestError> {
        let mut fresh = 0;
        for env in envs {
            if self.ingest(env)? == OlapApply::Fresh {
                fresh += 1;
            }
        }
        Ok(fresh)
    }

    /// The underlying read model (the CQRS read side) — for parity/telemetry reads.
    pub fn store(&self) -> &OlapReadStore {
        &self.store
    }

    /// The read model's reindex-parity bytes (the F4 comparison: cold == live byte-for-byte).
    pub fn parity_bytes(&self) -> Vec<u8> {
        self.store.parity_bytes()
    }
}

/// **The OLAP analytics owner's reindex-from-source side (the reference owner the F4 drill replays).**
/// A [`ReindexSource`] whose `replay` re-emits the owner's source-of-truth facts as `*.snapshot`
/// drafts through the SAME live consumer path (contract 2.6). On the real floor the OWNER (Issues,
/// CI, …) implements its `replay` reading ITS rows — that per-owner body is the Bus's **EB-26
/// (P-246, M3)** floor. This reference owner models that source of truth so the OLAP reindex-parity
/// (F4) is exercisable now; it is NOT a stand-in for a real owner (the SAME posture as
/// `myelin_events::ReferenceReindexSource` for BUS-D5).
///
/// The owner token is `"olap_src"` (the scope this drill dispatches to); a real OLAP reindex
/// dispatches the scope to whichever upstream owner's facts the analytics rows project.
pub struct OlapAnalyticsSource {
    owner: String,
    /// The owner's source of truth: `aggregate_row → (version, subject?)`. A `BTreeMap` so the
    /// replay order is deterministic (ascending aggregate) — a rebuild is byte-reproducible.
    truth: BTreeMap<String, (u64, Option<String>)>,
}

impl OlapAnalyticsSource {
    /// A reference OLAP-analytics source under `owner` (e.g. `"olap_src"`).
    pub fn new(owner: impl Into<String>) -> OlapAnalyticsSource {
        OlapAnalyticsSource {
            owner: owner.into(),
            truth: BTreeMap::new(),
        }
    }

    /// Record/update the owner's analytics-relevant truth for `aggregate_row` at `version` (about
    /// `subject`, if any). The owner's live write — the fact the live event projected, and the fact
    /// a `*.snapshot` re-emits identically (cold == live).
    pub fn upsert(&mut self, aggregate_row: &str, version: u64, subject: Option<&str>) {
        self.truth.insert(
            aggregate_row.to_string(),
            (version, subject.map(str::to_string)),
        );
    }

    /// The `<owner>.analytics.snapshot` event type for this owner.
    fn snapshot_type(&self) -> myelin_events::EventType {
        myelin_events::EventType(format!(
            "{}.analytics.{}",
            self.owner,
            reindex::SNAPSHOT_EVENT_NAME
        ))
    }
}

impl ReindexSource for OlapAnalyticsSource {
    fn owner_token(&self) -> &str {
        &self.owner
    }

    fn replay(&self, _scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        // Deterministic ascending-aggregate replay; skip aggregates at/below the `since` cursor (the
        // incremental backfill). The payload carries ONLY routing refs (PII-free, references-not-
        // payloads) — the OLAP consumer's `from_envelope` reads `aggregate`/`subject`, never a PII
        // body.
        self.truth
            .iter()
            .filter(|(_, (v, _))| since.is_none_or(|s| *v > s))
            .map(|(agg, (v, subject))| {
                let mut payload = serde_json::json!({ "aggregate_row": agg });
                if let Some(s) = subject {
                    payload["subject"] = serde_json::json!(s);
                }
                SnapshotDraft {
                    aggregate: myelin_events::AggregateKey(agg.clone()),
                    version: *v,
                    type_: self.snapshot_type(),
                    subject: ArtifactRef(
                        subject.clone().unwrap_or_else(|| {
                            format!("myelin://t/{}/analytics/{agg}", self.owner)
                        }),
                    ),
                    payload,
                    data_role: DataRole::Processor,
                    visibility: Visibility::Internal,
                }
            })
            .collect()
    }
}

/// **`reindex(scope)` for the OLAP store — through the REAL `*.snapshot` re-emit seam (the ONLY
/// rebuild path, contract 2.6 / EI-04 §5).** Rebuild a WIPED OLAP read model BYTE-IDENTICALLY by:
/// 1. asking the OWNER of `scope` to `replay(scope, None)` → `*.snapshot` drafts emitted through the
///    REAL [`OutboxStore`] (the SAME outbox→bus path a live event takes — no backdoor);
/// 2. the REAL [`Relay`] draining those snapshots to the [`InProcessBus`];
/// 3. the OLAP consumer ([`OlapBusConsumer::ingest`]) ingesting the published snapshots off the bus
///    (the EXACT live consumer path — `from_envelope` → `apply`, dedup on `event_id`).
///
/// Returns the rebuilt [`OlapBusConsumer`] (its `parity_bytes` byte-match live — the F4 gate) and the
/// [`ReindexReceipt`] (so a re-run is provably idempotent: a second reindex emits 0 new snapshots).
/// This is the storage face of BUS-D5; it reuses the Bus's `reindex`/relay/bus seam wholesale (it
/// does NOT fork a bespoke OLAP recovery reader — that absence is the §3.4 contract).
///
/// `bus` + `relay` are SHARED across runs (the broker retains the delivered snapshots), so a
/// **second** reindex over the same `outbox` re-emits 0 NEW snapshots (the deterministic-`event_id`
/// `ON CONFLICT DO NOTHING`) yet still rebuilds a wiped consumer byte-identically by re-consuming the
/// retained delivered snapshots off the bus — that is the idempotency proof, the EXACT BUS-D5 shape.
#[allow(clippy::too_many_arguments)]
pub fn reindex_olap_from_bus(
    region: Region,
    scope: &SnapshotScope,
    sources: &[&dyn ReindexSource],
    outbox: &mut OutboxStore,
    bus: &InProcessBus,
    relay: &Relay<InProcessBus>,
    ctx_base: EmitContextBase,
    subject_prefix: &str,
) -> Result<(OlapBusConsumer, ReindexReceipt), ReindexError> {
    // (1) reindex-from-source: ask the owner to replay → emit `*.snapshot`s through the REAL outbox
    //     (the deterministic-event_id idempotent re-emit; a re-run is `ON CONFLICT DO NOTHING`).
    let receipt = reindex::reindex(scope, None, sources, outbox, ctx_base)?;

    // (2) the REAL relay drains any newly-staged snapshots to the bus (the outbox→relay→bus path a
    //     live event rides — no backdoor). A re-run stages nothing new, so this drains nothing new;
    //     the bus still RETAINS the snapshots delivered by the first run (the broker's durable log).
    relay.drain_to_empty();

    // (3) the OLAP consumer ingests EVERY snapshot the bus holds for this scope (the live consumer
    //     path — `from_envelope` → `apply`, dedup on `event_id`). A cold rebuild and a live feed take
    //     the SAME `ingest` — cold == live by construction; a re-run rebuilds a WIPED consumer from
    //     the retained delivered snapshots (idempotent, byte-stable).
    let mut consumer = OlapBusConsumer::boot(region);
    let published: Vec<EventEnvelope> = bus.consume(subject_prefix);
    consumer
        .ingest_batch(&published)
        .map_err(|e| ReindexError::OutboxFailed(format!("OLAP reindex ingest: {e}")))?;

    Ok((consumer, receipt))
}

/// **The OLAP reindex-parity (F4) signal — the dated GREEN artifact (storage.md §3.4 / the P-ST-18
/// gate).** The PII-free aggregate of the OLAP-fed-by-the-bus drill: the cold rebuild byte-matches
/// the live projection (`reindex_parity_hash` matches), there is no OLTP-scan backdoor
/// (`oltp_scan_path_count == 0`), and the re-run is idempotent (a second reindex emits 0 new
/// snapshots). Observability is part of the pass (EI-01 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OlapReindexParitySignal {
    /// The OLAP warehouse this ran for (PII-free name).
    pub store: &'static str,
    /// **The headline: did the COLD reindex byte-match the LIVE projection?** (`reindex_parity_hash`
    /// matches — cold == live.) The F4 gate's green.
    pub reindex_matches_live: bool,
    /// The no-OLTP-scan structural count (`0` — reindex-from-source is the ONLY rebuild path).
    pub oltp_scan_path_count: u64,
    /// Snapshots emitted by the FIRST reindex (the rebuild).
    pub snapshots_emitted_first: usize,
    /// Snapshots emitted by a SECOND reindex — MUST be `0` (the deterministic-`event_id` re-run is
    /// an idempotent no-op; the outbox `ON CONFLICT DO NOTHING` skips them).
    pub snapshots_emitted_second: usize,
}

impl OlapReindexParitySignal {
    /// Is this a GREEN F4 artifact? Cold == live AND no OLTP-scan backdoor AND the re-run emitted 0.
    pub fn is_green(&self) -> bool {
        self.reindex_matches_live
            && self.oltp_scan_path_count == 0
            && self.snapshots_emitted_second == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, EventId, EventType, OutboxStore, TenantId, Timestamp,
        OUTBOX_MIGRATION,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn region() -> Region {
        Region("fr-par".into())
    }
    fn tenant() -> TenantId {
        TenantId("01J0ACME".into())
    }
    fn now() -> Timestamp {
        Timestamp("2026-06-20T00:00:00Z".into())
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: now(),
            recorded_at: now(),
            caused_by: None,
        }
    }

    /// A live bus envelope for one of the owner's facts — the SAME shape a `*.snapshot` of that
    /// `(aggregate, version)` carries (so the cold snapshot is byte-indistinct from the live event).
    fn live_envelope(
        agg: &str,
        version: u64,
        event_id: &str,
        subject: Option<&str>,
    ) -> EventEnvelope {
        let mut payload = serde_json::json!({ "aggregate_row": agg, "version": version });
        if let Some(s) = subject {
            payload["subject"] = serde_json::json!(s);
        }
        EventEnvelope {
            event_id: EventId(event_id.into()),
            type_: EventType("olap_src.analytics.created".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant(),
            )),
            subject: ArtifactRef(
                subject
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("myelin://t/olap_src/analytics/{agg}")),
            ),
            aggregate: AggregateKey(agg.into()),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: now(),
            recorded_at: now(),
            payload,
        }
    }

    /// The reference owner's source of truth (the analytics facts the OLAP read model projects).
    fn olap_source() -> OlapAnalyticsSource {
        let mut src = OlapAnalyticsSource::new("olap_src");
        src.upsert("issue:PROJ-1", 1, Some("subj:alice"));
        src.upsert("issue:PROJ-2", 2, Some("subj:bob"));
        src.upsert("issue:PROJ-3", 1, None);
        src
    }

    /// Build the LIVE projection: the owner emits its facts as live events and the OLAP consumer
    /// ingests them (the steady-state feed). The `event_id` here is the SAME deterministic snapshot
    /// id the cold rebuild emits (so the dedup ledger would absorb either order — and so the bytes
    /// compared are the read-model projection, not the id stream).
    fn live_projection(src: &OlapAnalyticsSource) -> OlapBusConsumer {
        let mut consumer = OlapBusConsumer::boot(region());
        for draft in src.replay(&SnapshotScope::new("olap_src", "all"), None) {
            let subject = draft.payload.get("subject").and_then(|s| s.as_str());
            let env = live_envelope(
                &draft.aggregate.0,
                draft.version,
                &draft.event_id().0,
                subject,
            );
            consumer
                .ingest(&env)
                .expect("an in-region live event is admitted");
        }
        consumer
    }

    /// A fresh outbox over the frozen 2.3 DDL + a relay→InProcessBus the `*.snapshot`s drain through
    /// (the relay holds a SHARED clone of the outbox, so it sees the reindex-staged rows). The bus +
    /// relay are stable across reindex runs (the broker retains delivered snapshots).
    fn booted_bus() -> (OutboxStore, InProcessBus, Relay<InProcessBus>) {
        assert!(
            OUTBOX_MIGRATION.contains("event_id"),
            "the frozen outbox DDL is present"
        );
        let outbox = OutboxStore::new();
        let bus = InProcessBus::new();
        // A deterministic relay clock (the `published_at` stamp; the real clock is wired at serve).
        let relay = Relay::new(outbox.clone(), bus.clone(), || {
            Timestamp("2026-06-20T00:00:02Z".into())
        });
        (outbox, bus, relay)
    }

    /// **The live feed: the OLAP store is fed by the bus, idempotently (dedup on `event_id`).** A
    /// redelivery of the same event is a no-op — the steady-state consumer is effectively-once.
    #[test]
    fn olap_store_is_fed_by_the_bus_idempotently() {
        let mut consumer = OlapBusConsumer::boot(region());
        let env = live_envelope("issue:PROJ-1", 1, "01J-1", Some("subj:alice"));
        assert_eq!(consumer.ingest(&env).unwrap(), OlapApply::Fresh);
        assert_eq!(
            consumer.ingest(&env).unwrap(),
            OlapApply::Duplicate,
            "a redelivery off the bus is a no-op (dedup on event_id)"
        );
        assert_eq!(consumer.store().doc_count(), 1, "exactly one projected doc");
        // No OLTP-scan backdoor — the read model is fed off the bus only.
        assert_eq!(consumer.store().oltp_scan_path_count(), 0);
    }

    /// **The out-of-region bus event is REJECTED (the per-cell residency boundary, not a global
    /// warehouse).** The live feed inherits the frame's residency WRITE boundary.
    #[test]
    fn an_out_of_region_bus_event_is_rejected_by_the_feed() {
        let mut consumer = OlapBusConsumer::boot(region());
        let mut env = live_envelope("issue:PROJ-1", 1, "01J-1", None);
        env.region = Region("us-east".into());
        let err = consumer
            .ingest(&env)
            .expect_err("out-of-region is rejected");
        assert!(matches!(err, OlapIngestError::OutOfRegion { .. }));
        assert_eq!(
            consumer.store().doc_count(),
            0,
            "nothing projected out-of-region"
        );
    }

    /// **MANDATORY-CORE — the F4 gate: `reindex(scope)` rebuilds the OLAP read model BYTE-MATCHING
    /// live, through the REAL outbox→relay→bus→consumer path (reindex-from-source the ONLY rebuild
    /// path, EI-04 §5).** The cold rebuild's `parity_bytes` are byte-identical to the live
    /// projection's. This is the storage face of BUS-D5.
    #[test]
    fn reindex_from_bus_byte_matches_live() {
        let src = olap_source();

        // LIVE projection (steady-state feed).
        let live = live_projection(&src);
        assert_eq!(
            live.store().doc_count(),
            3,
            "all three facts projected live"
        );

        // COLD rebuild through the REAL reindex `*.snapshot` re-emit seam.
        let (mut outbox, bus, relay) = booted_bus();
        let scope = SnapshotScope::new("olap_src", "all");
        let sources: Vec<&dyn ReindexSource> = vec![&src];
        let (cold, receipt) = reindex_olap_from_bus(
            region(),
            &scope,
            &sources,
            &mut outbox,
            &bus,
            &relay,
            ctx_base(),
            subject_prefix(),
        )
        .expect("the OLAP reindex-from-bus succeeds");

        assert_eq!(
            receipt.snapshots_emitted, 3,
            "three snapshots re-emitted (the rebuild)"
        );
        assert_eq!(
            cold.store().doc_count(),
            3,
            "the cold rebuild projected all three"
        );
        assert_eq!(
            cold.parity_bytes(),
            live.parity_bytes(),
            "COLD reindex == LIVE projection, BYTE-FOR-BYTE (the F4 reindex-parity gate)"
        );
        assert_eq!(
            cold.store().oltp_scan_path_count(),
            0,
            "no OLTP-scan backdoor"
        );
    }

    /// **A second reindex is an idempotent no-op (the deterministic-`event_id` `ON CONFLICT DO
    /// NOTHING`).** Re-running the rebuild emits 0 NEW snapshots and the parity bytes are unchanged
    /// (cold == live stays byte-stable across re-runs).
    #[test]
    fn a_second_reindex_emits_zero_new_snapshots() {
        let src = olap_source();
        let (mut outbox, bus, relay) = booted_bus();
        let scope = SnapshotScope::new("olap_src", "all");
        let sources: Vec<&dyn ReindexSource> = vec![&src];

        let (first, r1) = reindex_olap_from_bus(
            region(),
            &scope,
            &sources,
            &mut outbox,
            &bus,
            &relay,
            ctx_base(),
            subject_prefix(),
        )
        .unwrap();
        assert_eq!(r1.snapshots_emitted, 3, "the first rebuild emits three");

        let (second, r2) = reindex_olap_from_bus(
            region(),
            &scope,
            &sources,
            &mut outbox,
            &bus,
            &relay,
            ctx_base(),
            subject_prefix(),
        )
        .unwrap();
        assert_eq!(
            r2.snapshots_emitted, 0,
            "the re-run emits 0 NEW snapshots (idempotent)"
        );
        assert_eq!(
            r2.snapshots_skipped_duplicate, 3,
            "all three skipped as duplicate"
        );
        assert_eq!(
            first.parity_bytes(),
            second.parity_bytes(),
            "the parity bytes are byte-stable across re-runs"
        );
    }

    /// **An unknown-owner reindex is a LOUD error (never a silent empty rebuild).** Reindexing a
    /// scope whose owner has no registered source fails — a silent empty OLAP rebuild would mask a
    /// wiring bug (EI-02 §4).
    #[test]
    fn an_unknown_owner_reindex_fails_loudly() {
        let src = olap_source();
        let (mut outbox, bus, relay) = booted_bus();
        let scope = SnapshotScope::new("not_registered", "all");
        let sources: Vec<&dyn ReindexSource> = vec![&src];
        let err = reindex_olap_from_bus(
            region(),
            &scope,
            &sources,
            &mut outbox,
            &bus,
            &relay,
            ctx_base(),
            subject_prefix(),
        )
        .expect_err("an unknown-owner reindex must fail loudly");
        assert!(matches!(err, ReindexError::NoSourceForOwner(_)));
    }

    /// **The F4 OLAP reindex-parity signal is GREEN:** cold == live byte-for-byte, no OLTP-scan
    /// backdoor, and the re-run emitted 0 new snapshots. The dated artifact the gate reads.
    #[test]
    fn olap_reindex_parity_signal_is_green() {
        let src = olap_source();
        let live = live_projection(&src);
        let (mut outbox, bus, relay) = booted_bus();
        let scope = SnapshotScope::new("olap_src", "all");
        let sources: Vec<&dyn ReindexSource> = vec![&src];

        let (cold, r1) = reindex_olap_from_bus(
            region(),
            &scope,
            &sources,
            &mut outbox,
            &bus,
            &relay,
            ctx_base(),
            subject_prefix(),
        )
        .unwrap();
        let (_again, r2) = reindex_olap_from_bus(
            region(),
            &scope,
            &sources,
            &mut outbox,
            &bus,
            &relay,
            ctx_base(),
            subject_prefix(),
        )
        .unwrap();

        let signal = OlapReindexParitySignal {
            store: "issue_analytics_olap",
            reindex_matches_live: cold.parity_bytes() == live.parity_bytes(),
            oltp_scan_path_count: cold.store().oltp_scan_path_count(),
            snapshots_emitted_first: r1.snapshots_emitted,
            snapshots_emitted_second: r2.snapshots_emitted,
        };
        assert!(
            signal.is_green(),
            "the F4 OLAP reindex-parity artifact is green: {signal:?}"
        );
        assert_eq!(signal.snapshots_emitted_first, 3);
        assert_eq!(signal.snapshots_emitted_second, 0);
    }

    /// The F4 signal reads RED when ANY invariant fails (the gate is a conjunction — no single green
    /// hides a breach). A cold≠live drift, an OLTP-scan backdoor, or a non-idempotent re-run each
    /// flips `is_green` to false.
    #[test]
    fn olap_reindex_parity_signal_reads_red_when_any_invariant_fails() {
        let green = OlapReindexParitySignal {
            store: "issue_analytics_olap",
            reindex_matches_live: true,
            oltp_scan_path_count: 0,
            snapshots_emitted_first: 3,
            snapshots_emitted_second: 0,
        };
        assert!(green.is_green());
        assert!(!OlapReindexParitySignal {
            reindex_matches_live: false,
            ..green.clone()
        }
        .is_green());
        assert!(!OlapReindexParitySignal {
            oltp_scan_path_count: 1,
            ..green.clone()
        }
        .is_green());
        assert!(!OlapReindexParitySignal {
            snapshots_emitted_second: 1,
            ..green.clone()
        }
        .is_green());
    }

    /// **No OLTP-scan backdoor — STRUCTURAL source assertion (the §3.4 headline, on the LIVE feed).**
    /// The OLAP-feed production code carries NO OLTP-reading construct — reindex-from-source / the bus
    /// consumer are the ONLY feed paths. A future writer who adds an OLTP scan to the feed FAILS this.
    #[test]
    fn no_oltp_scan_backdoor_in_the_feed_structural() {
        let src = include_str!("olap_feed.rs");
        let prod = src
            .split("#[cfg(test)]")
            .next()
            .expect("a production half above tests");
        let code: String = prod
            .lines()
            .filter(|l| !l.trim_start().starts_with("//") && !l.trim_start().starts_with("//!"))
            .collect::<Vec<_>>()
            .join("\n");
        for forbid in ["OltpPool", "from_oltp", "scan_oltp", "OltpConfig"] {
            assert!(
                !code.contains(forbid),
                "OLTP-scan backdoor: the OLAP live feed must not reference `{forbid}` — \
                 reindex-from-source / the bus consumer are the ONLY feed paths (§3.4)"
            );
        }
    }

    /// The subject prefix `bus.consume` reads the drained snapshots back off of — the empty prefix
    /// matches every published subject (the relay publishes each snapshot under its derived subject;
    /// this drill consumes them all, the SAME posture as the BUS-D5 drill's `consume`).
    fn subject_prefix() -> &'static str {
        ""
    }
}
