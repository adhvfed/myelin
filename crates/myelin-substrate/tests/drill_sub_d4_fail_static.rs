use myelin_harness::{
    Dependency, DependencyBreaker, DrillRegistry, DrillScenario, Label, Predicate, Scope,
    SignalName, SignalSource,
};
use myelin_identity::{Consistency, ConsistencyMode, Decision, Zookie};
use myelin_substrate::{AuthzServed, FailStaticAuthz, ServeError, TestClock, Thresholds};
use myelin_tenancy::TenantId;

fn bounded_stale() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::BoundedStale,
    }
}
fn strong(z: &str) -> Consistency {
    Consistency {
        at_least: Zookie(z.into()),
        mode: ConsistencyMode::Strong,
    }
}

fn injector_source<'a>(
    breaker: &'a DependencyBreaker,
    scope: &'a Scope,
) -> impl Fn() -> Result<Decision, ServeError> + 'a {
    move || {
        if breaker.is_broken(&Dependency::Identity, scope) {
            Err(ServeError("identity transient hiccup (injected)".into()))
        } else {
            Ok(Decision::Allow)
        }
    }
}

fn run_sub_d4_sequence() -> (SignalSource, FailStaticAuthz<TestClock>, i64) {
    let thresholds = Thresholds::load_canonical().expect("load canonical thresholds");
    let sla_secs: u64 = thresholds.revocation.sla_mins * 60;
    let fs_threshold = thresholds.fail_static.clone();
    let agent_token_ttl: u64 = fs_threshold.agent_token_ttl_secs;

    let fs = FailStaticAuthz::try_new_with_clock(sla_secs, &fs_threshold, TestClock::at(10_000))
        .expect("the authz fail-static cache constructs against the thresholds-file bound");
    assert!(
        fs.static_max() <= sla_secs,
        "the fail-static window sits under the revocation SLA (4.11): W={}s ≤ N={}s",
        fs.static_max(),
        sla_secs
    );
    assert!(
        fs.static_max() >= agent_token_ttl,
        "the window CONTAINS the short-lived agent token (4.11): W={}s ≥ agent-token-TTL={}s - an \
         agent token whose life == its run expires INSIDE the window",
        fs.static_max(),
        agent_token_ttl
    );

    let breaker = DependencyBreaker::new();
    let scope = Scope::Tenant(TenantId("acme".into()));
    let key = "acme|eu-west|p:alice|view@repo:core";

    let src = injector_source(&breaker, &scope);
    let healthy = fs.serve(key, &bounded_stale(), false, &src);
    assert!(
        matches!(healthy.served, AuthzServed::Fresh),
        "healthy read is fresh + caches"
    );
    assert!(
        healthy.is_allow(),
        "alice's grant is allowed and the coarse answer is cached"
    );

    breaker.break_dependency(Dependency::Identity, scope.clone());
    assert!(
        breaker.is_broken(&Dependency::Identity, &scope),
        "the injected hiccup must be in effect for the drill to be meaningful"
    );
    fs.clock().advance(31);
    let survived = fs.serve(key, &bounded_stale(), false, &src);
    assert!(
        matches!(survived.served, AuthzServed::Static),
        "during the Id hiccup the default-consistency read survives on the coarse fail-static cache"
    );
    assert!(
        survived.is_allow(),
        "authenticated traffic SURVIVES the hiccup (still Allow)"
    );

    let strong_during_hiccup = fs.serve(key, &strong("z-strong"), false, &src);
    assert!(
        matches!(strong_during_hiccup.served, AuthzServed::BypassClosed),
        "a zookie-stamped read BYPASSES the cache (the new-enemy guard)"
    );
    assert!(
        strong_during_hiccup.is_deny(),
        "a strong read fails CLOSED during the hiccup (never served stale)"
    );

    let mut allowed_after_revoke: i64 = 0;
    let mut revoked_after_window_denied = true;
    for i in 0..8 {
        if i == 4 {
            fs.clock().advance(fs.static_max() + 1);
        }
        let d = fs.serve(key, &bounded_stale(), true, &src);
        if d.is_allow() {
            allowed_after_revoke += 1;
        } else {
            if !matches!(d.served, AuthzServed::Revoked) && !matches!(d.served, AuthzServed::Closed)
            {
                revoked_after_window_denied = false;
            }
        }
    }
    assert!(
        revoked_after_window_denied,
        "every post-revoke read denied (revoked OR window-closed)"
    );

    breaker.restore_dependency(Dependency::Identity, scope.clone());
    assert!(
        !breaker.is_broken(&Dependency::Identity, &scope),
        "the injector restored to working"
    );

    let sig = fs.signals();
    let mut signals = SignalSource::new();
    signals.set_labelled(
        SignalName::FailStaticRatio,
        vec![Label::new("answer_class", "stale")],
        sig.stale as i64,
    );
    signals.set_scalar(
        SignalName::FailStaticStalenessSecs,
        sig.last_staleness_secs as i64,
    );
    signals.set_scalar(SignalName::CrossTenantCount, allowed_after_revoke);

    (signals, fs, allowed_after_revoke)
}

#[test]
fn sub_d4_fail_static_survives_hiccup_and_denies_revoked() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let sla_secs = (thresholds.revocation.sla_mins * 60) as i64;

    let (signals, fs, allowed_after_revoke) = run_sub_d4_sequence();
    let sig = fs.signals();

    signals
        .assert_labelled(
            SignalName::FailStaticRatio,
            vec![Label::new("answer_class", "stale")],
            Predicate::Gte(1),
        )
        .expect_green();

    signals
        .assert_signal(
            SignalName::FailStaticStalenessSecs,
            Predicate::Lte(fs.static_max() as i64),
        )
        .expect_green();
    assert!(
        (sig.last_staleness_secs as i64) <= sla_secs,
        "staleness age ({}s) ≤ the revocation SLA ({sla_secs}s)",
        sig.last_staleness_secs
    );

    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        allowed_after_revoke, 0,
        "0 successful authz after the cache for a revoked subject once the window closes (SUB-D4)"
    );

    assert!(sig.fresh >= 1, "≥ 1 fresh answer (the live authenticate)");
    assert!(
        sig.stale >= 1,
        "≥ 1 stale answer (the hiccup survival rung)"
    );

    println!(
        "[P-087 DRILL GREEN 2026-06-19] SUB-D4 Id-hiccup / fail-static: tenant=acme subject=p:alice \
         object=repo:core → authenticated traffic SURVIVED on the coarse fail-static cache \
         (fresh={}, stale={}, closed={}, staleness_age={}s ≤ static_max={}s ≤ revocation_SLA={}s); \
         a Strong/zookie read BYPASSED the cache and failed closed (new-enemy guard); the window \
         CONTAINS the agent-token TTL (an agent token expires inside W); a revoked subject got \
         allowed_after_revoke=0 across an 8-read batch spanning the window-close (revoked actor \
         denied once the window closes) - thresholds read from the canonical file, never hardcoded; \
         W is [OPEN - LEGAL] (L-1), the static_max ≤ SLA ≥ token-TTL constraint enforced by the \
         constructor regardless",
        sig.fresh, sig.stale, sig.closed, sig.last_staleness_secs, fs.static_max(), sla_secs
    );
}

#[test]
fn sub_d4_registers_in_the_drill_registry_and_reruns_green() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "sub-d4-fail-static-vs-identity-hiccup",
        |_ctx| {
            let (signals, _fs, _allowed) = run_sub_d4_sequence();
            signals.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        },
    ));
    assert_eq!(registry.len(), 1);

    let first = registry.run_all();
    let second = registry.run_all();
    assert!(first[0].is_pass(), "SUB-D4 reads green: {first:?}");
    assert!(second[0].is_pass(), "SUB-D4 re-runs green: {second:?}");
    assert!(registry.all_green(), "the SUB-D4 drill suite is green");

    let row = first[0].artifact_row("2026-06-19");
    assert!(
        row.contains("sub-d4-fail-static-vs-identity-hiccup"),
        "the dated artifact names the drill"
    );
    println!("{row}");
}
