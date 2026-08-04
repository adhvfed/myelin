use myelin_events::firehose::{Firehose, DEFAULT_INFLIGHT_CAP};
use myelin_harness::{
    Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource,
};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::{
    cold_rebuild_item_ids, inbox_scope, inbox_stream, publish_inbox_frame, watch_open,
    watch_resume, AllowAllAuthorize, InboxProjection, RoutedInboxItem, WatchOutcome,
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
    Consistency {
        at_least: Zookie("zk".into()),
        mode: ConsistencyMode::Strong,
    }
}

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

#[test]
fn d_n11_reconnect_loses_zero_items() {
    let mut fh = Firehose::new();
    let me = principal("p-watcher");
    let stream = inbox_stream(&me);
    let scope = inbox_scope(&me).expect("a bounded inbox scope").selector();

    let breaker = DependencyBreaker::new();
    let watch = watch_open(&mut fh, &me)
        .unwrap()
        .into_live()
        .expect("live watch");
    for i in 1..=3 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }
    let seen_before: Vec<String> = watch.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(
        seen_before,
        vec!["itm-1", "itm-2", "itm-3"],
        "the watcher saw 1..3 while connected"
    );
    let last_seq = watch.last_seq();
    assert_eq!(last_seq, 3, "its resume cursor is the last delivered seq");

    breaker.break_dependency(Dependency::Firehose, Scope::Global);
    assert!(
        breaker.is_broken(&Dependency::Firehose, &Scope::Global),
        "the watch connection is down"
    );
    drop(watch);
    for i in 4..=7 {
        publish_inbox_frame(&mut fh, &me, &format!("itm-{i}")).unwrap();
    }

    breaker.restore_dependency(Dependency::Firehose, Scope::Global);
    let resumed = match watch_resume(&mut fh, &me, last_seq).unwrap() {
        WatchOutcome::Live(w) => w,
        WatchOutcome::ResyncRequired { .. } => panic!("an in-window reconnect must NOT resync"),
    };
    let backfilled: Vec<String> = resumed.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(
        backfilled,
        vec!["itm-4", "itm-5", "itm-6", "itm-7"],
        "the disconnected gap (last_seq, now] is replayed in order - 0 items lost"
    );

    publish_inbox_frame(&mut fh, &me, "itm-8").unwrap();
    let live: Vec<String> = resumed.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(
        live,
        vec!["itm-8"],
        "live continues contiguously - 0 duplicate across the boundary"
    );

    let mut total = seen_before;
    total.extend(backfilled);
    total.extend(live);
    let expected: Vec<String> = (1..=8).map(|i| format!("itm-{i}")).collect();
    assert_eq!(
        total, expected,
        "every item delivered exactly once across the reconnect: 0 lost, 0 dup"
    );

    let remaining_lag =
        (fh.head_seq(&stream, &inbox_scope(&me).unwrap()) - resumed.last_seq()) as i64;
    assert_eq!(remaining_lag, 0, "the seq-gap is closed after the backfill");
    assert_firehose_green(&stream, &scope, remaining_lag, 0);
}

#[test]
fn d_n11_over_old_cursor_resyncs_then_cold_rebuilds_zero_lost() {
    let me = principal("p-watcher");
    let stream = inbox_stream(&me);
    let scope = inbox_scope(&me).expect("a bounded inbox scope").selector();

    let inbox = InboxProjection::new();
    let all_items: Vec<String> = (1..=6).map(|i| format!("itm-{i}")).collect();
    for id in &all_items {
        inbox.upsert_for_test(routed(&me, id));
    }

    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let last_seq = 2u64;
    for id in &all_items {
        publish_inbox_frame(&mut fh, &me, id).unwrap();
    }

    let out = watch_resume(&mut fh, &me, last_seq).expect("the resync verdict is non-fatal");
    let resync_fired = match out {
        WatchOutcome::ResyncRequired {
            last_seq: ls,
            window_floor,
        } => {
            assert_eq!(ls, 2);
            assert_eq!(
                window_floor, 4,
                "the window floor is the oldest held seq (4)"
            );
            1
        }
        WatchOutcome::Live(_) => panic!("an over-old cursor MUST resync"),
    };
    assert_eq!(resync_fired, 1, "the resync_required path is exercised");

    let recovered = cold_rebuild_item_ids(&inbox, &me, &AllowAllAuthorize, &strong());
    assert_eq!(
        recovered, all_items,
        "the cold rebuild recovers every item - 0 lost across the recovery"
    );

    let _re_open = watch_open(&mut fh, &me).unwrap();
    assert_firehose_green(&stream, &scope, 0, resync_fired);
}

#[test]
fn d_n11_unbounded_scope_is_rejected() {
    let mut fh = Firehose::new();
    let me = principal("p-watcher");
    assert!(
        watch_open(&mut fh, &me).is_ok(),
        "a bounded inbox: scope watch opens"
    );
    let mut accepted_unbounded = 0;
    for raw in ["*", "inbox:*", "inbox:", "inbox", "", ">"] {
        if fh.subscribe_raw(&inbox_stream(&me), raw, None).is_ok() {
            accepted_unbounded += 1;
        }
    }
    assert_eq!(
        accepted_unbounded, 0,
        "0 unbounded scope accepted - the whitelist-not-* rule holds"
    );

    struct DenyAll;
    impl myelin_notif::ReadAuthorizePort for DenyAll {
        fn can_read(&self, _v: &Principal, _s: &ArtifactRef, _a: &Consistency) -> Decision {
            Decision::Deny
        }
    }
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(routed(&me, "itm-x"));
    let recovered = cold_rebuild_item_ids(&inbox, &me, &DenyAll, &strong());
    assert!(recovered.is_empty(), "a denied cold rebuild leaks nothing");
}
