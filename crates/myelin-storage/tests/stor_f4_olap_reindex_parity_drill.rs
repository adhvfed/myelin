//! # STOR F4 — OLAP reindex-parity (cold == live). The P-ST-18 / P-145 headline drill.
//!
//! Drill catalogue: the **F4 family** (`testing-strategy/01 …` §4.1) — the storage face of **BUS-D5**
//! (`reindex(scope)` byte-matches live) ON the OLAP derived store. The OLAP read store fed by the bus
//! (contract 11.6) is rebuilt BYTE-IDENTICALLY by re-emitting `*.snapshot` events through the SAME
//! outbox→relay→bus→live-consumer path — never by scanning OLTP (storage.md §3.4 / EI-04 §5: the
//! derived store rebuilds via the live consumer path ONLY, no bespoke recovery reader).
//!
//! The drill runs the FULL real path (not a shortcut):
//! 1. **LIVE**: the OLAP consumer ingests the owner's live events → the live read-model bytes.
//! 2. **WIPE** the OLAP store (a lost analytics warehouse — the recovery trigger).
//! 3. **`reindex(scope)`**: the OWNER `replay`s → `*.snapshot` via the REAL outbox; the REAL relay
//!    drains them to the InProcessBus; the OLAP consumer ingests the published snapshots (the EXACT
//!    outbox→relay→bus→consumer path a live event takes — `OlapEvent::from_envelope` → `apply`).
//! 4. **ASSERT cold == live**: the rebuilt `parity_bytes` are byte-identical to live.
//! 5. **IDEMPOTENT re-run**: reindexing again emits 0 new snapshots (the deterministic `event_id`
//!    dedups) and a wiped consumer rebuilds byte-stable from the retained delivered snapshots.
//!
//! Telemetry (the dated GREEN artifact): `reindex_parity_hash` matches (cold == live),
//! `oltp_scan_path_count == 0` (no OLTP-scan backdoor), `snapshots_emitted_second == 0` (idempotent).
//!
//! FLOOR (EI-01 §1): the real ClickHouse-class columnar backend + the per-owner real `replay` body
//! (CI/KN/Refs/Issues) land downstream (the columnar store behind the trait; the owner replays in
//! EB-26 / the owners' M3/M4 prompts). This drill proves the SEAM + the cold==live byte-parity over
//! the reference owner — the SAME posture as the Bus's BUS-D5 drill over its reference owner.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EmitContextBase, EventEnvelope,
    EventId, EventType, InProcessBus, OutboxStore, Region, Relay, ReindexSource, SnapshotScope,
    TenantId, Timestamp, Visibility, OUTBOX_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{
    reindex_olap_from_bus, OlapAnalyticsSource, OlapBusConsumer, OlapReindexParitySignal,
};

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

/// The owner's analytics source of truth (the facts the OLAP read model projects).
fn olap_source() -> OlapAnalyticsSource {
    let mut src = OlapAnalyticsSource::new("olap_src");
    src.upsert("issue:PROJ-1", 1, Some("subj:alice"));
    src.upsert("issue:PROJ-2", 2, Some("subj:bob"));
    src.upsert("issue:PROJ-3", 1, None);
    src.upsert("issue:PROJ-4", 3, Some("subj:carol"));
    src
}

/// A live bus envelope for one of the owner's facts (same `event_id`-by-content + same routing fields
/// as the `*.snapshot` of that `(aggregate, version)` — so the cold snapshot is byte-indistinct).
fn live_envelope(agg: &str, event_id: &str, subject: Option<&str>) -> EventEnvelope {
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
        payload: serde_json::json!({ "aggregate_row": agg }),
    }
}

/// The LIVE projection: the owner's facts arrive as live events; the OLAP consumer ingests them. The
/// live `event_id` is the SAME deterministic snapshot id (so the dedup ledger absorbs either order
/// and the BYTES compared are the read model, not the id stream).
fn live_projection(src: &OlapAnalyticsSource) -> OlapBusConsumer {
    let mut consumer = OlapBusConsumer::boot(region());
    for draft in src.replay(&SnapshotScope::new("olap_src", "all"), None) {
        let subject = draft.payload.get("subject").and_then(|s| s.as_str());
        let env = live_envelope(&draft.aggregate.0, &draft.event_id().0, subject);
        consumer.ingest(&env).expect("an in-region live event is admitted");
    }
    consumer
}

fn booted_bus() -> (OutboxStore, InProcessBus, Relay<InProcessBus>) {
    assert!(OUTBOX_MIGRATION.contains("event_id"), "the frozen 2.3 outbox DDL is present");
    let outbox = OutboxStore::new();
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || {
        Timestamp("2026-06-20T00:00:02Z".into())
    });
    (outbox, bus, relay)
}

/// **THE F4 DRILL (dated green artifact): `reindex(scope)` rebuilds the OLAP read model
/// BYTE-MATCHING live, through the REAL outbox→relay→bus→consumer path; the re-run is idempotent.**
#[test]
fn stor_f4_olap_reindex_parity_cold_equals_live() {
    let src = olap_source();

    // (1) LIVE projection (the steady-state feed).
    let live = live_projection(&src);
    let live_bytes = live.parity_bytes();
    assert_eq!(live.store().doc_count(), 4, "four facts projected live");

    // (2) WIPE is implicit: the cold consumer starts empty inside `reindex_olap_from_bus`.
    let (mut outbox, bus, relay) = booted_bus();
    let scope = SnapshotScope::new("olap_src", "all");
    let sources: Vec<&dyn ReindexSource> = vec![&src];

    // (3) REINDEX through the REAL path.
    let (cold, r1) = reindex_olap_from_bus(
        region(), &scope, &sources, &mut outbox, &bus, &relay, ctx_base(), "",
    )
    .expect("the OLAP reindex-from-bus succeeds");

    // (4) cold == live, BYTE-FOR-BYTE (the F4 gate).
    assert_eq!(r1.snapshots_emitted, 4, "reindex re-emitted all 4 aggregates as *.snapshot");
    assert_eq!(cold.store().doc_count(), 4, "the cold rebuild projected all 4");
    assert_eq!(
        cold.parity_bytes(),
        live_bytes,
        "F4: cold == live (byte-identical OLAP rebuild)"
    );
    assert_eq!(cold.store().oltp_scan_path_count(), 0, "no OLTP-scan backdoor");

    // (5) IDEMPOTENT re-run: 0 new snapshots; a WIPED consumer rebuilds byte-stable.
    let (again, r2) = reindex_olap_from_bus(
        region(), &scope, &sources, &mut outbox, &bus, &relay, ctx_base(), "",
    )
    .expect("the re-run succeeds");
    assert_eq!(r2.snapshots_emitted, 0, "the re-run emits 0 NEW snapshots (idempotent)");
    assert_eq!(r2.snapshots_skipped_duplicate, 4, "all four skipped as duplicate");
    assert_eq!(
        again.parity_bytes(),
        live_bytes,
        "the re-run rebuilds the wiped consumer byte-stable (cold == live across re-runs)"
    );

    // The dated GREEN artifact — emit the F4 telemetry observably (EI-01 §3).
    let signal = OlapReindexParitySignal {
        store: "issue_analytics_olap",
        reindex_matches_live: cold.parity_bytes() == live_bytes,
        oltp_scan_path_count: cold.store().oltp_scan_path_count(),
        snapshots_emitted_first: r1.snapshots_emitted,
        snapshots_emitted_second: r2.snapshots_emitted,
    };
    assert!(signal.is_green(), "the F4 OLAP reindex-parity artifact is GREEN: {signal:?}");
    println!(
        "[P-145 STOR-F4 DRILL GREEN 2026-06-20] OLAP reindex-parity: reindex(scope=olap_src) \
         rebuilt the OLAP read model BYTE-MATCHING live through the real outbox→relay→bus→consumer \
         path — reindex_matches_live={}, oltp_scan_path_count={}, snapshots_emitted_first={}, \
         snapshots_emitted_second={} (idempotent re-run). Cold == live, no OLTP-scan backdoor.",
        signal.reindex_matches_live,
        signal.oltp_scan_path_count,
        signal.snapshots_emitted_first,
        signal.snapshots_emitted_second,
    );
}

/// An OLTP-scan backdoor would be the §3.4 contract breach: reindex-from-source is the ONLY rebuild
/// path. The structural guard is `oltp_scan_path_count == 0` (proven by construction in the frame +
/// the feed source-grep); this drill re-asserts the GATE telemetry reads 0 on the rebuilt store.
#[test]
fn stor_f4_no_oltp_scan_backdoor_on_the_rebuild() {
    let src = olap_source();
    let (mut outbox, bus, relay) = booted_bus();
    let scope = SnapshotScope::new("olap_src", "all");
    let sources: Vec<&dyn ReindexSource> = vec![&src];
    let (cold, _r) = reindex_olap_from_bus(
        region(), &scope, &sources, &mut outbox, &bus, &relay, ctx_base(), "",
    )
    .unwrap();
    assert_eq!(
        cold.store().oltp_scan_path_count(),
        0,
        "reindex-from-source is the ONLY rebuild path — the rebuilt OLAP store has no OLTP-scan \
         backdoor (storage.md §3.4)"
    );
}
