use myelin_events::firehose::{Firehose, FirehoseError, DEFAULT_INFLIGHT_CAP};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::{publish_inbox_frame, watch_open, watch_resume, InboxFrame, WatchOutcome};
use myelin_tenancy::TenantId;

fn principal(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn provider_publishes(fh: &mut Firehose, recipient: &Principal, item_id: &str) -> InboxFrame {
    publish_inbox_frame(fh, recipient, item_id).expect("a real principal has a bounded inbox scope")
}

fn consumer_resume_drains(fh: &mut Firehose, watcher: &Principal, last_seq: u64) -> Vec<String> {
    let watch = watch_resume(fh, watcher, last_seq)
        .expect("an in-window resume succeeds")
        .into_live()
        .expect("an in-window resume is a live watch");
    watch.drain().into_iter().map(|f| f.item_id).collect()
}

#[test]
fn cdc_provider_seq_and_consumer_resume_agree_zero_lost_zero_dup() {
    let mut fh = Firehose::new();
    let me = principal("p-me");

    for i in 1..=5 {
        let f = provider_publishes(&mut fh, &me, &format!("itm-{i}"));
        assert_eq!(
            f.seq, i,
            "the PROVIDER does not mint the seq - the transport assigns it monotone"
        );
    }

    let backfilled = consumer_resume_drains(&mut fh, &me, 2);
    assert_eq!(
        backfilled,
        vec!["itm-3", "itm-4", "itm-5"],
        "the gap is replayed in order - 0 lost"
    );

    let live_watch = watch_resume(&mut fh, &me, 5)
        .unwrap()
        .into_live()
        .expect("caught-up live");
    provider_publishes(&mut fh, &me, "itm-6");
    let live: Vec<String> = live_watch.drain().into_iter().map(|f| f.item_id).collect();
    assert_eq!(
        live,
        vec!["itm-6"],
        "the live frame continues gap-free - 0 dup"
    );
}

#[test]
fn cdc_consumer_scope_is_bounded_unbounded_is_rejected() {
    let mut fh = Firehose::new();
    assert!(
        watch_open(&mut fh, &principal("p-me")).is_ok(),
        "a bounded inbox: scope watch opens"
    );
    for raw in ["inbox:*", "*", "inbox:"] {
        let r = fh.subscribe_raw("fan.acme.inbox", raw, None);
        assert!(r.is_err(), "an unbounded scope `{raw}` is rejected");
        assert!(
            r.unwrap_err().is_over_broad_scope(),
            "`{raw}` is an over-broad-scope rejection"
        );
    }
}

#[test]
fn cdc_over_old_cursor_yields_resync_required() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let me = principal("p-me");
    for i in 1..=6 {
        provider_publishes(&mut fh, &me, &format!("itm-{i}"));
    }
    let out = watch_resume(&mut fh, &me, 2).expect("the resync verdict is a non-fatal outcome");
    assert!(
        out.is_resync_required(),
        "an over-old cursor RAISES resync_required (NAMED)"
    );
    match out {
        WatchOutcome::ResyncRequired {
            last_seq,
            window_floor,
        } => {
            assert_eq!(last_seq, 2);
            assert_eq!(window_floor, 4, "the window floor is the oldest held seq");
        }
        WatchOutcome::Live(_) => panic!("expected resync_required for an over-old cursor"),
    }
}

#[test]
fn cdc_unexpected_transport_error_is_propagated() {
    let err = myelin_events::firehose::FirehoseScope::parse("inbox:*").expect_err("over-broad");
    assert!(
        matches!(err, FirehoseError::OverBroadScope { .. }),
        "the verdict is LOUD + typed"
    );
}
