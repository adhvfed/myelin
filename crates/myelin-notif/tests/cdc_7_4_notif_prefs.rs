//! # CDC — contract 7.4 `get_prefs / set_prefs` (prefs + quiet-hours over the frozen QueryAst) (P-188)
//!
//! **Architecture:** `notifications.md` §2.2 (the preference matcher binds the frozen `myelin-query`
//! [`QueryAst`] = the EventMatcher core 3.4 — Notif invents NO second predicate language; quiet-hours
//! in the recipient tz; critical/escalated pierce by default via `pierce_classes` — the one
//! deliberate override, you cannot silence an on-call page). **Contract:** **7.4** `get_prefs /
//! set_prefs(principal, routing, quiet_hours, digest)`. **Consumed:** **13.3 / 3.4** the frozen
//! `QueryAst`.
//!
//! This CDC pins the 7.4 seam from BOTH sides:
//!
//! - **PROVIDER (Notif owns 7.4):** `set_prefs` UPSERTs a principal's routing matrix (each rule a
//!   channel + a cost-bounded frozen-`QueryAst` matcher) + quiet-hours; `get_prefs` reads them back
//!   (safe defaults for a principal with none); `route` is the delivery decision (matcher ∧ ¬quiet,
//!   unless the class pierces). A critical item pierces quiet-hours; a non-crit item in a quiet
//!   window is suppressed off-cell (the inbox row is NEVER suppressed).
//! - **CONSUMER (the router / the inbox UI / the CLI / Identity's CaveatContext):** the matcher is
//!   the ONE predicate language — a consumer that wants a routing rule compiles a `QueryAst` (the
//!   SAME tree the EventMatcher/caveat read), never a second DSL. Proven here by the consumer-built
//!   matcher evaluating over the SAME bounded interpreter, and the CLI `notify test` previewing the
//!   SAME `route` decision the router uses (no second decision path).
//!
//! The two halves agree on the WIRE: the `QueryAst` predicate grammar + the `route_context`
//! variable namespace (`reason`/`class`/`subsystem`) + the `pierce_classes` override. A drift on
//! either side (a second predicate language, a pierce that silences on-call, a quiet-hours that
//! suppresses the audit row) breaks THIS build.

use myelin_identity::{Literal, Principal, PrincipalId, PrincipalKind};
use myelin_notif::cli::notify_test;
use myelin_notif::prefs::{
    build_routing_matcher, get_prefs, route, set_prefs, Channel, DigestConfig, NotifPrefs, PrefStore,
    QuietHours, QuietWindow, RoutingRule, Tz,
};
use myelin_notif::{Class, Reason, Subsystem};
use myelin_query::{CmpOp, Expr, Predicate, QueryAst};
use myelin_tenancy::TenantId;

fn principal(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, TenantId("acme".into()))
}

/// **PROVIDER side (Notif owns 7.4): set_prefs UPSERTs, get_prefs reads back, route decides.** The
/// stored routing matrix routes a critical item to mobile_push; quiet-hours suppress a fyi but
/// critical pierces — the provider's full decision.
#[test]
fn provider_set_get_and_route_decision() {
    let store = PrefStore::new();
    let me = principal("alice");

    // route critical → mobile_push AND in_app; everything else → in_app only.
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
        digest: DigestConfig { cadence: "off".into(), at: None, classes: vec![] },
    };
    // quiet all night, Paris tz; critical pierces by default.
    let quiet = QuietHours {
        tz: Tz::from_offset_minutes(60),
        windows: vec![QuietWindow { from: 22 * 60, to: 7 * 60, days: vec![] }],
        pierce_classes: vec![Class::Critical],
    };

    let stored = set_prefs(&store, &me, prefs.clone(), quiet.clone());
    assert_eq!(stored.prefs, prefs, "set_prefs echoes the stored routing matrix");

    let got = get_prefs(&store, &me);
    assert_eq!(got.quiet, quiet, "get_prefs reads back the quiet-hours");

    // 02:00 Paris = 01:00 UTC — inside the 22:00..07:00 quiet window.
    let utc_min = 60;
    // CRITICAL pierces: delivers to mobile_push even at 02:00 local.
    let crit = route(&got.prefs, &got.quiet, Reason::Escalated, Class::Critical, Subsystem::Issue, utc_min, 2);
    assert!(crit.contains(&Channel::MobilePush), "critical pierces quiet-hours (on-call cannot be silenced)");
    // FYI suppressed off-cell; the in-cell inbox still receives.
    let fyi = route(&got.prefs, &got.quiet, Reason::Fyi, Class::Fyi, Subsystem::Issue, utc_min, 2);
    assert!(!fyi.contains(&Channel::MobilePush), "a non-crit item in quiet-hours is suppressed off-cell");
    assert!(fyi.contains(&Channel::InApp), "the ONE inbox row is NEVER suppressed (only the off-cell push)");
}

/// **CONSUMER side: a consumer builds the routing matcher as the ONE frozen QueryAst.** A consumer
/// (the router / a subsystem registering a rule) compiles a predicate over the SAME bounded
/// interpreter the EventMatcher/caveat use — never a second predicate language. The consumer-built
/// matcher evaluates byte-for-byte through `QueryAst::eval`.
#[test]
fn consumer_matcher_is_the_one_query_ast() {
    // The consumer builds a matcher by hand (the same `Predicate` tree the EventMatcher reads).
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
            routing: vec![RoutingRule { channel: Channel::Email, matcher }],
            digest: DigestConfig::default(),
        },
        QuietHours::default(),
    );

    // The provider's route uses the consumer's matcher: critical/direct → email; watching → none.
    let got = get_prefs(&store, &me);
    assert!(
        route(&got.prefs, &got.quiet, Reason::Assigned, Class::Direct, Subsystem::Issue, 12 * 60, 0)
            .contains(&Channel::Email),
        "the direct item routes via the consumer-built matcher"
    );
    assert!(
        route(&got.prefs, &got.quiet, Reason::Watched, Class::Watching, Subsystem::Issue, 12 * 60, 0)
            .is_empty(),
        "the watching item does not match → no route"
    );
}

/// **CONSUMER side: the CLI `notify test` previews the SAME `route` decision the router uses.** A
/// preview can never disagree with real delivery (one decision path). The consumer (the operator
/// running `notify test`) sees exactly what the router would deliver.
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
    // CLI preview at noon.
    let preview = notify_test(&store, &me, Reason::Escalated, Class::Critical, Subsystem::Issue, 12 * 60, 0);
    assert_eq!(preview, vec!["email".to_string()], "the CLI preview drives the SAME route decision");
}

/// **The cost-bound is the WIRE invariant both sides honour: 0 unbounded matcher accepted.** An
/// over-budget predicate is rejected at construction, so neither side can store one.
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
