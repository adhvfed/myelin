use super::*;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

use crate::prefs::{QuietHours, QuietWindow, Tz};
use crate::router::RoutedInboxItem;
use crate::Reason;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

fn env_by(actor_id: &str) -> EventEnvelope {
    let actor = Principal::stub(PrincipalId(actor_id.into()), PrincipalKind::Human, tenant());
    EventEnvelope {
        event_id: EventId("evt-1".into()),
        type_: EventType("signal.opened".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(actor),
        subject: ArtifactRef("sig.acme.error.r".into()),
        aggregate: AggregateKey("signal:k".into()),
        causation_id: None,
        correlation_id: CorrelationId("evt-1".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

fn item(recipient: &str, subject: &str, class: Class) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: region(),
        item_id: format!("itm-{recipient}-{subject}"),
        recipient: recipient.into(),
        subject: ArtifactRef(subject.into()),
        reason: Reason::StateChanged,
        class,
        origin_event: ArtifactRef("myelin://acme/bus/event/evt-1".into()),
        dedup_key: subject.to_string(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    }
}

fn never_quiet() -> QuietHours {
    QuietHours::default()
}

fn ctx<'a>(tick: u64, quiet: &'a QuietHours) -> StormContext<'a> {
    StormContext {
        tick,
        utc_minute_of_day: 12 * 60,
        utc_weekday: 2,
        quiet,
        rate: RateConfig::default(),
    }
}

#[test]
fn mechanism_1_self_notification_is_suppressed() {
    let sc = StormControl::new();
    let q = never_quiet();
    let env = env_by("p-alice");
    let it = item("p-alice", "myelin://acme/chat/thread/T1", Class::Direct);
    let d = sc.decide(
        &env,
        &it,
        "myelin://acme/chat/thread/T1",
        false,
        &ctx(0, &q),
    );
    assert_eq!(d, StormDecision::Suppress(SuppressReason::SelfAction));
    assert!(!d.writes_row(), "a self-notification writes NO inbox row");
    assert!(!d.delivers(), "and pushes no delivery");
    assert!(
        !d.touches_audit(),
        "but the underlying event is untouched on the bus"
    );
}

#[test]
fn mechanism_1_notification_to_another_principal_is_not_suppressed() {
    let sc = StormControl::new();
    let q = never_quiet();
    let env = env_by("p-alice");
    let it = item("p-bob", "myelin://acme/chat/thread/T1", Class::Direct);
    let d = sc.decide(
        &env,
        &it,
        "myelin://acme/chat/thread/T1",
        false,
        &ctx(0, &q),
    );
    assert_eq!(
        d,
        StormDecision::Deliver,
        "a notification to a DIFFERENT principal delivers"
    );
}

#[test]
fn is_self_notification_matches_actor_principal_exactly() {
    assert!(
        is_self_notification(&env_by("p-x"), "p-x"),
        "actor == recipient → self"
    );
    assert!(
        !is_self_notification(&env_by("p-x"), "p-y"),
        "actor != recipient → not self"
    );
}

#[test]
fn mechanism_2_existing_row_collapses() {
    let sc = StormControl::new();
    let q = never_quiet();
    let env = env_by("p-actor");
    let it = item("p-bob", "myelin://acme/ci/run/42", Class::Direct);
    let first = sc.decide(&env, &it, "myelin://acme/ci/run/42", false, &ctx(0, &q));
    assert_eq!(first, StormDecision::Deliver);
    let second = sc.decide(&env, &it, "myelin://acme/ci/run/42", true, &ctx(0, &q));
    assert_eq!(
        second,
        StormDecision::Collapse,
        "a same-key candidate collapses into the existing row"
    );
    assert!(
        second.writes_row(),
        "a collapse touches the existing row (coalesce_count++)"
    );
    assert!(
        !second.delivers(),
        "a collapse does not re-push (the +N more is read at inbox open)"
    );
    assert!(!second.touches_audit());
}

#[test]
fn mechanism_3_second_participating_item_coalesces() {
    let sc = StormControl::new();
    let q = never_quiet();
    let env = env_by("p-actor");
    let a = item(
        "p-bob",
        "myelin://acme/chat/thread/T1#c1",
        Class::Participating,
    );
    let b = item(
        "p-bob",
        "myelin://acme/chat/thread/T1#c2",
        Class::Participating,
    );
    let root = "myelin://acme/chat/thread/T1";
    let first = sc.decide(&env, &a, root, false, &ctx(0, &q));
    assert_eq!(
        first,
        StormDecision::Deliver,
        "the first opens the digest marker (delivers)"
    );
    let second = sc.decide(&env, &b, root, false, &ctx(0, &q));
    assert_eq!(
        second,
        StormDecision::Coalesce,
        "the second participating item coalesces (digest)"
    );
    assert!(
        second.writes_row(),
        "coalescing touches the digest marker row"
    );
    assert!(
        !second.delivers(),
        "a coalesced item does not push a new delivery"
    );
}

#[test]
fn mechanism_3_direct_item_is_broken_out_never_coalesced() {
    let sc = StormControl::new();
    let q = never_quiet();
    let env = env_by("p-actor");
    let root = "myelin://acme/issues/issue/PROJ-1";
    sc.decide(
        &env,
        &item(
            "p-bob",
            "myelin://acme/issues/issue/PROJ-1#a",
            Class::Participating,
        ),
        root,
        false,
        &ctx(0, &q),
    );
    let direct = item(
        "p-bob",
        "myelin://acme/issues/issue/PROJ-1#b",
        Class::Direct,
    );
    let d = sc.decide(&env, &direct, root, false, &ctx(0, &q));
    assert_eq!(
        d,
        StormDecision::Deliver,
        "a Direct item is broken out (never digested)"
    );
    let crit = item(
        "p-bob",
        "myelin://acme/issues/issue/PROJ-1#c",
        Class::Critical,
    );
    let dc = sc.decide(&env, &crit, root, false, &ctx(0, &q));
    assert_eq!(
        dc,
        StormDecision::Deliver,
        "a Critical page is broken out (never digested)"
    );
}

#[test]
fn coalescer_break_out_vs_digest() {
    let c = Coalescer::new();
    assert!(!c.should_coalesce("r", "root", Class::Direct));
    assert!(!c.should_coalesce("r", "root", Class::Critical));
    assert!(
        !c.should_coalesce("r", "root", Class::Participating),
        "first ambient opens the marker"
    );
    assert!(
        c.should_coalesce("r", "root", Class::Watching),
        "second ambient coalesces"
    );
    assert!(
        !c.should_coalesce("r", "other", Class::Fyi),
        "a different root opens its own marker"
    );
}

#[test]
fn mechanism_4_token_bucket_burst_then_damp_then_refill() {
    let tb = TokenBucket::new();
    let cfg = RateConfig {
        capacity: 3.0,
        refill_per_tick: 1.0,
    };
    assert!(tb.try_take("r", "root", 0, cfg), "burst 1/3");
    assert!(tb.try_take("r", "root", 0, cfg), "burst 2/3");
    assert!(tb.try_take("r", "root", 0, cfg), "burst 3/3");
    assert!(
        !tb.try_take("r", "root", 0, cfg),
        "the 4th in the burst is damped (bucket empty)"
    );
    assert!(
        tb.try_take("r", "root", 1, cfg),
        "one tick refilled one token"
    );
    assert!(
        !tb.try_take("r", "root", 1, cfg),
        "and the bucket is empty again"
    );
    assert!(
        tb.try_take("r", "other", 0, cfg),
        "a different root has its own burst allowance"
    );
}

#[test]
fn token_bucket_multi_tick_refill_is_elapsed_times_rate_capped() {
    let tb = TokenBucket::new();
    let cfg = RateConfig {
        capacity: 6.0,
        refill_per_tick: 2.0,
    };
    for _ in 0..6 {
        assert!(tb.try_take("r", "root", 0, cfg));
    }
    assert!(
        !tb.try_take("r", "root", 0, cfg),
        "depleted: same-tick take is damped (no refill at elapsed 0)"
    );
    for k in 0..4 {
        assert!(
            tb.try_take("r", "root", 2, cfg),
            "tick+2 refilled 4 tokens (2*2): admit {k}"
        );
    }
    assert!(
        !tb.try_take("r", "root", 2, cfg),
        "after the 4 refilled tokens the bucket is empty"
    );
    for k in 0..6 {
        assert!(
            tb.try_take("r", "root", 100, cfg),
            "a long elapse refills to the capacity cap (6): admit {k}"
        );
    }
    assert!(
        !tb.try_take("r", "root", 100, cfg),
        "the refill is capped at capacity (not 98*2)"
    );
}

#[test]
fn mechanism_4_burst_on_hot_subject_is_rate_damped() {
    let sc = StormControl::new();
    let q = never_quiet();
    let env = env_by("p-actor");
    let root = "myelin://acme/git/pr/9";
    let mut delivered = 0;
    let mut damped = 0;
    for i in 0..10 {
        let it = item(
            "p-bob",
            &format!("myelin://acme/git/pr/9#c{i}"),
            Class::Direct,
        );
        match sc.decide(&env, &it, root, false, &ctx(0, &q)) {
            StormDecision::Deliver => delivered += 1,
            StormDecision::Suppress(SuppressReason::RateDamped) => {
                damped += 1;
            }
            other => panic!("unexpected verdict {other:?}"),
        }
    }
    assert_eq!(delivered, 5, "the burst allowance (capacity 5) delivered");
    assert_eq!(
        damped, 5,
        "the rest of the 10 were rate-damped (bounded, not a flood)"
    );
}

#[test]
fn mechanism_4_critical_page_is_never_rate_damped() {
    let sc = StormControl::new();
    let q = never_quiet();
    let env = env_by("p-actor");
    let root = "myelin://acme/oncall/page/1";
    for i in 0..100 {
        let it = item(
            "p-oncall",
            &format!("myelin://acme/oncall/page/1#x{i}"),
            Class::Critical,
        );
        let d = sc.decide(&env, &it, root, false, &ctx(0, &q));
        assert_eq!(
            d,
            StormDecision::Deliver,
            "a critical page is never damped (iter {i})"
        );
    }
}

#[test]
fn mechanism_5_muted_thread_suppresses_delivery_but_writes_the_row() {
    let sc = StormControl::new();
    let q = never_quiet();
    let env = env_by("p-actor");
    let root = "myelin://acme/chat/thread/T9";
    sc.prefs().mute("p-bob", root);
    let it = item(
        "p-bob",
        "myelin://acme/chat/thread/T9#c1",
        Class::Participating,
    );
    let d = sc.decide(&env, &it, root, false, &ctx(0, &q));
    assert_eq!(d, StormDecision::Suppress(SuppressReason::Muted));
    assert!(
        d.writes_row(),
        "a muted item STILL writes the inbox row (the ONE inbox receives)"
    );
    assert!(!d.delivers(), "but the channel push is suppressed");
    assert!(!d.touches_audit(), "the underlying event is untouched");
}

#[test]
fn mechanism_5_quiet_hours_suppresses_non_piercing_pierces_critical() {
    let sc = StormControl::new();
    let env = env_by("p-actor");
    let root = "myelin://acme/issues/issue/PROJ-2";
    let quiet = QuietHours {
        tz: Tz::UTC,
        windows: vec![QuietWindow {
            from: 22 * 60,
            to: 7 * 60,
            days: vec![],
        }],
        pierce_classes: vec![Class::Critical],
    };
    let in_window = StormContext {
        tick: 0,
        utc_minute_of_day: 23 * 60,
        utc_weekday: 2,
        quiet: &quiet,
        rate: RateConfig::default(),
    };
    let fyi = item("p-bob", "myelin://acme/issues/issue/PROJ-2#a", Class::Fyi);
    let d = sc.decide(&env, &fyi, root, false, &in_window);
    assert_eq!(
        d,
        StormDecision::Suppress(SuppressReason::QuietHours),
        "a non-piercing item is quiet-suppressed"
    );
    assert!(
        d.writes_row(),
        "the row is still written (only the push is suppressed)"
    );

    let crit = item(
        "p-bob",
        "myelin://acme/issues/issue/PROJ-2#b",
        Class::Critical,
    );
    let dc = sc.decide(&env, &crit, root, false, &in_window);
    assert_eq!(
        dc,
        StormDecision::Deliver,
        "a critical item pierces quiet-hours (you cannot silence a page)"
    );
}

#[test]
fn ordering_self_suppression_wins_over_all_others() {
    let sc = StormControl::new();
    let root = "myelin://acme/chat/thread/T1";
    sc.prefs().mute("p-alice", root);
    let quiet = QuietHours {
        tz: Tz::UTC,
        windows: vec![QuietWindow {
            from: 0,
            to: 1440,
            days: vec![],
        }],
        pierce_classes: vec![],
    };
    let env = env_by("p-alice");
    let it = item("p-alice", "myelin://acme/chat/thread/T1#c1", Class::Fyi);
    let cx = StormContext {
        tick: 0,
        utc_minute_of_day: 0,
        utc_weekday: 0,
        quiet: &quiet,
        rate: RateConfig::default(),
    };
    assert_eq!(
        sc.decide(&env, &it, root, false, &cx),
        StormDecision::Suppress(SuppressReason::SelfAction),
        "self-suppression (mechanism 1) wins over mute/quiet/rate (it runs first)"
    );
}

#[test]
fn ordering_dedup_collapse_wins_over_mute_and_quiet() {
    let sc = StormControl::new();
    let root = "myelin://acme/chat/thread/T2";
    sc.prefs().mute("p-bob", root);
    let env = env_by("p-actor");
    let it = item("p-bob", "myelin://acme/chat/thread/T2#c1", Class::Fyi);
    let q = never_quiet();
    let d = sc.decide(&env, &it, root, true, &ctx(0, &q));
    assert_eq!(
        d,
        StormDecision::Collapse,
        "an existing row collapses (mechanism 2) before mute (5)"
    );
}

#[test]
fn every_verdict_leaves_the_audit_untouched() {
    let verdicts = [
        StormDecision::Deliver,
        StormDecision::Collapse,
        StormDecision::Coalesce,
        StormDecision::Suppress(SuppressReason::SelfAction),
        StormDecision::Suppress(SuppressReason::RateDamped),
        StormDecision::Suppress(SuppressReason::Muted),
        StormDecision::Suppress(SuppressReason::QuietHours),
    ];
    for v in verdicts {
        assert!(
            !v.touches_audit(),
            "{v:?} must NOT touch the audit (EI-04 §5.3)"
        );
    }
}

#[test]
fn suppress_reason_writes_row_mapping_is_frozen() {
    assert!(
        !SuppressReason::SelfAction.writes_row(),
        "self-action writes no row"
    );
    assert!(
        !SuppressReason::RateDamped.writes_row(),
        "rate-damp writes no row"
    );
    assert!(
        SuppressReason::Muted.writes_row(),
        "mute writes the row (only push suppressed)"
    );
    assert!(
        SuppressReason::QuietHours.writes_row(),
        "quiet-hours writes the row (only push suppressed)"
    );
    assert_eq!(SuppressReason::SelfAction.token(), "self_action");
    assert_eq!(SuppressReason::RateDamped.token(), "rate_damped");
    assert_eq!(SuppressReason::Muted.token(), "muted");
    assert_eq!(SuppressReason::QuietHours.token(), "quiet_hours");
}

#[test]
fn subject_root_strips_sub_fragment() {
    assert_eq!(
        subject_root_of("myelin://acme/chat/thread/T1#c5"),
        "myelin://acme/chat/thread/T1"
    );
    assert_eq!(
        subject_root_of("myelin://acme/ci/run/42"),
        "myelin://acme/ci/run/42",
        "no fragment → identity"
    );
    assert_eq!(subject_root_of("a#b#c"), "a", "splits on the FIRST #");
}

#[test]
fn dedup_collapse_ratio_basis_points() {
    assert_eq!(
        dedup_collapse_ratio_bps(0, 0),
        0,
        "no inbound → 0 (nothing to measure)"
    );
    assert_eq!(
        dedup_collapse_ratio_bps(1000, 999),
        9990,
        "1000 → 1 row: 999 collapsed → 9990 bps"
    );
    assert_eq!(
        dedup_collapse_ratio_bps(2, 1),
        5000,
        "2 → 1 row: 1 collapsed → 5000 bps (50%)"
    );
    assert_eq!(dedup_collapse_ratio_bps(10, 0), 0, "no collapse → 0 bps");
    assert_eq!(
        dedup_collapse_ratio_bps(5, 100),
        10000,
        "collapsed capped at inbound → 10000 bps"
    );
}

#[test]
fn notif_d2_chained_storm_collapses_to_one_row_ratio_measured() {
    let sc = StormControl::new();
    let q = never_quiet();
    let env = env_by("p-ci-bot");
    let root = "myelin://acme/ci/run/42";
    let it = item("p-oncall", "myelin://acme/ci/run/42", Class::Critical);

    let inbound = 1000u64;
    let mut collapsed = 0u64;
    let mut rows = 0u64;
    for i in 0..inbound {
        let row_exists = i > 0;
        match sc.decide(&env, &it, root, row_exists, &ctx(0, &q)) {
            StormDecision::Deliver => rows += 1,
            StormDecision::Collapse => collapsed += 1,
            other => panic!("unexpected verdict {other:?}"),
        }
    }
    assert_eq!(
        rows, 1,
        "NOTIF-D2: 1000 near-identical CI failures → ONE inbox row (N→1)"
    );
    assert_eq!(
        collapsed, 999,
        "the other 999 collapsed (coalesce_count would be 1000)"
    );

    let ratio = dedup_collapse_ratio_bps(inbound, collapsed);
    assert_eq!(
        ratio, 9990,
        "the dedup-collapse-ratio is ~99.9% (the storm was absorbed write-time)"
    );
    assert!(
        ratio >= 9000,
        "ratio floor: a regression that stops collapsing fails this LOUDLY"
    );
}

#[test]
fn notif_d2_chained_self_burst_produces_zero_items() {
    let sc = StormControl::new();
    let q = never_quiet();
    let env = env_by("p-bob");
    let root = "myelin://acme/chat/thread/T1";
    let mut self_suppressed = 0;
    for i in 0..30 {
        let it = item(
            "p-bob",
            &format!("myelin://acme/chat/thread/T1#c{i}"),
            Class::Participating,
        );
        let d = sc.decide(&env, &it, root, false, &ctx(0, &q));
        assert_eq!(d, StormDecision::Suppress(SuppressReason::SelfAction));
        assert!(!d.writes_row() && !d.delivers());
        self_suppressed += 1;
    }
    assert_eq!(
        self_suppressed, 30,
        "NOTIF-D2: a 30-event self-burst → 0 inbox items (0 self-notifications)"
    );
}

#[test]
fn notif_d2_chained_30_comment_pr_burst_is_bounded_audit_untouched() {
    let sc = StormControl::new();
    let q = never_quiet();
    let env = env_by("p-author");
    let root = "myelin://acme/git/pr/9";
    let mut delivered = 0;
    let mut bounded = 0;
    for i in 0..30 {
        let it = item(
            "p-watcher",
            &format!("myelin://acme/git/pr/9#c{i}"),
            Class::Participating,
        );
        let d = sc.decide(&env, &it, root, false, &ctx(0, &q));
        assert!(
            !d.touches_audit(),
            "no verdict touches the audit (EI-04 §5.3)"
        );
        match d {
            StormDecision::Deliver => delivered += 1,
            StormDecision::Coalesce | StormDecision::Suppress(_) => bounded += 1,
            StormDecision::Collapse => bounded += 1,
        }
    }
    assert_eq!(
        delivered, 1,
        "the FIRST comment delivers (opens the digest marker)"
    );
    assert_eq!(
        bounded, 29,
        "the other 29 are bounded (coalesced/damped) - not a flood"
    );
}
