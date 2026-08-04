use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::thresholds::FailStaticThreshold;
use crate::{Seconds, ServeError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer<T> {
    Fresh(T),
    Static(T),
    Closed,
}

impl<T> Answer<T> {
    pub fn is_fresh(&self) -> bool {
        matches!(self, Answer::Fresh(_))
    }

    pub fn is_static(&self) -> bool {
        matches!(self, Answer::Static(_))
    }

    pub fn is_closed(&self) -> bool {
        matches!(self, Answer::Closed)
    }

    pub fn is_degraded(&self) -> bool {
        self.is_static()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StalenessBound {
    pub revocation_sla_secs: Seconds,
    pub agent_token_ttl_secs: Seconds,
}

impl StalenessBound {
    pub fn from_threshold(revocation_sla_secs: Seconds, t: &FailStaticThreshold) -> Self {
        StalenessBound {
            revocation_sla_secs,
            agent_token_ttl_secs: t.agent_token_ttl_secs,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailStaticError {
    ExceedsRevocationSla {
        static_max: Seconds,
        revocation_sla: Seconds,
    },
    BelowAgentTokenTtl {
        static_max: Seconds,
        agent_token_ttl: Seconds,
    },
    FreshExceedsStatic {
        fresh_ttl: Seconds,
        static_max: Seconds,
    },
}

impl std::fmt::Display for FailStaticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FailStaticError::ExceedsRevocationSla {
                static_max,
                revocation_sla,
            } => write!(
                f,
                "fail-static static_max ({static_max}s) > revocation SLA ({revocation_sla}s) - \
                 a revoked actor would keep working past N; rejected (architecture §8.2)"
            ),
            FailStaticError::BelowAgentTokenTtl {
                static_max,
                agent_token_ttl,
            } => write!(
                f,
                "fail-static static_max ({static_max}s) < agent-token TTL ({agent_token_ttl}s) - \
                 the window must contain the short-lived agent token; rejected (architecture §8.2)"
            ),
            FailStaticError::FreshExceedsStatic {
                fresh_ttl,
                static_max,
            } => write!(
                f,
                "fail-static fresh_ttl ({fresh_ttl}s) > static_max ({static_max}s) - \
                 the Fresh→Static→Closed ladder must be monotone; rejected"
            ),
        }
    }
}

impl std::error::Error for FailStaticError {}

pub trait Clock: Send + Sync {
    fn now_secs(&self) -> u64;
}

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

#[derive(Debug, Default)]
pub struct TestClock {
    now: AtomicU64,
}

impl TestClock {
    pub fn at(t0: u64) -> Self {
        TestClock {
            now: AtomicU64::new(t0),
        }
    }

    pub fn advance(&self, secs: u64) {
        self.now.fetch_add(secs, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_secs(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

struct Entry<T> {
    value: T,
    cached_at_secs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FailStaticSignals {
    pub fresh: u64,
    pub stale: u64,
    pub closed: u64,
    pub last_staleness_secs: u64,
}

impl FailStaticSignals {
    pub fn total(&self) -> u64 {
        self.fresh + self.stale + self.closed
    }

    pub fn fresh_ratio_pct(&self) -> Option<u64> {
        (self.fresh * 100).checked_div(self.total())
    }
}

pub struct FailStatic<K, T, C: Clock = SystemClock> {
    fresh_ttl: Seconds,
    static_max: Seconds,
    clock: C,
    cache: Mutex<HashMap<K, Entry<T>>>,
    fresh_count: AtomicU64,
    stale_count: AtomicU64,
    closed_count: AtomicU64,
    last_staleness_secs: AtomicU64,
}

impl<K, T, C: Clock> std::fmt::Debug for FailStatic<K, T, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FailStatic")
            .field("fresh_ttl", &self.fresh_ttl)
            .field("static_max", &self.static_max)
            .field("fresh", &self.fresh_count.load(Ordering::SeqCst))
            .field("stale", &self.stale_count.load(Ordering::SeqCst))
            .field("closed", &self.closed_count.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl<K: Hash + Eq, T: Clone> FailStatic<K, T, SystemClock> {
    pub fn try_new(
        fresh_ttl: Seconds,
        static_max: Seconds,
        bound: StalenessBound,
    ) -> Result<Self, FailStaticError> {
        Self::try_new_with_clock(fresh_ttl, static_max, bound, SystemClock)
    }
}

impl<K: Hash + Eq, T: Clone, C: Clock> FailStatic<K, T, C> {
    pub fn try_new_with_clock(
        fresh_ttl: Seconds,
        static_max: Seconds,
        bound: StalenessBound,
        clock: C,
    ) -> Result<Self, FailStaticError> {
        if static_max > bound.revocation_sla_secs {
            return Err(FailStaticError::ExceedsRevocationSla {
                static_max,
                revocation_sla: bound.revocation_sla_secs,
            });
        }
        if static_max < bound.agent_token_ttl_secs {
            return Err(FailStaticError::BelowAgentTokenTtl {
                static_max,
                agent_token_ttl: bound.agent_token_ttl_secs,
            });
        }
        if fresh_ttl > static_max {
            return Err(FailStaticError::FreshExceedsStatic {
                fresh_ttl,
                static_max,
            });
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

    pub fn clock(&self) -> &C {
        &self.clock
    }

    pub fn fresh_ttl(&self) -> Seconds {
        self.fresh_ttl
    }

    pub fn static_max(&self) -> Seconds {
        self.static_max
    }

    pub fn get(&self, key: K, refresh: impl Fn() -> Result<T, ServeError>) -> Answer<T> {
        match refresh() {
            Ok(value) => {
                let now = self.clock.now_secs();
                {
                    let mut cache = self.cache.lock().expect("fail-static cache poisoned");
                    cache.insert(
                        key,
                        Entry {
                            value: value.clone(),
                            cached_at_secs: now,
                        },
                    );
                }
                self.fresh_count.fetch_add(1, Ordering::SeqCst);
                self.last_staleness_secs.store(0, Ordering::SeqCst);
                Answer::Fresh(value)
            }
            Err(_hiccup) => self.serve_from_cache(key, &refresh),
        }
    }

    fn serve_from_cache(
        &self,
        key: K,
        refresh: &impl Fn() -> Result<T, ServeError>,
    ) -> Answer<T> {
        let now = self.clock.now_secs();
        let cache = self.cache.lock().expect("fail-static cache poisoned");
        let Some(entry) = cache.get(&key) else {
            drop(cache);
            self.closed_count.fetch_add(1, Ordering::SeqCst);
            return Answer::Closed;
        };
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
            self.try_background_refresh(key, refresh);
            self.stale_count.fetch_add(1, Ordering::SeqCst);
            self.last_staleness_secs.store(age, Ordering::SeqCst);
            Answer::Static(value)
        } else {
            drop(cache);
            self.closed_count.fetch_add(1, Ordering::SeqCst);
            Answer::Closed
        }
    }

    fn try_background_refresh(&self, key: K, refresh: &impl Fn() -> Result<T, ServeError>) {
        if let Ok(value) = refresh() {
            let now = self.clock.now_secs();
            let mut cache = self.cache.lock().expect("fail-static cache poisoned");
            cache.insert(
                key,
                Entry {
                    value,
                    cached_at_secs: now,
                },
            );
        }
    }

    pub fn signals(&self) -> FailStaticSignals {
        FailStaticSignals {
            fresh: self.fresh_count.load(Ordering::SeqCst),
            stale: self.stale_count.load(Ordering::SeqCst),
            closed: self.closed_count.load(Ordering::SeqCst),
            last_staleness_secs: self.last_staleness_secs.load(Ordering::SeqCst),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drill_bound() -> StalenessBound {
        StalenessBound {
            revocation_sla_secs: 300,
            agent_token_ttl_secs: 60,
        }
    }

    fn fail_once() -> impl Fn() -> Result<u32, ServeError> {
        || Err(ServeError("identity hiccup".into()))
    }

    #[test]
    fn constructor_rejects_static_max_over_revocation_sla() {
        let err =
            FailStatic::<&str, u32>::try_new(30, 301, drill_bound()).expect_err("must reject > SLA");
        assert_eq!(
            err,
            FailStaticError::ExceedsRevocationSla {
                static_max: 301,
                revocation_sla: 300
            }
        );
    }

    #[test]
    fn constructor_rejects_static_max_under_agent_token_ttl() {
        let err =
            FailStatic::<&str, u32>::try_new(30, 59, drill_bound()).expect_err("must reject < token TTL");
        assert_eq!(
            err,
            FailStaticError::BelowAgentTokenTtl {
                static_max: 59,
                agent_token_ttl: 60
            }
        );
    }

    #[test]
    fn constructor_rejects_fresh_over_static() {
        let err = FailStatic::<&str, u32>::try_new(200, 120, drill_bound())
            .expect_err("must reject fresh>static");
        assert_eq!(
            err,
            FailStaticError::FreshExceedsStatic {
                fresh_ttl: 200,
                static_max: 120
            }
        );
    }

    #[test]
    fn constructor_admits_at_the_exact_boundaries() {
        FailStatic::<&str, u32>::try_new(30, 300, drill_bound()).expect("== SLA is admitted (≤)");
        FailStatic::<&str, u32>::try_new(30, 60, drill_bound()).expect("== token TTL is admitted (≥)");
        FailStatic::<&str, u32>::try_new(120, 120, drill_bound()).expect("fresh==static is admitted");
    }

    #[test]
    fn fresh_within_ttl_then_stale_then_closed_at_the_boundaries() {
        let clock = TestClock::at(1_000);
        let fs = FailStatic::<&str, u32, _>::try_new_with_clock(30, 300, drill_bound(), clock)
            .expect("valid bound");

        assert_eq!(fs.get("k", || Ok(7u32)), Answer::Fresh(7));

        fs_clock(&fs).advance(30);
        assert_eq!(
            fs.get("k", fail_once()),
            Answer::Fresh(7),
            "age == fresh_ttl is fresh"
        );

        fs_clock(&fs).advance(1);
        let a = fs.get("k", fail_once());
        assert_eq!(
            a,
            Answer::Static(7),
            "age just past fresh_ttl is degraded-stale"
        );
        assert!(a.is_degraded());

        fs_clock(&fs).advance(300 - 31);
        assert_eq!(
            fs.get("k", fail_once()),
            Answer::Static(7),
            "age == static_max is stale"
        );

        fs_clock(&fs).advance(1);
        assert_eq!(
            fs.get("k", fail_once()),
            Answer::Closed,
            "past static_max is closed"
        );
    }

    #[test]
    fn never_fails_open_with_no_cached_value() {
        let fs = FailStatic::<&str, u32>::try_new(30, 300, drill_bound()).expect("valid");
        assert_eq!(fs.get("cold", fail_once()), Answer::Closed);
    }

    #[test]
    fn stale_serves_the_last_known_good_never_escalates() {
        let clock = TestClock::at(0);
        let fs =
            FailStatic::<&str, u32, _>::try_new_with_clock(10, 100, drill_bound(), clock).expect("valid");
        assert_eq!(fs.get("k", || Ok(42u32)), Answer::Fresh(42));
        fs_clock(&fs).advance(50);
        match fs.get("k", fail_once()) {
            Answer::Static(v) => {
                assert_eq!(v, 42, "stale serves the cached value, not an escalation")
            }
            other => panic!("expected Static(42), got {other:?}"),
        }
    }

    #[test]
    fn stale_while_revalidate_refreshes_when_upstream_recovers() {
        let clock = TestClock::at(0);
        let fs =
            FailStatic::<&str, u32, _>::try_new_with_clock(10, 100, drill_bound(), clock).expect("valid");
        assert_eq!(fs.get("k", || Ok(1u32)), Answer::Fresh(1));
        fs_clock(&fs).advance(50);
        assert_eq!(fs.get("k", || Ok(2u32)), Answer::Fresh(2));
        assert_eq!(
            fs.get("k", fail_once()),
            Answer::Fresh(2),
            "re-stamped cache is fresh"
        );
    }

    #[test]
    fn signals_count_fresh_stale_closed_and_staleness_age() {
        let clock = TestClock::at(0);
        let fs =
            FailStatic::<&str, u32, _>::try_new_with_clock(10, 100, drill_bound(), clock).expect("valid");
        assert_eq!(fs.get("k", || Ok(5u32)), Answer::Fresh(5));
        fs_clock(&fs).advance(50);
        assert_eq!(fs.get("k", fail_once()), Answer::Static(5));
        fs_clock(&fs).advance(60);
        assert_eq!(fs.get("k", fail_once()), Answer::Closed);

        let s = fs.signals();
        assert_eq!(s.fresh, 1, "one fresh answer");
        assert_eq!(s.stale, 1, "one stale answer");
        assert_eq!(s.closed, 1, "one closed answer");
        assert_eq!(s.total(), 3);
        assert_eq!(
            s.last_staleness_secs, 50,
            "last stale age == 50 (≤ static_max)"
        );
        assert!(
            s.last_staleness_secs <= fs.static_max(),
            "staleness never exceeds the budget"
        );
        assert_eq!(s.fresh_ratio_pct(), Some(33), "1/3 fresh ≈ 33%");
    }

    #[test]
    fn fresh_ratio_is_absent_before_any_answer() {
        let s = FailStaticSignals::default();
        assert_eq!(
            s.fresh_ratio_pct(),
            None,
            "no ratio over zero answers (never a fabricated 100)"
        );
        assert_eq!(s.total(), 0);
    }

    #[test]
    fn answer_classifiers_are_exact_per_rung() {
        let fresh: Answer<u8> = Answer::Fresh(1);
        let stale: Answer<u8> = Answer::Static(1);
        let closed: Answer<u8> = Answer::Closed;

        assert!(
            fresh.is_fresh() && !fresh.is_static() && !fresh.is_closed() && !fresh.is_degraded()
        );
        assert!(
            !stale.is_fresh() && stale.is_static() && !stale.is_closed() && stale.is_degraded()
        );
        assert!(
            !closed.is_fresh()
                && !closed.is_static()
                && closed.is_closed()
                && !closed.is_degraded()
        );
    }

    #[test]
    fn fresh_ratio_is_multiplicative_not_additive() {
        let s = FailStaticSignals {
            fresh: 3,
            stale: 1,
            closed: 0,
            last_staleness_secs: 0,
        };
        assert_eq!(
            s.fresh_ratio_pct(),
            Some(75),
            "3/4 fresh == 75% (multiplicative)"
        );
        assert_eq!(s.total(), 4);
    }

    #[test]
    fn background_refresh_restamps_the_cache_on_recovery() {
        let clock = TestClock::at(0);
        let fs =
            FailStatic::<&str, u32, _>::try_new_with_clock(10, 100, drill_bound(), clock).expect("valid");
        assert_eq!(fs.get("k", || Ok(1u32)), Answer::Fresh(1));
        fs_clock(&fs).advance(50);

        let calls = std::cell::Cell::new(0u32);
        let flaky = || {
            let n = calls.get();
            calls.set(n + 1);
            if n == 0 {
                Err(ServeError("foreground hiccup".into()))
            } else {
                Ok(99u32)
            }
        };
        assert_eq!(
            fs.get("k", flaky),
            Answer::Static(1),
            "served the OLD stale value to the caller"
        );
        assert_eq!(
            fs.get("k", fail_once()),
            Answer::Fresh(99),
            "background refresh re-stamped to 99"
        );
    }

    #[test]
    fn distinct_keys_do_not_share_a_cache_bucket() {
        let fs = FailStatic::<&str, u32>::try_new(30, 300, drill_bound()).expect("valid");
        assert_eq!(fs.get("actor:alice", || Ok(1u32)), Answer::Fresh(1));
        assert_eq!(
            fs.get("actor:bob", fail_once()),
            Answer::Closed,
            "bob has no cache of his own - must NOT borrow alice's bucket"
        );
    }

    #[test]
    fn colliding_hash_distinct_keys_never_alias_each_others_cached_answer() {
        #[derive(Clone, Debug, PartialEq, Eq)]
        struct Collide(&'static str);
        impl std::hash::Hash for Collide {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                0u8.hash(state);
            }
        }

        let fs = FailStatic::<Collide, u32>::try_new(30, 300, drill_bound()).expect("valid");
        let alice = Collide("principal:alice|read@doc:1");
        let bob = Collide("principal:bob|read@doc:1");
        assert_ne!(alice, bob, "the two keys are DISTINCT by Eq");

        assert_eq!(fs.get(alice.clone(), || Ok(1u32)), Answer::Fresh(1));

        let bob_answer = fs.get(bob, fail_once());
        assert_eq!(
            bob_answer,
            Answer::Closed,
            "a hash-colliding distinct key must MISS (Closed) - never borrow the other key's entry"
        );
        assert!(
            !matches!(bob_answer, Answer::Fresh(_) | Answer::Static(_)),
            "a colliding key must NEVER observe another key's cached answer (the R2.3 leak)"
        );

        assert_eq!(
            fs.get(alice, fail_once()),
            Answer::Fresh(1),
            "the real key still resolves to its own entry after the colliding lookup"
        );
    }

    #[test]
    fn test_clock_starts_at_the_given_offset() {
        let c = TestClock::at(777);
        assert_eq!(c.now_secs(), 777);
        c.advance(3);
        assert_eq!(c.now_secs(), 780);
    }

    #[test]
    fn error_display_names_the_violated_bound() {
        let e = FailStaticError::ExceedsRevocationSla {
            static_max: 301,
            revocation_sla: 300,
        };
        let m = e.to_string();
        assert!(
            m.contains("301") && m.contains("300") && m.contains("revocation"),
            "got: {m}"
        );
        let e = FailStaticError::BelowAgentTokenTtl {
            static_max: 5,
            agent_token_ttl: 60,
        };
        assert!(
            e.to_string().contains("agent-token"),
            "names the agent-token bound"
        );
        let e = FailStaticError::FreshExceedsStatic {
            fresh_ttl: 9,
            static_max: 8,
        };
        assert!(
            e.to_string().contains("monotone"),
            "names the ladder violation"
        );
    }

    #[test]
    fn debug_shows_bounds_and_counters_not_cached_values() {
        let clock = TestClock::at(0);
        let fs =
            FailStatic::<&str, u32, _>::try_new_with_clock(10, 100, drill_bound(), clock).expect("valid");
        assert_eq!(fs.get("k", || Ok(57005_u32)), Answer::Fresh(57005));
        let dbg = format!("{fs:?}");
        assert!(
            dbg.contains("fresh_ttl"),
            "prints the fresh_ttl bound: {dbg}"
        );
        assert!(
            dbg.contains("static_max"),
            "prints the static_max bound: {dbg}"
        );
        assert!(dbg.contains("10"), "prints the fresh_ttl value (10): {dbg}");
        assert!(
            dbg.contains("100"),
            "prints the static_max value (100): {dbg}"
        );
        assert!(
            !dbg.contains("57005"),
            "the cached coarse-grant value must NOT leak into Debug: {dbg}"
        );
    }

    #[test]
    fn system_clock_returns_real_wall_seconds() {
        let c = SystemClock;
        let a = c.now_secs();
        assert!(
            a > 1_577_836_800,
            "SystemClock reads real wall time, got {a}"
        );
        let b = c.now_secs();
        assert!(b >= a, "wall time does not run backwards across two reads");
    }

    fn fs_clock<K, T: Clone>(fs: &FailStatic<K, T, TestClock>) -> &TestClock {
        &fs.clock
    }
}
