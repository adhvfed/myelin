//! `fail_static_authz` — the M0 [`FailStatic`] mechanism WIRED into the Identity authz read
//! path (P-S25 → global P-087; SUB-D4). The "fail-static-wiring module".
//!
//! CANON:
//!   - `external-insights/01-process-and-quality-doctrine.md` §2 (a shared-dependency cascade is
//!     a platform-wide kill — **fail-static, not fail-closed, is the AVAILABILITY default**) and
//!     §3 (prove-it: a property is not real until a drill forces the failure and observability
//!     watches the system survive — the SUB-D4 drill, not just the mechanism).
//!   - `planning/05-refined-shared-systems-architecture/00-platform-substrate.md` §8 (fail-static
//!     — full), §8.2 (the staleness bound `static_max ≤ revocation-SLA ≥ agent-token-TTL`; the
//!     **zookie bypass** — a security-sensitive read carrying a zookie BYPASSES the cache and
//!     forces a fresh read), §8.3 (composes with readiness, §4.3: fail-static handles a TRANSIENT
//!     hiccup, readiness a SUSTAINED outage).
//!   - `planning/05-refined-shared-systems-architecture/contract-index.md` rows 1.10
//!     (`FailStatic<T>`), 4.10 (zookie reads bypass the cache), 4.11 (the Id-usage bound; the
//!     value W is `[OPEN — LEGAL]`).
//!
//! ## What P-S18 built vs what P-S25 builds here
//! P-S18 ([`crate::fail_static`]) shipped the bounded-staleness MECHANISM ([`FailStatic<T>`]) +
//! the §8.2 constructor constraint, unit-drilled at its boundaries. It named the floor: *fail-
//! static is PROVEN against a real Identity hiccup in M1 (P-S25)*. THIS module is that wiring —
//! the thin authz read-path adapter that:
//!   1. routes a default-consistency (`BoundedStale`) authz read through the [`FailStatic`]
//!      cache, so a TRANSIENT Identity hiccup is survived on the coarse `{actor_active,
//!      coarse_grants}` answer within `static_max` (never an escalation of access — never fail
//!      open);
//!   2. enforces the **zookie bypass** (4.10): a `Strong`/zookie-stamped read BYPASSES the cache
//!      and goes straight to the authoritative source — on a hiccup it fails CLOSED, never serves
//!      stale (the new-enemy guard);
//!   3. enforces the **revoked-actor deny** on EVERY served answer: a subject revoked since the
//!      grant was cached is denied THROUGH the stale cache (a revoked actor is denied once the
//!      window closes — the SUB-D4 quantified property), via a caller-supplied revocation consult.
//!
//! The Identity service owns the equivalent S6 cache + the ID-D2 drill (P-073); this is the
//! SUBSTRATE half of the same seam — the generic, dependency-free wiring every service rides to
//! call the Identity dependency root, drilled against the **P-S03 dependency-break injector**
//! (the harness drives the hiccup through `is_broken(Identity, scope)`) and the **P-S04**
//! telemetry-assertion library (SUB-D4, the harness drill). One cache primitive (EI-01 §7):
//! this wraps the SAME [`FailStatic`] the platform is built on, not a bespoke availability path.
//!
//! ## Floor named (EI-01 §3)
//! The VALUE of `static_max` (W) stays `[OPEN — LEGAL]` (DPO-ratified, L-1) — the MECHANISM and
//! the `static_max ≤ revocation-SLA ≥ agent-token-TTL` constraint are PROVEN here against a real
//! Identity hiccup regardless of the final number; the constraint is enforced structurally by
//! [`FailStatic::try_new`] (P-S18). Reading W as a ratified number stays a loud error
//! ([`crate::thresholds::FailStaticThreshold::ratified_static_max_secs`]).

use myelin_identity::{Consistency, ConsistencyMode, Decision};

use crate::fail_static::{Answer, Clock, FailStatic, FailStaticSignals, SystemClock};
use crate::thresholds::FailStaticThreshold;
use crate::{FailStaticError, Seconds, ServeError, StalenessBound};

/// The freshness window seed (seconds) of the authz fail-static ladder: within it a cached coarse
/// answer is served fresh even on a hiccup (no degradation). A v1 seed well under `static_max`;
/// the measured value is tuned at scale. `static_max` (W) is read from the thresholds file
/// (`[fail_static]`), never hardcoded.
pub const AUTHZ_FRESH_TTL_SECS: Seconds = 30;

/// The coarse authorization answer the cache holds (architecture §8/§10: bounded-staleness
/// `{actor_active, coarse_grants}`). It is **coarse on purpose** — the availability fallback, not
/// the authoritative fine-grained decision. The cache NEVER fabricates an `Allow`: a `CoarseAuthz`
/// is only ever written from a real authoritative decision, so the availability fallback can only
/// ever REPLAY a previously-authoritative answer (never escalate access).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoarseAuthz {
    /// The last authoritative coarse decision (`Allow`/`Deny`/`Conditional`). Never fabricated —
    /// written only from a real authoritative source answer.
    pub decision: Decision,
}

impl CoarseAuthz {
    /// Wrap an authoritative decision as the coarse cached answer.
    pub fn of(decision: Decision) -> CoarseAuthz {
        CoarseAuthz { decision }
    }
}

/// Which branch of the fail-static authz path produced an answer — the observable provenance the
/// SUB-D4 drill asserts (e.g. "a `Strong` read bypassed the cache" / "a `BoundedStale` read
/// survived on the static fallback" / "a revoked subject was denied through the cache").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthzServed {
    /// A zookie-stamped (`Strong`) read served the authoritative source directly (cache bypassed).
    SourceBypass,
    /// A zookie-stamped (`Strong`) read failed CLOSED on a hiccup (never served stale).
    BypassClosed,
    /// A `BoundedStale` read served a fresh answer (live source, or cached within `fresh_ttl`).
    Fresh,
    /// A `BoundedStale` read served the last coarse grant STATIC (degraded) during a hiccup — the
    /// availability win the drill asserts authenticated traffic survives on.
    Static,
    /// A `BoundedStale` read failed CLOSED (staleness budget spent / no fallback). Never open.
    Closed,
    /// The subject was revoked since the grant was cached — denied regardless of any cached grant
    /// (the revoked-actor-denied-at-window-close property).
    Revoked,
}

/// A served authz decision + its provenance (so the drill / mutation floor can assert the BRANCH
/// as well as the answer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthzDecision {
    /// The authorization decision served (fail-closed `Deny` on bypass-closed / closed / revoked).
    pub decision: Decision,
    /// Which fail-static branch produced it.
    pub served: AuthzServed,
}

impl AuthzDecision {
    /// Was this an explicit `Allow`? (the "authenticated traffic survives" assertion).
    pub fn is_allow(&self) -> bool {
        matches!(self.decision, Decision::Allow)
    }

    /// Was this a `Deny`? (the "revoked actor denied once the window closes" assertion).
    pub fn is_deny(&self) -> bool {
        matches!(self.decision, Decision::Deny)
    }

    /// Was this served from the static (degraded) fallback — the availability-survival rung?
    pub fn is_degraded(&self) -> bool {
        matches!(self.served, AuthzServed::Static)
    }
}

/// **The M0 [`FailStatic`] mechanism wired into the Identity authz read path (P-S25; contract
/// 1.10 / 4.10 / 4.11).** A thin, dependency-free adapter: it fronts the authoritative authz
/// `source` with the bounded-staleness cache so a transient Identity hiccup is survived, honours
/// the zookie bypass, and denies a revoked subject through a stale grant.
///
/// **The whole point (§8 / EI-01 §2).** Authorization CORRECTNESS stays fail-CLOSED (deny when
/// genuinely unsure); AVAILABILITY fails STATIC (an Identity-dependency hiccup keeps already-
/// authenticated traffic alive on the bounded-staleness cache, rather than failing every request
/// closed and turning the one shared dependency into a platform-wide cascade). A zookie-stamped
/// (`Strong`) read BYPASSES the cache and is fail-closed-or-wait; a default-consistency
/// (`BoundedStale`) read is served static during a hiccup. A just-revoked subject is denied
/// THROUGH the stale cache (the revocation consult on every served answer).
///
/// Generic over the [`Clock`] so the SUB-D4 drill/CDC advance a [`crate::TestClock`] across the
/// `fresh_ttl` / `static_max` boundaries deterministically (production wires [`SystemClock`]).
pub struct FailStaticAuthz<C: Clock = SystemClock> {
    /// The substrate bounded-staleness mechanism (P-S18). The `static_max` (W) it was constructed
    /// with is the thresholds-file bound; the constructor enforced
    /// `agent_token_ttl ≤ static_max ≤ revocation_sla` structurally. Keyed by the OWNED `String`
    /// authz discriminator (the verified `(tenant, region, subject, permission@object)` the caller
    /// builds) — the cache compares the FULL key, so two distinct questions that collide in a hash
    /// can never share one cached grant (R2.3; no 64-bit-hash aliasing).
    inner: FailStatic<String, CoarseAuthz, C>,
}

impl<C: Clock> std::fmt::Debug for FailStaticAuthz<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Delegate to the inner `FailStatic` Debug, which deliberately prints only the window
        // bounds + the signal counters, NEVER the cached coarse-grant values (they are authz
        // answers, not for the log).
        f.debug_struct("FailStaticAuthz")
            .field("inner", &self.inner)
            .finish()
    }
}

impl FailStaticAuthz<SystemClock> {
    /// Wire the authz fail-static cache against the wall clock, reading the §8.2 bound from the
    /// thresholds file (`[fail_static]`). `static_max` (W) is the engineering seed
    /// (`static_max_default_secs`, == the revocation SLA, the largest the constraint admits — the
    /// ratified W is `[OPEN — LEGAL]`); the constructor enforces the bound structurally, so a bad
    /// bound does NOT construct (P-S18). `revocation_sla_secs` is N (the deprovision SLA).
    pub fn try_new(
        revocation_sla_secs: Seconds,
        threshold: &FailStaticThreshold,
    ) -> Result<FailStaticAuthz<SystemClock>, FailStaticError> {
        FailStaticAuthz::try_new_with_clock(revocation_sla_secs, threshold, SystemClock)
    }
}

impl<C: Clock> FailStaticAuthz<C> {
    /// Wire the authz fail-static cache against an injected clock (the SUB-D4 drill/CDC seam). The
    /// W is the thresholds-file seed `static_max_default_secs`; `fresh_ttl` is
    /// [`AUTHZ_FRESH_TTL_SECS`]. The bound is enforced structurally by [`FailStatic::try_new_with_clock`]
    /// — a `static_max` violating `agent_token_ttl ≤ static_max ≤ revocation_sla` REJECTS here,
    /// never reaches a read.
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

    /// The staleness budget W (seconds) the cache was constructed with (the thresholds-file seed;
    /// the ratified W is `[OPEN — LEGAL]`). `static_max ≤ revocation SLA`.
    pub fn static_max(&self) -> Seconds {
        self.inner.static_max()
    }

    /// A borrow of the injected clock — drills advance a [`crate::TestClock`] across the staleness
    /// boundaries through it.
    pub fn clock(&self) -> &C {
        self.inner.clock()
    }

    /// The underlying fail-static fresh/stale/closed signals (architecture §10.2 row 6) — the
    /// SUB-D4 drill reads the answer ratio + the staleness age off this.
    pub fn signals(&self) -> FailStaticSignals {
        self.inner.signals()
    }

    /// **The authz read through the fail-static availability cache — the load-bearing entrypoint
    /// (architecture §8 / §10).** Serve an authz answer, honouring the zookie bypass and the
    /// revoked-actor deny.
    ///
    /// - `key` is the cache key (the verified `(tenant, region, subject, permission@object)`
    ///   discriminator built by the caller — distinct authz questions never share one cached
    ///   grant; the partition prefix comes from a verified token, never a path). Accepted as
    ///   anything that owns into a `String`; the cache compares the FULL key by value (never a
    ///   64-bit digest), so a hash collision between two distinct questions cannot alias grants (R2.3).
    /// - `at` is the read consistency: `Strong` (zookie-stamped) BYPASSES the cache (4.10);
    ///   `BoundedStale` consults it.
    /// - `subject_revoked` is the caller's revocation consult (the S7 denylist): a subject revoked
    ///   since the grant was cached is denied on EVERY mode, BEFORE the cache is even read — the
    ///   revoked-actor-denied-once-the-window-closes property the SUB-D4 drill quantifies.
    /// - `source` is the authoritative authz read (the depth-bounded Zanzibar `check`). On a
    ///   healthy path it returns `Ok(decision)` (cached + served fresh); on a TRANSIENT Identity
    ///   hiccup it returns `Err` (the cache serves the bounded-staleness fallback, or fails
    ///   closed). A drill drives the hiccup by making `source` return `Err` (the harness routes
    ///   the **P-S03 `DependencyBreaker`** consult into this).
    pub fn serve(
        &self,
        key: impl Into<String>,
        at: &Consistency,
        subject_revoked: bool,
        source: impl Fn() -> Result<Decision, ServeError>,
    ) -> AuthzDecision {
        // The revoked-actor deny (the §4 authoritative path) is ALWAYS applied first — a subject
        // revoked since the grant was cached is denied on EVERY consistency mode, BEFORE the cache
        // is consulted. A stale ALLOW in the cache never overrides a revoke (the SUB-D4 "revoked
        // denied once the window closes" property; never an escalation of access).
        if subject_revoked {
            return AuthzDecision {
                decision: Decision::Deny,
                served: AuthzServed::Revoked,
            };
        }

        match at.mode {
            // ── Zookie-stamped (Strong) read → BYPASS the cache (4.10). ──
            // Fail-closed-or-wait: hit the authoritative source; on a hiccup fail CLOSED. The cache
            // is neither read nor written — a strong read never serves (or caches) stale.
            ConsistencyMode::Strong => match source() {
                Ok(decision) => AuthzDecision {
                    decision,
                    served: AuthzServed::SourceBypass,
                },
                // The new-enemy guard: a zookie read does NOT fall back to stale — it fails closed.
                Err(_hiccup) => AuthzDecision {
                    decision: Decision::Deny,
                    served: AuthzServed::BypassClosed,
                },
            },

            // ── Default-consistency (BoundedStale) read → consult the cache (the availability win). ──
            ConsistencyMode::BoundedStale => {
                // `FailStatic::get` runs the source; on success it caches the coarse answer + serves
                // it Fresh; on a hiccup it serves the last coarse grant Static (within W) or Closed
                // (past W / no fallback). Only a real authoritative answer is ever cached.
                let answer = self.inner.get(key.into(), || source().map(CoarseAuthz::of));
                serve_answer(answer)
            }
        }
    }
}

/// Map a [`FailStatic`] `Answer<CoarseAuthz>` into the served authz decision. A `Closed` answer
/// (budget spent / no fallback) is the fail-CLOSED `Deny` — never an open fall-through. Factored
/// out so the branch logic is one testable unit (the mutation floor reads it).
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
        // Closed: budget spent or no fallback → fail CLOSED (deny is correct). NEVER open.
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

    /// The drill bound: agent-token TTL = 60s (lower), revocation SLA = 300s (upper). `static_max`
    /// seed = 300 (the largest the constraint admits). Mirrors the thresholds.toml `[fail_static]`
    /// row (the engineering seed; the ratified W is `[OPEN — LEGAL]`).
    fn threshold() -> FailStaticThreshold {
        FailStaticThreshold {
            status: "OPEN — LEGAL".into(),
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

    /// The constructor enforces the §8.2 bound (4.11): a `static_max` over the revocation SLA does
    /// not construct — the wiring cannot stand up a window that outlives the revocation SLA.
    #[test]
    fn constructor_enforces_the_fail_static_bound() {
        let mut bad = threshold();
        bad.static_max_default_secs = 301; // > 300 SLA
        match FailStaticAuthz::try_new_with_clock(300, &bad, TestClock::at(0)) {
            Err(FailStaticError::ExceedsRevocationSla { .. }) => {}
            other => panic!("a static_max over the revocation SLA must reject, got {other:?}"),
        }
        // the valid seed (300 == SLA) constructs; its static_max is the seed W.
        assert_eq!(
            authz_at(0).static_max(),
            300,
            "W is the thresholds-file engineering seed"
        );
    }

    /// **Within W a hiccup serves STALE + degraded (never open).** A `BoundedStale` read caches a
    /// fresh authoritative Allow, then — on a hiccup past `fresh_ttl`, within `static_max` — keeps
    /// serving it STATIC (degraded): authenticated traffic survives the hiccup.
    #[test]
    fn bounded_stale_serves_static_during_a_hiccup() {
        let fs = authz_at(1_000);
        // healthy read caches the Allow + serves it Fresh.
        let fresh = fs.serve("acme|eu|alice|read@doc:1", &bounded_stale(), false, allow);
        assert_eq!(fresh.served, AuthzServed::Fresh);
        assert!(fresh.is_allow(), "the healthy read allows");

        // past fresh_ttl (age 31 > 30), within static_max → STATIC (degraded) on a hiccup.
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

    /// **Past W a `BoundedStale` read fails CLOSED (never open).** The staleness budget is bounded
    /// ≤ the revocation SLA — past `static_max` the answer is `Deny`, it does not serve stale
    /// forever.
    #[test]
    fn past_the_budget_bounded_stale_fails_closed() {
        let fs = authz_at(1_000);
        let _ = fs.serve("k", &bounded_stale(), false, allow);
        fs.clock().advance(301); // age 301 > 300 static_max
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

    /// **A cold `BoundedStale` hiccup with no fallback fails CLOSED (never open).** A hiccup before
    /// any cached grant exists denies — the cache never fabricates an allow.
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

    /// **A zookie-stamped (`Strong`) read BYPASSES the cache (4.10).** It goes straight to the
    /// authoritative source — even when the cache holds a stale grant — and on a hiccup fails
    /// CLOSED (never serves stale). The new-enemy guard.
    #[test]
    fn strong_read_bypasses_the_cache_and_fails_closed_on_hiccup() {
        let fs = authz_at(1_000);
        // warm the cache with a stale-eligible Allow via a BoundedStale read.
        let _ = fs.serve("k", &bounded_stale(), false, allow);
        fs.clock().advance(31);

        // a STRONG read with a hiccup does NOT serve the stale allow — it fails CLOSED.
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

        // a STRONG read with a healthy source serves the AUTHORITATIVE answer (bypass), not the cache.
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

    /// **A revoked subject is denied even with a stale cache ALLOW (the revoked-at-window-close
    /// property).** Even when the cache holds a stale ALLOW, a `subject_revoked` consult denies the
    /// subject THROUGH the cache — the revoke is enforced before the cache is read.
    #[test]
    fn revoked_subject_is_denied_even_with_a_stale_allow() {
        let fs = authz_at(1_000);
        // warm a stale Allow.
        let _ = fs.serve("k", &bounded_stale(), false, allow);
        fs.clock().advance(31);
        // absent a revoke it WOULD serve a stale allow.
        let before = fs.serve("k", &bounded_stale(), false, hiccup);
        assert!(
            before.is_allow() && before.is_degraded(),
            "the stale allow is live before revoke"
        );

        // now revoke: even the stale allow is DENIED, before the cache is read.
        let after = fs.serve("k", &bounded_stale(), /* revoked */ true, hiccup);
        assert_eq!(
            after.served,
            AuthzServed::Revoked,
            "the revoke is enforced before the cache is read"
        );
        assert!(
            after.is_deny(),
            "0 successful authz after the cache for a revoked subject"
        );

        // a revoke also denies a STRONG read (every mode), before bypass.
        let strong_revoked = fs.serve("k", &strong("z"), true, allow);
        assert_eq!(
            strong_revoked.served,
            AuthzServed::Revoked,
            "revoke is applied on every mode"
        );
        assert!(strong_revoked.is_deny());
    }

    /// **The cache NEVER escalates: a `Deny` source caches a Deny, replayed stale.** The
    /// availability fallback can only REPLAY a previously-authoritative answer, never fabricate an
    /// `Allow`.
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
            "the stale fallback replays the cached Deny — never escalates to Allow"
        );
    }

    /// **Distinct keys do not share a cache bucket — no cross-actor/cross-question leak (R2.3).** A
    /// grant cached for one full key does not serve a DIFFERENT full key's hiccup. The cache compares
    /// the whole `(tenant, region, subject, permission@object)` string by value (never a 64-bit
    /// digest), so a different subject or a different question can never replay another's cached
    /// ALLOW — even while that ALLOW is live-and-stale in the cache (the cross-actor authz leak the
    /// full-key comparison shuts).
    #[test]
    fn distinct_keys_do_not_share_a_cache_bucket() {
        let fs = authz_at(1_000);
        let _ = fs.serve("acme|eu|alice|read@doc:1", &bounded_stale(), false, allow);
        fs.clock().advance(31);
        // Alice's OWN grant is live-and-stale right now (proving the cache is NOT simply empty) …
        let alice = fs.serve("acme|eu|alice|read@doc:1", &bounded_stale(), false, hiccup);
        assert!(
            alice.is_allow() && alice.is_degraded(),
            "alice's stale ALLOW is live in the cache"
        );
        // … yet a different subject / question / tenant has no cached grant → a hiccup fails closed.
        let other = fs.serve("acme|eu|bob|read@doc:1", &bounded_stale(), false, hiccup);
        assert!(other.is_deny(), "bob must not borrow alice's cached grant");
        let other_q = fs.serve("acme|eu|alice|write@doc:1", &bounded_stale(), false, hiccup);
        assert!(
            other_q.is_deny(),
            "a different question must not borrow the read grant"
        );
    }

    /// **The survival signals are exposed (architecture §10.2 row 6).** The SUB-D4 drill reads the
    /// fresh/stale/closed ratio + the staleness age off this; the staleness never exceeds the budget.
    #[test]
    fn survival_signals_are_exposed_and_bounded() {
        let fs = authz_at(1_000);
        let _ = fs.serve("k", &bounded_stale(), false, allow); // fresh
        fs.clock().advance(31);
        let _ = fs.serve("k", &bounded_stale(), false, hiccup); // stale
        fs.clock().advance(300); // age now > static_max
        let _ = fs.serve("k", &bounded_stale(), false, hiccup); // closed
        let s = fs.signals();
        assert_eq!(s.fresh, 1, "one fresh answer");
        assert_eq!(
            s.stale, 1,
            "one stale (degraded) answer — the survival rung"
        );
        assert_eq!(s.closed, 1, "one fail-closed answer past the window");
        assert!(
            s.last_staleness_secs <= fs.static_max(),
            "staleness ≤ the budget (≤ revocation SLA)"
        );
    }

    /// **`AuthzDecision` classifiers are exact per rung** (kills the `is_allow`/`is_deny`/
    /// `is_degraded → true|false` mutants: a flattened classifier would mis-label the survival
    /// buckets the drill asserts).
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

    /// **`CoarseAuthz::of` carries the exact decision** (kills a constant-decision mutant: a cache
    /// that always cached `Allow` would escalate every Deny).
    #[test]
    fn coarse_authz_carries_the_exact_decision() {
        assert_eq!(CoarseAuthz::of(Decision::Deny).decision, Decision::Deny);
        assert_eq!(CoarseAuthz::of(Decision::Allow).decision, Decision::Allow);
        assert_eq!(
            CoarseAuthz::of(Decision::Conditional).decision,
            Decision::Conditional
        );
    }

    /// **The `Debug` impl prints the window bounds + the live signal counters and does NOT leak the
    /// cached coarse-grant values** (kills the Debug `fmt → Ok(default)` mutant; delegates to the
    /// inner `FailStatic` Debug, which omits the cached authz answers — they are not for the log).
    #[test]
    fn debug_shows_bounds_and_counters_not_cached_values() {
        let fs = authz_at(0);
        let _ = fs.serve("k", &bounded_stale(), false, allow); // one fresh answer (counter ticks)
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
