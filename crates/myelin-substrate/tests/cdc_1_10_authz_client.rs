//! # CDC 1.10 — `FailStatic<T>` wired against the Identity authz client (P-S25 → P-087, SUB-D4)
//!
//! **Contract-index rows 1.10** (`FailStatic<T>`) **+ 4.10** (zookie reads BYPASS the cache) **+
//! 4.11** (the staleness bound `static_max ≤ revocation-SLA ≥ agent-token-TTL`; W is `[OPEN —
//! LEGAL]`). This is the CDC the P-S25 TESTS field names: the 1.10 contract exercised **against
//! the Identity authz client** — the substrate's [`FailStaticAuthz`] wiring as the CONSUMER, with
//! the Identity authz read modelled as the PROVIDER that HICCUPS.
//!
//! - the **PROVIDER** is the Identity authz read path: a depth-bounded `check` that on a transient
//!   Identity-dependency hiccup becomes unreachable (the source returns `Err`). Here the hiccup is
//!   driven through the **P-S03 dependency-break injector** (the SAME seam the SUB-D4 drill uses),
//!   so the CDC pins the consumer against a really-severed dependency, not a hand-rolled flag.
//! - the **CONSUMER** is the substrate [`FailStaticAuthz`] cache (the M0 [`FailStatic`] mechanism
//!   wired into the authz read path): on a healthy `check` it caches the coarse answer; on a hiccup
//!   it serves the bounded-staleness fallback (`BoundedStale`) or fails CLOSED (`Strong` /
//!   past-budget); a zookie-stamped read BYPASSES it; a revoked subject is denied through it.
//!
//! The provider's promise (a hiccup makes the source unreachable) and the consumer's promise (it
//! wraps the `check` in [`FailStatic`], survives a transient hiccup on the coarse grant, never
//! serves a strong read stale, never escalates a Deny to an Allow, and bounds the staleness ≤ the
//! revocation SLA) are pinned here so a change to either side fails this test in the same CI job
//! (CDC: the two sides cannot drift apart). The CONSUMER-against-the-mechanism-only side (a
//! scripted upstream, no injector) is `cdc_1_10_fail_static.rs` (P-S18); this is the
//! against-the-real-Identity-client side P-S25 owns.
//!
//! Thresholds are read from the canonical file (never hardcoded); the value W is `[OPEN — LEGAL]`,
//! the constraint ships + is enforced regardless (the floor does not wait).

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

/// Build the consumer cache against the canonical thresholds-file bound + a deterministic clock,
/// returning `(cache, revocation_sla_secs, agent_token_ttl_secs)`.
fn cache_from_file(t0: u64) -> (FailStaticAuthz<TestClock>, u64, u64) {
    let t = Thresholds::load_canonical().expect("load canonical thresholds");
    let sla = t.revocation.sla_mins * 60;
    let token_ttl = t.fail_static.agent_token_ttl_secs;
    let fs = FailStaticAuthz::try_new_with_clock(sla, &t.fail_static, TestClock::at(t0))
        .expect("the authz fail-static cache constructs against the thresholds-file bound");
    (fs, sla, token_ttl)
}

/// The Identity authz PROVIDER, driven by the P-S03 injector: `Ok(Allow)` while up, `Err` (the
/// transient hiccup) while `is_broken(Identity, scope)`.
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

/// **CDC 1.10 — the constructor enforces the §8.2 / 4.11 bound read from the thresholds file.** The
/// consumer cache cannot be wired with a window that outlives the revocation SLA or is shorter than
/// the agent-token TTL; the seed value (== the SLA) wires.
#[test]
fn cdc_1_10_constructor_enforces_the_threshold_file_bound() {
    let t = Thresholds::load_canonical().expect("load");
    let sla = t.revocation.sla_mins * 60;
    let mut over = t.fail_static.clone();
    over.static_max_default_secs = sla + 1; // > SLA
    assert!(
        FailStaticAuthz::try_new_with_clock(sla, &over, TestClock::at(0)).is_err(),
        "a static_max over the revocation SLA must reject (4.11 / §8.2)"
    );
    // the seed (== SLA, ≥ token TTL) wires; W sits ≤ SLA ≥ token TTL.
    let (fs, sla, token_ttl) = cache_from_file(0);
    assert!(fs.static_max() <= sla, "W ≤ revocation SLA");
    assert!(
        fs.static_max() >= token_ttl,
        "W ≥ agent-token TTL (the window contains the token)"
    );
}

/// **CDC 1.10 — the sequence property (EI-01 §4) against the Identity authz client:** authenticate
/// → hiccup (injector) → serve-stale → window-closes → deny. The consumer cache survives the
/// transient hiccup on the coarse grant and NEVER falls through to open.
#[test]
fn cdc_1_10_sequence_authenticated_hiccup_stale_then_denied_at_window_close() {
    let (fs, sla, _token_ttl) = cache_from_file(10_000);
    let breaker = DependencyBreaker::new();
    let scope = Scope::Tenant(TenantId("acme".into()));
    let src = provider(&breaker, &scope);
    let key = "acme|eu|alice|view@repo:core";

    // 1) authenticated: the provider is UP → a fresh authoritative Allow is cached.
    let a = fs.serve(key, &bounded_stale(), false, &src);
    assert_eq!(a.served, AuthzServed::Fresh);
    assert!(a.is_allow());

    // 2) the Identity dependency BREAKS (the injector severs it); within fresh_ttl still fresh.
    breaker.break_dependency(Dependency::Identity, scope.clone());
    fs.clock().advance(30); // age == fresh_ttl
    let f = fs.serve(key, &bounded_stale(), false, &src);
    assert_eq!(
        f.served,
        AuthzServed::Fresh,
        "age == fresh_ttl is still fresh"
    );

    // 3) past fresh_ttl, inside static_max → Static (degraded), serving the last known-good grant.
    fs.clock().advance(100); // age == 130, inside static_max
    let s = fs.serve(key, &bounded_stale(), false, &src);
    assert_eq!(
        s.served,
        AuthzServed::Static,
        "the consumer survives the hiccup on the coarse grant"
    );
    assert!(s.is_allow() && s.is_degraded());

    // 4) the window CLOSES: past static_max → Closed (deny) — NEVER open.
    fs.clock().advance(fs.static_max()); // age now > static_max
    let c = fs.serve(key, &bounded_stale(), false, &src);
    assert_eq!(
        c.served,
        AuthzServed::Closed,
        "past W → deny, never fail open"
    );
    assert!(c.is_deny());

    // the exported signals tracked the sequence; staleness never exceeded the budget ≤ the SLA.
    let sig = fs.signals();
    assert_eq!(sig.fresh, 2, "the live read + the within-ttl cached read");
    assert_eq!(sig.stale, 1, "one degraded answer");
    assert_eq!(sig.closed, 1, "one denied answer at window close");
    assert!(
        (sig.last_staleness_secs) <= sla,
        "staleness ≤ the revocation SLA"
    );
}

/// **CDC 1.10 — the zookie bypass (4.10): a Strong read bypasses the consumer cache and fails
/// CLOSED on a hiccup.** Even with a stale grant cached, a zookie-stamped read does not serve it.
#[test]
fn cdc_1_10_zookie_bypass_fails_closed_on_hiccup() {
    let (fs, _sla, _ttl) = cache_from_file(0);
    let breaker = DependencyBreaker::new();
    let scope = Scope::Tenant(TenantId("acme".into()));
    let src = provider(&breaker, &scope);
    let key = "acme|eu|alice|view@repo:core";

    // warm a stale-eligible grant.
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

/// **CDC 1.10 — never escalate + never fail open:** a `Deny` provider answer is replayed stale as a
/// Deny (never an Allow), and a cold hiccup (no cached grant) denies.
#[test]
fn cdc_1_10_never_escalates_and_never_fails_open() {
    let (fs, _sla, _ttl) = cache_from_file(0);

    // a cold hiccup with no cached grant → Closed (never a fabricated open answer).
    let cold = fs.serve("cold", &bounded_stale(), false, || {
        Err(ServeError("hiccup".into()))
    });
    assert_eq!(cold.served, AuthzServed::Closed);
    assert!(
        cold.is_deny(),
        "a cold hiccup denies — never fabricates an open answer"
    );

    // a Deny provider answer is cached + replayed stale as a Deny (never escalated to Allow).
    let _ = fs.serve("k", &bounded_stale(), false, || Ok(Decision::Deny));
    fs.clock().advance(31);
    let stale = fs.serve("k", &bounded_stale(), false, || {
        Err(ServeError("hiccup".into()))
    });
    assert_eq!(stale.served, AuthzServed::Static);
    assert!(
        stale.is_deny(),
        "the stale fallback replays the Deny — never escalates to Allow"
    );
}

/// **CDC 1.10 — the value W is `[OPEN — LEGAL]` but the mechanism ships.** Reading the ratified W
/// as a number is a loud error; the consumer cache wires against the engineering seed regardless.
#[test]
fn cdc_1_10_value_w_is_open_legal_but_the_wiring_ships() {
    let t = Thresholds::load_canonical().expect("load");
    assert_eq!(t.fail_static.status, "OPEN — LEGAL", "W is the legal flag");
    assert!(
        t.fail_static.ratified_static_max_secs().is_err(),
        "reading the unratified W as a number is a loud error (never a silent default)"
    );
    // but the constraint + the wiring ship: the consumer cache constructs against the seed.
    let (fs, sla, _ttl) = cache_from_file(0);
    assert!(fs.static_max() <= sla, "the wiring ships regardless of W");
}
