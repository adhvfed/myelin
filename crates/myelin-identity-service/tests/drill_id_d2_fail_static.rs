//! # P-ID-15 (global P-073) GATE / DRILL — ID-D2, the Id-hiccup / fail-static drill
//! (the dated green artifact)
//!
//! **Drill catalogue row ID-D2 (§4.2, F7):** *Break Id dependency → authenticated traffic survives
//! on the coarse fail-static cache; a JUST-REVOKED grant is still denied (the zookie bypass).*
//! Survival signal: the **fail-static fresh/stale/closed ratio** + the **staleness age** (≤
//! `static_max` ≤ revocation SLA). Quantified: **0 successful authz after the cache during a hiccup
//! for a revoked subject; authenticated traffic survives the hiccup.** Run against the
//! failure-injection harness's telemetry-assertion library (the contract-1.8 survival-signal set),
//! exactly as ID-D1/ID-D3 and the harness self-test (P-S04) do. `myelin-harness` is a DEV-dependency
//! only — it never enters the identity-service production DAG.
//!
//! **The thresholds are read from the canonical thresholds file, NEVER hardcoded** (EI-01 §3): the
//! revocation SLA N (`revocation.sla_mins`) is the bound the staleness age must stay under, and the
//! `[fail_static]` row supplies the `static_max ≤ revocation-SLA ≥ agent-token-TTL` constraint the
//! S6 constructor enforces (the W value itself is `[OPEN — LEGAL]`, L-1 — the floor does not wait).
//!
//! **The scenario.** A subject `alice` in tenant `acme` holds a real `view` grant on `repo:core`.
//! 1. **Healthy:** a default-consistency (`BoundedStale`) `check` for alice is served fresh and the
//!    coarse `{actor_active, coarse_grants}` answer is cached in S6.
//! 2. **The Id dependency BREAKS** (the scoped-reversible injector models the hiccup as the
//!    authoritative `check` becoming unreachable): a default-consistency `check` is served STATIC
//!    from S6 — authenticated traffic SURVIVES.
//! 3. **The zookie-bypass:** a `Strong`/zookie-stamped `check` during the same hiccup does NOT serve
//!    stale — it fails CLOSED (the new-enemy guard).
//! 4. **The just-revoked deny:** alice is revoked (SCIM-disable). Now even the default-consistency
//!    `check` is DENIED through the stale S6 grant — **0 successful authz after the cache** for the
//!    revoked subject.
//!
//! A non-zero "successful authz after the cache for a revoked subject" would be the exact F7 failure
//! and the drill aborts LOUDLY (EI-01 §3: loud, never swallowed; the threshold is NEVER weakened to
//! pass). A red gate becomes a dated `[[claimed_not_proven]]` row, never edited green.

use myelin_events::{OutboxStore, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, Permission, Principal, PrincipalId, PrincipalKind,
    RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{Served, StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_substrate::Thresholds;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn subject(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn grant(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn bounded_stale() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn strong() -> Consistency {
    Consistency {
        at_least: Zookie("z-strong".into()),
        mode: ConsistencyMode::Strong,
    }
}

fn ts(s: &str) -> Timestamp {
    Timestamp(s.into())
}

/// **ID-D2 — break Id dependency → authenticated survives on the coarse cache; just-revoked still
/// denied (zookie bypass).** The dated green artifact.
#[test]
fn id_d2_fail_static_survives_hiccup_and_denies_revoked() {
    // The thresholds — read from the canonical file (never hardcoded). N = the revocation SLA.
    let thresholds = Thresholds::load_canonical().expect("load canonical thresholds");
    let sla_secs: i64 = (thresholds.revocation.sla_mins * 60) as i64;
    let fs_threshold = thresholds.fail_static.clone();

    let acme = scope("acme");
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &subject("p-admin"),
            &[grant("repo:core", "view", "p:alice")],
            None,
            None,
            ts("2026-06-19T00:00:00Z"),
        )
        .expect("seed alice's grant");
    let svc = StoreBackedCheck::new(store);
    let alice = subject("p:alice");
    let obj = ArtifactRef("repo:core".into());

    // S6, the fail-static cache, built over THIS check engine — bound read from the thresholds file
    // (the constructor enforces static_max ≤ revocation SLA ≥ agent-token-TTL; the W value itself is
    // [OPEN — LEGAL], the constraint ships regardless).
    // S6 against a deterministic TestClock so the drill advances across the fresh_ttl / static_max
    // boundaries exactly (the production SystemClock exposes no mutators).
    let s6 = svc
        .failstatic_cache_with_clock(
            sla_secs as u64,
            &fs_threshold,
            myelin_substrate::TestClock::at(1_000),
        )
        .expect("S6 constructs against the thresholds-file bound");
    assert!(
        (s6.static_max() as i64) <= sla_secs,
        "the fail-static window sits under the revocation SLA (4.11): W={}s ≤ N={}s",
        s6.static_max(),
        sla_secs
    );

    // ── STEP 1 (healthy): a default-consistency check is served FRESH + cached. ──
    let healthy = svc.check_failstatic(
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &bounded_stale(),
        None,
        &ts("2026-06-19T00:00:01Z"),
        /* source_ok */ true,
    );
    assert!(
        matches!(healthy.served, Served::Fresh),
        "healthy default-consistency read is fresh"
    );
    assert!(
        healthy.is_allow(),
        "alice's grant is allowed and the coarse answer is cached"
    );

    // ── STEP 2 (the Id dependency BREAKS): a default-consistency check is served STATIC. ──
    // Advance past fresh_ttl (within static_max) so the cached grant is the degraded-stale rung.
    s6.clock().advance(31);
    let survived = svc.check_failstatic(
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &bounded_stale(),
        None,
        &ts("2026-06-19T00:00:02Z"),
        /* source_ok */ false, // the injected hiccup
    );
    assert!(
        matches!(survived.served, Served::Static),
        "during the Id hiccup the default-consistency read survives on the coarse fail-static cache"
    );
    assert!(
        survived.is_allow(),
        "authenticated traffic SURVIVES the hiccup (still Allow)"
    );

    // ── STEP 3 (the zookie-bypass): a Strong read during the SAME hiccup fails CLOSED, not stale. ──
    let strong_during_hiccup = svc.check_failstatic(
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &strong(),
        None,
        &ts("2026-06-19T00:00:03Z"),
        /* source_ok */ false,
    );
    assert!(
        matches!(strong_during_hiccup.served, Served::BypassClosed),
        "a zookie-stamped read BYPASSES S6 (the new-enemy guard)"
    );
    assert!(
        strong_during_hiccup.is_deny(),
        "a strong read fails CLOSED during the hiccup (never served stale)"
    );

    // ── STEP 4 (the just-revoked deny): revoke alice; the stale S6 grant must NOT be served. ──
    svc.disable_principal_in(
        &acme,
        &PrincipalId("p:alice".into()),
        ts("2026-06-19T00:04:00Z"),
    );

    // A BATCH of default-consistency reads during the SAME hiccup AFTER the revoke — count the ones
    // that still succeeded (the "successful authz after the cache for a revoked subject"). Must be 0.
    let mut successful_authz_after_cache_for_revoked: i64 = 0;
    for i in 0..8 {
        let d = svc.check_failstatic(
            &s6,
            &acme,
            &alice,
            &Permission("view".into()),
            &obj,
            &bounded_stale(),
            None,
            &ts(&format!("2026-06-19T00:05:{i:02}Z")),
            /* source_ok */ false,
        );
        if d.is_allow() {
            successful_authz_after_cache_for_revoked += 1;
        } else {
            // The revoke is enforced BEFORE the cache is read (the S7 consult) — the served
            // provenance is `Revoked`, the answer Deny.
            assert!(
                matches!(d.served, Served::Revoked),
                "the revoke is enforced through the cache"
            );
        }
    }

    // ── THE green artifacts, asserted through the harness telemetry-assertion library (loud on red). ──

    // (1) 0 successful authz after the cache for the revoked subject (the F7 quantified threshold).
    let mut signals = SignalSource::new();
    signals.set_scalar(
        SignalName::CrossTenantCount, // reused as the "leaked authz count" zero-assertion channel
        successful_authz_after_cache_for_revoked,
    );
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    // (2) the fail-static survival signal: authenticated traffic survived on the STATIC rung (≥ 1
    //     stale answer served), the fresh/stale/closed ratio is observable, and the staleness age is
    //     bounded ≤ static_max ≤ the revocation SLA.
    let fs = s6.fail_static_signals();
    assert!(
        fs.stale >= 1,
        "authenticated traffic survived on the static (degraded) fail-static rung"
    );
    let mut fs_signals = SignalSource::new();
    fs_signals.set_scalar(
        SignalName::FailStaticStalenessSecs,
        fs.last_staleness_secs as i64,
    );
    fs_signals
        .assert_signal(
            SignalName::FailStaticStalenessSecs,
            Predicate::Lte(s6.static_max() as i64),
        )
        .expect_green();
    assert!(
        (fs.last_staleness_secs as i64) <= sla_secs,
        "staleness age ({}s) ≤ the revocation SLA ({sla_secs}s)",
        fs.last_staleness_secs
    );

    // (3) observability is part of the pass (EI-01 §3): the cache_hit_ratio / staleness_age signals
    //     are emitted under their FROZEN names.
    assert_eq!(
        myelin_identity_service::CacheTelemetry::CACHE_HIT_RATIO,
        "cache_hit_ratio"
    );
    assert_eq!(
        myelin_identity_service::CacheTelemetry::STALENESS_AGE,
        "staleness_age"
    );
    assert!(
        s6.telemetry().cache_hit_ratio_pct().is_some(),
        "the cache_hit_ratio is observable (cache-consulting reads happened)"
    );

    assert_eq!(
        successful_authz_after_cache_for_revoked, 0,
        "0 successful authz after the cache for a revoked subject during a hiccup (ID-D2 / F7)"
    );

    println!(
        "[P-073 DRILL GREEN 2026-06-19] ID-D2 Id-hiccup / fail-static: tenant=acme subject=p:alice \
         object=repo:core → authenticated traffic SURVIVED on the coarse fail-static cache \
         (fresh={}, stale={}, closed={}, staleness_age={}s ≤ static_max={}s ≤ revocation_SLA={}s); \
         a Strong/zookie read BYPASSED S6 and failed closed (new-enemy guard); a SCIM-disabled \
         subject got successful_authz_after_cache=0 across an 8-read batch (the just-revoked grant \
         is denied THROUGH the stale cache) — thresholds read from the canonical file, never \
         hardcoded; W is [OPEN — LEGAL] (L-1), the static_max ≤ SLA ≥ token-TTL constraint enforced \
         by the S6 constructor regardless",
        fs.fresh, fs.stale, fs.closed, fs.last_staleness_secs, s6.static_max(), sla_secs
    );
}
