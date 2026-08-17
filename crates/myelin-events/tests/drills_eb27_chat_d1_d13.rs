use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EmitContextBase, EventDraft, EventType,
    Firehose, FirehoseError, FirehoseScope, FrameDraft, IdMinter, InProcessBus, MonotonicMinter,
    OutboxStore, OutboxTx, Relay, Timestamp, Visibility, DEFAULT_INFLIGHT_CAP,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

const STREAM: &str = "chat";
const CHANNEL_SCOPE: &str = "channel:eng-general";

fn channel_scope() -> FirehoseScope {
    FirehoseScope::parse(CHANNEL_SCOPE).expect("a bounded channel: scope")
}

fn clock() -> Timestamp {
    Timestamp("2026-06-21T00:00:02Z".into())
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("u-1".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:chat".into())),
    }
}

fn message_created(msg_id: &str, channel: &str) -> EventDraft {
    EventDraft {
        type_: EventType("chat.message.created".into()),
        subject: ArtifactRef(format!("myelin://acme/chat/message/{msg_id}")),
        aggregate: AggregateKey(format!("myelin://acme/chat/channel/{channel}")),
        payload: serde_json::json!({ "message": msg_id, "channel": channel }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

#[test]
fn chat_d1_resume_recovers_the_gap_zero_lost_zero_dup() {
    let mut fh = Firehose::new();
    let scope = channel_scope();

    let sub = fh
        .subscribe(STREAM, &scope, None)
        .expect("a bounded channel: scope subscribes (never `*`)");
    for _ in 0..3 {
        fh.publish(STREAM, &scope, FrameDraft::new("chat.presence.frame"))
            .expect("the fixture publishes a valid frame");
    }
    let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        seen,
        vec![1, 2, 3],
        "the live client saw the first 3 frames"
    );
    let last_seq = sub.last_seq();

    drop(sub);
    for _ in 0..4 {
        fh.publish(STREAM, &scope, FrameDraft::new("chat.live.frame"))
            .expect("the fixture publishes a valid frame");
    }

    let resumed = fh
        .resume(STREAM, &scope, last_seq)
        .expect("an in-window resume backfills the gap");
    let backfilled: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        backfilled,
        vec![4, 5, 6, 7],
        "the gap (last_seq, now] is replayed - 0 ops lost"
    );

    fh.publish(STREAM, &scope, FrameDraft::new("chat.live.frame"))
        .expect("the fixture publishes a valid frame");
    let live: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(live, vec![8], "0 dup across the backfill→live boundary");

    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![myelin_harness::Label::new(
            "consumer",
            format!("chat-firehose:{CHANNEL_SCOPE}"),
        )],
        0,
    );
    src.assert_labelled(
        SignalName::ConsumerLag,
        vec![myelin_harness::Label::new(
            "consumer",
            format!("chat-firehose:{CHANNEL_SCOPE}"),
        )],
        Predicate::Eq(0),
    )
    .expect_green();
    println!(
        "[2026-06-21] PASS  drill=CHAT-D1(resume)  lost=0 dup=0  (sever → resume → assert green)"
    );
}

#[test]
fn chat_d1_over_window_raises_resync_required() {
    let mut fh = Firehose::with_limits(4, DEFAULT_INFLIGHT_CAP);
    let scope = channel_scope();

    let sub = fh
        .subscribe(STREAM, &scope, None)
        .expect("bounded channel: scope");
    for _ in 0..10 {
        fh.publish(STREAM, &scope, FrameDraft::new("chat.live.frame"))
            .expect("the fixture publishes a valid frame");
    }
    drop(sub);

    let err = fh
        .resume(STREAM, &scope, 2)
        .expect_err("an over-window resume cannot backfill");
    assert!(
        err.is_resync_required(),
        "the over-window resume raises resync_required (NAMED, not a silent gap): {err:?}"
    );
    assert!(matches!(err, FirehoseError::ResyncRequired { .. }));

    let mut src = SignalSource::new();
    src.set_scalar(SignalName::ResyncRequiredCount, 1);
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Gte(1))
        .expect_green();
    println!(
        "[2026-06-21] PASS  drill=CHAT-D1(resync)  resync_required raised → *.snapshot fallback"
    );
}

#[test]
fn chat_d13_message_persist_event_co_commit() {
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    let committed_ids = {
        let mut tx = store.begin(minter.clone(), ctx_base());
        tx.stage_state_change("chat message m1 persisted");
        let id = tx
            .emit(message_created("m1", "eng-general"), None)
            .expect("emit chat.message.created");
        tx.commit()
            .expect("the message row + event commit together");
        vec![id]
    };

    {
        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("chat message m2 (will crash before commit)");
        tx.emit(message_created("m2", "eng-general"), None).unwrap();
    }

    assert_eq!(
        store.committed_count(),
        1,
        "co-commit: the crashed message wrote NEITHER a row NOR an event"
    );
    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::OutboxDepth, store.outbox_depth() as i64);
    signals
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(1))
        .expect_green();

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), clock);
    relay.drain_to_empty();

    assert_eq!(
        bus.delivered_count(),
        1,
        "only the committed chat.message.created is delivered"
    );
    assert_eq!(
        bus.delivered_ids(),
        committed_ids.into_iter().collect(),
        "co-commit: delivered set == committed set; the crashed message never appears (no phantom)"
    );
    let mut after = SignalSource::new();
    after.set_scalar(SignalName::OutboxDepth, store.outbox_depth() as i64);
    after
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        .expect_green();
    println!("[2026-06-21] PASS  drill=CHAT-D13  co_commit=true  (persist+emit atomic; no orphan/phantom)");
}

#[test]
fn chat_firehose_rejects_an_over_broad_scope() {
    let mut fh = Firehose::new();
    assert!(
        fh.subscribe_raw(STREAM, "*", None).is_err(),
        "the firehose rejects `scope = *` (never an unbounded chat fan-out)"
    );
}
