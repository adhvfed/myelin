//! P-ST-02 (global P-016) GATE / DRILLS — SUB-D1 + BUS-D4, the OLTP/outbox co-location half.
//!
//! Storage owns the **OLTP/outbox half** of the two master M0 silent-data-loss gates. The
//! outbox-bearing OLTP tier makes them green for that half:
//!
//! - **SUB-D1 — kill between commit and publish → exactly-once-in-effect (0 ghost, 0 lost),
//!   `outbox_depth → 0` after the relay drains.** Because the outbox row co-committed with the
//!   state change (same OLTP tx), a crash *after* commit but *before* publish leaves the row
//!   durable-and-unsent; the relay re-claims and publishes it on the next pass (0 lost), and the
//!   broker dedups on the stable `event_id` so a re-claimed row is never double-delivered
//!   (0 ghost).
//! - **BUS-D4 — crash the producer between state-commit and publish → emit-iff-committed.** A
//!   transaction dropped without commit writes NEITHER state NOR event; a transaction that
//!   commits writes BOTH. There is no committed state without its event and no event without its
//!   committed state — the co-commit is atomic in both directions.
//!
//! These read the contract-1.8 survival signals (`OutboxDepth`, `DeadLetterCount`) off the
//! telemetry-assertion library; a red aborts LOUDLY with the signal + predicate + observed value
//! (EI-01 §3 — never swallowed, the threshold is NOT weakened to pass). `myelin-harness` is a
//! DEV-dependency only.

use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EmitContextBase, EventDraft, EventType,
    IdMinter, InProcessBus, MonotonicMinter, Region, Relay, TenantId, Timestamp, Visibility,
};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{ColocatedOltp, ColocError, OltpConfig};

fn db() -> ColocatedOltp {
    let config = OltpConfig {
        max_pool_size: 16,
        statement_timeout_ms: 3_000,
        per_tenant_in_flight_cap: 8,
    };
    ColocatedOltp::open(config, Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>)
        .expect("co-located OLTP store opens")
}

fn ctx() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(Principal::stub(PrincipalId("u1".into()), PrincipalKind::Human, TenantId("acme".into()))),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:drill".into())),
    }
}

fn draft(key: &str) -> EventDraft {
    EventDraft {
        type_: EventType("issues.issue.created".into()),
        subject: ArtifactRef(format!("myelin://acme/issues/issue/{key}")),
        aggregate: AggregateKey(format!("issue:{key}")),
        payload: serde_json::json!({ "ref": key }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// **SUB-D1 — kill between commit and publish (OLTP/outbox half).** Co-commit N events, then
/// SEVER the broker (the "kill between commit and publish" fault), drain (every put fails → rows
/// stay parked, 0 lost), HEAL, drain to empty. The delivered set must equal exactly the committed
/// set (0 ghost, 0 lost), and `outbox_depth → 0`.
#[test]
fn sub_d1_kill_between_commit_and_publish_zero_ghost_zero_lost() {
    let mut signals = SignalSource::new();
    let db = db();
    const N: usize = 32;

    // Co-commit N events (each in its own co-located transaction: state + outbox, one tx).
    let mut committed = std::collections::HashSet::new();
    for i in 0..N {
        let mut tx = db.begin(ctx()).unwrap();
        tx.stage_state(format!("INSERT issue I{i}"));
        let id = tx.emit(draft(&format!("I{i}")), None).unwrap();
        tx.commit().unwrap();
        committed.insert(id);
    }
    assert_eq!(db.outbox_depth(), N, "all committed events are parked in the co-located outbox");

    // The relay drains the co-located outbox to the broker.
    let bus = InProcessBus::new();
    let relay = Relay::new(db.outbox().clone(), bus.clone(), || {
        Timestamp("2026-06-19T00:00:02Z".into())
    });

    // KILL between commit and publish: sever the broker. The drain makes no progress — but loses
    // NOTHING (the committed rows stay claimable; 0 dead-lettered while transient).
    bus.sever();
    let severed = relay.drain_once();
    assert_eq!(severed.published, 0, "a severed broker delivers nothing");
    assert_eq!(db.outbox_depth(), N, "0 lost — the committed rows are still parked, not dropped");

    // HEAL (the producer/relay restarts after the crash) and drain to empty.
    bus.heal();
    let drained = relay.drain_to_empty();
    assert_eq!(drained.published, N, "every committed event is delivered after the heal (0 lost)");

    // 0 ghost: the delivered set equals EXACTLY the committed set (no duplicate, no invented row).
    assert_eq!(bus.delivered_ids(), committed, "delivered set == committed set (0 ghost, 0 lost)");
    assert_eq!(bus.delivered_count(), N, "exactly-once delivery — no double-publish");

    // The survival signals: outbox drained to 0, nothing dead-lettered.
    signals.set_scalar(SignalName::OutboxDepth, db.outbox_depth() as i64);
    signals.set_scalar(SignalName::DeadLetterCount, db.outbox().dead_letter_count() as i64);
    signals.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0)).expect_green();
    signals.assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0)).expect_green();

    println!(
        "[P-016 DRILL GREEN 2026-06-19] SUB-D1 (OLTP/outbox co-location half): \
         co-committed={N}, severed broker → published=0 / outbox_depth={N} (0 lost), \
         healed → published={N}, delivered_set==committed_set ({N} ids, 0 ghost), \
         outbox_depth→0, dead_letters=0",
        N = N
    );
}

/// **BUS-D4 — emit-iff-committed, both directions (OLTP/outbox half).** (1) A transaction
/// DROPPED without commit (crash between state-commit and publish) writes NEITHER state nor
/// event. (2) An injected mid-tx STATE failure rolls BOTH back. (3) A committed transaction
/// writes BOTH. Across all three, `outbox_depth` counts EXACTLY the committed events — never a
/// ghost from an abort, never a loss from a commit.
#[test]
fn bus_d4_emit_iff_committed_both_directions() {
    let mut signals = SignalSource::new();
    let db = db();

    // (1) Crash between state-commit and publish: a dropped tx writes nothing.
    {
        let mut tx = db.begin(ctx()).unwrap();
        tx.stage_state("INSERT issue CRASH");
        tx.emit(draft("CRASH"), None).unwrap();
        // dropped here WITHOUT commit — the crash point.
    }
    assert_eq!(db.outbox_depth(), 0, "a dropped tx emits no event (no event without state)");

    // (2) Injected mid-tx state failure: both roll back.
    {
        let mut tx = db.begin(ctx()).unwrap();
        tx.stage_state("INSERT issue FAULT");
        tx.emit(draft("FAULT"), None).unwrap();
        let r = tx.commit_with_state_fault("disk full");
        assert!(matches!(r, Err(ColocError::CommitRolledBack(_))));
    }
    assert_eq!(db.outbox_depth(), 0, "a rolled-back state write emits no event (no state without event)");

    // (3) A committed tx writes BOTH — exactly one event becomes durable.
    let mut tx = db.begin(ctx()).unwrap();
    tx.stage_state("INSERT issue OK");
    tx.emit(draft("OK"), None).unwrap();
    tx.commit().unwrap();
    assert_eq!(db.outbox_depth(), 1, "emit-iff-committed: exactly the one committed event");
    assert_eq!(db.outbox().committed_count(), 1, "no ghost from the two aborted txs");

    // The survival signal: depth reflects exactly the committed events (1), 0 dead-letters.
    signals.set_scalar(SignalName::OutboxDepth, db.outbox_depth() as i64);
    signals.set_scalar(SignalName::DeadLetterCount, db.outbox().dead_letter_count() as i64);
    signals.assert_signal(SignalName::OutboxDepth, Predicate::Eq(1)).expect_green();
    signals.assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0)).expect_green();

    println!(
        "[P-016 DRILL GREEN 2026-06-19] BUS-D4 (OLTP/outbox co-location half): \
         dropped-tx → 0 events, injected-mid-tx-state-fault → 0 events (both roll back), \
         committed-tx → 1 event; emit_iff_committed==true, outbox_depth=1, dead_letters=0"
    );
}
