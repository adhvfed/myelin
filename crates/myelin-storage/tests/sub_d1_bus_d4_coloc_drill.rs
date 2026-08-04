use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EmitContextBase, EventDraft, EventType,
    IdMinter, InProcessBus, MonotonicMinter, OutboxStore, Region, Relay, TenantId, Timestamp,
    Visibility,
};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{ColocError, ColocatedOltp, OltpConfig};

fn db() -> ColocatedOltp {
    let config = OltpConfig {
        max_pool_size: 16,
        statement_timeout_ms: 3_000,
        per_tenant_in_flight_cap: 8,
    };
    ColocatedOltp::open(
        config,
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
    )
    .expect("co-located OLTP store opens")
}

fn ctx() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(Principal::stub(
            PrincipalId("u1".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
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

#[test]
fn sub_d1_kill_between_commit_and_publish_zero_ghost_zero_lost() {
    let mut signals = SignalSource::new();
    let db = db();
    const N: usize = 32;

    let mut committed = std::collections::HashSet::new();
    for i in 0..N {
        let mut tx = db.begin(ctx()).unwrap();
        tx.stage_state(format!("INSERT issue I{i}"));
        let id = tx.emit(draft(&format!("I{i}")), None).unwrap();
        tx.commit().unwrap();
        committed.insert(id);
    }
    assert_eq!(
        db.outbox_depth(),
        N,
        "all committed events are parked in the co-located outbox"
    );

    let bus = InProcessBus::new();
    let relay = Relay::new(db.outbox().clone(), bus.clone(), || {
        Timestamp("2026-06-19T00:00:02Z".into())
    });

    bus.sever();
    let severed = relay.drain_once();
    assert_eq!(severed.published, 0, "a severed broker delivers nothing");
    assert_eq!(
        db.outbox_depth(),
        N,
        "0 lost - the committed rows are still parked, not dropped"
    );

    bus.heal();
    let drained = relay.drain_to_empty();
    assert_eq!(
        drained.published, N,
        "every committed event is delivered after the heal (0 lost)"
    );

    assert_eq!(
        bus.delivered_ids(),
        committed,
        "delivered set == committed set (0 ghost, 0 lost)"
    );
    assert_eq!(
        bus.delivered_count(),
        N,
        "exactly-once delivery - no double-publish"
    );

    signals.set_scalar(SignalName::OutboxDepth, db.outbox_depth() as i64);
    signals.set_scalar(
        SignalName::DeadLetterCount,
        db.outbox().dead_letter_count() as i64,
    );
    signals
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        .expect_green();
    signals
        .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-016 DRILL GREEN 2026-06-19] SUB-D1 (OLTP/outbox co-location half): \
         co-committed={N}, severed broker → published=0 / outbox_depth={N} (0 lost), \
         healed → published={N}, delivered_set==committed_set ({N} ids, 0 ghost), \
         outbox_depth→0, dead_letters=0",
        N = N
    );
}

#[test]
fn bus_d4_emit_iff_committed_both_directions() {
    let mut signals = SignalSource::new();
    let db = db();

    {
        let mut tx = db.begin(ctx()).unwrap();
        tx.stage_state("INSERT issue CRASH");
        tx.emit(draft("CRASH"), None).unwrap();
    }
    assert_eq!(
        db.outbox_depth(),
        0,
        "a dropped tx emits no event (no event without state)"
    );

    {
        let mut tx = db.begin(ctx()).unwrap();
        tx.stage_state("INSERT issue FAULT");
        tx.emit(draft("FAULT"), None).unwrap();
        let r = tx.commit_with_state_fault("disk full");
        assert!(matches!(r, Err(ColocError::CommitRolledBack(_))));
    }
    assert_eq!(
        db.outbox_depth(),
        0,
        "a rolled-back state write emits no event (no state without event)"
    );

    let mut tx = db.begin(ctx()).unwrap();
    tx.stage_state("INSERT issue OK");
    tx.emit(draft("OK"), None).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        db.outbox_depth(),
        1,
        "emit-iff-committed: exactly the one committed event"
    );
    assert_eq!(
        db.outbox().committed_count(),
        1,
        "no ghost from the two aborted txs"
    );

    signals.set_scalar(SignalName::OutboxDepth, db.outbox_depth() as i64);
    signals.set_scalar(
        SignalName::DeadLetterCount,
        db.outbox().dead_letter_count() as i64,
    );
    signals
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(1))
        .expect_green();
    signals
        .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-016 DRILL GREEN 2026-06-19] BUS-D4 (OLTP/outbox co-location half): \
         dropped-tx → 0 events, injected-mid-tx-state-fault → 0 events (both roll back), \
         committed-tx → 1 event; emit_iff_committed==true, outbox_depth=1, dead_letters=0"
    );
}
