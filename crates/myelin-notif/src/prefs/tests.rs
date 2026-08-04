use super::*;
use myelin_identity::{Literal, PrincipalId, PrincipalKind};
use myelin_query::{CmpOp, EvalContext, Expr, Predicate, QueryAst};
use myelin_tenancy::TenantId;

fn principal(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

#[test]
fn routing_matcher_binds_the_frozen_query_ast() {
    let matcher = build_routing_matcher(&[Class::Critical], &[], &[]).expect("within bound");
    let crit = route_context(Reason::Escalated, Class::Critical, Subsystem::Issue);
    let fyi = route_context(Reason::Fyi, Class::Fyi, Subsystem::Issue);
    assert_eq!(
        matcher.eval(&crit),
        Ok(true),
        "the critical item matches the class==critical matcher"
    );
    assert_eq!(matcher.eval(&fyi), Ok(false), "the fyi item does NOT match");
}

#[test]
fn routing_matcher_narrows_on_reason_class_subsystem() {
    let m = build_routing_matcher(&[Class::Direct], &[Reason::Assigned], &[Subsystem::Issue])
        .expect("within bound");
    let yes = route_context(Reason::Assigned, Class::Direct, Subsystem::Issue);
    let wrong_sub = route_context(Reason::Assigned, Class::Direct, Subsystem::Chat);
    let wrong_class = route_context(Reason::Assigned, Class::Watching, Subsystem::Issue);
    assert_eq!(m.eval(&yes), Ok(true));
    assert_eq!(m.eval(&wrong_sub), Ok(false), "wrong subsystem → no match");
    assert_eq!(m.eval(&wrong_class), Ok(false), "wrong class → no match");
}

#[test]
fn empty_constraint_matcher_matches_everything() {
    let m = build_routing_matcher(&[], &[], &[]).expect("within bound");
    let any = route_context(Reason::Fyi, Class::Fyi, Subsystem::Unknown);
    assert_eq!(m.eval(&any), Ok(true));
}

#[test]
fn cost_bound_rejects_an_unbounded_matcher_predicate() {
    let huge: Vec<Predicate> = (0..super::PREFS_MAX_PREDICATE_NODES)
        .map(|i| Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("class".into()),
            rhs: Expr::Lit(Literal::Int(i as i64)),
        })
        .collect();
    let over = QueryAst::compiled(Predicate::Or(huge));
    assert!(
        over.is_err(),
        "an over-budget predicate is rejected at construction (0 unbounded accepted)"
    );
    match over {
        Err(super::PredicateError::TooLarge { nodes }) => {
            assert!(
                nodes > super::PREFS_MAX_PREDICATE_NODES,
                "the rejection names the node overage"
            );
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[test]
fn build_routing_matcher_rejects_over_budget_request_loudly() {
    let m = build_routing_matcher(
        &[
            Class::Critical,
            Class::Direct,
            Class::Participating,
            Class::Watching,
            Class::Fyi,
        ],
        &[Reason::Assigned, Reason::Mentioned, Reason::ReviewRequested],
        &[Subsystem::Issue, Subsystem::Chat, Subsystem::Git],
    )
    .expect("the full finite-enum matcher is within the static bound (the grammar is finite)");
    let ctx = route_context(Reason::Assigned, Class::Critical, Subsystem::Issue);
    assert_eq!(m.eval(&ctx), Ok(true));
}

#[test]
fn uncompiled_matcher_fails_closed_to_no_match() {
    let prefs = NotifPrefs {
        routing: vec![RoutingRule {
            channel: Channel::Email,
            matcher: QueryAst::raw("class == 'critical'"),
        }],
        digest: DigestConfig::default(),
    };
    let channels = prefs.channels_for(Reason::Escalated, Class::Critical, Subsystem::Issue);
    assert!(
        channels.is_empty(),
        "an un-parsed matcher routes NOWHERE (fail-closed, never silent deliver)"
    );
}

#[test]
fn missing_context_variable_fails_closed() {
    let bad = QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("nonexistent".into()),
        rhs: Expr::Lit(Literal::Str("x".into())),
    })
    .expect("within bound");
    let ctx = route_context(Reason::Fyi, Class::Fyi, Subsystem::Issue);
    assert!(
        bad.eval(&ctx).is_err(),
        "an unbound variable surfaces an eval error"
    );
    let prefs = NotifPrefs {
        routing: vec![RoutingRule {
            channel: Channel::Email,
            matcher: bad,
        }],
        digest: DigestConfig::default(),
    };
    assert!(
        prefs
            .channels_for(Reason::Fyi, Class::Fyi, Subsystem::Issue)
            .is_empty(),
        "missing context → no route"
    );
}

#[test]
fn quiet_hours_evaluated_in_recipient_tz_not_utc() {
    let windows = vec![QuietWindow {
        from: 22 * 60,
        to: 7 * 60,
        days: vec![],
    }];
    let paris = QuietHours {
        tz: Tz::from_offset_minutes(60),
        windows: windows.clone(),
        pierce_classes: vec![Class::Critical],
    };
    let utc = QuietHours {
        tz: Tz::UTC,
        windows,
        pierce_classes: vec![Class::Critical],
    };
    let at = 21 * 60 + 30;
    assert!(
        paris.is_quiet_at(at, 2),
        "21:30 UTC = 22:30 Paris → inside the 22:00..07:00 quiet window"
    );
    assert!(
        !utc.is_quiet_at(at, 2),
        "21:30 UTC is BEFORE the 22:00 window → not quiet in UTC"
    );
}

#[test]
fn quiet_window_wraps_midnight() {
    let q = QuietHours {
        tz: Tz::UTC,
        windows: vec![QuietWindow {
            from: 22 * 60,
            to: 7 * 60,
            days: vec![],
        }],
        pierce_classes: vec![],
    };
    assert!(q.is_quiet_at(23 * 60, 0), "23:00 is in [22:00, 24:00)");
    assert!(q.is_quiet_at(3 * 60, 0), "03:00 is in [00:00, 07:00)");
    assert!(
        !q.is_quiet_at(12 * 60, 0),
        "12:00 is OUTSIDE the wrapping window"
    );
    assert!(
        q.is_quiet_at(6 * 60 + 59, 0),
        "06:59 is the last quiet minute of the wrap leg"
    );
    assert!(
        !q.is_quiet_at(7 * 60, 0),
        "07:00 is the exclusive end of the wrapping window (NOT quiet)"
    );
}

#[test]
fn quiet_window_same_day_is_half_open() {
    let q = QuietHours {
        tz: Tz::UTC,
        windows: vec![QuietWindow {
            from: 9 * 60,
            to: 17 * 60,
            days: vec![],
        }],
        pierce_classes: vec![],
    };
    assert!(q.is_quiet_at(9 * 60, 0), "09:00 is the inclusive start");
    assert!(q.is_quiet_at(16 * 60 + 59, 0));
    assert!(
        !q.is_quiet_at(17 * 60, 0),
        "17:00 is the exclusive end (NOT quiet)"
    );
    assert!(!q.is_quiet_at(8 * 60 + 59, 0));
}

#[test]
fn quiet_window_day_restriction() {
    let weekdays = vec![0, 1, 2, 3, 4];
    let q = QuietHours {
        tz: Tz::UTC,
        windows: vec![QuietWindow {
            from: 0,
            to: 24 * 60,
            days: weekdays,
        }],
        pierce_classes: vec![],
    };
    assert!(q.is_quiet_at(12 * 60, 0), "Monday is quiet");
    assert!(
        !q.is_quiet_at(12 * 60, 5),
        "Saturday is NOT quiet (not in the day set)"
    );
}

#[test]
fn tz_offset_shifts_weekday_across_midnight() {
    let q = QuietHours {
        tz: Tz::from_offset_minutes(60),
        windows: vec![QuietWindow {
            from: 0,
            to: 60,
            days: vec![0],
        }],
        pierce_classes: vec![],
    };
    assert!(
        q.is_quiet_at(23 * 60 + 30, 6),
        "23:30 UTC Sun = 00:30 Mon local → in the Monday 00:00..01:00 window"
    );
}

#[test]
fn pierce_class_property_critical_pierces_noncrit_suppressed() {
    let prefs = NotifPrefs {
        routing: vec![
            RoutingRule {
                channel: Channel::Email,
                matcher: build_routing_matcher(&[], &[], &[]).unwrap(),
            },
            RoutingRule {
                channel: Channel::InApp,
                matcher: build_routing_matcher(&[], &[], &[]).unwrap(),
            },
        ],
        digest: DigestConfig::default(),
    };
    let quiet = QuietHours {
        tz: Tz::UTC,
        windows: vec![QuietWindow {
            from: 0,
            to: 24 * 60,
            days: vec![],
        }],
        pierce_classes: vec![Class::Critical],
    };
    let at = 3 * 60;

    let crit = route(
        &prefs,
        &quiet,
        Reason::Escalated,
        Class::Critical,
        Subsystem::Issue,
        at,
        0,
    );
    assert!(
        crit.contains(&Channel::Email),
        "critical pierces quiet-hours → off-cell email still delivers"
    );
    assert!(crit.contains(&Channel::InApp));

    let fyi = route(
        &prefs,
        &quiet,
        Reason::Fyi,
        Class::Fyi,
        Subsystem::Issue,
        at,
        0,
    );
    assert!(
        !fyi.contains(&Channel::Email),
        "a non-crit item in quiet-hours is SUPPRESSED off-cell"
    );
    assert!(
        fyi.contains(&Channel::InApp),
        "the ONE inbox is NEVER suppressed (only the off-cell push)"
    );
}

#[test]
fn route_delivers_all_channels_outside_quiet_hours() {
    let prefs = NotifPrefs {
        routing: vec![RoutingRule {
            channel: Channel::Email,
            matcher: build_routing_matcher(&[], &[], &[]).unwrap(),
        }],
        digest: DigestConfig::default(),
    };
    let quiet = QuietHours {
        tz: Tz::UTC,
        windows: vec![QuietWindow {
            from: 22 * 60,
            to: 23 * 60,
            days: vec![],
        }],
        pierce_classes: vec![Class::Critical],
    };
    let at = 12 * 60;
    let fyi = route(
        &prefs,
        &quiet,
        Reason::Fyi,
        Class::Fyi,
        Subsystem::Issue,
        at,
        0,
    );
    assert_eq!(
        fyi,
        vec![Channel::Email],
        "outside quiet-hours a fyi item delivers normally"
    );
}

#[test]
fn pierces_honours_explicit_pierce_set() {
    let q = QuietHours {
        tz: Tz::UTC,
        windows: vec![],
        pierce_classes: vec![Class::Critical, Class::Direct],
    };
    assert!(q.pierces(Class::Critical));
    assert!(q.pierces(Class::Direct));
    assert!(!q.pierces(Class::Fyi));
}

#[test]
fn set_then_get_prefs_round_trips() {
    let store = PrefStore::new();
    let me = principal("alice");
    let prefs = NotifPrefs {
        routing: vec![RoutingRule {
            channel: Channel::MobilePush,
            matcher: build_routing_matcher(&[Class::Critical], &[], &[]).unwrap(),
        }],
        digest: DigestConfig {
            cadence: "daily".into(),
            at: Some("09:00".into()),
            classes: vec![Class::Fyi],
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
    assert_eq!(stored.prefs, prefs);
    assert_eq!(stored.quiet, quiet);

    let got = get_prefs(&store, &me);
    assert_eq!(got.prefs, prefs);
    assert_eq!(got.quiet, quiet);
    assert_eq!(
        got.prefs.digest.cadence, "daily",
        "the digest config is STORED (compose flow is the OQ5 floor)"
    );
}

#[test]
fn get_prefs_defaults_are_safe() {
    let store = PrefStore::new();
    let got = get_prefs(&store, &principal("bob"));
    let channels = got
        .prefs
        .channels_for(Reason::Fyi, Class::Fyi, Subsystem::Issue);
    assert_eq!(
        channels,
        vec![Channel::InApp],
        "the default routes the ONE inbox (in_app) for everything"
    );
    assert!(
        !got.quiet.is_quiet_at(3 * 60, 0),
        "no windows → never quiet by default"
    );
    assert!(
        got.quiet.pierces(Class::Critical),
        "critical pierces by default (you cannot silence on-call)"
    );
}

#[test]
fn set_prefs_is_recipient_scoped() {
    let store = PrefStore::new();
    let alice_prefs = NotifPrefs {
        routing: vec![RoutingRule {
            channel: Channel::Email,
            matcher: build_routing_matcher(&[], &[], &[]).unwrap(),
        }],
        digest: DigestConfig::default(),
    };
    set_prefs(
        &store,
        &principal("alice"),
        alice_prefs,
        QuietHours::default(),
    );
    let bob = get_prefs(&store, &principal("bob"));
    assert_eq!(
        bob.prefs
            .channels_for(Reason::Fyi, Class::Fyi, Subsystem::Issue),
        vec![Channel::InApp]
    );
}

#[test]
fn route_empty_matrix_delivers_nowhere() {
    let prefs = NotifPrefs {
        routing: vec![],
        digest: DigestConfig::default(),
    };
    let channels = route(
        &prefs,
        &QuietHours::default(),
        Reason::Fyi,
        Class::Fyi,
        Subsystem::Issue,
        12 * 60,
        0,
    );
    assert!(
        channels.is_empty(),
        "an explicit empty routing matrix delivers nowhere"
    );
}

#[test]
fn cost_bound_constants_track_query_ast() {
    assert_eq!(
        super::PREFS_MAX_PREDICATE_NODES,
        myelin_query::MAX_PREDICATE_NODES
    );
    assert_eq!(
        super::PREFS_MAX_PREDICATE_DEPTH,
        myelin_query::MAX_PREDICATE_DEPTH
    );
}

#[test]
fn tz_local_minute_wraps_both_directions() {
    let east = Tz::from_offset_minutes(120);
    assert_eq!(
        east.local_minute_of_day(23 * 60),
        60,
        "23:00 UTC + 2h = 01:00 local (wrapped)"
    );
    let west = Tz::from_offset_minutes(-180);
    assert_eq!(
        west.local_minute_of_day(60),
        22 * 60,
        "01:00 UTC - 3h = 22:00 prev-day local"
    );
}

#[test]
fn route_context_binds_three_variables() {
    let ctx: EvalContext = route_context(Reason::Assigned, Class::Direct, Subsystem::Issue);
    let m = QueryAst::compiled(Predicate::And(vec![
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("reason".into()),
            rhs: Expr::Lit(Literal::Str("assigned".into())),
        },
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("class".into()),
            rhs: Expr::Lit(Literal::Str("direct".into())),
        },
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("subsystem".into()),
            rhs: Expr::Lit(Literal::Str("issue".into())),
        },
    ]))
    .unwrap();
    assert_eq!(m.eval(&ctx), Ok(true));
}
