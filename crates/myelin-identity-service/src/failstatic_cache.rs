use myelin_events::Timestamp;
use myelin_identity::{Consistency, ConsistencyMode, Decision, PrincipalId, RevokeTarget};
use myelin_storage::TenantScope;
use myelin_substrate::thresholds::FailStaticThreshold;
use myelin_substrate::{
    Answer, Clock, FailStatic, FailStaticError, FailStaticSignals, MonotonicClock, Seconds,
    ServeError, StalenessBound,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::revocation::RevocationStore;

pub const S6_STORE: &str = "authz_failstatic_cache";

pub const FRESH_TTL_SECS: Seconds = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoarseGrant {
    pub actor_active: bool,
    pub coarse_grant: Decision,
}

impl CoarseGrant {
    pub fn from_decision(decision: Decision, actor_active: bool) -> CoarseGrant {
        CoarseGrant {
            actor_active,
            coarse_grant: decision,
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct S6Key {
    tenant: String,
    region: String,
    subject: String,
    question: String,
}

#[derive(Clone)]
pub struct FailStaticCache<C: Clock = MonotonicClock> {
    inner: Arc<FailStatic<S6Key, CoarseGrant, C>>,
    revocations: RevocationStore,
    telemetry: Arc<CacheTelemetry>,
}

impl FailStaticCache<MonotonicClock> {
    pub fn try_new(
        revocation_sla_secs: Seconds,
        threshold: &FailStaticThreshold,
        revocations: RevocationStore,
    ) -> Result<FailStaticCache<MonotonicClock>, FailStaticError> {
        FailStaticCache::try_new_with_clock(
            revocation_sla_secs,
            threshold,
            revocations,
            MonotonicClock::default(),
        )
    }
}

impl<C: Clock> FailStaticCache<C> {
    pub fn try_new_with_clock(
        revocation_sla_secs: Seconds,
        threshold: &FailStaticThreshold,
        revocations: RevocationStore,
        clock: C,
    ) -> Result<FailStaticCache<C>, FailStaticError> {
        let bound = StalenessBound::from_threshold(revocation_sla_secs, threshold);
        let static_max = threshold.static_max_default_secs;
        let inner = FailStatic::try_new_with_clock(FRESH_TTL_SECS, static_max, bound, clock)?;
        Ok(FailStaticCache {
            inner: Arc::new(inner),
            revocations,
            telemetry: Arc::new(CacheTelemetry::default()),
        })
    }

    pub fn static_max(&self) -> Seconds {
        self.inner.static_max()
    }

    pub fn clock(&self) -> &C {
        self.inner.clock()
    }

    pub fn revocations(&self) -> &RevocationStore {
        &self.revocations
    }

    pub fn telemetry(&self) -> &CacheTelemetry {
        &self.telemetry
    }

    pub fn fail_static_signals(&self) -> FailStaticSignals {
        self.inner.signals()
    }

    pub fn check_cached(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
        question: &str,
        consistency: &Consistency,
        now: &Timestamp,
        source: impl Fn() -> Result<Decision, ServeError>,
    ) -> CachedDecision {
        if self.subject_revoked(scope, subject, now) {
            return CachedDecision {
                decision: Decision::Deny,
                served: Served::Revoked,
            };
        }

        match consistency.mode {
            ConsistencyMode::Strong => match source() {
                Ok(decision) => {
                    self.telemetry.observe_bypass();
                    CachedDecision {
                        decision,
                        served: Served::SourceBypass,
                    }
                }
                Err(_hiccup) => {
                    self.telemetry.observe_bypass_closed();
                    CachedDecision {
                        decision: Decision::Deny,
                        served: Served::BypassClosed,
                    }
                }
            },

            ConsistencyMode::BoundedStale => {
                let key = self.key(scope, subject, question);
                let answer = self.inner.get(key, || {
                    source().map(|d| CoarseGrant::from_decision(d, !matches!(d, Decision::Deny)))
                });
                self.serve_answer(answer)
            }
        }
    }

    fn serve_answer(&self, answer: Answer<CoarseGrant>) -> CachedDecision {
        match answer {
            Answer::Fresh(grant) => {
                self.telemetry.observe_hit();
                CachedDecision {
                    decision: grant.coarse_grant,
                    served: Served::Fresh,
                }
            }
            Answer::Static(grant) => {
                let age = self.inner.signals().last_staleness_secs;
                self.telemetry.observe_stale(age);
                CachedDecision {
                    decision: grant.coarse_grant,
                    served: Served::Static,
                }
            }
            Answer::Closed => {
                self.telemetry.observe_closed();
                CachedDecision {
                    decision: Decision::Deny,
                    served: Served::Closed,
                }
            }
        }
    }

    fn subject_revoked(&self, scope: &TenantScope, subject: &PrincipalId, now: &Timestamp) -> bool {
        self.revocations
            .is_revoked(scope, &RevokeTarget::Principal(subject.clone()), now)
    }

    fn key(&self, scope: &TenantScope, subject: &PrincipalId, question: &str) -> S6Key {
        S6Key {
            tenant: scope.tenant().0.clone(),
            region: scope.region().0.clone(),
            subject: subject.0.clone(),
            question: question.to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Served {
    SourceBypass,
    BypassClosed,
    Fresh,
    Static,
    Closed,
    Revoked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachedDecision {
    pub decision: Decision,
    pub served: Served,
}

impl CachedDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self.decision, Decision::Allow)
    }

    pub fn is_deny(&self) -> bool {
        matches!(self.decision, Decision::Deny)
    }

    pub fn is_degraded(&self) -> bool {
        matches!(self.served, Served::Static)
    }
}

#[derive(Debug, Default)]
pub struct CacheTelemetry {
    hits: AtomicU64,
    misses: AtomicU64,
    bypasses: AtomicU64,
    last_staleness_secs: AtomicU64,
}

impl CacheTelemetry {
    pub const CACHE_HIT_RATIO: &'static str = myelin_identity::iam_events::signals::CACHE_HIT_RATIO;
    pub const STALENESS_AGE: &'static str = myelin_identity::iam_events::signals::STALENESS_AGE;

    fn observe_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    fn observe_stale(&self, age_secs: u64) {
        self.hits.fetch_add(1, Ordering::Relaxed);
        self.last_staleness_secs.store(age_secs, Ordering::Relaxed);
    }

    fn observe_closed(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    fn observe_bypass(&self) {
        self.bypasses.fetch_add(1, Ordering::Relaxed);
    }

    fn observe_bypass_closed(&self) {
        self.bypasses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    pub fn bypasses(&self) -> u64 {
        self.bypasses.load(Ordering::Relaxed)
    }

    pub fn cache_hit_ratio_pct(&self) -> Option<u64> {
        let hits = self.hits();
        let total = hits + self.misses();
        (hits * 100).checked_div(total)
    }

    pub fn last_staleness_secs(&self) -> u64 {
        self.last_staleness_secs.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalKind};
    use myelin_substrate::TestClock;
    use myelin_tenancy::{Region, TenantId};

    fn threshold() -> FailStaticThreshold {
        FailStaticThreshold {
            status: "OPEN - LEGAL".into(),
            owner: "DPO / Legal".into(),
            static_max_secs: None,
            static_max_default_secs: 300,
            agent_token_ttl_secs: 60,
            constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
        }
    }

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn ts(s: &str) -> Timestamp {
        Timestamp(s.into())
    }

    fn strong(z: &str) -> Consistency {
        Consistency {
            at_least: myelin_identity::Zookie(z.into()),
            mode: ConsistencyMode::Strong,
        }
    }

    fn bounded_stale() -> Consistency {
        Consistency {
            at_least: myelin_identity::Zookie(String::new()),
            mode: ConsistencyMode::BoundedStale,
        }
    }

    fn cache_at(t0: u64) -> FailStaticCache<TestClock> {
        FailStaticCache::try_new_with_clock(
            300,
            &threshold(),
            RevocationStore::new(),
            TestClock::at(t0),
        )
        .expect("valid bound")
    }

    fn allow() -> Result<Decision, ServeError> {
        Ok(Decision::Allow)
    }
    fn hiccup() -> Result<Decision, ServeError> {
        Err(ServeError("identity hiccup".into()))
    }

    #[test]
    fn constructor_enforces_the_fail_static_bound() {
        let mut bad = threshold();
        bad.static_max_default_secs = 301;
        match FailStaticCache::try_new_with_clock(
            300,
            &bad,
            RevocationStore::new(),
            TestClock::at(0),
        ) {
            Err(FailStaticError::ExceedsRevocationSla { .. }) => {}
            Err(other) => panic!("expected ExceedsRevocationSla, got {other:?}"),
            Ok(_) => panic!("a static_max over the revocation SLA must reject (4.11 / §8.2)"),
        }
        let c = cache_at(0);
        assert_eq!(
            c.static_max(),
            300,
            "S6's W is the thresholds-file engineering seed"
        );
    }

    #[test]
    fn bounded_stale_serves_static_during_a_hiccup() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());

        let fresh = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            allow,
        );
        assert_eq!(fresh.served, Served::Fresh);
        assert!(fresh.is_allow(), "the healthy read allows");

        c.clock().advance(31);
        let stale = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            hiccup,
        );
        assert_eq!(
            stale.served,
            Served::Static,
            "the hiccup is survived on the static fallback"
        );
        assert!(
            stale.is_allow(),
            "authenticated traffic survives the hiccup (still Allow)"
        );
        assert!(
            stale.is_degraded(),
            "the answer is marked degraded (bounded-staleness win)"
        );
    }

    #[test]
    fn strong_read_bypasses_the_cache_and_fails_closed_on_hiccup() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());

        let _ = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            allow,
        );
        c.clock().advance(31);

        let strong_hiccup =
            c.check_cached(&acme, &subj, "read@doc:1", &strong("z2"), &ts("z2"), hiccup);
        assert_eq!(
            strong_hiccup.served,
            Served::BypassClosed,
            "strong read bypassed the cache"
        );
        assert!(
            strong_hiccup.is_deny(),
            "a strong read fails CLOSED on a hiccup (never stale)"
        );

        let strong_ok =
            c.check_cached(&acme, &subj, "read@doc:1", &strong("z2"), &ts("z2"), || {
                Ok(Decision::Deny)
            });
        assert_eq!(
            strong_ok.served,
            Served::SourceBypass,
            "strong read served from source"
        );
        assert!(
            strong_ok.is_deny(),
            "the strong read returns the live authoritative Deny, not the cached Allow"
        );
    }

    #[test]
    fn revoked_subject_is_denied_even_with_a_stale_s6_allow() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());

        let _ = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            allow,
        );
        c.clock().advance(31);
        let before = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            hiccup,
        );
        assert!(
            before.is_allow() && before.is_degraded(),
            "the stale allow is live before revoke"
        );

        c.revocations()
            .disable_principal(&acme, &subj, ts("2026-06-19T00:00:00Z"))
            .expect("record principal disablement");

        let after = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("2026-06-19T00:01:00Z"),
            hiccup,
        );
        assert_eq!(
            after.served,
            Served::Revoked,
            "the revoke is enforced before the cache is read"
        );
        assert!(
            after.is_deny(),
            "0 successful authz after the cache for a revoked subject (F7)"
        );
    }

    #[test]
    fn past_the_budget_bounded_stale_fails_closed() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());
        let _ = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            allow,
        );

        c.clock().advance(301);
        let closed = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            hiccup,
        );
        assert_eq!(
            closed.served,
            Served::Closed,
            "past the budget the cache fails closed"
        );
        assert!(
            closed.is_deny(),
            "fail CLOSED (deny is correct), never fail open"
        );
    }

    #[test]
    fn cold_bounded_stale_hiccup_fails_closed() {
        let c = cache_at(0);
        let acme = scope("acme");
        let subj = PrincipalId("p:bob".into());
        let cold = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            hiccup,
        );
        assert_eq!(cold.served, Served::Closed, "no fallback → fail closed");
        assert!(
            cold.is_deny(),
            "S6 never fabricates an allow (never fail open)"
        );
    }

    #[test]
    fn cache_key_isolates_tenant_subject_and_question() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let evil = scope("evil-corp");
        let alice = PrincipalId("p:alice".into());
        let bob = PrincipalId("p:bob".into());

        let _ = c.check_cached(
            &acme,
            &alice,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            allow,
        );
        c.clock().advance(31);

        let cross_tenant = c.check_cached(
            &evil,
            &alice,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            hiccup,
        );
        assert!(cross_tenant.is_deny(), "no cross-tenant cache leak");
        let cross_subject = c.check_cached(
            &acme,
            &bob,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            hiccup,
        );
        assert!(cross_subject.is_deny(), "no cross-subject cache leak");
        let cross_question = c.check_cached(
            &acme,
            &alice,
            "write@doc:1",
            &bounded_stale(),
            &ts("z"),
            hiccup,
        );
        assert!(cross_question.is_deny(), "no cross-question cache leak");
    }

    #[test]
    fn telemetry_records_hit_ratio_and_staleness_under_frozen_names() {
        assert_eq!(CacheTelemetry::CACHE_HIT_RATIO, "cache_hit_ratio");
        assert_eq!(CacheTelemetry::STALENESS_AGE, "staleness_age");

        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());

        let _ = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            allow,
        );
        c.clock().advance(31);
        let _ = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            hiccup,
        );
        c.clock().advance(301);
        let _ = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            hiccup,
        );

        let t = c.telemetry();
        assert_eq!(t.hits(), 2, "two cache hits (fresh + stale)");
        assert_eq!(t.misses(), 1, "one fail-closed miss");
        assert_eq!(t.cache_hit_ratio_pct(), Some(66), "2/3 hits ≈ 66%");
        assert!(t.last_staleness_secs() > 0, "a staleness age was recorded");
        assert!(
            t.last_staleness_secs() <= c.static_max(),
            "staleness_age never exceeds static_max (≤ the revocation SLA)"
        );
    }

    #[test]
    fn bypass_reads_are_not_in_the_hit_ratio() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());
        let _ = c.check_cached(&acme, &subj, "read@doc:1", &strong("z"), &ts("z"), allow);
        let _ = c.check_cached(&acme, &subj, "read@doc:1", &strong("z"), &ts("z"), hiccup);
        let t = c.telemetry();
        assert_eq!(t.bypasses(), 2, "both strong reads bypassed the cache");
        assert_eq!(t.hits(), 0);
        assert_eq!(t.misses(), 0);
        assert_eq!(
            t.cache_hit_ratio_pct(),
            None,
            "no cache-consulting read → no ratio (never fabricated)"
        );
    }

    #[test]
    fn fail_static_survival_signals_are_exposed() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());
        let _ = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            allow,
        );
        c.clock().advance(31);
        let _ = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            hiccup,
        );
        let s = c.fail_static_signals();
        assert_eq!(s.fresh, 1, "one fresh answer");
        assert_eq!(
            s.stale, 1,
            "one stale (degraded) answer - the survival rung"
        );
        assert!(
            s.last_staleness_secs <= c.static_max(),
            "staleness ≤ the budget"
        );
    }

    #[test]
    fn coarse_grant_never_escalates() {
        let c = cache_at(1_000);
        let acme = scope("acme");
        let subj = PrincipalId("p:alice".into());
        let d = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            || Ok(Decision::Deny),
        );
        assert!(d.is_deny(), "a Deny source serves Deny");
        c.clock().advance(31);
        let stale = c.check_cached(
            &acme,
            &subj,
            "read@doc:1",
            &bounded_stale(),
            &ts("z"),
            hiccup,
        );
        assert_eq!(stale.served, Served::Static);
        assert!(
            stale.is_deny(),
            "the stale fallback replays the cached Deny - never escalates to Allow"
        );
    }
}
