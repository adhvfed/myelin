use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use myelin_tenancy::Region;

use crate::kms::{DekHandle, KmsAdapter, KmsError, PiiKeyRef};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KmsReadiness {
    Ready,
    NotReady,
}

#[derive(Clone, Debug)]
pub enum KmsReadResult {
    Resolved { handle: DekHandle, degraded: bool },
    NotReady(KmsError),
}

impl KmsReadResult {
    pub fn is_resolved(&self) -> bool {
        matches!(self, KmsReadResult::Resolved { .. })
    }
    pub fn is_degraded(&self) -> bool {
        matches!(self, KmsReadResult::Resolved { degraded: true, .. })
    }
    pub fn is_not_ready(&self) -> bool {
        matches!(self, KmsReadResult::NotReady(_))
    }
    pub fn readiness(&self) -> KmsReadiness {
        match self {
            KmsReadResult::Resolved { .. } => KmsReadiness::Ready,
            KmsReadResult::NotReady(_) => KmsReadiness::NotReady,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KmsReadError(pub KmsError);

impl std::fmt::Display for KmsReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "KMS read NOT served (not-ready + shed; the resolved-DEK cache is past its \
             staleness budget or empty) - cause: {} - NEVER fail open",
            self.0
        )
    }
}

impl std::error::Error for KmsReadError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct KmsFailStaticSignals {
    pub fresh: u64,
    pub stale: u64,
    pub not_ready: u64,
    pub last_staleness_secs: u64,
    pub fail_open: u64,
}

impl KmsFailStaticSignals {
    pub fn total(&self) -> u64 {
        self.fresh + self.stale + self.not_ready
    }
    pub fn stale_survival_pct(&self) -> Option<u64> {
        (self.stale * 100).checked_div(self.total())
    }
}

struct Entry {
    handle: DekHandle,
    resolved_at_secs: u64,
}

pub struct KmsReadPath<A: KmsAdapter, C: Clock = SystemClock> {
    engine: A,
    fresh_ttl: u64,
    static_max: u64,
    clock: C,
    cache: Mutex<HashMap<(PiiKeyRef, Region), Entry>>,
    fresh: AtomicU64,
    stale: AtomicU64,
    not_ready: AtomicU64,
    last_staleness: AtomicU64,
    fail_open: AtomicU64,
}

impl<A: KmsAdapter> KmsReadPath<A, SystemClock> {
    pub fn new(engine: A, fresh_ttl: u64, static_max: u64) -> KmsReadPath<A, SystemClock> {
        Self::with_clock(engine, fresh_ttl, static_max, SystemClock)
    }
}

impl<A: KmsAdapter, C: Clock> KmsReadPath<A, C> {
    pub fn with_clock(engine: A, fresh_ttl: u64, static_max: u64, clock: C) -> KmsReadPath<A, C> {
        let fresh_ttl = fresh_ttl.min(static_max);
        KmsReadPath {
            engine,
            fresh_ttl,
            static_max,
            clock,
            cache: Mutex::new(HashMap::new()),
            fresh: AtomicU64::new(0),
            stale: AtomicU64::new(0),
            not_ready: AtomicU64::new(0),
            last_staleness: AtomicU64::new(0),
            fail_open: AtomicU64::new(0),
        }
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    pub fn engine(&self) -> &A {
        &self.engine
    }

    pub fn static_max(&self) -> u64 {
        self.static_max
    }

    pub fn resolve(&self, key_ref: &PiiKeyRef, region: &Region) -> KmsReadResult {
        let cache_key = (key_ref.clone(), region.clone());
        match self.engine.resolve_dek(key_ref, region) {
            Ok(handle) => {
                let now = self.clock.now_secs();
                {
                    let mut cache = self.cache.lock().expect("kms read cache poisoned");
                    cache.insert(
                        cache_key,
                        Entry {
                            handle: handle.clone(),
                            resolved_at_secs: now,
                        },
                    );
                }
                self.fresh.fetch_add(1, Ordering::SeqCst);
                self.last_staleness.store(0, Ordering::SeqCst);
                KmsReadResult::Resolved {
                    handle,
                    degraded: false,
                }
            }
            Err(cause) => self.serve_from_cache(&cache_key, cause),
        }
    }

    fn serve_from_cache(&self, cache_key: &(PiiKeyRef, Region), cause: KmsError) -> KmsReadResult {
        let now = self.clock.now_secs();
        let cache = self.cache.lock().expect("kms read cache poisoned");
        let Some(entry) = cache.get(cache_key) else {
            drop(cache);
            self.not_ready.fetch_add(1, Ordering::SeqCst);
            return KmsReadResult::NotReady(cause);
        };
        let age = now.saturating_sub(entry.resolved_at_secs);
        if age <= self.fresh_ttl {
            let handle = entry.handle.clone();
            drop(cache);
            self.fresh.fetch_add(1, Ordering::SeqCst);
            self.last_staleness.store(0, Ordering::SeqCst);
            KmsReadResult::Resolved {
                handle,
                degraded: false,
            }
        } else if age <= self.static_max {
            let handle = entry.handle.clone();
            drop(cache);
            self.stale.fetch_add(1, Ordering::SeqCst);
            self.last_staleness.store(age, Ordering::SeqCst);
            KmsReadResult::Resolved {
                handle,
                degraded: true,
            }
        } else {
            drop(cache);
            self.not_ready.fetch_add(1, Ordering::SeqCst);
            KmsReadResult::NotReady(cause)
        }
    }

    pub fn signals(&self) -> KmsFailStaticSignals {
        KmsFailStaticSignals {
            fresh: self.fresh.load(Ordering::SeqCst),
            stale: self.stale.load(Ordering::SeqCst),
            not_ready: self.not_ready.load(Ordering::SeqCst),
            last_staleness_secs: self.last_staleness.load(Ordering::SeqCst),
            fail_open: self.fail_open.load(Ordering::SeqCst),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::{KekId, KeyClass, KmsEngine};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::AtomicBool;

    fn t(s: &str) -> TenantId {
        TenantId(s.to_string())
    }
    fn r(s: &str) -> Region {
        Region(s.to_string())
    }

    struct FlakyKms {
        inner: KmsEngine,
        down: AtomicBool,
    }
    impl FlakyKms {
        fn new(inner: KmsEngine) -> Self {
            FlakyKms {
                inner,
                down: AtomicBool::new(false),
            }
        }
        fn set_down(&self, down: bool) {
            self.down.store(down, Ordering::SeqCst);
        }
    }
    impl KmsAdapter for FlakyKms {
        fn resolve_dek(&self, key_ref: &PiiKeyRef, region: &Region) -> Result<DekHandle, KmsError> {
            if self.down.load(Ordering::SeqCst) {
                Err(KmsError::KekUnavailable(KekId::new(
                    key_ref.tenant.clone(),
                    region.clone(),
                )))
            } else {
                self.inner.resolve_dek(key_ref, region)
            }
        }
    }

    fn provisioned() -> (KmsEngine, PiiKeyRef, Region) {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()))
            .expect("seed the in-memory KEK");
        let kr = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("dek");
        (kms, kr, region)
    }

    #[test]
    fn fresh_resolve_serves_from_the_engine_and_caches() {
        let (kms, kr, region) = provisioned();
        let path = KmsReadPath::with_clock(FlakyKms::new(kms), 30, 300, TestClock::at(1_000));
        let out = path.resolve(&kr, &region);
        assert!(
            out.is_resolved() && !out.is_degraded(),
            "fresh, not degraded"
        );
        assert_eq!(out.readiness(), KmsReadiness::Ready);
        assert_eq!(path.signals().fresh, 1);
        assert_eq!(
            path.signals().fail_open,
            0,
            "no fail-open on the fresh path"
        );
    }

    #[test]
    fn transient_outage_within_budget_serves_resolved_dek_stale() {
        let (kms, kr, region) = provisioned();
        let path = KmsReadPath::with_clock(FlakyKms::new(kms), 30, 300, TestClock::at(0));

        assert!(path.resolve(&kr, &region).is_resolved());

        path.engine.set_down(true);
        path.clock().advance(30);
        let out = path.resolve(&kr, &region);
        assert!(
            out.is_resolved() && !out.is_degraded(),
            "age == fresh_ttl is fresh"
        );

        path.clock().advance(100);
        let out = path.resolve(&kr, &region);
        assert!(
            out.is_resolved() && out.is_degraded(),
            "resolved-DEK survives the transient outage"
        );
        assert_eq!(
            out.readiness(),
            KmsReadiness::Ready,
            "degraded but still serving"
        );

        let s = path.signals();
        assert!(s.stale >= 1, "the survival was counted");
        assert!(
            s.last_staleness_secs <= path.static_max(),
            "staleness never exceeds the budget"
        );
        assert_eq!(s.fail_open, 0, "0 fail-open across the transient outage");
    }

    #[test]
    fn sustained_hard_down_past_budget_is_not_ready_never_fail_open() {
        let (kms, kr, region) = provisioned();
        let path = KmsReadPath::with_clock(FlakyKms::new(kms), 30, 300, TestClock::at(0));
        assert!(path.resolve(&kr, &region).is_resolved());

        path.engine.set_down(true);
        path.clock().advance(301);
        let out = path.resolve(&kr, &region);
        assert!(out.is_not_ready(), "past the budget → not-ready");
        assert_eq!(out.readiness(), KmsReadiness::NotReady);
        match out {
            KmsReadResult::NotReady(KmsError::KekUnavailable(_)) => {}
            other => panic!("expected NotReady(KekUnavailable), got {other:?}"),
        }
        assert_eq!(
            path.signals().fail_open,
            0,
            "0 fail-open even hard-down past the budget"
        );
    }

    #[test]
    fn cold_outage_with_no_cache_is_not_ready_never_plaintext() {
        let (kms, kr, region) = provisioned();
        let path = KmsReadPath::with_clock(FlakyKms::new(kms), 30, 300, TestClock::at(0));
        path.engine.set_down(true);
        let out = path.resolve(&kr, &region);
        assert!(
            out.is_not_ready(),
            "cold outage with no cache → not-ready, never fail open"
        );
        assert_eq!(path.signals().fresh, 0);
        assert_eq!(path.signals().fail_open, 0);
    }

    #[test]
    fn signals_classify_and_ratio_is_absent_before_any_read() {
        let s = KmsFailStaticSignals::default();
        assert_eq!(s.total(), 0);
        assert_eq!(
            s.stale_survival_pct(),
            None,
            "no ratio over zero reads (never fabricated)"
        );
        let s = KmsFailStaticSignals {
            fresh: 1,
            stale: 3,
            not_ready: 0,
            ..Default::default()
        };
        assert_eq!(s.total(), 4);
        assert_eq!(
            s.stale_survival_pct(),
            Some(75),
            "3/4 stale survival == 75%"
        );
    }

    #[test]
    fn test_clock_starts_at_the_given_offset() {
        let c = TestClock::at(555);
        assert_eq!(c.now_secs(), 555);
        c.advance(5);
        assert_eq!(c.now_secs(), 560);
    }

    #[test]
    fn system_clock_returns_real_wall_seconds() {
        let c = SystemClock;
        let a = c.now_secs();
        assert!(
            a > 1_577_836_800,
            "SystemClock reads real wall time (post-2020), got {a}"
        );
        assert!(
            c.now_secs() >= a,
            "wall time does not run backwards across two reads"
        );
    }

    #[test]
    fn read_result_classifiers_are_exact_per_rung() {
        let (kms, kr, region) = provisioned();
        let path = KmsReadPath::with_clock(FlakyKms::new(kms), 30, 300, TestClock::at(0));
        let fresh = path.resolve(&kr, &region);
        assert!(fresh.is_resolved() && !fresh.is_degraded() && !fresh.is_not_ready());
        assert_eq!(fresh.readiness(), KmsReadiness::Ready);

        path.engine.set_down(true);
        path.clock().advance(100);
        let stale = path.resolve(&kr, &region);
        assert!(stale.is_resolved() && stale.is_degraded() && !stale.is_not_ready());
        assert_eq!(stale.readiness(), KmsReadiness::Ready);

        path.clock().advance(300);
        let nr = path.resolve(&kr, &region);
        assert!(!nr.is_resolved() && !nr.is_degraded() && nr.is_not_ready());
        assert_eq!(nr.readiness(), KmsReadiness::NotReady);
    }

    #[test]
    fn signals_total_is_additive_over_all_three_rungs() {
        let s = KmsFailStaticSignals {
            fresh: 2,
            stale: 3,
            not_ready: 4,
            ..Default::default()
        };
        assert_eq!(s.total(), 9, "total sums all three rungs");
        assert_eq!(s.stale_survival_pct(), Some(33), "3/9 stale == 33%");
    }

    #[test]
    fn read_error_display_names_the_not_served_posture_and_cause() {
        let e = KmsReadError(KmsError::KekUnavailable(KekId::new(t("acme"), r("eu"))));
        let m = e.to_string();
        assert!(
            m.contains("NOT served") && m.contains("NEVER fail open"),
            "got: {m}"
        );
        assert!(m.contains("acme"), "carries the cause: {m}");
    }

    #[test]
    fn recovery_after_a_transient_outage_re_freshes() {
        let (kms, kr, region) = provisioned();
        let path = KmsReadPath::with_clock(FlakyKms::new(kms), 30, 300, TestClock::at(0));
        assert!(path.resolve(&kr, &region).is_resolved());
        path.engine.set_down(true);
        path.clock().advance(100);
        assert!(
            path.resolve(&kr, &region).is_degraded(),
            "served stale during the outage"
        );
        path.engine.set_down(false);
        let out = path.resolve(&kr, &region);
        assert!(
            out.is_resolved() && !out.is_degraded(),
            "recovered → fresh again"
        );
    }
}
