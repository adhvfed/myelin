//! # CHAT-D1 / CHAT-D13 — the Bus's firehose + co-commit under Chat's load (EB-27 / P-327, M4)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` rows
//! **CHAT-D1** (F5/OQ-J — sever the gateway↔firehose mid-publish → `resume(stream, scope, last_seq)`
//! recovers the gap **0 lost / 0 dup**; an over-window `last_seq` → `resync_required` → `*.snapshot`
//! fallback, still 0 lost) + **CHAT-D13** (F5 — crash between message persist and event emit →
//! either BOTH committed or NEITHER; the message and `chat.message.created` are ATOMIC; no
//! orphan/phantom — the co-commit proof).
//!
//! ## What these prove (the Bus's NARROW carriage under Chat's load)
//! Chat is the heaviest firehose producer (presence/typing/live delivery over `channel:<id>` scopes)
//! and a durable producer (`chat.message.created` via the outbox). The Bus owns the CARRIAGE: the
//! firehose resume-cursor transport (contract 3.5) + the outbox emit-iff-committed co-commit (BUS-D4,
//! contract 2.2). These drills assert the Bus's guarantees UNDER Chat's specific load shape:
//! - **CHAT-D1** rides the Bus firehose on a `channel:<id>` scope (Chat's hot-channel storm surface);
//! - **CHAT-D13** rides the Bus outbox co-commit (the message row + its `chat.message.created` event
//!   share ONE transaction — both or neither).
//!
//! Chat OWNS its message model + presence semantics; this Bus prompt provides + drills the CARRIAGE
//! they ride (the §4.12-style narrow role). The verdict is read off the FROZEN §10.2 harness
//! assertion library (the firehose seq-gap → `ConsumerLag`; the resync count → `ResyncRequiredCount`;
//! the outbox co-commit → `OutboxDepth`).

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
/// Chat's hot-channel firehose scope (the `channel:<id>` storm surface — the same bounded selector
/// grammar a hot board / hot doc uses, OQ-J).
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

/// A `chat.message.created` durable draft (the message row's co-committed event). The body PII rides
/// behind a per-subject DEK in the real store; the drill payload is references-only.
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

/// **CHAT-D1 LEG 1 — sever the gateway↔firehose mid-publish; `resume(last_seq)` recovers the gap
/// (0 lost / 0 dup) on a Chat `channel:` scope.**
#[test]
fn chat_d1_resume_recovers_the_gap_zero_lost_zero_dup() {
    let mut fh = Firehose::new();
    let scope = channel_scope();

    // A client subscribes live to the hot channel and sees the first 3 presence/live frames.
    let sub = fh
        .subscribe(STREAM, &scope, None)
        .expect("a bounded channel: scope subscribes (never `*`)");
    for _ in 0..3 {
        fh.publish(STREAM, &scope, FrameDraft::new("chat.presence.frame"));
    }
    let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        seen,
        vec![1, 2, 3],
        "the live client saw the first 3 frames"
    );
    let last_seq = sub.last_seq();

    // SEVER the gateway↔firehose connection. While down, the producer keeps publishing the gap.
    drop(sub);
    for _ in 0..4 {
        fh.publish(STREAM, &scope, FrameDraft::new("chat.live.frame"));
    }

    // RECONNECT: resume(last_seq=3) → backfill (3, now] = {4,5,6,7}, then live.
    let resumed = fh
        .resume(STREAM, &scope, last_seq)
        .expect("an in-window resume backfills the gap");
    let backfilled: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        backfilled,
        vec![4, 5, 6, 7],
        "the gap (last_seq, now] is replayed — 0 ops lost"
    );

    // A new live frame after the resume continues the seq with NO duplicate across the boundary.
    fh.publish(STREAM, &scope, FrameDraft::new("chat.live.frame"));
    let live: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(live, vec![8], "0 dup across the backfill→live boundary");

    // The seq-gap is 0 (contiguous) → through the §10.2 harness assertion library.
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

/// **CHAT-D1 LEG 2 — an over-window `last_seq` → `resync_required` (NAMED, not silent) → the
/// `*.snapshot` replay fallback, still 0 lost.** The retention window is bounded; a client behind the
/// window floor cannot backfill — the Bus raises `resync_required` so Chat falls back to a
/// `chat.*.snapshot` replay (EB-22), never a silent gap.
#[test]
fn chat_d1_over_window_raises_resync_required() {
    // A small bounded window (the Chat hot-channel retention bound).
    let mut fh = Firehose::with_limits(4, DEFAULT_INFLIGHT_CAP);
    let scope = channel_scope();

    let sub = fh
        .subscribe(STREAM, &scope, None)
        .expect("bounded channel: scope");
    // Publish far past the window capacity → the window floor advances beyond an old last_seq.
    for _ in 0..10 {
        fh.publish(STREAM, &scope, FrameDraft::new("chat.live.frame"));
    }
    drop(sub);

    // A client whose last_seq (2) is now older than the window floor → resync_required.
    let err = fh
        .resume(STREAM, &scope, 2)
        .expect_err("an over-window resume cannot backfill");
    assert!(
        err.is_resync_required(),
        "the over-window resume raises resync_required (NAMED, not a silent gap): {err:?}"
    );
    assert!(matches!(err, FirehoseError::ResyncRequired { .. }));

    // The resync signal fired → the §10.2 ResyncRequiredCount row reads >= 1 (the snapshot fallback
    // is the EB-22 `*.snapshot` replay, proven cold == live by BUS-D5).
    let mut src = SignalSource::new();
    src.set_scalar(SignalName::ResyncRequiredCount, 1);
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Gte(1))
        .expect_green();
    println!(
        "[2026-06-21] PASS  drill=CHAT-D1(resync)  resync_required raised → *.snapshot fallback"
    );
}

/// **CHAT-D13 — message-persist ↔ `chat.message.created` co-commit (BUS-D4 under Chat's load):
/// either BOTH committed or NEITHER; no orphan/phantom.** The Bus's outbox carries Chat's durable
/// message event in the SAME transaction as the message row — a crash between persist and emit writes
/// NEITHER (emit-iff-committed).
#[test]
fn chat_d13_message_persist_event_co_commit() {
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    // (A) A COMMITTED message-persist: the message row + its chat.message.created commit TOGETHER.
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

    // (B) A CRASHED message-persist: the message staged + the event emitted, then the transaction is
    //     DROPPED without commit (the crash between persist and emit). Co-commit: this writes NOTHING
    //     — no orphan message, no phantom event.
    {
        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("chat message m2 (will crash before commit)");
        tx.emit(message_created("m2", "eng-general"), None).unwrap();
        // crash: tx dropped here without commit.
    }

    // The crashed transaction left no rows: exactly the committed 1 (not 2) — no phantom event.
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

    // The relay delivers exactly the committed message event — never the crashed one (no orphan).
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

/// Guard: the firehose is a `channel:` scope (Chat's hot-channel surface), never `*` — the
/// over-broad-scope rejection is the transport chokepoint Chat's load rides through.
#[test]
fn chat_firehose_rejects_an_over_broad_scope() {
    let mut fh = Firehose::new();
    assert!(
        fh.subscribe_raw(STREAM, "*", None).is_err(),
        "the firehose rejects `scope = *` (never an unbounded chat fan-out)"
    );
}
