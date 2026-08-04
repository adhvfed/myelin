use myelin_harness::{Dependency, DependencyBreaker, Scope};
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

fn cache_from_file(t0: u64) -> (FailStaticAuthz<TestClock>, u64, u64) {
    let t = Thresholds::load_canonical().expect("load canonical thresholds");
    let sla = t.revocation.sla_mins * 60;
    let token_ttl = t.fail_static.agent_token_ttl_secs;
    let fs = FailStaticAuthz::try_new_with_clock(sla, &t.fail_static, TestClock::at(t0))
        .expect("the authz fail-static cache constructs against the thresholds-file bound");
    (fs, sla, token_ttl)
}

fn provider<'a>(
    breaker: &'a DependencyBreaker,
    scope: &'a Scope,
) -> impl Fn() -> Result<Decision, ServeError> + 'a {
    move || {
        if breaker.is_broken(&Dependency::Identity, scope) {
            Err(ServeError("identity authz hiccup".into()))
        } else {
            Ok(Decision::Allow)
        }
    }
}

#[test]
fn cdc_1_10_constructor_enforces_the_threshold_file_bound() {
    let t = Thresholds::load_canonical().expect("load");
    let sla = t.revocation.sla_mins * 60;
    let mut over = t.fail_static.clone();
    over.static_max_default_secs = sla + 1;
    assert!(
        FailStaticAuthz::try_new_with_clock(sla, &over, TestClock::at(0)).is_err(),
        "a static_max over the revocation SLA must reject (4.11 / §8.2)"
    );
    let (fs, sla, token_ttl) = cache_from_file(0);
    assert!(fs.static_max() <= sla, "W ≤ revocation SLA");
    assert!(
        fs.static_max() >= token_ttl,
        "W ≥ agent-token TTL (the window contains the token)"
    );
}

#[test]
fn cdc_1_10_sequence_authenticated_hiccup_stale_then_denied_at_window_close() {
    let (fs, sla, _token_ttl) = cache_from_file(10_000);
    let breaker = DependencyBreaker::new();
    let scope = Scope::Tenant(TenantId("acme".into()));
    let src = provider(&breaker, &scope);
    let key = "acme|eu|alice|view@repo:core";

    let a = fs.serve(key, &bounded_stale(), false, &src);
    assert_eq!(a.served, AuthzServed::Fresh);
    assert!(a.is_allow());

    breaker.break_dependency(Dependency::Identity, scope.clone());
    fs.clock().advance(30);
    let f = fs.serve(key, &bounded_stale(), false, &src);
    assert_eq!(
        f.served,
        AuthzServed::Fresh,
        "age == fresh_ttl is still fresh"
    );

    fs.clock().advance(100);
    let s = fs.serve(key, &bounded_stale(), false, &src);
    assert_eq!(
        s.served,
        AuthzServed::Static,
        "the consumer survives the hiccup on the coarse grant"
    );
    assert!(s.is_allow() && s.is_degraded());

    fs.clock().advance(fs.static_max());
    let c = fs.serve(key, &bounded_stale(), false, &src);
    assert_eq!(
        c.served,
        AuthzServed::Closed,
        "past W → deny, never fail open"
    );
    assert!(c.is_deny());

    let sig = fs.signals();
    assert_eq!(sig.fresh, 2, "the live read + the within-ttl cached read");
    assert_eq!(sig.stale, 1, "one degraded answer");
    assert_eq!(sig.closed, 1, "one denied answer at window close");
    assert!(
        (sig.last_staleness_secs) <= sla,
        "staleness ≤ the revocation SLA"
    );
}

#[test]
fn cdc_1_10_zookie_bypass_fails_closed_on_hiccup() {
    let (fs, _sla, _ttl) = cache_from_file(0);
    let breaker = DependencyBreaker::new();
    let scope = Scope::Tenant(TenantId("acme".into()));
    let src = provider(&breaker, &scope);
    let key = "acme|eu|alice|view@repo:core";

    let _ = fs.serve(key, &bounded_stale(), false, &src);
    breaker.break_dependency(Dependency::Identity, scope.clone());
    fs.clock().advance(31);

    let z = fs.serve(key, &strong("z-strong"), false, &src);
    assert_eq!(
        z.served,
        AuthzServed::BypassClosed,
        "a zookie read bypasses the cache"
    );
    assert!(
        z.is_deny(),
        "a strong read fails CLOSED on a hiccup (never stale)"
    );
}

#[test]
fn cdc_1_10_never_escalates_and_never_fails_open() {
    let (fs, _sla, _ttl) = cache_from_file(0);

    let cold = fs.serve("cold", &bounded_stale(), false, || {
        Err(ServeError("hiccup".into()))
    });
    assert_eq!(cold.served, AuthzServed::Closed);
    assert!(
        cold.is_deny(),
        "a cold hiccup denies - never fabricates an open answer"
    );

    let _ = fs.serve("k", &bounded_stale(), false, || Ok(Decision::Deny));
    fs.clock().advance(31);
    let stale = fs.serve("k", &bounded_stale(), false, || {
        Err(ServeError("hiccup".into()))
    });
    assert_eq!(stale.served, AuthzServed::Static);
    assert!(
        stale.is_deny(),
        "the stale fallback replays the Deny - never escalates to Allow"
    );
}

#[test]
fn cdc_1_10_value_w_is_open_legal_but_the_wiring_ships() {
    let t = Thresholds::load_canonical().expect("load");
    assert_eq!(t.fail_static.status, "OPEN - LEGAL", "W is the legal flag");
    assert!(
        t.fail_static.ratified_static_max_secs().is_err(),
        "reading the unratified W as a number is a loud error (never a silent default)"
    );
    let (fs, sla, _ttl) = cache_from_file(0);
    assert!(fs.static_max() <= sla, "the wiring ships regardless of W");
}
