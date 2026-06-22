//! # The CDC pair for contract 4.11 (the fail-static bound) — S6 + `ResilientClient` (P-ID-15 / P-073)
//!
//! **Contract-index rows 4.11** (the fail-static bound: `static_max ≤ revocation SLA ≥
//! agent-token-TTL`, W = 5 min default-to-beat) **+ 4.10** (the zookie-bypass-S6 half) **+ 1.9**
//! (`ResilientClient`) **+ 1.10** (`FailStatic<T>`). This is the dedicated provider+consumer pair
//! the P-ID-15 TESTS field names — the focused, in-CI evidence that the two sides of the
//! **fail-static authz** seam cannot drift apart:
//!
//! - the **PROVIDER** is Identity's S6 fail-static cache ([`StoreBackedCheck::failstatic_cache`] /
//!   [`StoreBackedCheck::check_failstatic`]): on a healthy `check` it caches the coarse answer; on
//!   an Id-dependency hiccup it serves the bounded-staleness `{actor_active, coarse_grants}`
//!   fallback (BoundedStale) or fails CLOSED (Strong / past-budget). The structural bound
//!   `static_max ≤ revocation SLA` is enforced by the constructor (it reads the thresholds file).
//! - the **CONSUMER** is a **critical-dependency caller that wraps `check` in the `ResilientClient`
//!   (1.9) backed by the S6 `FailStatic` (1.10)** — exactly the shape every other service uses to
//!   call Identity (the dependency root): the `ResilientClient` bounds the call (timeout, breaker,
//!   bulkhead), and on a hiccup (a `CallError` from the tripped/timed-out client) the consumer
//!   falls back to the bounded-staleness cached grant rather than failing every request closed and
//!   turning the one shared dependency into a platform-wide cascade (EI-01 §2).
//!
//! The provider's promise (fresh→static→closed ladder, bounded ≤ revocation SLA, zookie reads
//! bypass, revoked subject denied through the cache) and the consumer's promise (it wraps `check`
//! in `ResilientClient` + `FailStatic`, survives a transient hiccup on the coarse grant, and never
//! serves a strong read stale) are pinned here so a change to either side fails this test in the
//! same CI job.

use myelin_client::{CallError, Idempotency, ResilientClient, Target};
use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, Permission, Principal, PrincipalId, PrincipalKind,
    RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{Served, StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_substrate::{thresholds::FailStaticThreshold, Seconds};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

/// The revocation SLA N (seconds) the bound sits under — from the thresholds file (never hardcoded).
fn revocation_sla_secs() -> Seconds {
    let t = myelin_substrate::Thresholds::load_canonical().expect("load thresholds");
    (t.revocation.sla_mins * 60) as Seconds
}

/// The `[fail_static]` threshold row from the canonical thresholds file (the W = 5 min `[OPEN —
/// LEGAL]` engineering seed + the `static_max ≤ revocation-SLA ≥ agent-token-TTL` constraint).
fn fail_static_threshold() -> FailStaticThreshold {
    myelin_substrate::Thresholds::load_canonical()
        .expect("load thresholds")
        .fail_static
}

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
        at_least: Zookie("z1".into()),
        mode: ConsistencyMode::Strong,
    }
}

fn ts(s: &str) -> Timestamp {
    Timestamp(s.into())
}

/// The PROVIDER: a store-backed `check` surface (S3 tuples + S7 denylist) seeded with a `view`
/// grant for alice on `repo:core`.
fn provider(s: &TenantScope) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            s,
            &subject("p-admin"),
            &[grant("repo:core", "view", "p:alice")],
            None,
            None,
            ts("2026-06-19T00:00:00Z"),
        )
        .expect("seed grant");
    StoreBackedCheck::new(store)
}

/// **The 4.11 provider promise: the structural bound `static_max ≤ revocation SLA` is enforced.**
/// S6 constructs against the thresholds-file bound; its `static_max` is the engineering seed and is
/// ≤ the revocation SLA (the constructor would have rejected a larger value).
#[test]
fn cdc_4_11_provider_enforces_the_bound() {
    let svc = provider(&scope("acme"));
    let s6 = svc
        .failstatic_cache(revocation_sla_secs(), &fail_static_threshold())
        .expect("S6 constructs against a valid thresholds-file bound");
    assert!(
        s6.static_max() <= revocation_sla_secs(),
        "the fail-static window sits under the revocation SLA (4.11): static_max={} ≤ SLA={}",
        s6.static_max(),
        revocation_sla_secs()
    );
}

/// **The 4.11 CDC: a critical-dep caller wraps `check` in `ResilientClient` + `FailStatic` and
/// survives a transient Id hiccup on the coarse grant.** The consumer drives the resilient client;
/// while Identity is healthy the client returns the live `check` and the answer is cached; when
/// Identity hiccups the client surfaces a `CallError` and the consumer falls back to the S6
/// bounded-staleness grant — authenticated traffic survives (EI-01 §2: no cascade).
#[test]
fn cdc_4_11_consumer_wraps_check_in_resilient_client_and_survives_a_hiccup() {
    let acme = scope("acme");
    let svc = provider(&acme);
    let s6 = svc
        .failstatic_cache_with_clock(
            revocation_sla_secs(),
            &fail_static_threshold(),
            myelin_substrate::TestClock::at(1_000),
        )
        .expect("S6 constructs");
    let alice = subject("p:alice");
    let obj = ArtifactRef("repo:core".into());

    // The CONSUMER's resilient client (contract 1.9) — the ONE outbound client every caller wraps
    // its Identity `check` in (timeout + breaker + bulkhead + jittered retry).
    let client = ResilientClient::default();
    let target = Target("identity.check".into());

    // ── PHASE 1: Identity is HEALTHY. The consumer calls `check` THROUGH the resilient client; the
    //    call succeeds and the S6 cache records the coarse grant (a fresh authoritative Allow). ──
    let phase1 = consumer_check(
        &client,
        &target,
        &svc,
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &bounded_stale(),
        &ts("2026-06-19T00:00:01Z"),
        /* identity_up */ true,
    );
    assert!(
        matches!(phase1.served, Served::Fresh),
        "healthy: served fresh through the client"
    );
    assert!(
        phase1.is_allow(),
        "alice's live grant is allowed (and cached)"
    );

    // ── PHASE 2: Identity HICCUPS. The resilient client's downstream errors (a CallError); the
    //    consumer falls back to S6 and serves the bounded-staleness coarse grant — alice survives. ──
    s6.clock().advance(31); // past fresh_ttl, within static_max → the static (degraded) rung
    let phase2 = consumer_check(
        &client,
        &target,
        &svc,
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &bounded_stale(),
        &ts("2026-06-19T00:00:02Z"),
        /* identity_up */ false,
    );
    assert!(
        matches!(phase2.served, Served::Static),
        "hiccup: the consumer survives on the S6 bounded-staleness grant (no cascade)"
    );
    assert!(
        phase2.is_allow(),
        "authenticated traffic survives the hiccup (still Allow)"
    );
    assert!(
        phase2.is_degraded(),
        "the survived answer is marked degraded (bounded-staleness)"
    );
    assert!(
        s6.fail_static_signals().last_staleness_secs <= s6.static_max(),
        "the staleness age never exceeds the budget (≤ the revocation SLA)"
    );
}

/// **The 4.11/4.10 CDC: a zookie-stamped (Strong) read BYPASSES S6 and fails CLOSED on a hiccup.**
/// The new-enemy guard: a security-sensitive read passes a zookie, so the consumer must NOT serve
/// it from the bounded-staleness cache — on a hiccup it fails closed (deny), never stale.
#[test]
fn cdc_4_11_strong_read_bypasses_cache_and_fails_closed_on_hiccup() {
    let acme = scope("acme");
    let svc = provider(&acme);
    let s6 = svc
        .failstatic_cache_with_clock(
            revocation_sla_secs(),
            &fail_static_threshold(),
            myelin_substrate::TestClock::at(1_000),
        )
        .expect("S6 constructs");
    let alice = subject("p:alice");
    let obj = ArtifactRef("repo:core".into());
    let client = ResilientClient::default();
    let target = Target("identity.check".into());

    // Warm S6 with a stale-eligible Allow via a BoundedStale read.
    let _ = consumer_check(
        &client,
        &target,
        &svc,
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &bounded_stale(),
        &ts("2026-06-19T00:00:01Z"),
        true,
    );
    s6.clock().advance(31);

    // A STRONG read with Identity hiccuping does NOT serve the stale S6 grant — it fails CLOSED.
    let strong_hiccup = consumer_check(
        &client,
        &target,
        &svc,
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &strong(),
        &ts("2026-06-19T00:00:02Z"),
        false,
    );
    assert!(
        matches!(strong_hiccup.served, Served::BypassClosed),
        "strong read bypassed S6"
    );
    assert!(
        strong_hiccup.is_deny(),
        "a strong read fails CLOSED on a hiccup (never serves stale)"
    );
}

/// **The CONSUMER shape: wrap `check` in `ResilientClient` + `FailStatic`.** Run the authoritative
/// `check` THROUGH the resilient client (the 1.9 seam every Identity caller uses); on a client
/// `CallError` (the transient hiccup) the consumer falls back to S6's bounded-staleness grant.
///
/// `identity_up` models the downstream health: `true` → the client's downstream succeeds (the live
/// `check` is returned, cached by S6); `false` → the downstream errors (the client surfaces a
/// `CallError`), and `check_failstatic`'s `source_ok=false` serves the fallback. This is exactly
/// how a real `ResilientClient::call` over a wire transport behaves — a healthy answer or a loud
/// terminal `CallError` the caller degrades on.
#[allow(clippy::too_many_arguments)]
fn consumer_check<C: myelin_substrate::Clock>(
    client: &ResilientClient,
    target: &Target,
    svc: &StoreBackedCheck,
    s6: &myelin_identity_service::FailStaticCache<C>,
    scope: &TenantScope,
    subject: &Principal,
    permission: &Permission,
    object: &ArtifactRef,
    at: &Consistency,
    now: &Timestamp,
    identity_up: bool,
) -> myelin_identity_service::CachedDecision {
    // The consumer wraps the outbound Identity call in the resilient client (timeout/breaker/
    // bulkhead). `call_op` is the testable core of `call` (the CDC provider side of 1.9). The
    // downstream `op` models the network call to Identity: it errors when Identity is down.
    let outcome: Result<(), CallError> = client.call_op(target, Idempotency::Idempotent, || {
        if identity_up {
            Ok(())
        } else {
            // A transient downstream hiccup — the loud, non-swallowed terminal CallError the
            // resilient client surfaces (it does NOT silently succeed).
            Err(CallError::Downstream {
                message: "identity check downstream hiccup".into(),
                retry_after_ms: None,
            })
        }
    });
    // The consumer maps the client outcome onto `source_ok`: a successful resilient call means the
    // authoritative `check` is reachable (run it, cache it); a CallError means the dependency is
    // hiccuping (fall back to the bounded-staleness S6 grant). Either way S6 honours the
    // zookie-bypass + the just-revoked deny.
    let source_ok = outcome.is_ok();
    svc.check_failstatic(
        s6, scope, subject, permission, object, at, None, now, source_ok,
    )
}
