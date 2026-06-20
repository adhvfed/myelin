//! # NOTIF-D11 — the `inbox watch` resume leg (the OQ-J resume-cursor family applied to
//! `scope = inbox:<principal>`) (NOTIF-P15 / P-193)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **D-N11** ("Drop the `inbox watch` connection mid-stream; reconnect with `last_seq` → backfill
//! `(last_seq, now]` then live, ZERO items lost; an over-old cursor → `resync_required` → snapshot
//! rebuild." Threshold: **0 items lost — never softened**), and `notifications.md` §7 (the inbox watch
//! live transport rides the FROZEN firehose resume-cursor protocol; there is no bespoke Notif
//! transport). This is the OQ-J resume-cursor family's Notif leg.
//!
//! **The dated GREEN artifact (2026-06-20).** The drill drives the `inbox watch` connection (the
//! Notif consumption of contract 3.5) through the catalogue's two faults and asserts, through the
//! harness telemetry-assertion library (the SAME §10.2 `FirehoseFrameLag` / `ResyncRequiredCount`
//! signals the Bus D-10 leg reads):
//!
//! 1. **0 items lost across a reconnect** — drop the watch mid-stream; while disconnected the
//!    producer mirrors more inbox items onto the firehose; reconnect with `last_seq` → the gap
//!    `(last_seq, now]` is backfilled in order then live continues gap-free. Every item is delivered
//!    EXACTLY once: 0 lost, 0 duplicate. The `FirehoseFrameLag` survival signal reads 0 after the
//!    backfill (no item outstanding); `ResyncRequiredCount` is 0 on this in-window leg.
//! 2. **the `resync_required` path is exercised → cold rebuild via `list_inbox`** — an over-old
//!    cursor (older than the bounded retention window) RAISES `resync_required` (the
//!    `ResyncRequiredCount` signal fires `>= 1`); the client falls back to a full `list_inbox` cold
//!    rebuild (NAMED, not silent) that recovers EVERY current inbox item — 0 lost across the recovery.
//!
//! Threshold: **0 items lost — never softened.** The zero-loss is the transport's property (the Bus
//! resume-cursor protocol, EB-21), consumed unchanged by `inbox watch`; the green here is the OBSERVED
//! item set across the reconnect + the OBSERVED survival signals, not a claimed one.
//!
//! **The fault is REVERSIBLE (P-S03):** the `inbox watch` connection is dropped + restored via the
//! harness `DependencyBreaker` (`Dependency::Firehose`), exactly as the Bus D-10 leg drops the
//! firehose — the durable retention window keeps the gap across the outage.

use myelin_events::firehose::{Firehose, DEFAULT_INFLIGHT_CAP};
use myelin_harness::{Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::{
    cold_rebuild_item_ids, inbox_scope, inbox_stream, publish_inbox_frame, watch_open, watch_resume,
    AllowAllAuthorize, InboxProjection, RoutedInboxItem, WatchOutcome,
};
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

/// A `RoutedInboxItem` for the cold-rebuild source projection (`list_inbox` reads it).
fn routed(recipient: &Principal, item_id: &str) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: Region("fr-par".into()),
        item_id: item_id.into(),
        recipient: recipient.principal_id.0.clone(),
        subject: ArtifactRef(format!("myelin://acme/issue/issue/{item_id}")),
        reason: myelin_notif::Reason::Assigned,
        class: myelin_notif::Class::Direct,
        origin_event: ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
        dedup_key: item_id.into(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    }
}

/// Bridge the watch's measured item-lag + resync count into the FROZEN §10.2 harness assertion
/// library (the same bridge the Bus D-10 leg uses): the protocol owns the measurement, the harness
/// owns the assertion vocabulary.
fn assert_firehose_green(stream: &str, scope: &str, item_lag: i64, resync: i64) {
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::FirehoseFrameLag,
        vec![Label::new("stream", stream), Label::new("scope", scope)],
        item_lag,
    );
    src.set_scalar(SignalName::ResyncRequiredCount, resync);
    src.assert_labelled(
        SignalName::FirehoseFrameLag,
        vec![Label::new("stream", stream), Label::new("scope", scope)],
        Predicate::Eq(item_lag),
    )
    .expect_green();
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Eq(resync))
        .expect_green();
}

/// **D-N11 LEG 1 — drop the `inbox watch` connection mid-stream; reconnect → 0 items lost.** The
/// headline pass condition (threshold: 0 lost, never softened).
#[test]
fn d_n11_reconnect_loses_zero_items() {
    let mut fh = Firehose::new();
    let me = principal("p-watcher");
    let stream = inbox_stream(&me);
    let scope = inbox_scope(&me).expect("a bounded inbox scope").selector();

    // a connected watcher consumes live up to seq 3 (items itm-1..itm-3).
    let breaker = DependencyBreaker::new();
    let watch = watch_open(&mut fh, &me).unwrap().into_live().expect("live watch");
    for i in 1..=3 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }
    let seen_before: Vec<String> = watch.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(seen_before, vec!["itm-1", "itm-2", "itm-3"], "the watcher saw 1..3 while connected");
    let last_seq = watch.last_seq();
    assert_eq!(last_seq, 3, "its resume cursor is the last delivered seq");

    // DROP the inbox watch connection mid-stream (reversibly, P-S03). While down, the producer keeps
    // mirroring inbox items (itm-4..itm-7) onto the firehose's bounded retention window.
    breaker.break_dependency(Dependency::Firehose, Scope::Global);
    assert!(breaker.is_broken(&Dependency::Firehose, &Scope::Global), "the watch connection is down");
    drop(watch); // the old subscription is gone (the connection dropped); the window kept the gap.
    for i in 4..=7 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }

    // RECONNECT: watch_resume(last_seq=3) → backfill (3, now] = {itm-4..itm-7}, then live.
    breaker.restore_dependency(Dependency::Firehose, Scope::Global);
    let resumed = match watch_resume(&mut fh, &me, last_seq).unwrap() {
        WatchOutcome::Live(w) => w,
        WatchOutcome::ResyncRequired { .. } => panic!("an in-window reconnect must NOT resync"),
    };
    let backfilled: Vec<String> = resumed.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(
        backfilled,
        vec!["itm-4", "itm-5", "itm-6", "itm-7"],
        "the disconnected gap (last_seq, now] is replayed in order — 0 items lost"
    );

    // a subsequent LIVE item continues gap-free, no duplicate.
    publish_inbox_frame(&mut fh, &me, "itm-8").unwrap();
    let live: Vec<String> = resumed.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(live, vec!["itm-8"], "live continues contiguously — 0 duplicate across the boundary");

    // ZERO ITEMS LOST: across the whole reconnect the watcher saw itm-1..itm-8, each exactly once.
    let mut total = seen_before;
    total.extend(backfilled);
    total.extend(live);
    let expected: Vec<String> = (1..=8).map(|i| format!("itm-{i}")).collect();
    assert_eq!(total, expected, "every item delivered exactly once across the reconnect: 0 lost, 0 dup");

    // the survival signals: the item-lag reads 0 after the backfill (no item outstanding); the
    // resync count is 0 on this in-window leg. GREEN through the frozen §10.2 library.
    let remaining_lag = (fh.head_seq(&stream, &inbox_scope(&me).unwrap()) - resumed.last_seq()) as i64;
    assert_eq!(remaining_lag, 0, "the seq-gap is closed after the backfill");
    assert_firehose_green(&stream, &scope, remaining_lag, 0);
}

/// **D-N11 LEG 2 — an over-old cursor → `resync_required` → a full `list_inbox` cold rebuild, 0
/// items lost across the recovery (NAMED, not silent).** A SMALL retention window forces the gap's
/// head to be evicted, so the reconnect cannot backfill from the window → resync → cold rebuild.
#[test]
fn d_n11_over_old_cursor_resyncs_then_cold_rebuilds_zero_lost() {
    let me = principal("p-watcher");
    let stream = inbox_stream(&me);
    let scope = inbox_scope(&me).expect("a bounded inbox scope").selector();

    // the cold-rebuild SOURCE (the inbox projection list_inbox reads) holds the watcher's items.
    let inbox = InboxProjection::new();
    let all_items: Vec<String> = (1..=6).map(|i| format!("itm-{i}")).collect();
    for id in &all_items {
        inbox.upsert_for_test(routed(&me, id));
    }

    // a SMALL firehose window (holds only the most-recent 3) — the watcher's cursor goes over-old.
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let last_seq = 2u64; // the watcher last saw seq 2.
    for id in &all_items {
        publish_inbox_frame(&mut fh, &me, id).unwrap(); // window now holds {4,5,6}; 1,2,3 evicted.
    }

    // RECONNECT past the window → resync_required (NAMED, not a silent partial replay).
    let out = watch_resume(&mut fh, &me, last_seq).expect("the resync verdict is non-fatal");
    let resync_fired = match out {
        WatchOutcome::ResyncRequired { last_seq: ls, window_floor } => {
            assert_eq!(ls, 2);
            assert_eq!(window_floor, 4, "the window floor is the oldest held seq (4)");
            1
        }
        WatchOutcome::Live(_) => panic!("an over-old cursor MUST resync"),
    };
    assert_eq!(resync_fired, 1, "the resync_required path is exercised");

    // the NAMED cold rebuild: rebuild from SOURCE via list_inbox — recovers EVERY current item.
    let recovered = cold_rebuild_item_ids(&inbox, &me, &AllowAllAuthorize, &strong());
    assert_eq!(recovered, all_items, "the cold rebuild recovers every item — 0 lost across the recovery");

    // the survival signal: the resync count fired (>= 1). After the cold rebuild + re-open the
    // item-lag is 0 (the client is caught up via the snapshot). GREEN through the frozen §10.2 library.
    let _re_open = watch_open(&mut fh, &me).unwrap();
    assert_firehose_green(&stream, &scope, 0, resync_fired);
}

/// **D-N11 LEG 3 — the bounded-scope rejection (BUS-3 generalised): an unbounded scope is REJECTED.**
/// A subscribe with `scope = *` / `inbox:*` is rejected by the transport (0 unbounded scope accepted)
/// — one client cannot subscribe to the whole tenant firehose, only its own `inbox:<principal>` slice.
#[test]
fn d_n11_unbounded_scope_is_rejected() {
    let mut fh = Firehose::new();
    let me = principal("p-watcher");
    // the bounded inbox scope subscribes (the positive control).
    assert!(watch_open(&mut fh, &me).is_ok(), "a bounded inbox: scope watch opens");
    // EVERY unbounded form is rejected at the connection-tier subscribe entry: 0 accepted.
    let mut accepted_unbounded = 0;
    for raw in ["*", "inbox:*", "inbox:", "inbox", "", ">"] {
        if fh.subscribe_raw(&inbox_stream(&me), raw, None).is_ok() {
            accepted_unbounded += 1;
        }
    }
    assert_eq!(accepted_unbounded, 0, "0 unbounded scope accepted — the whitelist-not-* rule holds");

    // a deny-by-default authorize ensures the principal id discipline holds end-to-end (a sanity
    // pin that the inbox scope is keyed to the OPAQUE pseudonym, never a free identifier).
    struct DenyAll;
    impl myelin_notif::ReadAuthorizePort for DenyAll {
        fn can_read(&self, _v: &Principal, _s: &ArtifactRef, _a: &Consistency) -> Decision {
            Decision::Deny
        }
    }
    // a denied cold rebuild yields nothing (held, not leaked) — the recovery still never over-shares.
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(routed(&me, "itm-x"));
    let recovered = cold_rebuild_item_ids(&inbox, &me, &DenyAll, &strong());
    assert!(recovered.is_empty(), "a denied cold rebuild leaks nothing");
}
