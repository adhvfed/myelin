//! # CDC — the Chat gateway CONSUMES the firehose resume-cursor protocol (row 3.5) + the
//! `MessageStore::resync_from` snapshot fallback (row 2.6). (CHAT-P9 / P-403)
//!
//! The gateway is a CONSUMER of two frozen seams; this CDC pins the gateway's side of each against
//! the REAL provider (no mock of the protocol or the store):
//!  - **3.5 (CONSUMER leg)** — the gateway's per-view subscription rides the BOUNDED `channel:<id>`
//!    scope through the Bus's `*`-rejecting chokepoint (`FirehoseScope::parse`), and a reconnect
//!    `resume(stream, scope, last_seq)` backfills `(last_seq, now]` (0 lost) → an over-window cursor
//!    yields the `resync_required` verdict the gateway turns into the `*.snapshot` fallback. The
//!    PROVIDER is the real `myelin_events::Firehose`.
//!  - **2.6 (CONSUMER leg)** — when `resync_required` fires, the gateway falls back to the real
//!    `MessageStore::resync_from(conversation, cursor)` (the gap-free, ordered clustering-range read)
//!    — the durable snapshot the over-window client cold-rebuilds from. The PROVIDER is the real
//!    `myelin_chat::store::MemHotTier`.
//!
//! Together these prove the gateway speaks the FROZEN protocol shapes with NO local divergence — the
//! scope strings, the per-`(stream,scope)` seq, the `resync_required` vocabulary, and the
//! `resync_from` cursor line up 1:1 across the seam.

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

// ============================================================================================
// 3.5 — the gateway's per-view scope is BOUNDED `channel:<id>` through the *-rejecting chokepoint
// ============================================================================================

/// **CONSUMER 3.5 — the gateway scope is `channel:<id>`, parsed by the BUS-owned `FirehoseScope`
/// (the `*`-rejecting chokepoint); the same selector string the firehose keys its window on.** The
/// gateway's [`chat_channel_scope`] builds the SAME bounded `Channel` scope the Bus protocol admits;
/// an over-broad scope is rejected by the SAME validator (no second chat scope validator).
#[test]
fn gateway_scope_is_the_bus_bounded_channel_scope() {
    // the gateway scope and a directly-parsed Bus scope are the SAME value (1:1 across the seam).
    let gw_scope = chat_channel_scope(CHANNEL).expect("bounded channel scope");
    let bus_scope = FirehoseScope::parse(&format!("channel:{CHANNEL}")).expect("bus scope");
    assert_eq!(
        gw_scope, bus_scope,
        "the gateway speaks the Bus scope shape, no divergence"
    );
    assert_eq!(gw_scope.kind(), ScopeKind::Channel);
    assert_eq!(gw_scope.selector(), format!("channel:{CHANNEL}"));

    // the SAME `*`-rejection applies (the gateway cannot open an unbounded subscription).
    assert!(
        chat_channel_scope("*").is_err(),
        "scope = * is rejected at the gateway's chokepoint"
    );
    assert!(
        chat_channel_scope("").is_err(),
        "an empty scope is rejected"
    );
}

/// **CONSUMER 3.5 — a reconnect backfills `(last_seq, now]` over the REAL Bus transport (0 lost).**
/// The gateway's resume rides the Bus `Firehose::resume`; the provider backfills the gap then live.
#[test]
fn gateway_resume_backfills_the_gap_over_the_real_transport() {
    let mut fh = Firehose::with_limits(4096, DEFAULT_INFLIGHT_CAP);
    let stream = format!("fan.{TENANT}");
    let scope = chat_channel_scope(CHANNEL).unwrap();
    for p in ["m1", "m2", "m3", "m4", "m5"] {
        fh.publish(&stream, &scope, FrameDraft::new(p));
    }
    // resume at last_seq=2 → the Bus backfills {3,4,5} (the gateway's in-window recovery).
    let sub = fh.resume(&stream, &scope, 2).expect("in-window resume");
    let seqs: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        seqs,
        vec![3, 4, 5],
        "the gap is replayed over the real transport — 0 lost"
    );
}

/// **CONSUMER 3.5 — an over-window cursor yields the `resync_required` verdict (NAMED not silent).**
/// The gateway turns THIS verdict into the 2.6 `*.snapshot` fallback (proven below); here the CDC
/// pins that the REAL Bus transport raises it for an evicted gap head.
#[test]
fn gateway_over_window_cursor_yields_resync_required() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP); // window holds 3 frames.
    let stream = format!("fan.{TENANT}");
    let scope = chat_channel_scope(CHANNEL).unwrap();
    for _ in 0..6 {
        fh.publish(&stream, &scope, FrameDraft::new("f"));
    }
    // the gap head for last_seq=2 (seq 3) was evicted → resync_required.
    let err = fh
        .resume(&stream, &scope, 2)
        .expect_err("over-window cursor");
    assert!(
        matches!(err, FirehoseError::ResyncRequired { .. }),
        "the Bus raises resync_required for an evicted gap (the gateway's snapshot trigger)"
    );
}

// ============================================================================================
// 2.6 — the resync_required fallback reads the REAL MessageStore::resync_from snapshot
// ============================================================================================

/// **CONSUMER 2.6 — the `resync_required` fallback is `MessageStore::resync_from` (the gap-free,
/// ordered durable snapshot the over-window client cold-rebuilds from).** Everything strictly after
/// the cursor is returned, in per-conversation order, gap-free — 0 lost. This is the EXACT read the
/// gateway's [`ResumeOutcome::Resync`] returns.
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
    let cursor = all[1].message_id.clone(); // client last rendered m2.

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
        "resync_from returns everything after the cursor, gap-free, ordered — 0 lost"
    );
    // and the snapshot is strictly ordered (0 out-of-order ids).
    for w in snapshot.windows(2) {
        assert!(
            w[0].message_id < w[1].message_id,
            "the snapshot is per-conversation ordered"
        );
    }
}

/// **CONSUMER 3.5/2.6 — the `resync_required -> *.snapshot` fallback names the FROZEN chat snapshot
/// tokens.** The cold-rebuild a resync client re-renders from is the chat channel/message/thread
/// `*.snapshot` reindex projections (already registered in `myelin_chat::events`); the gateway's
/// fallback contract names them, never a literal.
#[test]
fn resync_fallback_names_the_frozen_snapshot_tokens() {
    assert!(
        CHAT_RESYNC_SNAPSHOT_TOKENS.contains(&myelin_chat::events::CHAT_MESSAGE_SNAPSHOT),
        "the message *.snapshot is the resync fallback projection"
    );
    assert!(CHAT_RESYNC_SNAPSHOT_TOKENS.contains(&myelin_chat::events::CHAT_CHANNEL_SNAPSHOT));
    assert!(CHAT_RESYNC_SNAPSHOT_TOKENS.contains(&myelin_chat::events::CHAT_THREAD_SNAPSHOT));
}
