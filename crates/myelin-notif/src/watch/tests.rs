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
    Consistency {
        at_least: Zookie("zk".into()),
        mode: ConsistencyMode::Strong,
    }
}

#[test]
fn the_inbox_stream_and_scope_are_the_frozen_bounded_names() {
    let p = principal("p-opaque-1");
    assert_eq!(
        inbox_stream(&p),
        "fan.acme.inbox",
        "the stream is the per-tenant fan subject"
    );
    let scope = inbox_scope(&p).expect("a real principal makes a bounded inbox scope");
    assert_eq!(
        scope.selector(),
        "inbox:p-opaque-1",
        "the scope is the bounded inbox: selector"
    );
}

#[test]
fn an_inbox_scope_is_always_bounded() {
    let scope = inbox_scope(&principal("p-1")).expect("bounded");
    assert_eq!(scope.selector(), "inbox:p-1");
    let mut fh = Firehose::new();
    for raw in ["inbox:*", "*", "inbox:", "inbox"] {
        let r = fh.subscribe_raw("fan.acme.inbox", raw, None);
        assert!(
            r.is_err(),
            "an unbounded inbox scope `{raw}` must be rejected, got {r:?}"
        );
        assert!(
            r.unwrap_err().is_over_broad_scope(),
            "`{raw}` is an over-broad-scope rejection"
        );
    }
    assert!(
        fh.subscribe_raw("fan.acme.inbox", "inbox:p-1", None)
            .is_ok(),
        "a bounded inbox: scope subscribes"
    );
}

#[test]
fn watch_open_starts_live_from_now() {
    let mut fh = Firehose::new();
    let me = principal("p-me");
    publish_inbox_frame(&mut fh, &me, "itm-old-1").unwrap();
    publish_inbox_frame(&mut fh, &me, "itm-old-2").unwrap();

    let watch = watch_open(&mut fh, &me)
        .unwrap()
        .into_live()
        .expect("live watch");
    assert!(
        watch.drain().is_empty(),
        "a None-cursor watch has no backfill (live from now)"
    );

    publish_inbox_frame(&mut fh, &me, "itm-new-3").unwrap();
    publish_inbox_frame(&mut fh, &me, "itm-new-4").unwrap();
    let live: Vec<String> = watch.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(
        live,
        vec!["itm-new-3", "itm-new-4"],
        "a fresh watch receives only post-open frames"
    );
}

#[test]
fn resume_backfills_the_gap_then_goes_live_losing_zero_items() {
    let mut fh = Firehose::new();
    let me = principal("p-me");

    publish_inbox_frame(&mut fh, &me, "itm-1").unwrap();
    publish_inbox_frame(&mut fh, &me, "itm-2").unwrap();
    publish_inbox_frame(&mut fh, &me, "itm-3").unwrap();
    publish_inbox_frame(&mut fh, &me, "itm-4").unwrap();
    publish_inbox_frame(&mut fh, &me, "itm-5").unwrap();

    let watch = watch_resume(&mut fh, &me, 2)
        .unwrap()
        .into_live()
        .expect("in-window resume");
    let backfilled: Vec<(u64, String)> = watch
        .drain()
        .into_iter()
        .map(|f| (f.seq, f.item_id))
        .collect();
    assert_eq!(
        backfilled,
        vec![
            (3, "itm-3".into()),
            (4, "itm-4".into()),
            (5, "itm-5".into())
        ],
        "the gap (last_seq, now] is replayed in order - ZERO items lost"
    );
    assert_eq!(
        watch.last_seq(),
        5,
        "the resume cursor advanced to the head"
    );

    publish_inbox_frame(&mut fh, &me, "itm-6").unwrap();
    let live: Vec<String> = watch.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(
        live,
        vec!["itm-6"],
        "live continues gap-free after the backfill"
    );
}

#[test]
fn chained_subscribe_drop_reconnect_loses_zero_and_duplicates_zero() {
    let mut fh = Firehose::new();
    let me = principal("p-chain");

    let first = watch_open(&mut fh, &me).unwrap().into_live().expect("live");
    for i in 1..=3 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }
    let seen_before: Vec<String> = first.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(
        seen_before,
        vec!["itm-1", "itm-2", "itm-3"],
        "received 1..k before the drop"
    );
    let k = first.last_seq();
    assert_eq!(k, 3, "the cursor at the drop is k = 3");
    drop(first);

    for i in 4..=6 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }

    let again = watch_resume(&mut fh, &me, k)
        .unwrap()
        .into_live()
        .expect("in-window resume");
    let backfilled: Vec<String> = again.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(
        backfilled,
        vec!["itm-4", "itm-5", "itm-6"],
        "the disconnected gap k+1..m is backfilled in order - 0 lost"
    );

    let mut all = seen_before;
    all.extend(backfilled);
    assert_eq!(
        all,
        vec!["itm-1", "itm-2", "itm-3", "itm-4", "itm-5", "itm-6"],
        "across the reconnect: every item exactly once - 0 lost, 0 duplicate"
    );
}

#[test]
fn resume_at_head_is_a_no_op_not_a_resync() {
    let mut fh = Firehose::new();
    let me = principal("p-me");
    for i in 1..=4 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }
    let out = watch_resume(&mut fh, &me, 4).unwrap();
    assert!(
        !out.is_resync_required(),
        "a caught-up resume is NOT a resync"
    );
    let watch = out.into_live().expect("live");
    assert!(
        watch.drain().is_empty(),
        "a caught-up resume backfills nothing"
    );
}

#[test]
fn over_old_cursor_yields_resync_required_at_the_exact_window_boundary() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let me = principal("p-me");
    for i in 1..=6 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }

    let out = watch_resume(&mut fh, &me, 2).unwrap();
    assert!(
        out.is_resync_required(),
        "an over-old cursor RAISES resync_required (NAMED)"
    );
    if let WatchOutcome::ResyncRequired {
        last_seq,
        window_floor,
    } = out
    {
        assert_eq!(last_seq, 2);
        assert_eq!(
            window_floor, 4,
            "the window floor is the oldest held seq (4)"
        );
    } else {
        panic!("expected ResyncRequired");
    }

    let watch = watch_resume(&mut fh, &me, 3)
        .unwrap()
        .into_live()
        .expect("first-missing == floor");
    let ids: Vec<String> = watch.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(
        ids,
        vec!["itm-4", "itm-5", "itm-6"],
        "the boundary cursor backfills exactly"
    );
}

#[test]
fn resync_required_falls_back_to_a_full_cold_rebuild_zero_lost() {
    let me = principal("p-me");
    let inbox = InboxProjection::new();
    for id in ["itm-a", "itm-b", "itm-c"] {
        inbox.upsert_for_test(routed(&me, id));
    }

    let mut fh = Firehose::with_limits(2, DEFAULT_INFLIGHT_CAP);
    for id in ["itm-a", "itm-b", "itm-c"] {
        publish_inbox_frame(&mut fh, &me, id).unwrap();
    }
    let out = watch_resume(&mut fh, &me, 0).unwrap();
    assert!(
        out.is_resync_required(),
        "an evicted-head fresh cursor resyncs"
    );

    let auth = AllowAllAuthorize;
    let recovered = cold_rebuild_item_ids(&inbox, &me, &auth, &strong());
    assert_eq!(
        recovered,
        vec!["itm-a", "itm-b", "itm-c"],
        "the cold rebuild recovers every item - ZERO lost across the resync recovery"
    );

    let live = watch_open(&mut fh, &me)
        .unwrap()
        .into_live()
        .expect("re-open live");
    publish_inbox_frame(&mut fh, &me, "itm-d").unwrap();
    assert_eq!(
        live.drain()
            .into_iter()
            .map(|f| f.item_id)
            .collect::<Vec<_>>(),
        vec!["itm-d"],
        "the re-opened watch resumes live delivery"
    );
}

#[test]
fn a_slow_watcher_is_dropped_to_resync_required_with_bounded_memory() {
    let mut fh = Firehose::with_limits(1024, 3);
    let me = principal("p-slow");
    let watch = watch_open(&mut fh, &me).unwrap().into_live().expect("live");

    for i in 1..=3 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }
    assert_eq!(
        watch.ready_len(),
        3,
        "the in-flight queue filled to the cap"
    );
    assert!(
        !watch.resync_required(),
        "not dropped yet (at the cap, not over it)"
    );

    publish_inbox_frame(&mut fh, &me, "itm-over").unwrap();
    assert!(
        watch.resync_required(),
        "a slow watcher is dropped to resync_required (NAMED)"
    );
    assert_eq!(
        watch.ready_len(),
        0,
        "the buffer is RELEASED - memory bounded, the gap NOT buffered"
    );
    assert!(
        watch.next().is_none(),
        "a dropped watch delivers nothing until it cold-rebuilds"
    );
}

#[test]
fn a_keeping_up_watcher_is_never_dropped() {
    let mut fh = Firehose::with_limits(1024, 4);
    let me = principal("p-fast");
    let watch = watch_open(&mut fh, &me).unwrap().into_live().expect("live");
    for i in 1..=50u64 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
        let f = watch
            .next()
            .expect("a keeping-up watcher always has its frame");
        assert_eq!(f.seq, i, "delivered in order");
        assert!(
            watch.ready_len() <= 1,
            "in-flight stays bounded for a keeping-up watcher"
        );
    }
    assert!(
        !watch.resync_required(),
        "a keeping-up watcher is never dropped"
    );
}

#[test]
fn a_frame_carries_only_the_item_id_pointer() {
    let mut fh = Firehose::new();
    let me = principal("p-me");
    let frame = publish_inbox_frame(&mut fh, &me, "itm-pointer").unwrap();
    assert_eq!(
        frame.item_id, "itm-pointer",
        "the frame body is the item_id pointer, never a payload"
    );
    assert_eq!(
        frame.seq, 1,
        "the transport assigns the per-(stream,scope) monotone seq (1)"
    );
}

#[test]
fn publish_fans_out_to_every_open_watch_on_the_inbox() {
    let mut fh = Firehose::new();
    let me = principal("p-me");
    let a = watch_open(&mut fh, &me).unwrap().into_live().expect("a");
    let b = watch_open(&mut fh, &me).unwrap().into_live().expect("b");
    publish_inbox_frame(&mut fh, &me, "itm-fanout").unwrap();
    assert_eq!(
        a.drain().into_iter().map(|f| f.item_id).collect::<Vec<_>>(),
        vec!["itm-fanout"]
    );
    assert_eq!(
        b.drain().into_iter().map(|f| f.item_id).collect::<Vec<_>>(),
        vec!["itm-fanout"],
        "both devices receive the live frame"
    );
}

#[test]
fn different_principals_have_independent_inbox_slices() {
    let mut fh = Firehose::new();
    let alice = principal("p-alice");
    let bob = principal("p-bob");
    let alice_watch = watch_open(&mut fh, &alice)
        .unwrap()
        .into_live()
        .expect("alice");
    let bob_watch = watch_open(&mut fh, &bob).unwrap().into_live().expect("bob");

    publish_inbox_frame(&mut fh, &alice, "itm-for-alice").unwrap();
    publish_inbox_frame(&mut fh, &bob, "itm-for-bob").unwrap();

    assert_eq!(
        alice_watch
            .drain()
            .into_iter()
            .map(|f| f.item_id)
            .collect::<Vec<_>>(),
        vec!["itm-for-alice"],
        "alice's watch sees ONLY alice's inbox slice"
    );
    assert_eq!(
        bob_watch
            .drain()
            .into_iter()
            .map(|f| f.item_id)
            .collect::<Vec<_>>(),
        vec!["itm-for-bob"],
        "bob's watch sees ONLY bob's inbox slice - never alice's"
    );
}

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
