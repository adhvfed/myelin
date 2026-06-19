//! # SUB-D4 — fail-static proven against a real Identity hiccup (P-S25 → global P-087)
//!
//! **Drill catalogue:** `planning/05-refined-shared-systems-architecture/testing-strategy/
//! 01-whole-system-e2e-and-drill-catalogue.md` §4.2 row **SUB-D4**: *inject a transient Identity
//! hiccup → already-authenticated traffic survives on the coarse cache within W; a revoked actor
//! is denied once the window closes; an agent token expires inside the window; a zookie-stamped
//! read bypasses the cache.* Survival signals: the fail-static fresh/stale/closed ratio read
//! green, staleness never exceeds `static_max ≤ revocation-SLA`, `revoked_after_window_denied ==
//! true`. Surface: CI. This is the substrate mirror of Identity's ID-D2 (P-073) — the same
//! mechanism at the substrate seam.
//!
//! This is the **dated green artifact** the P-S25 GATE/DRILLS names. It is the EI-01 §3 / §4 drill
//! shape — *inject one fault, drive a CHAINED sequence, read one telemetry assertion green*:
//!   - **inject** — the **P-S03 dependency-break injector** ([`DependencyBreaker::break_dependency`])
//!     severs `Dependency::Identity` for the drill's tenant. The fail-static authz read path's
//!     authoritative source consults `is_broken(Identity, scope)`, so a really-severed dependency
//!     makes the source `Err` (the hiccup is driven through the injector, not a hand-passed flag).
//!   - **sequence (EI-01 §4)** — authenticate → hiccup → serve-stale → revoke → window-closes →
//!     deny, threaded across an advancing `TestClock` (a real session chains mutations and reads
//!     state mid-flight — exactly where the bugs live).
//!   - **assert** — through the **P-S04 telemetry-assertion library** (the contract-1.8 survival
//!     signal set): the fail-static answer ratio + the staleness age (≤ `static_max` ≤ the
//!     revocation SLA), and the `revoked_after_window_denied` zero-leak signal. Loud on red
//!     (`expect_green` panics with the failing signal; the threshold is NEVER weakened to pass).
//!
//! **Thresholds are read from the canonical thresholds file, never hardcoded** (EI-01 §3): the
//! revocation SLA N (`revocation.sla_mins`) bounds the staleness age, and the `[fail_static]` row
//! supplies the `static_max ≤ revocation-SLA ≥ agent-token-TTL` constraint the constructor
//! enforces. The value W itself is `[OPEN — LEGAL]` (L-1) — the floor does not wait; the mechanism
//! is PROVEN here regardless of the final number.
//!
//! `myelin-harness` is a DEV-dependency only — it never enters the substrate production DAG.

use myelin_harness::{
    Dependency, DependencyBreaker, DrillRegistry, DrillScenario, Label, Predicate, Scope,
    SignalName, SignalSource,
};
use myelin_identity::{Consistency, ConsistencyMode, Decision, Zookie};
use myelin_substrate::{
    AuthzServed, FailStaticAuthz, ServeError, TestClock, Thresholds,
};
use myelin_tenancy::TenantId;

fn bounded_stale() -> Consistency {
    Consistency { at_least: Zookie(String::new()), mode: ConsistencyMode::BoundedStale }
}
fn strong(z: &str) -> Consistency {
    Consistency { at_least: Zookie(z.into()), mode: ConsistencyMode::Strong }
}

/// The authoritative authz source, driven by the **P-S03 injector**: it returns `Ok(Allow)` while
/// Identity is up, and `Err` (the transient hiccup) while `is_broken(Identity, scope)`. This is
/// the wiring the architecture names — the fail-static read path's source consults the SAME severed
/// dependency truth the injector exposes (a really-severed dependency, not a fake flag).
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

/// **SUB-D4 — the chained sequence drill** (the dated green artifact). Returns the harness
/// `SignalSource` carrying the survival signals so the registry scenario and the direct test both
/// read the same green/red verdicts off it.
fn run_sub_d4_sequence() -> (SignalSource, FailStaticAuthz<TestClock>, i64) {
    // Thresholds from the canonical file (never hardcoded). N = the revocation SLA.
    let thresholds = Thresholds::load_canonical().expect("load canonical thresholds");
    let sla_secs: u64 = thresholds.revocation.sla_mins * 60;
    let fs_threshold = thresholds.fail_static.clone();
    // The agent-token TTL lower bound — the SUB-D4 leg "an agent token expires inside the window"
    // means: the staleness window CONTAINS the short-lived agent token, so a token whose life ==
    // its run expires WHILE still inside W (the constructor enforces static_max ≥ this).
    let agent_token_ttl: u64 = fs_threshold.agent_token_ttl_secs;

    // The fail-static authz cache wired against a deterministic TestClock so the sequence advances
    // across the fresh_ttl / static_max boundaries exactly. The bound is read from the file; the
    // constructor enforces static_max ≤ revocation SLA ≥ agent-token-TTL (W is [OPEN — LEGAL]).
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
        "the window CONTAINS the short-lived agent token (4.11): W={}s ≥ agent-token-TTL={}s — an \
         agent token whose life == its run expires INSIDE the window",
        fs.static_max(),
        agent_token_ttl
    );

    // The P-S03 dependency-break injector + the tenant scope it severs Identity for.
    let breaker = DependencyBreaker::new();
    let scope = Scope::Tenant(TenantId("acme".into()));
    let key = "acme|eu-west|p:alice|view@repo:core";

    // ── STEP 1 (authenticate, healthy): a default-consistency read is served FRESH + cached. ──
    let src = injector_source(&breaker, &scope);
    let healthy = fs.serve(key, &bounded_stale(), false, &src);
    assert!(matches!(healthy.served, AuthzServed::Fresh), "healthy read is fresh + caches");
    assert!(healthy.is_allow(), "alice's grant is allowed and the coarse answer is cached");

    // ── STEP 2 (the Id dependency BREAKS — the hiccup): a default-consistency read survives STATIC. ──
    breaker.break_dependency(Dependency::Identity, scope.clone());
    assert!(
        breaker.is_broken(&Dependency::Identity, &scope),
        "the injected hiccup must be in effect for the drill to be meaningful"
    );
    // advance past fresh_ttl (within static_max) → the cached grant is the degraded-stale rung.
    fs.clock().advance(31);
    let survived = fs.serve(key, &bounded_stale(), false, &src);
    assert!(
        matches!(survived.served, AuthzServed::Static),
        "during the Id hiccup the default-consistency read survives on the coarse fail-static cache"
    );
    assert!(survived.is_allow(), "authenticated traffic SURVIVES the hiccup (still Allow)");

    // ── STEP 3 (the zookie-bypass): a Strong read during the SAME hiccup fails CLOSED, not stale. ──
    let strong_during_hiccup = fs.serve(key, &strong("z-strong"), false, &src);
    assert!(
        matches!(strong_during_hiccup.served, AuthzServed::BypassClosed),
        "a zookie-stamped read BYPASSES the cache (the new-enemy guard)"
    );
    assert!(
        strong_during_hiccup.is_deny(),
        "a strong read fails CLOSED during the hiccup (never served stale)"
    );

    // ── STEP 4 (REVOKE, then the WINDOW CLOSES → DENY): a revoked actor is denied once W closes. ──
    // The subject is revoked mid-hiccup. A BATCH of default-consistency reads must yield 0 allows
    // (the revoked-actor-denied-through-the-cache property). We measure `revoked_after_window`: the
    // count of reads that still allowed AFTER the revoke — must be 0.
    let mut allowed_after_revoke: i64 = 0;
    let mut revoked_after_window_denied = true;
    for i in 0..8 {
        // Past STEP 2 the clock keeps advancing; on the later reads we cross static_max so the
        // window CLOSES — but the revoke must deny BEFORE the cache is even consulted regardless.
        if i == 4 {
            // close the window (age now > static_max) to exercise both the revoke gate AND the
            // window-close fail-closed on the remaining reads.
            fs.clock().advance(fs.static_max() + 1);
        }
        let d = fs.serve(key, &bounded_stale(), /* subject_revoked */ true, &src);
        if d.is_allow() {
            allowed_after_revoke += 1;
        } else {
            // before the window closes the deny is `Revoked`; after it could be `Revoked` (the
            // revoke gate fires first) — either way it must be a DENY.
            if !matches!(d.served, AuthzServed::Revoked) && !matches!(d.served, AuthzServed::Closed) {
                revoked_after_window_denied = false;
            }
        }
    }
    assert!(revoked_after_window_denied, "every post-revoke read denied (revoked OR window-closed)");

    // ── STEP 5 (HEAL): restore the dependency so a re-run starts clean (reversibility). ──
    breaker.restore_dependency(Dependency::Identity, scope.clone());
    assert!(!breaker.is_broken(&Dependency::Identity, &scope), "the injector restored to working");

    // ── Record the survival signals into the harness telemetry-assertion library (P-S04). ──
    let sig = fs.signals();
    let mut signals = SignalSource::new();
    // (1) the fail-static fresh/stale/closed ratio is observable + the survival rung was served:
    //     at least one STALE (degraded) answer (authenticated traffic survived the hiccup).
    signals.set_labelled(
        SignalName::FailStaticRatio,
        vec![Label::new("answer_class", "stale")],
        sig.stale as i64,
    );
    // (2) the staleness age never exceeds static_max (≤ the revocation SLA).
    signals.set_scalar(SignalName::FailStaticStalenessSecs, sig.last_staleness_secs as i64);
    // (3) the revoked-after-cache leak count — must be 0 (0 successful authz for a revoked actor).
    //     Reuse CrossTenantCount as the zero-leak assertion channel (as ID-D2 / the harness do).
    signals.set_scalar(SignalName::CrossTenantCount, allowed_after_revoke);

    (signals, fs, allowed_after_revoke)
}

/// **SUB-D4 — the dated green artifact.** Run the chained sequence and assert the survival signals
/// green through the harness telemetry-assertion library (loud on red).
#[test]
fn sub_d4_fail_static_survives_hiccup_and_denies_revoked() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let sla_secs = (thresholds.revocation.sla_mins * 60) as i64;

    let (signals, fs, allowed_after_revoke) = run_sub_d4_sequence();
    let sig = fs.signals();

    // (1) authenticated traffic survived on the STATIC (degraded) fail-static rung (≥ 1 stale).
    signals
        .assert_labelled(
            SignalName::FailStaticRatio,
            vec![Label::new("answer_class", "stale")],
            Predicate::Gte(1),
        )
        .expect_green();

    // (2) the staleness age is bounded ≤ static_max ≤ the revocation SLA (the §8.2 bound).
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

    // (3) revoked_after_window_denied == true: 0 successful authz after the cache for the revoked
    //     subject (the SUB-D4 quantified zero).
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        allowed_after_revoke, 0,
        "0 successful authz after the cache for a revoked subject once the window closes (SUB-D4)"
    );

    // The fresh/stale/closed answer ratio is OBSERVABLE (observability is part of the pass, EI-01 §3).
    assert!(sig.fresh >= 1, "≥ 1 fresh answer (the live authenticate)");
    assert!(sig.stale >= 1, "≥ 1 stale answer (the hiccup survival rung)");

    println!(
        "[P-087 DRILL GREEN 2026-06-19] SUB-D4 Id-hiccup / fail-static: tenant=acme subject=p:alice \
         object=repo:core → authenticated traffic SURVIVED on the coarse fail-static cache \
         (fresh={}, stale={}, closed={}, staleness_age={}s ≤ static_max={}s ≤ revocation_SLA={}s); \
         a Strong/zookie read BYPASSED the cache and failed closed (new-enemy guard); the window \
         CONTAINS the agent-token TTL (an agent token expires inside W); a revoked subject got \
         allowed_after_revoke=0 across an 8-read batch spanning the window-close (revoked actor \
         denied once the window closes) — thresholds read from the canonical file, never hardcoded; \
         W is [OPEN — LEGAL] (L-1), the static_max ≤ SLA ≥ token-TTL constraint enforced by the \
         constructor regardless",
        sig.fresh, sig.stale, sig.closed, sig.last_staleness_secs, fs.static_max(), sla_secs
    );
}

/// **SUB-D4 joins the every-incident-adds-a-drill registry (P-S04) — it re-runs forever.** The
/// drill registers a [`DrillScenario`] whose closure re-runs the chained sequence and reads the
/// survival signal green; the registry re-runs it on every change (a regression re-reds it loudly).
#[test]
fn sub_d4_registers_in_the_drill_registry_and_reruns_green() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new("sub-d4-fail-static-vs-identity-hiccup", |_ctx| {
        // Re-run the full chained sequence (it owns its own injector + cache so it is reproducible),
        // then assert the survival signal off the harness telemetry library.
        let (signals, _fs, _allowed) = run_sub_d4_sequence();
        signals.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
    }));
    assert_eq!(registry.len(), 1);

    // run twice to prove "re-runs forever".
    let first = registry.run_all();
    let second = registry.run_all();
    assert!(first[0].is_pass(), "SUB-D4 reads green: {first:?}");
    assert!(second[0].is_pass(), "SUB-D4 re-runs green: {second:?}");
    assert!(registry.all_green(), "the SUB-D4 drill suite is green");

    // the dated green-artifact row (observability is part of the pass).
    let row = first[0].artifact_row("2026-06-19");
    assert!(row.contains("sub-d4-fail-static-vs-identity-hiccup"), "the dated artifact names the drill");
    println!("{row}");
}
