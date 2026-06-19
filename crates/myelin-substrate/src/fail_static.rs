//! `FailStatic<T>` — the bounded-staleness fail-static mechanism (P-S18; contract 1.10 / 4.11).
//!
//! CANON:
//!   - `planning/05-refined-shared-systems-architecture/00-platform-substrate.md` §8 (fail-static —
//!     distinguish fail-CLOSED from fail-STATIC; `FailStatic<T>{ fresh_ttl, static_max }`;
//!     `Answer<T> = Fresh | Static(degraded) | Closed`; stale-while-revalidate; **never fail open**)
//!     and §8.2 (the staleness bound `static_max ≤ revocation-SLA ≥ agent-token-TTL`; the VALUE W is
//!     `[OPEN — LEGAL]`, DPO-ratified, L-1) and §8.3 (composes with readiness, §4.3: fail-static
//!     handles a TRANSIENT hiccup, readiness a SUSTAINED outage).
//!   - `planning/05-refined-shared-systems-architecture/contract-index.md` rows 1.10
//!     (`FailStatic<T>`), 4.11 (the Id-usage bound; `[OPEN — LEGAL]` ratification),
//!     1.8 / §10.2 row 6 (the fresh/stale/closed ratio + staleness-age survival signal).
//!   - `external-insights/01-process-and-quality-doctrine.md` §2 (a shared-dependency cascade is a
//!     platform-wide kill — fail-static, not fail-closed, is the AVAILABILITY default) and §3 (name
//!     the floors; the value W stays `[OPEN — LEGAL]` honestly).
//!
//! THE DISTINCTION (§8, the whole point). Fail-CLOSED is the right *authorization-correctness*
//! default: deny when you cannot prove a grant (ADR-03). Fail-STATIC is the right *availability*
//! default: on a TRANSIENT hiccup of a shared critical dependency (Identity), serve a
//! bounded-staleness *cached* answer so already-authenticated traffic keeps working — rather than
//! failing every request closed and turning one shared dependency into a whole-platform cascade.
//! The static answer is the coarse "actor still active / coarse grants" — it is NEVER an escalation
//! of access (we never fail *open*); past the staleness budget, deny is correct again (`Closed`).
//!
//! FLOOR (named, EI-01 §3): the VALUE of `static_max` (W) is `[OPEN — LEGAL]` — the DPO ratifies it
//! (L-1; it is the residual GDPR-revocation exposure window). The MECHANISM and the
//! `≤ revocation-SLA ≥ agent-token-TTL` constraint ship in M0 REGARDLESS (here); the constraint is
//! enforced structurally in the constructor ([`FailStatic::try_new`]). Fail-static is PROVEN against
//! a real Identity hiccup in **M1 (P-S25, SUB-D4)** — this prompt builds + unit-drills the mechanism
//! at its boundaries only (named, not skipped). P-S25 (global P-087) lands the authz read-path
//! wiring ([`crate::fail_static_authz::FailStaticAuthz`]) + the SUB-D4 chained-sequence drill
//! against the P-S03 dependency-break injector.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::thresholds::FailStaticThreshold;
use crate::{Seconds, ServeError};

/// The fail-static answer (architecture §8; contract 1.10). Fail-static is the correct
/// AVAILABILITY default on a transient dependency hiccup; we NEVER fail open — the `Static`
/// answer carries the *previously-cached* value (the coarse "actor still active / coarse grants"),
/// never a fabricated escalation of access, and once the staleness budget is exhausted the answer
/// is `Closed` (deny), never an open fall-through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer<T> {
    /// Served within `fresh_ttl` — a fresh upstream value (either a successful live read, or a
    /// cached value still inside its freshness window).
    Fresh(T),
    /// Served STALE: between `fresh_ttl` and `static_max` the upstream hiccupped, so the last
    /// known-good cached value is served with a **degraded** marker (and a background refresh is
    /// triggered, stale-while-revalidate). This is the bounded-staleness availability win.
    Static(T),
    /// Past `static_max` (the staleness budget is exhausted) OR no cached value exists to fall back
    /// on — **fail closed** (deny is now correct). We NEVER fall through to open.
    Closed,
}

impl<T> Answer<T> {
    /// Is this the fresh rung? (used by the signal/telemetry classification + tests).
    pub fn is_fresh(&self) -> bool {
        matches!(self, Answer::Fresh(_))
    }

    /// Is this the degraded-but-served (stale) rung?
    pub fn is_static(&self) -> bool {
        matches!(self, Answer::Static(_))
    }

    /// Is this the fail-closed rung (the staleness budget is spent / no fallback)?
    pub fn is_closed(&self) -> bool {
        matches!(self, Answer::Closed)
    }

    /// `true` exactly when the answer is degraded (stale) — the marker a caller surfaces to
    /// degrade gracefully (architecture §8: "serve stale + mark degraded").
    pub fn is_degraded(&self) -> bool {
        self.is_static()
    }
}

/// The two-sided staleness bound (architecture §8.2; contract 4.11). The fail-static window must sit
/// UNDER the revocation SLA (a revoked actor still falls inside the window and is denied once it
/// closes) and OVER the short-lived agent-token TTL (an agent token whose life == its run life
/// expires inside the window). Both bounds are read from the versioned thresholds file (P-S22) — the
/// VALUE W is `[OPEN — LEGAL]`, but the *constraint* ships regardless and is enforced here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StalenessBound {
    /// `static_max ≤ revocation_sla_secs` (the upper bound — N, the deprovision SLA).
    pub revocation_sla_secs: Seconds,
    /// `static_max ≥ agent_token_ttl_secs` (the lower bound — the window must CONTAIN the
    /// short-lived agent token, ID-1 / GD-3 / ADR-17).
    pub agent_token_ttl_secs: Seconds,
}

impl StalenessBound {
    /// Build the bound straight from the `[fail_static]` row of the thresholds file. Reads the
    /// revocation-SLA upper bound and the agent-token-TTL lower bound (both in seconds).
    pub fn from_threshold(revocation_sla_secs: Seconds, t: &FailStaticThreshold) -> Self {
        StalenessBound {
            revocation_sla_secs,
            agent_token_ttl_secs: t.agent_token_ttl_secs,
        }
    }
}

/// The constructor-constraint violation (architecture §8.2). A `FailStatic` whose `static_max`
/// violates `agent_token_ttl ≤ static_max ≤ revocation_sla` (or whose `fresh_ttl > static_max`)
/// **does not construct** — the bound is structural, not a runtime check skipped on the hot path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailStaticError {
    /// `static_max > revocation_sla` — the window would outlive the revocation SLA (a revoked actor
    /// would keep working past N). Carries `(static_max, revocation_sla)`.
    ExceedsRevocationSla {
        /// the rejected `static_max` (seconds).
        static_max: Seconds,
        /// the revocation SLA N (seconds) it exceeded.
        revocation_sla: Seconds,
    },
    /// `static_max < agent_token_ttl` — the window would be too short to contain the short-lived
    /// agent token. Carries `(static_max, agent_token_ttl)`.
    BelowAgentTokenTtl {
        /// the rejected `static_max` (seconds).
        static_max: Seconds,
        /// the agent-token TTL (seconds) it fell under.
        agent_token_ttl: Seconds,
    },
    /// `fresh_ttl > static_max` — the freshness window cannot exceed the staleness budget (the
    /// `Fresh → Static → Closed` ladder must be monotone). Carries `(fresh_ttl, static_max)`.
    FreshExceedsStatic {
        /// the rejected `fresh_ttl` (seconds).
        fresh_ttl: Seconds,
        /// the `static_max` (seconds) it exceeded.
        static_max: Seconds,
    },
}

impl std::fmt::Display for FailStaticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FailStaticError::ExceedsRevocationSla { static_max, revocation_sla } => write!(
                f,
                "fail-static static_max ({static_max}s) > revocation SLA ({revocation_sla}s) — \
                 a revoked actor would keep working past N; rejected (architecture §8.2)"
            ),
            FailStaticError::BelowAgentTokenTtl { static_max, agent_token_ttl } => write!(
                f,
                "fail-static static_max ({static_max}s) < agent-token TTL ({agent_token_ttl}s) — \
                 the window must contain the short-lived agent token; rejected (architecture §8.2)"
            ),
            FailStaticError::FreshExceedsStatic { fresh_ttl, static_max } => write!(
                f,
                "fail-static fresh_ttl ({fresh_ttl}s) > static_max ({static_max}s) — \
                 the Fresh→Static→Closed ladder must be monotone; rejected"
            ),
        }
    }
}

impl std::error::Error for FailStaticError {}

/// A monotonic clock seam so the staleness boundaries are deterministically drillable (the unit
/// tests advance a fake clock across the `fresh_ttl` / `static_max` boundaries; production wires the
/// real wall clock). Returns "seconds since some fixed epoch" — only DIFFERENCES are meaningful.
pub trait Clock: Send + Sync {
    /// Seconds elapsed since a fixed (implementation-defined) epoch.
    fn now_secs(&self) -> u64;
}

/// The production clock — wall time in whole seconds since the Unix epoch.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// A deterministic test clock: seconds advance only when the test advances them (the boundary
/// drills set it exactly at `fresh_ttl` and `static_max`).
#[derive(Debug, Default)]
pub struct TestClock {
    now: AtomicU64,
}

impl TestClock {
    /// A clock starting at `t0` seconds.
    pub fn at(t0: u64) -> Self {
        TestClock { now: AtomicU64::new(t0) }
    }

    /// Advance the clock by `secs` seconds (the drill steps across a boundary).
    pub fn advance(&self, secs: u64) {
        self.now.fetch_add(secs, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_secs(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

/// One cached entry: the last known-good value and the wall-second it was cached at.
struct Entry<T> {
    value: T,
    cached_at_secs: u64,
}

/// Producer-side contract-1.8 signal counters (architecture §10.2 row 6: "Fail-static: fresh/stale/
/// closed answer ratio, staleness age"). A snapshot is read back via [`FailStatic::signals`]; the
/// SUB-D4 drill (P-S25) asserts against these. Exporting them is part of the pass condition (EI-01
/// §3: "observability is part of the pass").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FailStaticSignals {
    /// count of `Fresh` answers served.
    pub fresh: u64,
    /// count of `Static` (degraded) answers served.
    pub stale: u64,
    /// count of `Closed` (fail-closed) answers served.
    pub closed: u64,
    /// the staleness age (seconds) of the MOST-RECENT `Static` answer — 0 when the last answer was
    /// not stale. The drill asserts this never exceeds `static_max` (≤ the revocation SLA).
    pub last_staleness_secs: u64,
}

impl FailStaticSignals {
    /// Total answers served (the ratio denominator).
    pub fn total(&self) -> u64 {
        self.fresh + self.stale + self.closed
    }

    /// The fresh-answer ratio as an integer percentage (0..=100), or `None` before any answer (no
    /// ratio is defined over zero answers — an empty ratio is honestly absent, never a fabricated
    /// 100). This is the `{answer_class=fresh}` bucket the §10.2-row-6 ratio signal feeds.
    pub fn fresh_ratio_pct(&self) -> Option<u64> {
        // checked_div returns None over a zero denominator (no ratio over zero answers — never a
        // fabricated 100); the numerator is the fresh count scaled to a percentage.
        (self.fresh * 100).checked_div(self.total())
    }
}

/// The bounded-staleness fail-static cache (architecture §8; contract 1.10; ADR-17).
///
/// `get(key, refresh)` calls the upstream `refresh`; on success it caches and returns
/// [`Answer::Fresh`]; on a hiccup it falls back to the last cached value, returning [`Answer::Fresh`]
/// while inside `fresh_ttl`, [`Answer::Static`] (degraded, background-refresh) while inside
/// `static_max`, and [`Answer::Closed`] once the staleness budget is spent or no fallback exists —
/// **never fail open**.
///
/// Units (frozen, §2.10): `fresh_ttl` / `static_max` are **seconds**. The constructor enforces
/// `agent_token_ttl ≤ static_max ≤ revocation_sla` (architecture §8.2) — a violating value does not
/// construct. The VALUE of `static_max` (W) is `[OPEN — LEGAL]`; the DPO ratifies it (L-1).
pub struct FailStatic<T, C: Clock = SystemClock> {
    /// serve fresh within this (seconds).
    fresh_ttl: Seconds,
    /// serve STALE (degraded marker) up to here on a hiccup (seconds); ≤ revocation SLA,
    /// ≥ agent-token TTL.
    static_max: Seconds,
    clock: C,
    cache: Mutex<HashMap<u64, Entry<T>>>,
    // Hash of the actual key → bucket; we key the cache by the key's hash so `K` need not be stored.
    fresh_count: AtomicU64,
    stale_count: AtomicU64,
    closed_count: AtomicU64,
    last_staleness_secs: AtomicU64,
}

impl<T, C: Clock> std::fmt::Debug for FailStatic<T, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately does NOT print the cached values (they are the coarse authz answers —
        // not for the log). Only the window bounds + the signal counters.
        f.debug_struct("FailStatic")
            .field("fresh_ttl", &self.fresh_ttl)
            .field("static_max", &self.static_max)
            .field("fresh", &self.fresh_count.load(Ordering::SeqCst))
            .field("stale", &self.stale_count.load(Ordering::SeqCst))
            .field("closed", &self.closed_count.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl<T: Clone> FailStatic<T, SystemClock> {
    /// Construct against the wall clock, enforcing the §8.2 staleness bound. The `static_max` value
    /// is the DPO-ratified W (read from the thresholds file by the caller); a value violating
    /// `agent_token_ttl ≤ static_max ≤ revocation_sla` (or `fresh_ttl > static_max`) is REJECTED.
    pub fn try_new(
        fresh_ttl: Seconds,
        static_max: Seconds,
        bound: StalenessBound,
    ) -> Result<Self, FailStaticError> {
        Self::try_new_with_clock(fresh_ttl, static_max, bound, SystemClock)
    }
}

impl<T: Clone, C: Clock> FailStatic<T, C> {
    /// Construct against an injected clock (the boundary drills use a [`TestClock`]). Enforces the
    /// §8.2 staleness bound structurally — the only place `static_max` is validated, so a bad value
    /// cannot reach the hot path.
    pub fn try_new_with_clock(
        fresh_ttl: Seconds,
        static_max: Seconds,
        bound: StalenessBound,
        clock: C,
    ) -> Result<Self, FailStaticError> {
        // Upper bound: static_max ≤ revocation SLA (a revoked actor must be denied within N).
        if static_max > bound.revocation_sla_secs {
            return Err(FailStaticError::ExceedsRevocationSla {
                static_max,
                revocation_sla: bound.revocation_sla_secs,
            });
        }
        // Lower bound: static_max ≥ agent-token TTL (the window must contain the short-lived token).
        if static_max < bound.agent_token_ttl_secs {
            return Err(FailStaticError::BelowAgentTokenTtl {
                static_max,
                agent_token_ttl: bound.agent_token_ttl_secs,
            });
        }
        // Monotone ladder: fresh_ttl ≤ static_max.
        if fresh_ttl > static_max {
            return Err(FailStaticError::FreshExceedsStatic { fresh_ttl, static_max });
        }
        Ok(FailStatic {
            fresh_ttl,
            static_max,
            clock,
            cache: Mutex::new(HashMap::new()),
            fresh_count: AtomicU64::new(0),
            stale_count: AtomicU64::new(0),
            closed_count: AtomicU64::new(0),
            last_staleness_secs: AtomicU64::new(0),
        })
    }

    /// A borrow of the injected clock — used by drills/CDC to advance a [`TestClock`] across the
    /// staleness boundaries from outside the crate. The production `SystemClock` exposes no mutators,
    /// so this leaks no control over wall time.
    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// The freshness window (seconds).
    pub fn fresh_ttl(&self) -> Seconds {
        self.fresh_ttl
    }

    /// The staleness budget (seconds) — the bounded exposure window W.
    pub fn static_max(&self) -> Seconds {
        self.static_max
    }

    /// Serve `key`, refreshing from upstream when possible (architecture §8).
    ///
    /// - upstream succeeds → cache the value, return [`Answer::Fresh`].
    /// - upstream hiccups (refresh `Err`) and a cached value exists:
    ///   - cached age `≤ fresh_ttl` → [`Answer::Fresh`] (still inside freshness; no degradation).
    ///   - `fresh_ttl < age ≤ static_max` → [`Answer::Static`] (degraded + a background refresh is
    ///     triggered, stale-while-revalidate).
    ///   - `age > static_max` → [`Answer::Closed`] (staleness budget spent; deny is correct).
    /// - upstream hiccups and NO cached value exists → [`Answer::Closed`] (we never fabricate an
    ///   answer; **never fail open**).
    pub fn get<K: Hash>(&self, key: K, refresh: impl Fn() -> Result<T, ServeError>) -> Answer<T> {
        let bucket = hash_key(&key);
        match refresh() {
            Ok(value) => {
                let now = self.clock.now_secs();
                {
                    let mut cache = self.cache.lock().expect("fail-static cache poisoned");
                    cache.insert(bucket, Entry { value: value.clone(), cached_at_secs: now });
                }
                self.fresh_count.fetch_add(1, Ordering::SeqCst);
                self.last_staleness_secs.store(0, Ordering::SeqCst);
                Answer::Fresh(value)
            }
            Err(_hiccup) => self.serve_from_cache(bucket, &refresh),
        }
    }

    /// The hiccup path: fall back to the last known-good cached value within the bounded-staleness
    /// budget — NEVER fabricate or escalate. Factored out so the boundary logic is one testable unit.
    fn serve_from_cache(
        &self,
        bucket: u64,
        refresh: &impl Fn() -> Result<T, ServeError>,
    ) -> Answer<T> {
        let now = self.clock.now_secs();
        let cache = self.cache.lock().expect("fail-static cache poisoned");
        let Some(entry) = cache.get(&bucket) else {
            // No fallback to serve → fail CLOSED (never open).
            drop(cache);
            self.closed_count.fetch_add(1, Ordering::SeqCst);
            return Answer::Closed;
        };
        // saturating: a clock that went backwards is treated as zero age, never as a negative that
        // would wrap to a huge age and spuriously fail closed.
        let age = now.saturating_sub(entry.cached_at_secs);
        if age <= self.fresh_ttl {
            let value = entry.value.clone();
            drop(cache);
            self.fresh_count.fetch_add(1, Ordering::SeqCst);
            self.last_staleness_secs.store(0, Ordering::SeqCst);
            Answer::Fresh(value)
        } else if age <= self.static_max {
            let value = entry.value.clone();
            drop(cache);
            // stale-while-revalidate: trigger a single background refresh attempt. On this floor the
            // "background" refresh is best-effort + synchronous-but-result-discarded (the caller has
            // already been handed the stale value); the P-S25 authz wiring rides this synchronous
            // best-effort refresh, and a real ASYNC refresh task lands with the production runtime
            // (a `tokio` task pool) when the service shells carry one. We DO attempt it so a
            // recovered upstream re-freshes the cache.
            self.try_background_refresh(bucket, refresh);
            self.stale_count.fetch_add(1, Ordering::SeqCst);
            self.last_staleness_secs.store(age, Ordering::SeqCst);
            Answer::Static(value)
        } else {
            drop(cache);
            // staleness budget exhausted → fail CLOSED (deny is correct now). NEVER open.
            self.closed_count.fetch_add(1, Ordering::SeqCst);
            Answer::Closed
        }
    }

    /// Best-effort background re-fetch (stale-while-revalidate). If the upstream has recovered, the
    /// cache is refreshed so the NEXT read is fresh again; if it is still hiccupping, nothing
    /// changes (the staleness clock keeps running toward `static_max`). The caller's answer is NOT
    /// affected — they already hold the stale value.
    fn try_background_refresh(&self, bucket: u64, refresh: &impl Fn() -> Result<T, ServeError>) {
        if let Ok(value) = refresh() {
            let now = self.clock.now_secs();
            let mut cache = self.cache.lock().expect("fail-static cache poisoned");
            cache.insert(bucket, Entry { value, cached_at_secs: now });
        }
    }

    /// A snapshot of the contract-1.8 fresh/stale/closed counters + the last staleness age
    /// (architecture §10.2 row 6). The SUB-D4 drill (P-S25) asserts against this.
    pub fn signals(&self) -> FailStaticSignals {
        FailStaticSignals {
            fresh: self.fresh_count.load(Ordering::SeqCst),
            stale: self.stale_count.load(Ordering::SeqCst),
            closed: self.closed_count.load(Ordering::SeqCst),
            last_staleness_secs: self.last_staleness_secs.load(Ordering::SeqCst),
        }
    }
}

/// Hash a key into the cache bucket (so the cache need not store `K`, only `T`).
fn hash_key<K: Hash>(key: &K) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bound the drills construct against: agent-token TTL = 60s (lower), revocation SLA = 300s
    /// (upper, == thresholds.toml revocation 5min). `static_max = 300` is the engineering seed (the
    /// largest the constraint admits); the ratified W is `[OPEN — LEGAL]`.
    fn drill_bound() -> StalenessBound {
        StalenessBound { revocation_sla_secs: 300, agent_token_ttl_secs: 60 }
    }

    fn fail_once() -> impl Fn() -> Result<u32, ServeError> {
        || Err(ServeError("identity hiccup".into()))
    }

    // ----- constructor constraint (architecture §8.2) -----

    #[test]
    fn constructor_rejects_static_max_over_revocation_sla() {
        // static_max (301) > revocation SLA (300) → REJECTED (a revoked actor would outlive N).
        let err =
            FailStatic::<u32>::try_new(30, 301, drill_bound()).expect_err("must reject > SLA");
        assert_eq!(
            err,
            FailStaticError::ExceedsRevocationSla { static_max: 301, revocation_sla: 300 }
        );
    }

    #[test]
    fn constructor_rejects_static_max_under_agent_token_ttl() {
        // static_max (59) < agent-token TTL (60) → REJECTED (the window must contain the token).
        let err =
            FailStatic::<u32>::try_new(30, 59, drill_bound()).expect_err("must reject < token TTL");
        assert_eq!(
            err,
            FailStaticError::BelowAgentTokenTtl { static_max: 59, agent_token_ttl: 60 }
        );
    }

    #[test]
    fn constructor_rejects_fresh_over_static() {
        // fresh_ttl (200) > static_max (120) → REJECTED (the ladder must be monotone).
        let err =
            FailStatic::<u32>::try_new(200, 120, drill_bound()).expect_err("must reject fresh>static");
        assert_eq!(
            err,
            FailStaticError::FreshExceedsStatic { fresh_ttl: 200, static_max: 120 }
        );
    }

    #[test]
    fn constructor_admits_at_the_exact_boundaries() {
        // static_max == revocation SLA (300) AND == a value ≥ agent-token TTL: admitted (≤ / ≥).
        FailStatic::<u32>::try_new(30, 300, drill_bound()).expect("== SLA is admitted (≤)");
        // static_max == agent-token TTL (60): admitted (≥ is inclusive).
        FailStatic::<u32>::try_new(30, 60, drill_bound()).expect("== token TTL is admitted (≥)");
        // fresh_ttl == static_max: admitted (≤ is inclusive).
        FailStatic::<u32>::try_new(120, 120, drill_bound()).expect("fresh==static is admitted");
    }

    // ----- the get() boundaries: fresh / stale / closed (architecture §8) -----

    #[test]
    fn fresh_within_ttl_then_stale_then_closed_at_the_boundaries() {
        let clock = TestClock::at(1_000);
        let fs = FailStatic::<u32, _>::try_new_with_clock(30, 300, drill_bound(), clock)
            .expect("valid bound");

        // 1) a successful read caches + returns Fresh.
        assert_eq!(fs.get("k", || Ok(7u32)), Answer::Fresh(7));

        // advance to exactly fresh_ttl (age == 30) — a hiccup here is STILL fresh (age ≤ fresh_ttl).
        fs_clock(&fs).advance(30);
        assert_eq!(fs.get("k", fail_once()), Answer::Fresh(7), "age == fresh_ttl is fresh");

        // advance one past fresh_ttl (age == 31) — now STALE (degraded), still inside static_max.
        fs_clock(&fs).advance(1);
        let a = fs.get("k", fail_once());
        assert_eq!(a, Answer::Static(7), "age just past fresh_ttl is degraded-stale");
        assert!(a.is_degraded());

        // advance to exactly static_max (age == 300) — STILL served stale (age ≤ static_max).
        fs_clock(&fs).advance(300 - 31);
        assert_eq!(fs.get("k", fail_once()), Answer::Static(7), "age == static_max is stale");

        // advance one past static_max (age == 301) — fail CLOSED (budget exhausted). Never open.
        fs_clock(&fs).advance(1);
        assert_eq!(fs.get("k", fail_once()), Answer::Closed, "past static_max is closed");
    }

    #[test]
    fn never_fails_open_with_no_cached_value() {
        // a hiccup before ANY successful read → Closed (no fabricated answer; never open).
        let fs = FailStatic::<u32>::try_new(30, 300, drill_bound()).expect("valid");
        assert_eq!(fs.get("cold", fail_once()), Answer::Closed);
    }

    #[test]
    fn stale_serves_the_last_known_good_never_escalates() {
        // the static (degraded) answer is the LAST CACHED value — never a fabricated/escalated one.
        let clock = TestClock::at(0);
        let fs = FailStatic::<u32, _>::try_new_with_clock(10, 100, drill_bound(), clock)
            .expect("valid");
        assert_eq!(fs.get("k", || Ok(42u32)), Answer::Fresh(42));
        fs_clock(&fs).advance(50); // inside static_max, past fresh_ttl
        match fs.get("k", fail_once()) {
            Answer::Static(v) => assert_eq!(v, 42, "stale serves the cached value, not an escalation"),
            other => panic!("expected Static(42), got {other:?}"),
        }
    }

    #[test]
    fn stale_while_revalidate_refreshes_when_upstream_recovers() {
        let clock = TestClock::at(0);
        let fs = FailStatic::<u32, _>::try_new_with_clock(10, 100, drill_bound(), clock)
            .expect("valid");
        assert_eq!(fs.get("k", || Ok(1u32)), Answer::Fresh(1));
        fs_clock(&fs).advance(50);
        // upstream has recovered: the read returns the NEW value Fresh, AND the background path is
        // moot (the foreground refresh already succeeded → Fresh(2), cache re-stamped at now).
        assert_eq!(fs.get("k", || Ok(2u32)), Answer::Fresh(2));
        // a subsequent hiccup at the SAME time is fresh again (the cache was re-stamped).
        assert_eq!(fs.get("k", fail_once()), Answer::Fresh(2), "re-stamped cache is fresh");
    }

    // ----- the signals (contract 1.8 / §10.2 row 6) -----

    #[test]
    fn signals_count_fresh_stale_closed_and_staleness_age() {
        let clock = TestClock::at(0);
        let fs = FailStatic::<u32, _>::try_new_with_clock(10, 100, drill_bound(), clock)
            .expect("valid");
        assert_eq!(fs.get("k", || Ok(5u32)), Answer::Fresh(5)); // fresh #1
        fs_clock(&fs).advance(50);
        assert_eq!(fs.get("k", fail_once()), Answer::Static(5)); // stale #1, age 50
        fs_clock(&fs).advance(60); // age 110 > static_max(100)
        assert_eq!(fs.get("k", fail_once()), Answer::Closed); // closed #1

        let s = fs.signals();
        assert_eq!(s.fresh, 1, "one fresh answer");
        assert_eq!(s.stale, 1, "one stale answer");
        assert_eq!(s.closed, 1, "one closed answer");
        assert_eq!(s.total(), 3);
        assert_eq!(s.last_staleness_secs, 50, "last stale age == 50 (≤ static_max)");
        assert!(s.last_staleness_secs <= fs.static_max(), "staleness never exceeds the budget");
        assert_eq!(s.fresh_ratio_pct(), Some(33), "1/3 fresh ≈ 33%");
    }

    #[test]
    fn fresh_ratio_is_absent_before_any_answer() {
        let s = FailStaticSignals::default();
        assert_eq!(s.fresh_ratio_pct(), None, "no ratio over zero answers (never a fabricated 100)");
        assert_eq!(s.total(), 0);
    }

    /// The `Answer<T>` classifiers each report exactly their rung and nothing else (kills the
    /// `is_fresh`/`is_static`/`is_closed`/`is_degraded → true|false` mutants: a flattened classifier
    /// would mis-label the survival-signal buckets the §10.2-row-6 ratio is computed from).
    #[test]
    fn answer_classifiers_are_exact_per_rung() {
        let fresh: Answer<u8> = Answer::Fresh(1);
        let stale: Answer<u8> = Answer::Static(1);
        let closed: Answer<u8> = Answer::Closed;

        assert!(fresh.is_fresh() && !fresh.is_static() && !fresh.is_closed() && !fresh.is_degraded());
        assert!(!stale.is_fresh() && stale.is_static() && !stale.is_closed() && stale.is_degraded());
        assert!(!closed.is_fresh() && !closed.is_static() && closed.is_closed() && !closed.is_degraded());
    }

    /// The fresh-ratio is `fresh * 100 / total` (kills the `* → +` mutant: at 1 fresh / 2 total the
    /// correct 50% differs from `(1 + 100)/2 == 50`… so pick counts where `*` and `+` diverge).
    #[test]
    fn fresh_ratio_is_multiplicative_not_additive() {
        // 3 fresh, 1 stale → 4 total. `3*100/4 == 75`; the `+` mutant gives `(3+100)/4 == 25`.
        let s = FailStaticSignals { fresh: 3, stale: 1, closed: 0, last_staleness_secs: 0 };
        assert_eq!(s.fresh_ratio_pct(), Some(75), "3/4 fresh == 75% (multiplicative)");
        assert_eq!(s.total(), 4);
    }

    /// `try_background_refresh` actually re-stamps the cache when the upstream recovers DURING the
    /// stale window — so the NEXT read is fresh again (kills the `try_background_refresh → ()`
    /// mutant: a no-op background refresh would leave the cache stale and the next read would still
    /// be `Static`/eventually `Closed`).
    #[test]
    fn background_refresh_restamps_the_cache_on_recovery() {
        let clock = TestClock::at(0);
        let fs = FailStatic::<u32, _>::try_new_with_clock(10, 100, drill_bound(), clock)
            .expect("valid");
        assert_eq!(fs.get("k", || Ok(1u32)), Answer::Fresh(1));
        fs_clock(&fs).advance(50); // inside static_max, past fresh_ttl → stale

        // a flaky upstream: this call's FOREGROUND refresh fails (→ Static served), but we script the
        // BACKGROUND refresh to succeed by toggling on the SECOND invocation within this get().
        let calls = std::cell::Cell::new(0u32);
        let flaky = || {
            let n = calls.get();
            calls.set(n + 1);
            if n == 0 {
                Err(ServeError("foreground hiccup".into())) // the get()'s own refresh()
            } else {
                Ok(99u32) // the background-revalidate refresh()
            }
        };
        assert_eq!(fs.get("k", flaky), Answer::Static(1), "served the OLD stale value to the caller");
        // the background refresh re-stamped the cache to 99 at now(50); a hiccup at the SAME time is
        // now Fresh(99) — proving the background path ran (not a no-op).
        assert_eq!(fs.get("k", fail_once()), Answer::Fresh(99), "background refresh re-stamped to 99");
    }

    /// Distinct keys land in distinct cache buckets — a constant `hash_key` would collide every key
    /// onto one entry (kills `hash_key → 0|1`: two actors would share one cached grant, a real
    /// cross-actor authorization leak).
    #[test]
    fn distinct_keys_do_not_share_a_cache_bucket() {
        let fs = FailStatic::<u32>::try_new(30, 300, drill_bound()).expect("valid");
        assert_eq!(fs.get("actor:alice", || Ok(1u32)), Answer::Fresh(1));
        // a DIFFERENT key has no cached value → a hiccup must be Closed (never alice's value).
        assert_eq!(
            fs.get("actor:bob", fail_once()),
            Answer::Closed,
            "bob has no cache of his own — must NOT borrow alice's bucket"
        );
    }

    /// `TestClock::at(t0)` starts at exactly `t0` (kills `at → Default::default()` which would start
    /// at 0, mis-aligning every boundary advance).
    #[test]
    fn test_clock_starts_at_the_given_offset() {
        let c = TestClock::at(777);
        assert_eq!(c.now_secs(), 777);
        c.advance(3);
        assert_eq!(c.now_secs(), 780);
    }

    /// The constructor errors carry their offending numbers in the message (kills the Display
    /// `fmt → Ok(default)` mutant: an empty error message hides which bound was violated).
    #[test]
    fn error_display_names_the_violated_bound() {
        let e = FailStaticError::ExceedsRevocationSla { static_max: 301, revocation_sla: 300 };
        let m = e.to_string();
        assert!(m.contains("301") && m.contains("300") && m.contains("revocation"), "got: {m}");
        let e = FailStaticError::BelowAgentTokenTtl { static_max: 5, agent_token_ttl: 60 };
        assert!(e.to_string().contains("agent-token"), "names the agent-token bound");
        let e = FailStaticError::FreshExceedsStatic { fresh_ttl: 9, static_max: 8 };
        assert!(e.to_string().contains("monotone"), "names the ladder violation");
    }

    /// The `Debug` impl prints the window bounds + the live signal counters (kills the Debug
    /// `fmt → Ok(default)` mutant) and — load-bearing — does NOT print the cached coarse-grant
    /// values (they are authz answers, not for the log).
    #[test]
    fn debug_shows_bounds_and_counters_not_cached_values() {
        let clock = TestClock::at(0);
        let fs = FailStatic::<u32, _>::try_new_with_clock(10, 100, drill_bound(), clock)
            .expect("valid");
        // 57005 == 0xDEAD: a recognisable cached value we assert never appears in the debug output.
        assert_eq!(fs.get("k", || Ok(57005_u32)), Answer::Fresh(57005));
        let dbg = format!("{fs:?}");
        assert!(dbg.contains("fresh_ttl"), "prints the fresh_ttl bound: {dbg}");
        assert!(dbg.contains("static_max"), "prints the static_max bound: {dbg}");
        assert!(dbg.contains("10"), "prints the fresh_ttl value (10): {dbg}");
        assert!(dbg.contains("100"), "prints the static_max value (100): {dbg}");
        assert!(!dbg.contains("57005"), "the cached coarse-grant value must NOT leak into Debug: {dbg}");
    }

    /// The production `SystemClock` returns real wall-seconds — a plausibly-large value that does not
    /// go backwards across two reads (kills the `now_secs → 0|1` constant mutants: a clock pinned to
    /// 0 or 1 would make every cached entry look infinitely fresh, defeating the staleness budget).
    #[test]
    fn system_clock_returns_real_wall_seconds() {
        let c = SystemClock;
        let a = c.now_secs();
        // well past 2020 (1_577_836_800 == 2020-01-01Z) — proves it is NOT the constant 0 or 1.
        assert!(a > 1_577_836_800, "SystemClock reads real wall time, got {a}");
        let b = c.now_secs();
        assert!(b >= a, "wall time does not run backwards across two reads");
    }

    /// Helper: reach the test clock back out of a `FailStatic<T, TestClock>` to advance it.
    fn fs_clock<T: Clone>(fs: &FailStatic<T, TestClock>) -> &TestClock {
        &fs.clock
    }
}
