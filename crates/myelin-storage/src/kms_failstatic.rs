//! The KMS read path's fail-static availability posture (P-ST-06 / 11.3; the STOR-D6 gate).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §4.5 ("KMS
//! availability/blast radius: read-path DEK caching degrades like fail-static, hard-down →
//! not-ready (never fail-open)") and `00-platform-substrate.md` §8 (the fail-CLOSED vs
//! fail-STATIC distinction — fail-static is the AVAILABILITY default on a TRANSIENT hiccup; we
//! NEVER fail open; past the staleness budget, deny). Drill row STOR-D6 (testing-strategy §4.2).
//!
//! ## Why a storage-local bounded-staleness cache (RECONCILED — not a duplicate)
//! The canonical bounded-staleness mechanism is `myelin_substrate::FailStatic<T>` (P-S18). The
//! crate DAG (Cargo.toml) forbids a `myelin-storage → myelin-substrate` edge (storage is NOT a
//! downstream consumer of the substrate; cf. the same re-statement discipline in
//! [`crate::migration`], which re-states the contract-1.5 phase vocabulary rather than importing
//! it). So this module RE-STATES the same `Fresh → Static → Closed` ladder, specialised to the
//! ONE thing the KMS read path caches — a **resolved DEK handle** — and adds the two STOR-D6
//! signals (readiness + the 0-fail-open guarantee). It is the SAME doctrine (§8: serve
//! bounded-stale on a transient hiccup, never fail open, deny past the budget), narrowed to the
//! KMS read path; if the architecture later admits the storage→substrate edge, this collapses to
//! a `FailStatic<DekHandle>` unchanged in behaviour. Recorded here in writing (EI-01 §1).
//!
//! ## The STOR-D6 posture (the three rungs)
//! - **Fresh / transient hiccup within the budget** → serve the resolved-DEK from cache
//!   ([`KmsReadResult::Resolved`]). A transient KMS outage does NOT take reads down — the
//!   resolved DEK survives a bounded TTL. This is the availability win.
//! - **Past the staleness budget OR a SUSTAINED hard-down** → the read is NOT served from a stale
//!   key; the surface goes **not-ready + sheds** ([`KmsReadiness::NotReady`],
//!   [`KmsReadResult::NotReady`]). Readiness ≠ liveness (the process is healthy; it just cannot
//!   serve correct traffic without the KMS).
//! - **No cached value to fall back on** → [`KmsReadResult::NotReady`] — we NEVER fabricate a key
//!   and NEVER return plaintext-without-key. **0 fail-open** is structural: there is no code path
//!   that returns a usable DEK when the key cannot be (re)resolved AND no in-budget cache exists.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use myelin_tenancy::Region;

use crate::kms::{DekHandle, KmsAdapter, KmsError, PiiKeyRef};

/// A monotonic clock seam so the staleness boundaries are deterministically drillable (the
/// STOR-D6 drill advances a [`TestClock`] across the `fresh_ttl` / `static_max` boundaries;
/// production wires the wall clock). Mirrors `myelin_substrate::fail_static::Clock` (re-stated,
/// not imported — see the module note).
pub trait Clock: Send + Sync {
    /// Seconds since a fixed (implementation-defined) epoch — only DIFFERENCES are meaningful.
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

/// A deterministic test clock — seconds advance only when the drill advances them.
#[derive(Debug, Default)]
pub struct TestClock {
    now: AtomicU64,
}

impl TestClock {
    /// A clock starting at `t0` seconds.
    pub fn at(t0: u64) -> Self {
        TestClock { now: AtomicU64::new(t0) }
    }
    /// Advance the clock by `secs` (the drill steps across a boundary).
    pub fn advance(&self, secs: u64) {
        self.now.fetch_add(secs, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_secs(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

/// The KMS read-path readiness (the STOR-D6 survival signal; storage.md §4.5). A read that can be
/// served (fresh upstream, or a within-budget cached DEK) reads [`KmsReadiness::Ready`]; a read
/// whose key cannot be resolved and has no in-budget cache flips to [`KmsReadiness::NotReady`] and
/// the surface sheds. NEVER fails open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KmsReadiness {
    /// The KMS read path can serve correct traffic (fresh, or within the staleness budget).
    Ready,
    /// The KMS is hard-down past the budget (or no cache exists) — shed new traffic. The DEK is
    /// NOT served stale; deny is correct (never fail open).
    NotReady,
}

/// The outcome of a fail-static KMS read (storage.md §4.5; the STOR-D6 ladder).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KmsReadResult {
    /// The DEK was resolved — either freshly from the engine, or served from the bounded-staleness
    /// cache while the engine hiccupped (within `static_max`). Carries whether it was degraded
    /// (stale) so the caller can surface a degraded marker.
    Resolved {
        /// the resolved DEK handle (usable to seal/open).
        handle: DekHandle,
        /// `true` when served from the stale cache (a transient hiccup survived); `false` when
        /// fresh from the engine.
        degraded: bool,
    },
    /// The KMS is unavailable past the staleness budget OR no cached DEK exists — the read is NOT
    /// served, the surface is **not-ready + shed**. This is the fail-CLOSED rung: **never a
    /// plaintext-without-key, never fail open.** Carries the underlying engine error for telemetry.
    NotReady(KmsError),
}

impl KmsReadResult {
    /// Did this read resolve a usable DEK (fresh or stale)?
    pub fn is_resolved(&self) -> bool {
        matches!(self, KmsReadResult::Resolved { .. })
    }
    /// Was this read served from the degraded (stale) cache?
    pub fn is_degraded(&self) -> bool {
        matches!(self, KmsReadResult::Resolved { degraded: true, .. })
    }
    /// Did this read fail-static to not-ready (the surface sheds; NEVER fail open)?
    pub fn is_not_ready(&self) -> bool {
        matches!(self, KmsReadResult::NotReady(_))
    }
    /// The readiness this outcome implies (the STOR-D6 readiness signal).
    pub fn readiness(&self) -> KmsReadiness {
        match self {
            KmsReadResult::Resolved { .. } => KmsReadiness::Ready,
            KmsReadResult::NotReady(_) => KmsReadiness::NotReady,
        }
    }
}

/// A typed error a caller surfaces when a KMS read could not be served (the not-ready rung). It
/// carries the engine cause; it is NEVER convertible into "serve plaintext anyway".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KmsReadError(pub KmsError);

impl std::fmt::Display for KmsReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "KMS read NOT served (not-ready + shed; the resolved-DEK cache is past its \
             staleness budget or empty) — cause: {} — NEVER fail open",
            self.0
        )
    }
}

impl std::error::Error for KmsReadError {}

/// The STOR-D6 survival counters (storage.md §4.5; the `fail_static` ratio + the `0 fail-open`
/// assertion). A read is classified `fresh` (engine served), `stale` (degraded cache served), or
/// `not_ready` (shed). `fail_open` is the load-bearing zero: it counts any read that returned a
/// usable DEK WITHOUT a real resolution or an in-budget cache — it MUST stay `0` (a non-zero here
/// is a fail-open, the floor breach the drill asserts against).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct KmsFailStaticSignals {
    /// reads served fresh from the engine.
    pub fresh: u64,
    /// reads served stale (degraded) from the bounded-staleness cache (the survival win).
    pub stale: u64,
    /// reads shed (not-ready) — past the budget or no cache. Deny is correct.
    pub not_ready: u64,
    /// the staleness age (seconds) of the most-recent stale read (asserted ≤ `static_max`).
    pub last_staleness_secs: u64,
    /// **MUST stay 0.** Any read that returned a DEK without a fresh resolution or an in-budget
    /// cache. A non-zero value is a fail-OPEN — the floor breach STOR-D6 forbids.
    pub fail_open: u64,
}

impl KmsFailStaticSignals {
    /// Total reads served (the ratio denominator).
    pub fn total(&self) -> u64 {
        self.fresh + self.stale + self.not_ready
    }
    /// The `fail_static` ratio: the fraction of reads that survived a hiccup via the bounded-stale
    /// cache, as an integer percentage of the total, or `None` before any read (no fabricated
    /// ratio over zero reads).
    pub fn stale_survival_pct(&self) -> Option<u64> {
        (self.stale * 100).checked_div(self.total())
    }
}

/// One cached entry: the last-resolved DEK + the wall-second it was resolved.
struct Entry {
    handle: DekHandle,
    resolved_at_secs: u64,
}

/// The KMS read path with the fail-static availability posture (storage.md §4.5; STOR-D6).
///
/// Wraps a [`KmsAdapter`] (the engine, or any Vault/HSM backing). On a successful resolve it
/// caches the DEK and returns [`KmsReadResult::Resolved`] (`degraded: false`). On an engine hiccup
/// it falls back to the last cached DEK — served FRESH within `fresh_ttl`, served STALE
/// (`degraded: true`) up to `static_max`, and **not-served (not-ready + shed)** past `static_max`
/// or when no cache exists. **There is no path that returns a DEK when neither a fresh resolve nor
/// an in-budget cache exists — 0 fail-open is structural.**
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
    /// Build a KMS read path over `engine` with the bounded-staleness window
    /// `fresh_ttl ≤ static_max` (seconds), against the wall clock. `static_max` is the
    /// resolved-DEK survival budget — it sits UNDER the revocation SLA (the same `static_max ≤
    /// revocation-SLA` bound the substrate `FailStatic` enforces; the VALUE W is `[OPEN — LEGAL]`,
    /// seeded from the thresholds file by the caller).
    pub fn new(engine: A, fresh_ttl: u64, static_max: u64) -> KmsReadPath<A, SystemClock> {
        Self::with_clock(engine, fresh_ttl, static_max, SystemClock)
    }
}

impl<A: KmsAdapter, C: Clock> KmsReadPath<A, C> {
    /// Build against an injected clock (the STOR-D6 drill uses a [`TestClock`]).
    pub fn with_clock(engine: A, fresh_ttl: u64, static_max: u64, clock: C) -> KmsReadPath<A, C> {
        // Monotone ladder: fresh_ttl ≤ static_max (a freshness window cannot exceed the budget).
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

    /// Borrow the injected clock (drills advance a [`TestClock`]).
    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// Borrow the wrapped engine/adapter (the STOR-D6 drill toggles its outage state through this;
    /// production callers use [`Self::resolve`] only). Exposes no key material — only the adapter
    /// the read path was built over.
    pub fn engine(&self) -> &A {
        &self.engine
    }

    /// The staleness budget (seconds) — the resolved-DEK survival window.
    pub fn static_max(&self) -> u64 {
        self.static_max
    }

    /// Resolve a DEK with the fail-static posture (the STOR-D6 read path).
    ///
    /// - engine resolves → cache + [`KmsReadResult::Resolved`] (fresh).
    /// - engine hiccups + a cached DEK exists:
    ///   - age ≤ `fresh_ttl` → Resolved (still fresh).
    ///   - `fresh_ttl < age ≤ static_max` → Resolved (degraded — the survival win).
    ///   - age > `static_max` → **NotReady** (shed; deny is correct now).
    /// - engine hiccups + NO cache → **NotReady** (never fabricate a key; never fail open).
    pub fn resolve(&self, key_ref: &PiiKeyRef, region: &Region) -> KmsReadResult {
        let cache_key = (key_ref.clone(), region.clone());
        match self.engine.resolve_dek(key_ref, region) {
            Ok(handle) => {
                let now = self.clock.now_secs();
                {
                    let mut cache = self.cache.lock().expect("kms read cache poisoned");
                    cache.insert(
                        cache_key,
                        Entry { handle: handle.clone(), resolved_at_secs: now },
                    );
                }
                self.fresh.fetch_add(1, Ordering::SeqCst);
                self.last_staleness.store(0, Ordering::SeqCst);
                KmsReadResult::Resolved { handle, degraded: false }
            }
            Err(cause) => self.serve_from_cache(&cache_key, cause),
        }
    }

    /// The hiccup path: fall back to the last-resolved DEK within the bounded-staleness budget, or
    /// fail to NOT-READY. This is the single fail-open-prevention branch (mandatory-core): there is
    /// NO arm that returns a DEK past the budget or with no cache.
    fn serve_from_cache(&self, cache_key: &(PiiKeyRef, Region), cause: KmsError) -> KmsReadResult {
        let now = self.clock.now_secs();
        let cache = self.cache.lock().expect("kms read cache poisoned");
        let Some(entry) = cache.get(cache_key) else {
            // No fallback DEK → not-ready + shed (never fabricate a key; never fail open).
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
            KmsReadResult::Resolved { handle, degraded: false }
        } else if age <= self.static_max {
            let handle = entry.handle.clone();
            drop(cache);
            self.stale.fetch_add(1, Ordering::SeqCst);
            self.last_staleness.store(age, Ordering::SeqCst);
            KmsReadResult::Resolved { handle, degraded: true }
        } else {
            drop(cache);
            // Budget exhausted → not-ready + shed. Deny is correct. NEVER open.
            self.not_ready.fetch_add(1, Ordering::SeqCst);
            KmsReadResult::NotReady(cause)
        }
    }

    /// A snapshot of the STOR-D6 survival counters (the `fail_static` ratio + the `0 fail-open`
    /// assertion). The drill asserts `fail_open == 0` and `last_staleness_secs ≤ static_max`.
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
    use crate::kms::{KeyClass, KekId, KmsEngine};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::AtomicBool;

    fn t(s: &str) -> TenantId {
        TenantId(s.to_string())
    }
    fn r(s: &str) -> Region {
        Region(s.to_string())
    }

    /// A KMS adapter that proxies a real [`KmsEngine`] but can be switched "down" to simulate a
    /// transient/sustained KMS outage (so the read path's resolve fails) — the STOR-D6 fault
    /// injection. When down, EVERY resolve returns a loud error (never a key).
    struct FlakyKms {
        inner: KmsEngine,
        down: AtomicBool,
    }
    impl FlakyKms {
        fn new(inner: KmsEngine) -> Self {
            FlakyKms { inner, down: AtomicBool::new(false) }
        }
        fn set_down(&self, down: bool) {
            self.down.store(down, Ordering::SeqCst);
        }
    }
    impl KmsAdapter for FlakyKms {
        fn resolve_dek(
            &self,
            key_ref: &PiiKeyRef,
            region: &Region,
        ) -> Result<DekHandle, KmsError> {
            if self.down.load(Ordering::SeqCst) {
                // The KMS is "down" — a loud error, NEVER a fabricated key.
                Err(KmsError::KekUnavailable(KekId::new(key_ref.tenant.clone(), region.clone())))
            } else {
                self.inner.resolve_dek(key_ref, region)
            }
        }
    }

    fn provisioned() -> (KmsEngine, PiiKeyRef, Region) {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        let kr = kms.ensure_dek(&tenant, &region, KeyClass::Tenant).expect("dek");
        (kms, kr, region)
    }

    #[test]
    fn fresh_resolve_serves_from_the_engine_and_caches() {
        let (kms, kr, region) = provisioned();
        let path = KmsReadPath::with_clock(FlakyKms::new(kms), 30, 300, TestClock::at(1_000));
        let out = path.resolve(&kr, &region);
        assert!(out.is_resolved() && !out.is_degraded(), "fresh, not degraded");
        assert_eq!(out.readiness(), KmsReadiness::Ready);
        assert_eq!(path.signals().fresh, 1);
        assert_eq!(path.signals().fail_open, 0, "no fail-open on the fresh path");
    }

    #[test]
    fn transient_outage_within_budget_serves_resolved_dek_stale() {
        let (kms, kr, region) = provisioned();
        let path = KmsReadPath::with_clock(FlakyKms::new(kms), 30, 300, TestClock::at(0));

        // Warm the cache with a fresh resolve.
        assert!(path.resolve(&kr, &region).is_resolved());

        // KMS hiccups. Within fresh_ttl (age 30) the read is still FRESH.
        path.engine.set_down(true);
        path.clock().advance(30);
        let out = path.resolve(&kr, &region);
        assert!(out.is_resolved() && !out.is_degraded(), "age == fresh_ttl is fresh");

        // Past fresh_ttl but within static_max → degraded (the resolved-DEK SURVIVES the outage).
        path.clock().advance(100); // age 130 ≤ 300
        let out = path.resolve(&kr, &region);
        assert!(out.is_resolved() && out.is_degraded(), "resolved-DEK survives the transient outage");
        assert_eq!(out.readiness(), KmsReadiness::Ready, "degraded but still serving");

        let s = path.signals();
        assert!(s.stale >= 1, "the survival was counted");
        assert!(s.last_staleness_secs <= path.static_max(), "staleness never exceeds the budget");
        assert_eq!(s.fail_open, 0, "0 fail-open across the transient outage");
    }

    #[test]
    fn sustained_hard_down_past_budget_is_not_ready_never_fail_open() {
        let (kms, kr, region) = provisioned();
        let path = KmsReadPath::with_clock(FlakyKms::new(kms), 30, 300, TestClock::at(0));
        assert!(path.resolve(&kr, &region).is_resolved()); // warm cache

        // Hard-down, advance PAST static_max → not-ready + shed (deny is correct; never open).
        path.engine.set_down(true);
        path.clock().advance(301); // age 301 > 300
        let out = path.resolve(&kr, &region);
        assert!(out.is_not_ready(), "past the budget → not-ready");
        assert_eq!(out.readiness(), KmsReadiness::NotReady);
        // The outcome carries the loud cause and is NOT a usable DEK.
        match out {
            KmsReadResult::NotReady(KmsError::KekUnavailable(_)) => {}
            other => panic!("expected NotReady(KekUnavailable), got {other:?}"),
        }
        assert_eq!(path.signals().fail_open, 0, "0 fail-open even hard-down past the budget");
    }

    #[test]
    fn cold_outage_with_no_cache_is_not_ready_never_plaintext() {
        let (kms, kr, region) = provisioned();
        let path = KmsReadPath::with_clock(FlakyKms::new(kms), 30, 300, TestClock::at(0));
        // KMS is down BEFORE any successful resolve → no cache → not-ready (never a fabricated key).
        path.engine.set_down(true);
        let out = path.resolve(&kr, &region);
        assert!(out.is_not_ready(), "cold outage with no cache → not-ready, never fail open");
        assert_eq!(path.signals().fresh, 0);
        assert_eq!(path.signals().fail_open, 0);
    }

    #[test]
    fn signals_classify_and_ratio_is_absent_before_any_read() {
        let s = KmsFailStaticSignals::default();
        assert_eq!(s.total(), 0);
        assert_eq!(s.stale_survival_pct(), None, "no ratio over zero reads (never fabricated)");
        let s = KmsFailStaticSignals { fresh: 1, stale: 3, not_ready: 0, ..Default::default() };
        assert_eq!(s.total(), 4);
        assert_eq!(s.stale_survival_pct(), Some(75), "3/4 stale survival == 75%");
    }

    #[test]
    fn test_clock_starts_at_the_given_offset() {
        // kills `TestClock::at -> Default::default()` (start at 0), which would mis-align every
        // boundary advance in the STOR-D6 drill.
        let c = TestClock::at(555);
        assert_eq!(c.now_secs(), 555);
        c.advance(5);
        assert_eq!(c.now_secs(), 560);
    }

    #[test]
    fn system_clock_returns_real_wall_seconds() {
        // kills the `now_secs -> 0|1` constant mutants: a clock pinned to 0/1 would make every
        // cached DEK look infinitely fresh, defeating the staleness budget (a real fail-open risk).
        let c = SystemClock;
        let a = c.now_secs();
        assert!(a > 1_577_836_800, "SystemClock reads real wall time (post-2020), got {a}");
        assert!(c.now_secs() >= a, "wall time does not run backwards across two reads");
    }

    #[test]
    fn read_result_classifiers_are_exact_per_rung() {
        // kills the is_resolved/is_degraded/is_not_ready → true|false mutants: a flattened
        // classifier would mis-label the survival-signal buckets the STOR-D6 ratio reads.
        let (kms, kr, region) = provisioned();
        let path = KmsReadPath::with_clock(FlakyKms::new(kms), 30, 300, TestClock::at(0));
        let fresh = path.resolve(&kr, &region);
        assert!(fresh.is_resolved() && !fresh.is_degraded() && !fresh.is_not_ready());
        assert_eq!(fresh.readiness(), KmsReadiness::Ready);

        path.engine.set_down(true);
        path.clock().advance(100); // stale
        let stale = path.resolve(&kr, &region);
        assert!(stale.is_resolved() && stale.is_degraded() && !stale.is_not_ready());
        assert_eq!(stale.readiness(), KmsReadiness::Ready);

        path.clock().advance(300); // past budget
        let nr = path.resolve(&kr, &region);
        assert!(!nr.is_resolved() && !nr.is_degraded() && nr.is_not_ready());
        assert_eq!(nr.readiness(), KmsReadiness::NotReady);
    }

    #[test]
    fn signals_total_is_additive_over_all_three_rungs() {
        // kills the `+ → -` mutant in total(): fresh + stale + not_ready, not a subtraction.
        let s = KmsFailStaticSignals { fresh: 2, stale: 3, not_ready: 4, ..Default::default() };
        assert_eq!(s.total(), 9, "total sums all three rungs");
        assert_eq!(s.stale_survival_pct(), Some(33), "3/9 stale == 33%");
    }

    #[test]
    fn read_error_display_names_the_not_served_posture_and_cause() {
        // kills the Display `fmt → Ok(default)` mutant: an empty message hides the loud not-served
        // posture + the underlying cause.
        let e = KmsReadError(KmsError::KekUnavailable(KekId::new(t("acme"), r("eu"))));
        let m = e.to_string();
        assert!(m.contains("NOT served") && m.contains("NEVER fail open"), "got: {m}");
        assert!(m.contains("acme"), "carries the cause: {m}");
    }

    #[test]
    fn recovery_after_a_transient_outage_re_freshes() {
        let (kms, kr, region) = provisioned();
        let path = KmsReadPath::with_clock(FlakyKms::new(kms), 30, 300, TestClock::at(0));
        assert!(path.resolve(&kr, &region).is_resolved());
        path.engine.set_down(true);
        path.clock().advance(100);
        assert!(path.resolve(&kr, &region).is_degraded(), "served stale during the outage");
        // KMS recovers → the next read is FRESH again.
        path.engine.set_down(false);
        let out = path.resolve(&kr, &region);
        assert!(out.is_resolved() && !out.is_degraded(), "recovered → fresh again");
    }
}
