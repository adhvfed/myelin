//! # CDC 1.10 — `FailStatic<T>` the bounded-staleness fail-static mechanism (P-S18)
//!
//! **Contract-index:** row 1.10 (`FailStatic<T>`), carrying row 4.11 (the Id-usage staleness bound
//! `static_max ≤ revocation-SLA ≥ agent-token-TTL`; the value W is `[OPEN — LEGAL]`, L-1). This
//! consumer-driven contract test exercises the 1.10 PROVIDER shape from OUTSIDE the crate (a
//! caller — e.g. an authz read path — holds a `FailStatic` and reads `Fresh | Static | Closed`):
//!   - the constructor enforces the §8.2 bound read from the canonical thresholds file (P-S22): a
//!     `static_max` over the revocation SLA, or under the agent-token TTL, does NOT construct;
//!   - on a transient hiccup, already-cached traffic is served `Fresh` within `fresh_ttl`, `Static`
//!     (degraded) within `static_max`, and `Closed` (deny) once the budget is spent — **never open**;
//!   - the contract-1.8 fresh/stale/closed + staleness-age signals are exported.
//!
//! The CONSUMER-against-a-real-Identity side (inject a real Identity hiccup; the zookie bypass;
//! revoke-at-window-close) is **P-S25 (SUB-D4)** — named, not skipped. Here the provider mechanism
//! is exercised against a scripted upstream whose hiccup we control deterministically.
//!
//! FLOOR (EI-01 §3): the VALUE W (`static_max_secs`) is `[OPEN — LEGAL]` in the thresholds file, so
//! this CDC drives the mechanism against the engineering SEED (`static_max_default_secs`, the
//! largest value the constraint admits) + the agent-token-TTL lower bound — both read from the file,
//! no hardcoded magic number (the thresholds-file rule).

use myelin_substrate::thresholds::Thresholds;
use myelin_substrate::{
    Answer, FailStatic, FailStaticError, ServeError, StalenessBound, TestClock,
};

/// Read the §8.2 staleness bound from the canonical thresholds file (P-S22) — the upper bound is the
/// revocation SLA (minutes → seconds), the lower bound is the agent-token TTL (seconds). NO hardcoded
/// number: a drill reads its threshold from the file (the thresholds-file rule, EI-01 §3).
fn bound_from_file() -> (StalenessBound, u64) {
    let t = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    let revocation_sla_secs = t.revocation.sla_mins * 60;
    let bound = StalenessBound::from_threshold(revocation_sla_secs, &t.fail_static);
    // the engineering seed (== the largest value the constraint admits); the ratified W is
    // [OPEN — LEGAL] and is a loud error to read as a number (asserted below).
    let seed_static_max = t.fail_static.static_max_default_secs;
    (bound, seed_static_max)
}

/// **CDC 1.10 — the constructor enforces the §8.2 bound read from the thresholds file.** A
/// `static_max` over the revocation SLA, or under the agent-token TTL, does not construct; the seed
/// value (== revocation SLA) constructs.
#[test]
fn cdc_1_10_constructor_enforces_the_threshold_file_bound() {
    let (bound, seed) = bound_from_file();

    // the seed value (300s == revocation SLA) is exactly the upper boundary → admitted (≤).
    FailStatic::<&str, u8>::try_new(30, seed, bound)
        .expect("the seed static_max == revocation SLA admits");

    // one second over the revocation SLA → REJECTED (a revoked actor would outlive N).
    let err = FailStatic::<&str, u8>::try_new(30, bound.revocation_sla_secs + 1, bound)
        .expect_err("over the revocation SLA must be rejected");
    assert!(
        matches!(err, FailStaticError::ExceedsRevocationSla { .. }),
        "got {err:?}"
    );

    // one second under the agent-token TTL → REJECTED (the window must contain the token).
    let err = FailStatic::<&str, u8>::try_new(10, bound.agent_token_ttl_secs - 1, bound)
        .expect_err("under the agent-token TTL must be rejected");
    assert!(
        matches!(err, FailStaticError::BelowAgentTokenTtl { .. }),
        "got {err:?}"
    );
}

/// **CDC 1.10 — the value W is `[OPEN — LEGAL]` but the mechanism ships regardless.** Reading the
/// ratified W as a number is a loud error; the mechanism is drilled against the seed. (The floor is
/// named: W stays `[OPEN — LEGAL]`, DPO-ratified, L-1.)
#[test]
fn cdc_1_10_value_w_is_open_legal_but_the_mechanism_ships() {
    let t = Thresholds::load_canonical().expect("load");
    assert_eq!(t.fail_static.status, "OPEN — LEGAL", "W is the legal flag");
    assert!(
        t.fail_static.ratified_static_max_secs().is_err(),
        "reading the unratified W as a number is a loud error (never a silent default)"
    );
    // but the constraint + the mechanism ship: a FailStatic constructs against the seed.
    let (bound, seed) = bound_from_file();
    FailStatic::<&str, u8>::try_new(30, seed, bound).expect("the mechanism ships regardless of W");
}

/// **CDC 1.10 — the sequence property (EI-01 §4): authenticated → hiccup → serve-stale →
/// window-closes → deny.** Chains the operations across an advancing clock (a real session chains
/// mutations and reads state mid-flight — exactly where the bugs live), proving the `Fresh → Static
/// → Closed` ladder NEVER falls through to open and the exported signals track it.
#[test]
fn cdc_1_10_sequence_authenticated_hiccup_stale_then_denied_at_window_close() {
    let (bound, seed) = bound_from_file(); // seed == 300s; agent-token TTL == 60s
    let clock = TestClock::at(10_000);
    // fresh_ttl 30s, static_max == the seed (the largest the constraint admits).
    let fs: FailStatic<&'static str, &'static str, _> =
        FailStatic::try_new_with_clock(30, seed, bound, clock).expect("valid bound from the file");

    // model an upstream that is UP, then hiccups (the transient Identity hiccup of P-S25). We flip a
    // captured cell to script the upstream; the closure reads it on every call.
    let up = std::cell::Cell::new(true);
    let refresh = || {
        if up.get() {
            Ok("actor=active;grants=coarse")
        } else {
            Err(ServeError("identity transient hiccup".into()))
        }
    };

    // 1) authenticated: a successful read is Fresh and caches the coarse answer.
    assert_eq!(
        fs.get("actor:alice", refresh),
        Answer::Fresh("actor=active;grants=coarse")
    );

    // 2) the upstream hiccups; within fresh_ttl the cached answer is still Fresh (no degradation).
    up.set(false);
    advance(&fs, 30); // age == fresh_ttl
    assert_eq!(
        fs.get("actor:alice", refresh),
        Answer::Fresh("actor=active;grants=coarse"),
        "age == fresh_ttl is still fresh"
    );

    // 3) past fresh_ttl but inside static_max → Static (degraded), serving the LAST KNOWN-GOOD value
    //    (never an escalation of access).
    advance(&fs, 100); // age == 130, inside static_max(300)
    let a = fs.get("actor:alice", refresh);
    assert_eq!(
        a,
        Answer::Static("actor=active;grants=coarse"),
        "stale serves the cached coarse grants"
    );
    assert!(a.is_degraded(), "the stale answer is marked degraded");

    // 4) the window CLOSES: past static_max the answer is Closed (deny is correct) — NEVER open.
    advance(&fs, seed); // age == 130 + 300 > static_max
    assert_eq!(
        fs.get("actor:alice", refresh),
        Answer::Closed,
        "past W → deny, never fail open"
    );

    // the exported contract-1.8 signals tracked the sequence; the staleness age never exceeded the
    // budget (the §8.2 bound the SUB-D4 drill asserts).
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

/// **CDC 1.10 — never fail open: a hiccup with no cached value denies.** The other half of
/// never-fail-open: if there is nothing cached to fall back on, the answer is `Closed`, never a
/// fabricated grant.
#[test]
fn cdc_1_10_cold_hiccup_never_fails_open() {
    let (bound, seed) = bound_from_file();
    let fs: FailStatic<&str, u8> = FailStatic::try_new(30, seed, bound).expect("valid");
    let denied = fs.get("never-seen", || Err(ServeError("hiccup".into())));
    assert_eq!(
        denied,
        Answer::Closed,
        "a cold hiccup denies — never fabricates an open answer"
    );
    assert_eq!(fs.signals().closed, 1);
}

/// Advance the test-clock inside a `FailStatic<T, TestClock>` from this integration test (the clock
/// is owned by the cache; `FailStatic::clock()` hands back a borrow we advance to step across the
/// `fresh_ttl` / `static_max` boundaries deterministically).
fn advance<K: std::hash::Hash + Eq, T: Clone>(fs: &FailStatic<K, T, TestClock>, secs: u64) {
    fs.clock().advance(secs);
}
