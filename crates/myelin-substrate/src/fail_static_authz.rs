use myelin_identity::{Consistency, ConsistencyMode, Decision};

use crate::fail_static::{Answer, Clock, FailStatic, FailStaticSignals, MonotonicClock};
use crate::thresholds::FailStaticThreshold;
use crate::{FailStaticError, Seconds, ServeError, StalenessBound};

pub fn encode_authz_key(segments: &[&str]) -> String {
    let mut out = String::with_capacity(segments.iter().map(|s| s.len() + 3).sum());
    for seg in segments {
        out.push_str(&seg.len().to_string());
        out.push(':');
        out.push_str(seg);
    }
    out
}

pub const AUTHZ_FRESH_TTL_SECS: Seconds = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoarseAuthz {
    pub decision: Decision,
}

impl CoarseAuthz {
    pub fn of(decision: Decision) -> CoarseAuthz {
        CoarseAuthz { decision }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthzServed {
    SourceBypass,
    BypassClosed,
    Fresh,
    Static,
    Closed,
    Revoked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthzDecision {
    pub decision: Decision,
    pub served: AuthzServed,
}

impl AuthzDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self.decision, Decision::Allow)
    }

    pub fn is_deny(&self) -> bool {
        matches!(self.decision, Decision::Deny)
    }

    pub fn is_degraded(&self) -> bool {
        matches!(self.served, AuthzServed::Static)
    }
}

pub struct FailStaticAuthz<C: Clock = MonotonicClock> {
    inner: FailStatic<String, CoarseAuthz, C>,
}

impl<C: Clock> std::fmt::Debug for FailStaticAuthz<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FailStaticAuthz")
            .field("inner", &self.inner)
            .finish()
    }
}

impl FailStaticAuthz<MonotonicClock> {
    pub fn try_new(
        revocation_sla_secs: Seconds,
        threshold: &FailStaticThreshold,
    ) -> Result<FailStaticAuthz<MonotonicClock>, FailStaticError> {
        FailStaticAuthz::try_new_with_clock(
            revocation_sla_secs,
            threshold,
            MonotonicClock::default(),
        )
    }
}

impl<C: Clock> FailStaticAuthz<C> {
    pub fn try_new_with_clock(
        revocation_sla_secs: Seconds,
        threshold: &FailStaticThreshold,
        clock: C,
    ) -> Result<FailStaticAuthz<C>, FailStaticError> {
        let bound = StalenessBound::from_threshold(revocation_sla_secs, threshold);
        let static_max = threshold.static_max_default_secs;
        let inner = FailStatic::try_new_with_clock(AUTHZ_FRESH_TTL_SECS, static_max, bound, clock)?;
        Ok(FailStaticAuthz { inner })
    }

    pub fn static_max(&self) -> Seconds {
        self.inner.static_max()
    }

    pub fn clock(&self) -> &C {
        self.inner.clock()
    }

    pub fn signals(&self) -> FailStaticSignals {
        self.inner.signals()
    }

    pub fn serve(
        &self,
        key: impl Into<String>,
        at: &Consistency,
        subject_revoked: bool,
        source: impl Fn() -> Result<Decision, ServeError>,
    ) -> AuthzDecision {
        if subject_revoked {
            return AuthzDecision {
                decision: Decision::Deny,
                served: AuthzServed::Revoked,
            };
        }

        match at.mode {
            ConsistencyMode::Strong => match source() {
                Ok(decision) => AuthzDecision {
                    decision,
                    served: AuthzServed::SourceBypass,
                },
                Err(_hiccup) => AuthzDecision {
                    decision: Decision::Deny,
                    served: AuthzServed::BypassClosed,
                },
            },

            ConsistencyMode::BoundedStale => {
                let answer = self.inner.get(key.into(), || source().map(CoarseAuthz::of));
                serve_answer(answer)
            }
        }
    }
}

fn serve_answer(answer: Answer<CoarseAuthz>) -> AuthzDecision {
    match answer {
        Answer::Fresh(grant) => AuthzDecision {
            decision: grant.decision,
            served: AuthzServed::Fresh,
        },
        Answer::Static(grant) => AuthzDecision {
            decision: grant.decision,
            served: AuthzServed::Static,
        },
        Answer::Closed => AuthzDecision {
            decision: Decision::Deny,
            served: AuthzServed::Closed,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fail_static::TestClock;
    use myelin_identity::Zookie;

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

    fn authz_at(t0: u64) -> FailStaticAuthz<TestClock> {
        FailStaticAuthz::try_new_with_clock(300, &threshold(), TestClock::at(t0))
            .expect("valid bound")
    }

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

    fn allow() -> Result<Decision, ServeError> {
        Ok(Decision::Allow)
    }
    fn hiccup() -> Result<Decision, ServeError> {
        Err(ServeError("identity authz hiccup".into()))
    }

    #[test]
    fn constructor_enforces_the_fail_static_bound() {
        let mut bad = threshold();
        bad.static_max_default_secs = 301;
        match FailStaticAuthz::try_new_with_clock(300, &bad, TestClock::at(0)) {
            Err(FailStaticError::ExceedsRevocationSla { .. }) => {}
            other => panic!("a static_max over the revocation SLA must reject, got {other:?}"),
        }
        assert_eq!(
            authz_at(0).static_max(),
            300,
            "W is the thresholds-file engineering seed"
        );
    }

    #[test]
    fn bounded_stale_serves_static_during_a_hiccup() {
        let fs = authz_at(1_000);
        let fresh = fs.serve("acme|eu|alice|read@doc:1", &bounded_stale(), false, allow);
        assert_eq!(fresh.served, AuthzServed::Fresh);
        assert!(fresh.is_allow(), "the healthy read allows");

        fs.clock().advance(31);
        let stale = fs.serve("acme|eu|alice|read@doc:1", &bounded_stale(), false, hiccup);
        assert_eq!(
            stale.served,
            AuthzServed::Static,
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
    fn past_the_budget_bounded_stale_fails_closed() {
        let fs = authz_at(1_000);
        let _ = fs.serve("k", &bounded_stale(), false, allow);
        fs.clock().advance(301);
        let closed = fs.serve("k", &bounded_stale(), false, hiccup);
        assert_eq!(
            closed.served,
            AuthzServed::Closed,
            "past the budget the cache fails closed"
        );
        assert!(
            closed.is_deny(),
            "fail CLOSED (deny is correct), never fail open"
        );
    }

    #[test]
    fn cold_bounded_stale_hiccup_fails_closed() {
        let fs = authz_at(0);
        let cold = fs.serve("never-seen", &bounded_stale(), false, hiccup);
        assert_eq!(
            cold.served,
            AuthzServed::Closed,
            "no fallback → fail closed"
        );
        assert!(
            cold.is_deny(),
            "the cache never fabricates an allow (never fail open)"
        );
    }

    #[test]
    fn strong_read_bypasses_the_cache_and_fails_closed_on_hiccup() {
        let fs = authz_at(1_000);
        let _ = fs.serve("k", &bounded_stale(), false, allow);
        fs.clock().advance(31);

        let strong_hiccup = fs.serve("k", &strong("z2"), false, hiccup);
        assert_eq!(
            strong_hiccup.served,
            AuthzServed::BypassClosed,
            "strong read bypassed the cache"
        );
        assert!(
            strong_hiccup.is_deny(),
            "a strong read fails CLOSED on a hiccup (never stale)"
        );

        let strong_ok = fs.serve("k", &strong("z2"), false, || Ok(Decision::Deny));
        assert_eq!(
            strong_ok.served,
            AuthzServed::SourceBypass,
            "strong read served from source"
        );
        assert!(
            strong_ok.is_deny(),
            "the strong read returns the live authoritative Deny, not the cached Allow"
        );
    }

    #[test]
    fn revoked_subject_is_denied_even_with_a_stale_allow() {
        let fs = authz_at(1_000);
        let _ = fs.serve("k", &bounded_stale(), false, allow);
        fs.clock().advance(31);
        let before = fs.serve("k", &bounded_stale(), false, hiccup);
        assert!(
            before.is_allow() && before.is_degraded(),
            "the stale allow is live before revoke"
        );

        let after = fs.serve("k", &bounded_stale(), true, hiccup);
        assert_eq!(
            after.served,
            AuthzServed::Revoked,
            "the revoke is enforced before the cache is read"
        );
        assert!(
            after.is_deny(),
            "0 successful authz after the cache for a revoked subject"
        );

        let strong_revoked = fs.serve("k", &strong("z"), true, allow);
        assert_eq!(
            strong_revoked.served,
            AuthzServed::Revoked,
            "revoke is applied on every mode"
        );
        assert!(strong_revoked.is_deny());
    }

    #[test]
    fn cache_never_escalates_a_deny_to_an_allow() {
        let fs = authz_at(1_000);
        let d = fs.serve("k", &bounded_stale(), false, || Ok(Decision::Deny));
        assert!(d.is_deny(), "a Deny source serves Deny");
        fs.clock().advance(31);
        let stale = fs.serve("k", &bounded_stale(), false, hiccup);
        assert_eq!(stale.served, AuthzServed::Static);
        assert!(
            stale.is_deny(),
            "the stale fallback replays the cached Deny - never escalates to Allow"
        );
    }

    #[test]
    fn distinct_keys_do_not_share_a_cache_bucket() {
        let fs = authz_at(1_000);
        let _ = fs.serve("acme|eu|alice|read@doc:1", &bounded_stale(), false, allow);
        fs.clock().advance(31);
        let alice = fs.serve("acme|eu|alice|read@doc:1", &bounded_stale(), false, hiccup);
        assert!(
            alice.is_allow() && alice.is_degraded(),
            "alice's stale ALLOW is live in the cache"
        );
        let other = fs.serve("acme|eu|bob|read@doc:1", &bounded_stale(), false, hiccup);
        assert!(other.is_deny(), "bob must not borrow alice's cached grant");
        let other_q = fs.serve("acme|eu|alice|write@doc:1", &bounded_stale(), false, hiccup);
        assert!(
            other_q.is_deny(),
            "a different question must not borrow the read grant"
        );
    }

    #[test]
    fn survival_signals_are_exposed_and_bounded() {
        let fs = authz_at(1_000);
        let _ = fs.serve("k", &bounded_stale(), false, allow);
        fs.clock().advance(31);
        let _ = fs.serve("k", &bounded_stale(), false, hiccup);
        fs.clock().advance(300);
        let _ = fs.serve("k", &bounded_stale(), false, hiccup);
        let s = fs.signals();
        assert_eq!(s.fresh, 1, "one fresh answer");
        assert_eq!(
            s.stale, 1,
            "one stale (degraded) answer - the survival rung"
        );
        assert_eq!(s.closed, 1, "one fail-closed answer past the window");
        assert!(
            s.last_staleness_secs <= fs.static_max(),
            "staleness ≤ the budget (≤ revocation SLA)"
        );
    }

    #[test]
    fn authz_decision_classifiers_are_exact() {
        let allow_fresh = AuthzDecision {
            decision: Decision::Allow,
            served: AuthzServed::Static,
        };
        assert!(allow_fresh.is_allow() && !allow_fresh.is_deny() && allow_fresh.is_degraded());
        let deny_closed = AuthzDecision {
            decision: Decision::Deny,
            served: AuthzServed::Closed,
        };
        assert!(deny_closed.is_deny() && !deny_closed.is_allow() && !deny_closed.is_degraded());
        let allow_bypass = AuthzDecision {
            decision: Decision::Allow,
            served: AuthzServed::SourceBypass,
        };
        assert!(
            !allow_bypass.is_degraded(),
            "only the Static rung is degraded"
        );
    }

    #[test]
    fn coarse_authz_carries_the_exact_decision() {
        assert_eq!(CoarseAuthz::of(Decision::Deny).decision, Decision::Deny);
        assert_eq!(CoarseAuthz::of(Decision::Allow).decision, Decision::Allow);
        assert_eq!(
            CoarseAuthz::of(Decision::Conditional).decision,
            Decision::Conditional
        );
    }

    #[test]
    fn debug_shows_bounds_and_counters_not_cached_values() {
        let fs = authz_at(0);
        let _ = fs.serve("k", &bounded_stale(), false, allow);
        let dbg = format!("{fs:?}");
        assert!(
            dbg.contains("FailStaticAuthz"),
            "names the wiring type: {dbg}"
        );
        assert!(
            dbg.contains("static_max"),
            "prints the static_max bound (via the inner): {dbg}"
        );
        assert!(
            dbg.contains("300"),
            "prints the static_max value (300): {dbg}"
        );
        assert!(
            dbg.contains("fresh"),
            "prints the live signal counters (via the inner): {dbg}"
        );
    }
}
