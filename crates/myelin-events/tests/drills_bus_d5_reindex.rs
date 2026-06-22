//! # BUS-D5 — reindex-from-cold parity (cold == live). The EB-22 / P-142 headline drill.
//!
//! Drill catalogue row **BUS-D5** (`testing-strategy/01 …` D-5): *wipe a derived store; `reindex(scope)`;
//! assert the rebuild byte-matches live. Gate: **cold == live**.* This is the proof that
//! reindex-from-source is the SAME code path as steady-state ingestion (EI-04 §5.3) — a derived store
//! (Search/Refs/OLAP/Notif read-model) is rebuilt BYTE-IDENTICALLY by re-emitting `*.snapshot` events
//! through the outbox→relay→bus→live-consumer path, never by reading an owner DB.
//!
//! The drill runs the FULL real path (not a shortcut):
//! 1. **LIVE**: the owner writes its truth + the live consumer ingests the live events → the live
//!    projection bytes.
//! 2. **WIPE** the derived store (a cold cache / a lost index — the recovery trigger).
//! 3. **`reindex(scope)`**: ask the owner to `replay(scope, None)` → emit `*.snapshot` via the
//!    OUTBOX, then the **relay drains them to the InProcessBus**, and the cold consumer ingests the
//!    published snapshots (the EXACT outbox→relay→bus→consumer path a live event takes).
//! 4. **ASSERT cold == live**: the rebuilt projection bytes are byte-identical to live; and bridge
//!    the Bus survival signals into the harness §10.2 assertion library so the verdict is loud
//!    (after the drain, `outbox_depth == 0` + `dead_letter_count == 0` — nothing was lost rebuilding).
//! 5. **IDEMPOTENT re-run**: reindexing again emits 0 new snapshots (the deterministic `event_id`
//!    dedups) and the projection is unchanged (cold == live stays byte-stable across re-runs).
//!
//! FLOOR (EI-01 §1): the per-OWNER real `replay` body (CI one-run, KN page-subtree at block
//! granularity, Refs per-blob, Search full reindex) lands with each owner in EB-26 (P-246, M3) + the
//! owners' M3/M4 prompts. This drill proves the SEAM + the `*.snapshot` schema + the reference
//! consumer the per-owner reindexes will ride.

use myelin_events::{
    reindex, BusObservations, BusSignals, BusTransport, DerivedStore, EmitContextBase,
    InProcessBus, OutboxStore, ReferenceReindexSource, ReindexSource, Relay, SnapshotScope,
    Timestamp,
};
use myelin_events::{Actor, Region, TenantId};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn now() -> Timestamp {
    Timestamp("2026-06-20T00:00:00Z".into())
}
fn clock() -> Timestamp {
    Timestamp("2026-06-20T00:00:01Z".into())
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

/// The owner whose source-of-truth the drill replays (CI runs, the canonical sub-artifact-granular
/// scope — "CI one-run scope", contract 2.6).
fn ci_runs() -> ReferenceReindexSource {
    let mut src = ReferenceReindexSource::new("ci", "run");
    src.upsert(
        "ci.run:1",
        1,
        serde_json::json!({ "status": "success", "commit": "abc" }),
    );
    src.upsert(
        "ci.run:2",
        2,
        serde_json::json!({ "status": "failure", "commit": "def" }),
    );
    src.upsert(
        "ci.run:3",
        1,
        serde_json::json!({ "status": "running", "commit": "ghi" }),
    );
    src
}

/// Build the LIVE projection the way steady-state does: the owner emits its facts (modeled as the
/// same `*.snapshot` drafts — that they are the SAME shape is precisely the cold==live invariant) and
/// the live consumer ingests them.
fn live_projection(src: &ReferenceReindexSource, scope: &SnapshotScope) -> DerivedStore {
    let mut store = DerivedStore::new();
    for draft in src.replay(scope, None) {
        // The live event the consumer would have ingested for this aggregate@version.
        let env = live_envelope(&draft);
        store.ingest(&env);
    }
    store
}

/// The envelope a live event would carry for one of the owner's facts (same `event_id`-by-content,
/// same payload — so the cold snapshot of that fact is byte-indistinct from it).
fn live_envelope(draft: &myelin_events::SnapshotDraft) -> myelin_events::EventEnvelope {
    use myelin_events::{AggregateKey, CorrelationId, EventId};
    let id = draft.event_id();
    myelin_events::EventEnvelope {
        event_id: EventId(id.0.clone()),
        type_: draft.type_.clone(),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        subject: draft.subject.clone(),
        aggregate: AggregateKey(draft.aggregate.0.clone()),
        causation_id: None,
        correlation_id: CorrelationId(id.0),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: draft.data_role,
        visibility: draft.visibility,
        pii_key_ref: None,
        occurred_at: now(),
        recorded_at: now(),
        payload: draft.payload.clone(),
    }
}

/// **BUS-D5: wipe a derived store, `reindex(scope)`, the rebuild BYTE-MATCHES live (cold == live).**
#[test]
fn bus_d5_reindex_from_cold_is_byte_identical_to_live() {
    let src = ci_runs();
    let sources: &[&dyn ReindexSource] = &[&src];
    let scope = SnapshotScope::new("ci", "run:all");

    // (1) LIVE: the steady-state projection.
    let live = live_projection(&src, &scope);
    assert_eq!(live.len(), 3, "the live projection has the 3 CI runs");
    let live_bytes = live.parity_bytes();

    // (2) WIPE: the derived store is lost (a cold cache / a dropped index — the recovery trigger).
    let mut cold = DerivedStore::new();
    assert!(cold.is_empty(), "the derived store is wiped (cold)");

    // (3) REINDEX through the REAL outbox→relay→bus→consumer path.
    let mut outbox = OutboxStore::new();
    let receipt = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
    assert_eq!(
        receipt.snapshots_emitted, 3,
        "reindex re-emitted all 3 aggregates as *.snapshot"
    );

    // The relay drains the *.snapshot rows to the broker (the SAME relay a live event rides).
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), clock);
    let drain = relay.drain_to_empty();
    assert_eq!(drain.published, 3, "the relay published all 3 snapshots");

    // The cold consumer ingests every published snapshot (it does NOT read the owner DB).
    for env in bus.consume("myelin://") {
        cold.ingest(&env);
    }

    // (4) cold == live: byte-identical.
    assert_eq!(
        cold.len(),
        3,
        "the cold rebuild materialized all 3 aggregates"
    );
    assert_eq!(
        cold.parity_bytes(),
        live_bytes,
        "BUS-D5: cold == live (byte-identical rebuild)"
    );

    // Bridge the Bus survival signals into the harness §10.2 library — a LOUD green: after the drain
    // nothing is lost (the outbox is empty, no snapshot dead-lettered).
    let obs = BusObservations::default();
    let sig = BusSignals::snapshot(&outbox, &drain, &obs, &now(), 0);
    let mut rec = myelin_events::MetricRecorder::new();
    sig.emit_to(&mut rec);
    let mut signals = SignalSource::new();
    if let Some(v) = rec.scalar(myelin_events::BusSignal::OutboxDepth) {
        signals.set_scalar(SignalName::OutboxDepth, v);
    }
    if let Some(v) = rec.scalar(myelin_events::BusSignal::DeadLetterCount) {
        signals.set_scalar(SignalName::DeadLetterCount, v);
    }
    let depth_ok = signals.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0));
    let dlq_ok = signals.assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0));
    assert!(
        depth_ok.is_green(),
        "outbox drained after the reindex: {depth_ok:?}"
    );
    assert!(
        dlq_ok.is_green(),
        "no snapshot dead-lettered during the reindex: {dlq_ok:?}"
    );
}

/// **BUS-D5 idempotency leg: re-running the reindex emits 0 NEW snapshots and leaves cold == live
/// byte-stable.** The deterministic `event_id` from `(aggregate, version)` makes a re-run an
/// `ON CONFLICT DO NOTHING` no-op (the outbox) and a `consumer_dedup` no-op (the consumer) — so a
/// retried/repeated reindex never doubles an effect.
#[test]
fn bus_d5_reindex_rerun_is_idempotent_and_byte_stable() {
    let src = ci_runs();
    let sources: &[&dyn ReindexSource] = &[&src];
    let scope = SnapshotScope::new("ci", "run:all");
    let live_bytes = live_projection(&src, &scope).parity_bytes();

    let mut outbox = OutboxStore::new();
    let bus = InProcessBus::new();

    // First reindex + drain + ingest.
    let r1 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex 1");
    assert_eq!(r1.snapshots_emitted, 3);
    let relay = Relay::new(outbox.clone(), bus.clone(), clock);
    relay.drain_to_empty();
    let mut cold = DerivedStore::new();
    for env in bus.consume("myelin://") {
        cold.ingest(&env);
    }
    let after_first = cold.parity_bytes();
    assert_eq!(after_first, live_bytes, "first rebuild == live");

    // Re-run: 0 NEW snapshots (deterministic ids already present), and re-ingesting the same
    // delivered set is a dedup no-op → the projection bytes do not change.
    let r2 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex 2");
    assert_eq!(
        r2.snapshots_emitted, 0,
        "a re-run emits 0 NEW snapshots (idempotent)"
    );
    assert_eq!(
        r2.snapshots_skipped_duplicate, 3,
        "all 3 skipped as ON CONFLICT DO NOTHING"
    );
    relay.drain_to_empty();
    for env in bus.consume("myelin://") {
        cold.ingest(&env);
    }
    assert_eq!(
        cold.parity_bytes(),
        after_first,
        "byte-stable across a reindex re-run (no double effect)"
    );
}
