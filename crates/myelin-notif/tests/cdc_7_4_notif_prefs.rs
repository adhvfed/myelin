use myelin_identity::{Literal, Principal, PrincipalId, PrincipalKind};
use myelin_notif::cli::notify_test;
use myelin_notif::prefs::{
    build_routing_matcher, get_prefs, route, set_prefs, Channel, DigestConfig, NotifPrefs,
    PrefStore, QuietHours, QuietWindow, RoutingRule, Tz,
};
use myelin_notif::{Class, Reason, Subsystem};
use myelin_query::{CmpOp, Expr, Predicate, QueryAst};
use myelin_tenancy::TenantId;

fn principal(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

#[test]
fn provider_set_get_and_route_decision() {
    let store = PrefStore::new();
    let me = principal("alice");

    let prefs = NotifPrefs {
        routing: vec![
            RoutingRule {
                channel: Channel::MobilePush,
                matcher: build_routing_matcher(&[Class::Critical], &[], &[]).unwrap(),
            },
            RoutingRule {
                channel: Channel::InApp,
                matcher: build_routing_matcher(&[], &[], &[]).unwrap(),
            },
        ],
        digest: DigestConfig {
            cadence: "off".into(),
            at: None,
            classes: vec![],
        },
    };
    let quiet = QuietHours {
        tz: Tz::from_offset_minutes(60),
        windows: vec![QuietWindow {
            from: 22 * 60,
            to: 7 * 60,
            days: vec![],
        }],
        pierce_classes: vec![Class::Critical],
    };

    let stored = set_prefs(&store, &me, prefs.clone(), quiet.clone());
    assert_eq!(
        stored.prefs, prefs,
        "set_prefs echoes the stored routing matrix"
    );

    let got = get_prefs(&store, &me);
    assert_eq!(got.quiet, quiet, "get_prefs reads back the quiet-hours");

    let utc_min = 60;
    let crit = route(
        &got.prefs,
        &got.quiet,
        Reason::Escalated,
        Class::Critical,
        Subsystem::Issue,
        utc_min,
        2,
    );
    assert!(
        crit.contains(&Channel::MobilePush),
        "critical pierces quiet-hours (on-call cannot be silenced)"
    );
    let fyi = route(
        &got.prefs,
        &got.quiet,
        Reason::Fyi,
        Class::Fyi,
        Subsystem::Issue,
        utc_min,
        2,
    );
    assert!(
        !fyi.contains(&Channel::MobilePush),
        "a non-crit item in quiet-hours is suppressed off-cell"
    );
    assert!(
        fyi.contains(&Channel::InApp),
        "the ONE inbox row is NEVER suppressed (only the off-cell push)"
    );
}

#[test]
fn consumer_matcher_is_the_one_query_ast() {
    let matcher = QueryAst::compiled(Predicate::Or(vec![
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("class".into()),
            rhs: Expr::Lit(Literal::Str("critical".into())),
        },
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("class".into()),
            rhs: Expr::Lit(Literal::Str("direct".into())),
        },
    ]))
    .expect("the consumer's matcher is within the static cost bound");

    let store = PrefStore::new();
    let me = principal("bob");
    set_prefs(
        &store,
        &me,
        NotifPrefs {
            routing: vec![RoutingRule {
                channel: Channel::Email,
                matcher,
            }],
            digest: DigestConfig::default(),
        },
        QuietHours::default(),
    );

    let got = get_prefs(&store, &me);
    assert!(
        route(
            &got.prefs,
            &got.quiet,
            Reason::Assigned,
            Class::Direct,
            Subsystem::Issue,
            12 * 60,
            0
        )
        .contains(&Channel::Email),
        "the direct item routes via the consumer-built matcher"
    );
    assert!(
        route(
            &got.prefs,
            &got.quiet,
            Reason::Watched,
            Class::Watching,
            Subsystem::Issue,
            12 * 60,
            0
        )
        .is_empty(),
        "the watching item does not match → no route"
    );
}

#[test]
fn consumer_cli_notify_test_matches_route() {
    let store = PrefStore::new();
    let me = principal("carol");
    set_prefs(
        &store,
        &me,
        NotifPrefs {
            routing: vec![RoutingRule {
                channel: Channel::Email,
                matcher: build_routing_matcher(&[Class::Critical], &[], &[]).unwrap(),
            }],
            digest: DigestConfig::default(),
        },
        QuietHours::default(),
    );
    let preview = notify_test(
        &store,
        &me,
        Reason::Escalated,
        Class::Critical,
        Subsystem::Issue,
        12 * 60,
        0,
    );
    assert_eq!(
        preview,
        vec!["email".to_string()],
        "the CLI preview drives the SAME route decision"
    );
}

#[test]
fn wire_cost_bound_rejects_unbounded() {
    let huge: Vec<Predicate> = (0..myelin_query::MAX_PREDICATE_NODES)
        .map(|i| Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("class".into()),
            rhs: Expr::Lit(Literal::Int(i as i64)),
        })
        .collect();
    assert!(
        QueryAst::compiled(Predicate::Or(huge)).is_err(),
        "an unbounded matcher is rejected at construction (the wire cost-bound, 0 accepted)"
    );
}
