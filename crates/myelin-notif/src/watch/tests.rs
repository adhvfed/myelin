//! Unit tests for the `inbox watch` live transport (NOTIF-P15 / P-193 — §7, contract 3.5 C4).
//!
//! These pin the resume-cursor MATH (the backfill range `(last_seq, now]`; the `resync_required`
//! boundary at the retention-window edge), the bounded-scope construction (`inbox:<principal>`,
//! never `*`), and the chained reconnect property (subscribe → frames 1..k → drop → frames k+1..m
//! while disconnected → reconnect with `last_seq = k` → backfill k+1..m in order then live; 0 lost,
//! 0 dup). The whole-system D-N11 drill + the CDC pair live in `tests/`.

use super::*;
use crate::list_inbox::AllowAllAuthorize;
use crate::router::InboxProjection;
use crate::RoutedInboxItem;
use myelin_events::firehose::{Firehose, DEFAULT_INFLIGHT_CAP};
use myelin_identity::{Consistency, ConsistencyMode, PrincipalId, PrincipalKind, Zookie};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn principal(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn strong() -> Consistency {
    Consistency { at_least: Zookie("zk".into()), mode: ConsistencyMode::Strong }
}

// ---- the frozen (stream, scope) names + the bounded-scope rule -------------------------------

/// **The frozen `(stream, scope)` for inbox watch (§7).** The stream is the per-tenant fan subject
/// `fan.<tenant>.inbox`; the scope is the BOUNDED selector `inbox:<principal>` — never `*`.
#[test]
fn the_inbox_stream_and_scope_are_the_frozen_bounded_names() {
    let p = principal("p-opaque-1");
    assert_eq!(inbox_stream(&p), "fan.acme.inbox", "the stream is the per-tenant fan subject");
    let scope = inbox_scope(&p).expect("a real principal makes a bounded inbox scope");
    assert_eq!(scope.selector(), "inbox:p-opaque-1", "the scope is the bounded inbox: selector");
}

/// **An inbox scope is a BOUNDED selector, never `*` (the whitelist-not-`*` rule, BUS-3 generalised).**
/// The scope is constructed through the Bus `FirehoseScope::parse` (the one `*`-rejection chokepoint),
/// so an inbox scope is bounded by construction — there is no way to make an `inbox:*` watch.
#[test]
fn an_inbox_scope_is_always_bounded() {
    // a real principal makes a bounded scope; the kind is the (extended) Inbox kind.
    let scope = inbox_scope(&principal("p-1")).expect("bounded");
    assert_eq!(scope.selector(), "inbox:p-1");
    // the bounded-scope GUARANTEE: the transport rejects an unbounded inbox scope at subscribe_raw
    // (the connection-tier entry) — `inbox:*` is over-broad, exactly like `board:*`.
    let mut fh = Firehose::new();
    for raw in ["inbox:*", "*", "inbox:", "inbox"] {
        let r = fh.subscribe_raw("fan.acme.inbox", raw, None);
        assert!(r.is_err(), "an unbounded inbox scope `{raw}` must be rejected, got {r:?}");
        assert!(r.unwrap_err().is_over_broad_scope(), "`{raw}` is an over-broad-scope rejection");
    }
    // a bounded inbox scope subscribes fine (the positive control).
    assert!(
        fh.subscribe_raw("fan.acme.inbox", "inbox:p-1", None).is_ok(),
        "a bounded inbox: scope subscribes"
    );
}

// ---- the resume-cursor math: backfill (last_seq, now] then live, 0 lost ----------------------

/// **A `watch_open` (cursor = None) starts LIVE from now (no backfill).** A fresh viewer joining
/// their own inbox stream sees only frames published AFTER it opened.
#[test]
fn watch_open_starts_live_from_now() {
    let mut fh = Firehose::new();
    let me = principal("p-me");
    // two items already exist (published before the watch opened).
    publish_inbox_frame(&mut fh, &me, "itm-old-1").unwrap();
    publish_inbox_frame(&mut fh, &me, "itm-old-2").unwrap();

    let watch = watch_open(&mut fh, &me).unwrap().into_live().expect("live watch");
    assert!(watch.drain().is_empty(), "a None-cursor watch has no backfill (live from now)");

    // only frames published after the watch opened arrive.
    publish_inbox_frame(&mut fh, &me, "itm-new-3").unwrap();
    publish_inbox_frame(&mut fh, &me, "itm-new-4").unwrap();
    let live: Vec<String> = watch.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(live, vec!["itm-new-3", "itm-new-4"], "a fresh watch receives only post-open frames");
}

/// **THE D-N11 CORE (resume-cursor math): a reconnect backfills `(last_seq, now]` then live, losing
/// ZERO items (§7).** A watcher saw up to seq 2; while disconnected 3,4,5 are published; on
/// `watch_resume(last_seq = 2)` it gets EXACTLY 3,4,5 (the gap) then any subsequent live frame (6) —
/// contiguous, no gap, no duplicate.
#[test]
fn resume_backfills_the_gap_then_goes_live_losing_zero_items() {
    let mut fh = Firehose::new();
    let me = principal("p-me");

    // the watcher saw up to seq 2, then the connection dropped.
    publish_inbox_frame(&mut fh, &me, "itm-1").unwrap();
    publish_inbox_frame(&mut fh, &me, "itm-2").unwrap();
    // while disconnected, 3,4,5 are published (the gap).
    publish_inbox_frame(&mut fh, &me, "itm-3").unwrap();
    publish_inbox_frame(&mut fh, &me, "itm-4").unwrap();
    publish_inbox_frame(&mut fh, &me, "itm-5").unwrap();

    // reconnect with last_seq = 2 → backfill (2, now] = {3,4,5}.
    let watch = watch_resume(&mut fh, &me, 2).unwrap().into_live().expect("in-window resume");
    let backfilled: Vec<(u64, String)> =
        watch.drain().into_iter().map(|f| (f.seq, f.item_id)).collect();
    assert_eq!(
        backfilled,
        vec![(3, "itm-3".into()), (4, "itm-4".into()), (5, "itm-5".into())],
        "the gap (last_seq, now] is replayed in order — ZERO items lost"
    );
    assert_eq!(watch.last_seq(), 5, "the resume cursor advanced to the head");

    // a live frame published now is delivered with NO gap and NO duplicate.
    publish_inbox_frame(&mut fh, &me, "itm-6").unwrap();
    let live: Vec<String> = watch.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(live, vec!["itm-6"], "live continues gap-free after the backfill");
}

/// **The chained reconnect property (EI-01 §4): subscribe → 1..k → DROP → k+1..m (disconnected) →
/// reconnect(last_seq = k) → backfill k+1..m in order then live — 0 lost, 0 dup.** The whole D-N11
/// shape in one chained test over the watch surface.
#[test]
fn chained_subscribe_drop_reconnect_loses_zero_and_duplicates_zero() {
    let mut fh = Firehose::new();
    let me = principal("p-chain");

    // subscribe and receive frames 1..3 (k = 3).
    let first = watch_open(&mut fh, &me).unwrap().into_live().expect("live");
    for i in 1..=3 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }
    let seen_before: Vec<String> = first.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(seen_before, vec!["itm-1", "itm-2", "itm-3"], "received 1..k before the drop");
    let k = first.last_seq();
    assert_eq!(k, 3, "the cursor at the drop is k = 3");
    // "drop" — the first watch handle is dropped (the connection died).
    drop(first);

    // while disconnected, frames k+1..m (4,5,6) are published (m = 6).
    for i in 4..=6 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }

    // reconnect with last_seq = k → backfill k+1..m then live.
    let again = watch_resume(&mut fh, &me, k).unwrap().into_live().expect("in-window resume");
    let backfilled: Vec<String> = again.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(
        backfilled,
        vec!["itm-4", "itm-5", "itm-6"],
        "the disconnected gap k+1..m is backfilled in order — 0 lost"
    );

    // the FULL view across the reconnect is 1..6, each exactly once (0 lost, 0 dup).
    let mut all = seen_before;
    all.extend(backfilled);
    assert_eq!(
        all,
        vec!["itm-1", "itm-2", "itm-3", "itm-4", "itm-5", "itm-6"],
        "across the reconnect: every item exactly once — 0 lost, 0 duplicate"
    );
}

/// A resume at the CURRENT head (a watcher that never actually fell behind) backfills nothing and
/// just continues live — the no-op reconnect (not a resync).
#[test]
fn resume_at_head_is_a_no_op_not_a_resync() {
    let mut fh = Firehose::new();
    let me = principal("p-me");
    for i in 1..=4 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }
    let out = watch_resume(&mut fh, &me, 4).unwrap();
    assert!(!out.is_resync_required(), "a caught-up resume is NOT a resync");
    let watch = out.into_live().expect("live");
    assert!(watch.drain().is_empty(), "a caught-up resume backfills nothing");
}

// ---- the resync_required boundary at the retention-window edge -------------------------------

/// **THE D-N11 RESYNC LEG (the boundary math): an over-old `last_seq` → `resync_required` (§7,
/// NAMED not silent).** A deliberately-SMALL retention window (3 frames); the watcher's `last_seq` is
/// older than the window floor → the gap's head was evicted → `resync_required` → the client falls
/// back to a full `list_inbox` cold rebuild. The boundary is EXACT: the cursor whose first-missing op
/// equals the floor still backfills; one older resyncs.
#[test]
fn over_old_cursor_yields_resync_required_at_the_exact_window_boundary() {
    // the firehose retention window holds only the most-recent 3 frames.
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let me = principal("p-me");
    // publish 1..6 → the window now holds {4,5,6}; 1,2,3 were evicted.
    for i in 1..=6 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }

    // a watcher at last_seq = 2 needs op 3 first — but 3 was evicted → resync_required.
    let out = watch_resume(&mut fh, &me, 2).unwrap();
    assert!(out.is_resync_required(), "an over-old cursor RAISES resync_required (NAMED)");
    if let WatchOutcome::ResyncRequired { last_seq, window_floor } = out {
        assert_eq!(last_seq, 2);
        assert_eq!(window_floor, 4, "the window floor is the oldest held seq (4)");
    } else {
        panic!("expected ResyncRequired");
    }

    // the EXACT boundary: last_seq = 3 → first-missing op = 4 == floor → IN-WINDOW (backfills {4,5,6}).
    let watch = watch_resume(&mut fh, &me, 3).unwrap().into_live().expect("first-missing == floor");
    let ids: Vec<String> = watch.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(ids, vec!["itm-4", "itm-5", "itm-6"], "the boundary cursor backfills exactly");
}

/// **The `resync_required` → cold-rebuild recovery loses ZERO items end-to-end (§7).** After an
/// over-old cursor resyncs, the client rebuilds from SOURCE via `list_inbox` and re-opens live. The
/// rebuilt set is the FULL current inbox — every item the watcher could have lost is recovered (the
/// cold rebuild is the honest, NOT-silent recovery; the gap is never silently dropped).
#[test]
fn resync_required_falls_back_to_a_full_cold_rebuild_zero_lost() {
    let me = principal("p-me");
    // the inbox PROJECTION (the cold-rebuild source) holds the watcher's three items.
    let inbox = InboxProjection::new();
    for id in ["itm-a", "itm-b", "itm-c"] {
        inbox.upsert_for_test(routed(&me, id));
    }

    // the firehose has a tiny window; the watcher's cursor is over-old → resync.
    let mut fh = Firehose::with_limits(2, DEFAULT_INFLIGHT_CAP);
    for id in ["itm-a", "itm-b", "itm-c"] {
        publish_inbox_frame(&mut fh, &me, id).unwrap();
    }
    let out = watch_resume(&mut fh, &me, 0 /* never saw anything but the window rolled */).unwrap();
    // last_seq 0 with an evicted head (window holds {2,3}, floor 2; first-missing 1 < 2) → resync.
    assert!(out.is_resync_required(), "an evicted-head fresh cursor resyncs");

    // the NAMED cold-rebuild: rebuild from source via list_inbox — recovers the FULL inbox.
    let auth = AllowAllAuthorize;
    let recovered = cold_rebuild_item_ids(&inbox, &me, &auth, &strong());
    assert_eq!(
        recovered,
        vec!["itm-a", "itm-b", "itm-c"],
        "the cold rebuild recovers every item — ZERO lost across the resync recovery"
    );

    // after the rebuild the client re-opens a live watch from now (the recovery completes).
    let live = watch_open(&mut fh, &me).unwrap().into_live().expect("re-open live");
    publish_inbox_frame(&mut fh, &me, "itm-d").unwrap();
    assert_eq!(
        live.drain().into_iter().map(|f| f.item_id).collect::<Vec<_>>(),
        vec!["itm-d"],
        "the re-opened watch resumes live delivery"
    );
}

// ---- backpressure: a slow consumer is dropped to resync_required (no unbounded buffer) -------

/// **A SLOW watcher is dropped to `resync_required` (the connection-tier shed budget, OQ-K) — memory
/// stays bounded, the gap is NOT buffered (§7 backpressure).** A watch with a small in-flight cap;
/// the producer races ahead while the consumer never pulls → once the in-flight queue exceeds the cap
/// the watch is dropped to `resync_required` (it then cold-rebuilds), never buffering unboundedly.
#[test]
fn a_slow_watcher_is_dropped_to_resync_required_with_bounded_memory() {
    // in-flight cap 3; a large window so ONLY the slow-consumer drop fires (not the window edge).
    let mut fh = Firehose::with_limits(1024, 3);
    let me = principal("p-slow");
    let watch = watch_open(&mut fh, &me).unwrap().into_live().expect("live");

    // the consumer pulls NOTHING; the producer publishes 3 frames (fills the cap).
    for i in 1..=3 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }
    assert_eq!(watch.ready_len(), 3, "the in-flight queue filled to the cap");
    assert!(!watch.resync_required(), "not dropped yet (at the cap, not over it)");

    // the 4th frame is OVER the cap → the slow watcher is DROPPED to resync_required.
    publish_inbox_frame(&mut fh, &me, "itm-over").unwrap();
    assert!(watch.resync_required(), "a slow watcher is dropped to resync_required (NAMED)");
    assert_eq!(watch.ready_len(), 0, "the buffer is RELEASED — memory bounded, the gap NOT buffered");
    assert!(watch.next().is_none(), "a dropped watch delivers nothing until it cold-rebuilds");
}

/// A watcher that KEEPS UP is never dropped: it pulls each frame as it arrives, so the in-flight
/// queue stays near 0 — the happy path (no drop, no resync).
#[test]
fn a_keeping_up_watcher_is_never_dropped() {
    let mut fh = Firehose::with_limits(1024, 4);
    let me = principal("p-fast");
    let watch = watch_open(&mut fh, &me).unwrap().into_live().expect("live");
    for i in 1..=50u64 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
        let f = watch.next().expect("a keeping-up watcher always has its frame");
        assert_eq!(f.seq, i, "delivered in order");
        assert!(watch.ready_len() <= 1, "in-flight stays bounded for a keeping-up watcher");
    }
    assert!(!watch.resync_required(), "a keeping-up watcher is never dropped");
}

// ---- fan-out + the pointer-only frame body --------------------------------------------------

/// **A frame carries ONLY the `item_id` pointer (references-not-payloads, NOTIF-1).** The firehose
/// frame body is the opaque `item_id`, never the inbox payload — the watcher resolves + humanises the
/// row on a per-viewer READ, the live transport never carries a rendered string.
#[test]
fn a_frame_carries_only_the_item_id_pointer() {
    let mut fh = Firehose::new();
    let me = principal("p-me");
    let frame = publish_inbox_frame(&mut fh, &me, "itm-pointer").unwrap();
    assert_eq!(frame.item_id, "itm-pointer", "the frame body is the item_id pointer, never a payload");
    assert_eq!(frame.seq, 1, "the transport assigns the per-(stream,scope) monotone seq (1)");
}

/// Two watchers on the SAME inbox both receive every live frame (the fan-out property) — e.g. the
/// same principal watching from two devices.
#[test]
fn publish_fans_out_to_every_open_watch_on_the_inbox() {
    let mut fh = Firehose::new();
    let me = principal("p-me");
    let a = watch_open(&mut fh, &me).unwrap().into_live().expect("a");
    let b = watch_open(&mut fh, &me).unwrap().into_live().expect("b");
    publish_inbox_frame(&mut fh, &me, "itm-fanout").unwrap();
    assert_eq!(a.drain().into_iter().map(|f| f.item_id).collect::<Vec<_>>(), vec!["itm-fanout"]);
    assert_eq!(
        b.drain().into_iter().map(|f| f.item_id).collect::<Vec<_>>(),
        vec!["itm-fanout"],
        "both devices receive the live frame"
    );
}

/// Two DIFFERENT principals have INDEPENDENT inbox scopes — one principal's watch never receives
/// another principal's inbox frames (the per-view scope bounding: a client gets ONLY its own slice).
#[test]
fn different_principals_have_independent_inbox_slices() {
    let mut fh = Firehose::new();
    let alice = principal("p-alice");
    let bob = principal("p-bob");
    let alice_watch = watch_open(&mut fh, &alice).unwrap().into_live().expect("alice");
    let bob_watch = watch_open(&mut fh, &bob).unwrap().into_live().expect("bob");

    publish_inbox_frame(&mut fh, &alice, "itm-for-alice").unwrap();
    publish_inbox_frame(&mut fh, &bob, "itm-for-bob").unwrap();

    assert_eq!(
        alice_watch.drain().into_iter().map(|f| f.item_id).collect::<Vec<_>>(),
        vec!["itm-for-alice"],
        "alice's watch sees ONLY alice's inbox slice"
    );
    assert_eq!(
        bob_watch.drain().into_iter().map(|f| f.item_id).collect::<Vec<_>>(),
        vec!["itm-for-bob"],
        "bob's watch sees ONLY bob's inbox slice — never alice's"
    );
}

/// A `RoutedInboxItem` for the cold-rebuild source (the projection rows `list_inbox` reads).
fn routed(recipient: &Principal, item_id: &str) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: Region("fr-par".into()),
        item_id: item_id.into(),
        recipient: recipient.principal_id.0.clone(),
        subject: ArtifactRef(format!("myelin://acme/issue/issue/{item_id}")),
        reason: crate::Reason::Assigned,
        class: crate::Class::Direct,
        origin_event: ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
        dedup_key: item_id.into(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    }
}
