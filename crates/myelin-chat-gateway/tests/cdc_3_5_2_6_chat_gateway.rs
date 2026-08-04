use std::sync::Arc;

use myelin_chat::glue::{chat_channel_scope, CHAT_RESYNC_SNAPSHOT_TOKENS};
use myelin_chat::store::{AuthorKind, ConversationId, MemHotTier, MessageStore, NewMessage};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, Firehose, FirehoseError, FirehoseScope, FrameDraft,
    MonotonicMinter, OutboxStore, OutboxTransaction, ScopeKind, Timestamp, DEFAULT_INFLIGHT_CAP,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
const CHANNEL: &str = "01J0CHANNEL";

fn conv() -> ConversationId {
    ConversationId::new(TENANT, REGION, CHANNEL)
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId(TENANT.into()),
        region: Region(REGION.into()),
        actor: Actor(Principal::stub(
            PrincipalId("svc".into()),
            PrincipalKind::Service,
            TenantId(TENANT.into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:cdc".into())),
    }
}

fn append(store: &MemHotTier, ob: &OutboxStore, minter: &Arc<MonotonicMinter>, nonce: &str) {
    let mut tx: OutboxTransaction = ob.begin(minter.clone(), ctx_base());
    store
        .append(
            &mut tx,
            NewMessage {
                conv: conv(),
                thread_root_id: None,
                author: "alice".into(),
                author_kind: AuthorKind::Human,
                body_inline: nonce.as_bytes().to_vec(),
                body_nodes: Vec::new(),
                client_nonce: nonce.into(),
            },
        )
        .expect("append");
    tx.commit().expect("commit");
}

#[test]
fn gateway_scope_is_the_bus_bounded_channel_scope() {
    let gw_scope = chat_channel_scope(CHANNEL).expect("bounded channel scope");
    let bus_scope = FirehoseScope::parse(&format!("channel:{CHANNEL}")).expect("bus scope");
    assert_eq!(
        gw_scope, bus_scope,
        "the gateway speaks the Bus scope shape, no divergence"
    );
    assert_eq!(gw_scope.kind(), ScopeKind::Channel);
    assert_eq!(gw_scope.selector(), format!("channel:{CHANNEL}"));

    assert!(
        chat_channel_scope("*").is_err(),
        "scope = * is rejected at the gateway's chokepoint"
    );
    assert!(
        chat_channel_scope("").is_err(),
        "an empty scope is rejected"
    );
}

#[test]
fn gateway_resume_backfills_the_gap_over_the_real_transport() {
    let mut fh = Firehose::with_limits(4096, DEFAULT_INFLIGHT_CAP);
    let stream = format!("fan.{TENANT}");
    let scope = chat_channel_scope(CHANNEL).unwrap();
    for p in ["m1", "m2", "m3", "m4", "m5"] {
        fh.publish(&stream, &scope, FrameDraft::new(p));
    }
    let sub = fh.resume(&stream, &scope, 2).expect("in-window resume");
    let seqs: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        seqs,
        vec![3, 4, 5],
        "the gap is replayed over the real transport - 0 lost"
    );
}

#[test]
fn gateway_over_window_cursor_yields_resync_required() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let stream = format!("fan.{TENANT}");
    let scope = chat_channel_scope(CHANNEL).unwrap();
    for _ in 0..6 {
        fh.publish(&stream, &scope, FrameDraft::new("f"));
    }
    let err = fh
        .resume(&stream, &scope, 2)
        .expect_err("over-window cursor");
    assert!(
        matches!(err, FirehoseError::ResyncRequired { .. }),
        "the Bus raises resync_required for an evicted gap (the gateway's snapshot trigger)"
    );
}

#[test]
fn resync_from_is_the_gap_free_ordered_snapshot() {
    let store = MemHotTier::new();
    let (ob, minter) = (OutboxStore::new(), Arc::new(MonotonicMinter::new()));
    for n in ["m1", "m2", "m3", "m4", "m5", "m6"] {
        append(&store, &ob, &minter, n);
    }
    let all = store
        .range(&conv(), myelin_chat::store::RangeCursor::Recent, 100)
        .unwrap();
    let cursor = all[1].message_id.clone();

    let snapshot = store.resync_from(&conv(), &cursor).expect("resync_from");
    let bodies: Vec<Vec<u8>> = snapshot.iter().map(|m| m.body_inline.clone()).collect();
    assert_eq!(
        bodies,
        vec![
            b"m3".to_vec(),
            b"m4".to_vec(),
            b"m5".to_vec(),
            b"m6".to_vec()
        ],
        "resync_from returns everything after the cursor, gap-free, ordered - 0 lost"
    );
    for w in snapshot.windows(2) {
        assert!(
            w[0].message_id < w[1].message_id,
            "the snapshot is per-conversation ordered"
        );
    }
}

#[test]
fn resync_fallback_names_the_frozen_snapshot_tokens() {
    assert!(
        CHAT_RESYNC_SNAPSHOT_TOKENS.contains(&myelin_chat::events::CHAT_MESSAGE_SNAPSHOT),
        "the message *.snapshot is the resync fallback projection"
    );
    assert!(CHAT_RESYNC_SNAPSHOT_TOKENS.contains(&myelin_chat::events::CHAT_CHANNEL_SNAPSHOT));
    assert!(CHAT_RESYNC_SNAPSHOT_TOKENS.contains(&myelin_chat::events::CHAT_THREAD_SNAPSHOT));
}
