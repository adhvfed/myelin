use myelin_substrate::thresholds::Thresholds;
use myelin_substrate::{
    Answer, FailStatic, FailStaticError, ServeError, StalenessBound, TestClock,
};

fn bound_from_file() -> (StalenessBound, u64) {
    let t = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    let revocation_sla_secs = t.revocation.sla_mins * 60;
    let bound = StalenessBound::from_threshold(revocation_sla_secs, &t.fail_static);
    let seed_static_max = t.fail_static.static_max_default_secs;
    (bound, seed_static_max)
}

#[test]
fn cdc_1_10_constructor_enforces_the_threshold_file_bound() {
    let (bound, seed) = bound_from_file();

    FailStatic::<&str, u8>::try_new(30, seed, bound)
        .expect("the seed static_max == revocation SLA admits");

    let err = FailStatic::<&str, u8>::try_new(30, bound.revocation_sla_secs + 1, bound)
        .expect_err("over the revocation SLA must be rejected");
    assert!(
        matches!(err, FailStaticError::ExceedsRevocationSla { .. }),
        "got {err:?}"
    );

    let err = FailStatic::<&str, u8>::try_new(10, bound.agent_token_ttl_secs - 1, bound)
        .expect_err("under the agent-token TTL must be rejected");
    assert!(
        matches!(err, FailStaticError::BelowAgentTokenTtl { .. }),
        "got {err:?}"
    );
}

#[test]
fn cdc_1_10_value_w_is_open_legal_but_the_mechanism_ships() {
    let t = Thresholds::load_canonical().expect("load");
    assert_eq!(t.fail_static.status, "OPEN - LEGAL", "W is the legal flag");
    assert!(
        t.fail_static.ratified_static_max_secs().is_err(),
        "reading the unratified W as a number is a loud error (never a silent default)"
    );
    let (bound, seed) = bound_from_file();
    FailStatic::<&str, u8>::try_new(30, seed, bound).expect("the mechanism ships regardless of W");
}

#[test]
fn cdc_1_10_sequence_authenticated_hiccup_stale_then_denied_at_window_close() {
    let (bound, seed) = bound_from_file();
    let clock = TestClock::at(10_000);
    let fs: FailStatic<&'static str, &'static str, _> =
        FailStatic::try_new_with_clock(30, seed, bound, clock).expect("valid bound from the file");

    let up = std::cell::Cell::new(true);
    let refresh = || {
        if up.get() {
            Ok("actor=active;grants=coarse")
        } else {
            Err(ServeError("identity transient hiccup".into()))
        }
    };

    assert_eq!(
        fs.get("actor:alice", refresh),
        Answer::Fresh("actor=active;grants=coarse")
    );

    up.set(false);
    advance(&fs, 30);
    assert_eq!(
        fs.get("actor:alice", refresh),
        Answer::Fresh("actor=active;grants=coarse"),
        "age == fresh_ttl is still fresh"
    );

    advance(&fs, 100);
    let a = fs.get("actor:alice", refresh);
    assert_eq!(
        a,
        Answer::Static("actor=active;grants=coarse"),
        "stale serves the cached coarse grants"
    );
    assert!(a.is_degraded(), "the stale answer is marked degraded");

    advance(&fs, seed);
    assert_eq!(
        fs.get("actor:alice", refresh),
        Answer::Closed,
        "past W → deny, never fail open"
    );

    let s = fs.signals();
    assert_eq!(
        s.fresh, 2,
        "two fresh answers (the live read + the within-ttl cached read)"
    );
    assert_eq!(s.stale, 1, "one degraded answer");
    assert_eq!(s.closed, 1, "one denied answer at window close");
    assert!(
        s.last_staleness_secs <= fs.static_max(),
        "staleness never exceeds the budget (static_max ≤ revocation SLA)"
    );
}

#[test]
fn cdc_1_10_cold_hiccup_never_fails_open() {
    let (bound, seed) = bound_from_file();
    let fs: FailStatic<&str, u8> = FailStatic::try_new(30, seed, bound).expect("valid");
    let denied = fs.get("never-seen", || Err(ServeError("hiccup".into())));
    assert_eq!(
        denied,
        Answer::Closed,
        "a cold hiccup denies - never fabricates an open answer"
    );
    assert_eq!(fs.signals().closed, 1);
}

fn advance<K: std::hash::Hash + Eq, T: Clone>(fs: &FailStatic<K, T, TestClock>, secs: u64) {
    fs.clock().advance(secs);
}
