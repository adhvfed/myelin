use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Target(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Req(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Idempotency {
    Idempotent,
    NonIdempotent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    Timeout,
    BreakerOpen {
        retry_after_ms: u64,
    },
    BulkheadFull,
    Downstream {
        message: String,
        retry_after_ms: Option<u64>,
    },
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Timeout => write!(f, "resilient-client: per-call timeout elapsed"),
            CallError::BreakerOpen { retry_after_ms } => write!(
                f,
                "resilient-client: breaker open (retry after {retry_after_ms}ms)"
            ),
            CallError::BulkheadFull => {
                write!(f, "resilient-client: bulkhead saturated (fast-fail)")
            }
            CallError::Downstream {
                message,
                retry_after_ms,
            } => match retry_after_ms {
                Some(ms) => write!(
                    f,
                    "resilient-client: downstream error ({message}); retry-after {ms}ms"
                ),
                None => write!(f, "resilient-client: downstream error ({message})"),
            },
        }
    }
}

impl std::error::Error for CallError {}

pub type Result<T> = core::result::Result<T, CallError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryAfter {
    Absent,
    DeltaMs(u64),
    Unparseable,
}

impl RetryAfter {
    pub fn floor_ms(self, open_ms: u64) -> u64 {
        match self {
            RetryAfter::Absent => 0,
            RetryAfter::DeltaMs(ms) => ms,
            RetryAfter::Unparseable => open_ms,
        }
    }

    pub fn is_present(self) -> bool {
        !matches!(self, RetryAfter::Absent)
    }
}

pub fn parse_retry_after(header: Option<&str>) -> RetryAfter {
    let raw = match header {
        None => return RetryAfter::Absent,
        Some(s) => s.trim(),
    };
    if raw.is_empty() {
        return RetryAfter::Unparseable;
    }
    if raw.bytes().all(|b| b.is_ascii_digit()) {
        match raw.parse::<u64>() {
            Ok(secs) => RetryAfter::DeltaMs(secs.saturating_mul(1_000)),
            Err(_) => RetryAfter::DeltaMs(u64::MAX),
        }
    } else {
        RetryAfter::Unparseable
    }
}

fn retry_after_of(last_err: &Option<CallError>) -> RetryAfter {
    match last_err {
        Some(CallError::Downstream {
            retry_after_ms: Some(ms),
            ..
        }) => RetryAfter::DeltaMs(*ms),
        _ => RetryAfter::Absent,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResilientConfig {
    pub timeout_ms: u64,
    pub max_attempts: u32,
    pub backoff_base_ms: u64,
    pub breaker_failure_ratio: f64,
    pub breaker_min_requests: u32,
    pub breaker_window: u32,
    pub breaker_open_ms: u64,
    pub bulkhead_max_concurrency: u32,
}

impl Default for ResilientConfig {
    fn default() -> Self {
        ResilientConfig {
            timeout_ms: 2_000,
            max_attempts: 3,
            backoff_base_ms: 50,
            breaker_failure_ratio: 0.5,
            breaker_min_requests: 5,
            breaker_window: 20,
            breaker_open_ms: 5_000,
            bulkhead_max_concurrency: 64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

impl BreakerState {
    pub fn signal_value(self) -> i64 {
        match self {
            BreakerState::Closed => 0,
            BreakerState::HalfOpen => 1,
            BreakerState::Open => 2,
        }
    }
}

pub trait TimeSource: Send + Sync {
    fn now_ms(&self) -> u64;
    fn sleep(&self, dur: Duration);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemTime;

impl TimeSource for SystemTime {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
    fn sleep(&self, dur: Duration) {
        std::thread::sleep(dur);
    }
}

pub trait Jitter: Send + Sync {
    fn next_below(&self, n: u64) -> u64;
}

#[derive(Debug)]
pub struct SplitMix64 {
    state: AtomicU64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64 {
            state: AtomicU64::new(seed),
        }
    }
}

impl Default for SplitMix64 {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        SplitMix64::new(seed | 1)
    }
}

impl Jitter for SplitMix64 {
    fn next_below(&self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let mut z = self
            .state
            .fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed)
            .wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        z % n
    }
}

#[derive(Debug)]
struct Breaker {
    state: BreakerState,
    window: std::collections::VecDeque<bool>,
    opened_at_ms: u64,
    probe_in_flight: bool,
}

impl Breaker {
    fn new() -> Self {
        Breaker {
            state: BreakerState::Closed,
            window: std::collections::VecDeque::new(),
            opened_at_ms: 0,
            probe_in_flight: false,
        }
    }

    fn admit(&mut self, cfg: &ResilientConfig, now_ms: u64) -> Result<()> {
        match self.state {
            BreakerState::Closed => Ok(()),
            BreakerState::Open => {
                if now_ms.saturating_sub(self.opened_at_ms) >= cfg.breaker_open_ms {
                    self.state = BreakerState::HalfOpen;
                    self.probe_in_flight = true;
                    Ok(())
                } else {
                    Err(CallError::BreakerOpen {
                        retry_after_ms: cfg
                            .breaker_open_ms
                            .saturating_sub(now_ms.saturating_sub(self.opened_at_ms)),
                    })
                }
            }
            BreakerState::HalfOpen => {
                if self.probe_in_flight {
                    Err(CallError::BreakerOpen {
                        retry_after_ms: cfg.breaker_open_ms,
                    })
                } else {
                    self.probe_in_flight = true;
                    Ok(())
                }
            }
        }
    }

    fn record(&mut self, cfg: &ResilientConfig, success: bool, now_ms: u64) {
        match self.state {
            BreakerState::HalfOpen => {
                self.probe_in_flight = false;
                if success {
                    self.state = BreakerState::Closed;
                    self.window.clear();
                } else {
                    self.state = BreakerState::Open;
                    self.opened_at_ms = now_ms;
                }
            }
            BreakerState::Closed => {
                self.window.push_back(success);
                while self.window.len() > cfg.breaker_window as usize {
                    if self.window.pop_front().is_none() {
                        break;
                    }
                }
                let total = self.window.len() as u32;
                if total >= cfg.breaker_min_requests {
                    let failures = self.window.iter().filter(|ok| !**ok).count() as f64;
                    let ratio = failures / total as f64;
                    if ratio >= cfg.breaker_failure_ratio {
                        self.state = BreakerState::Open;
                        self.opened_at_ms = now_ms;
                    }
                }
            }
            BreakerState::Open => {
            }
        }
    }
}

#[derive(Debug)]
struct Bulkhead {
    in_flight: u32,
    max: u32,
}

impl Bulkhead {
    fn new(max: u32) -> Self {
        Bulkhead { in_flight: 0, max }
    }
}

#[derive(Debug)]
struct TargetState {
    breaker: Breaker,
    bulkhead: Bulkhead,
}

pub struct ResilientClient {
    cfg: ResilientConfig,
    time: Box<dyn TimeSource>,
    jitter: Box<dyn Jitter>,
    targets: Mutex<HashMap<Target, TargetState>>,
    bulkhead_rejections: AtomicU64,
    retry_through_tripped: AtomicU64,
    retry_admit_refusals: AtomicU64,
    retry_after_honoured: AtomicU64,
}

impl std::fmt::Debug for ResilientClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResilientClient")
            .field("cfg", &self.cfg)
            .field(
                "bulkhead_rejections",
                &self.bulkhead_rejections.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl Default for ResilientClient {
    fn default() -> Self {
        ResilientClient::new(ResilientConfig::default())
    }
}

impl ResilientClient {
    pub fn new(cfg: ResilientConfig) -> Self {
        ResilientClient {
            cfg,
            time: Box::new(SystemTime),
            jitter: Box::new(SplitMix64::default()),
            targets: Mutex::new(HashMap::new()),
            bulkhead_rejections: AtomicU64::new(0),
            retry_through_tripped: AtomicU64::new(0),
            retry_admit_refusals: AtomicU64::new(0),
            retry_after_honoured: AtomicU64::new(0),
        }
    }

    pub fn with_sources(
        cfg: ResilientConfig,
        time: Box<dyn TimeSource>,
        jitter: Box<dyn Jitter>,
    ) -> Self {
        ResilientClient {
            cfg,
            time,
            jitter,
            targets: Mutex::new(HashMap::new()),
            bulkhead_rejections: AtomicU64::new(0),
            retry_through_tripped: AtomicU64::new(0),
            retry_admit_refusals: AtomicU64::new(0),
            retry_after_honoured: AtomicU64::new(0),
        }
    }

    pub fn call<R>(&self, target: Target, req: Req, idem: Idempotency) -> Result<R>
    where
        R: Transport,
    {
        self.call_op(&target, idem, || R::send(&target, &req))
    }

    pub fn call_op<T, F>(&self, target: &Target, idem: Idempotency, mut op: F) -> Result<T>
    where
        F: FnMut() -> Result<T>,
    {
        let deadline_ms = self.time.now_ms().saturating_add(self.cfg.timeout_ms);
        let max_attempts = match idem {
            Idempotency::NonIdempotent => 1,
            Idempotency::Idempotent => self.cfg.max_attempts.max(1),
        };

        let mut last_err: Option<CallError> = None;
        let mut attempts_done: u32 = 0;
        for _iteration in 0..max_attempts {
            if attempts_done > 0 {
                let sleep_ms = self.full_jitter_backoff(attempts_done - 1, &last_err);
                if self.time.now_ms().saturating_add(sleep_ms) >= deadline_ms {
                    return Err(last_err.unwrap_or(CallError::Timeout));
                }
                self.time.sleep(Duration::from_millis(sleep_ms));
            }

            if self.time.now_ms() >= deadline_ms {
                return Err(last_err.unwrap_or(CallError::Timeout));
            }

            let _permit = match self.acquire_permit(target) {
                Some(p) => p,
                None => {
                    self.bulkhead_rejections.fetch_add(1, Ordering::Relaxed);
                    return Err(CallError::BulkheadFull);
                }
            };

            let now = self.time.now_ms();
            if let Err(open) = self.breaker_admit(target, now) {
                if attempts_done > 0 {
                    self.retry_admit_refusals.fetch_add(1, Ordering::Relaxed);
                }
                return Err(open);
            }

            let outcome = op();
            let now_after = self.time.now_ms();
            let timed_out = now_after >= deadline_ms;

            let success = outcome.is_ok() && !timed_out;
            self.breaker_record(target, success, now_after);
            attempts_done += 1;

            match outcome {
                Ok(v) if !timed_out => return Ok(v),
                Ok(_) => {
                    last_err = Some(CallError::Timeout);
                }
                Err(e) => {
                    last_err = Some(e);
                }
            }

            if attempts_done >= max_attempts {
                return Err(last_err.unwrap_or(CallError::Timeout));
            }
        }
        Err(last_err.unwrap_or(CallError::Timeout))
    }

    fn full_jitter_backoff(&self, attempt: u32, last_err: &Option<CallError>) -> u64 {
        let cap = self
            .cfg
            .backoff_base_ms
            .saturating_mul(1u64.checked_shl(attempt).unwrap_or(u64::MAX));
        let jittered = self.jitter.next_below(cap.saturating_add(1));
        let retry_after = retry_after_of(last_err);
        let retry_after_floor = retry_after.floor_ms(self.cfg.breaker_open_ms);
        if retry_after.is_present() {
            self.retry_after_honoured.fetch_add(1, Ordering::Relaxed);
        }
        jittered.max(retry_after_floor)
    }

    fn acquire_permit(&self, target: &Target) -> Option<BulkheadGuard<'_>> {
        let mut targets = self.targets.lock().expect("targets lock poisoned");
        let st = targets
            .entry(target.clone())
            .or_insert_with(|| TargetState {
                breaker: Breaker::new(),
                bulkhead: Bulkhead::new(self.cfg.bulkhead_max_concurrency),
            });
        if st.bulkhead.in_flight >= st.bulkhead.max {
            None
        } else {
            st.bulkhead.in_flight += 1;
            Some(BulkheadGuard {
                client: self,
                target: target.clone(),
            })
        }
    }

    fn breaker_admit(&self, target: &Target, now_ms: u64) -> Result<()> {
        let mut targets = self.targets.lock().expect("targets lock poisoned");
        let st = targets
            .entry(target.clone())
            .or_insert_with(|| TargetState {
                breaker: Breaker::new(),
                bulkhead: Bulkhead::new(self.cfg.bulkhead_max_concurrency),
            });
        st.breaker.admit(&self.cfg, now_ms)
    }

    fn breaker_record(&self, target: &Target, success: bool, now_ms: u64) {
        let mut targets = self.targets.lock().expect("targets lock poisoned");
        if let Some(st) = targets.get_mut(target) {
            st.breaker.record(&self.cfg, success, now_ms);
        }
    }

    pub fn breaker_state(&self, target: &Target) -> BreakerState {
        let targets = self.targets.lock().expect("targets lock poisoned");
        targets
            .get(target)
            .map(|st| st.breaker.state)
            .unwrap_or(BreakerState::Closed)
    }

    pub fn bulkhead_rejections(&self) -> u64 {
        self.bulkhead_rejections.load(Ordering::Relaxed)
    }

    pub fn retry_through_tripped(&self) -> u64 {
        self.retry_through_tripped.load(Ordering::Relaxed)
    }

    pub fn retry_admit_refusals(&self) -> u64 {
        self.retry_admit_refusals.load(Ordering::Relaxed)
    }

    pub fn retry_after_honoured(&self) -> u64 {
        self.retry_after_honoured.load(Ordering::Relaxed)
    }

    pub fn config(&self) -> &ResilientConfig {
        &self.cfg
    }
}

struct BulkheadGuard<'a> {
    client: &'a ResilientClient,
    target: Target,
}

impl Drop for BulkheadGuard<'_> {
    fn drop(&mut self) {
        let mut targets = self.client.targets.lock().expect("targets lock poisoned");
        if let Some(st) = targets.get_mut(&self.target) {
            st.bulkhead.in_flight = st.bulkhead.in_flight.saturating_sub(1);
        }
    }
}

pub trait Transport: Sized {
    fn send(target: &Target, req: &Req) -> Result<Self>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::AtomicU64;

    #[derive(Clone)]
    struct TestClock {
        now: std::sync::Arc<AtomicU64>,
    }
    impl TestClock {
        fn new() -> Self {
            TestClock {
                now: std::sync::Arc::new(AtomicU64::new(0)),
            }
        }
        fn set(&self, ms: u64) {
            self.now.store(ms, Ordering::SeqCst);
        }
    }
    impl TimeSource for TestClock {
        fn now_ms(&self) -> u64 {
            self.now.load(Ordering::SeqCst)
        }
        fn sleep(&self, dur: Duration) {
            self.now.fetch_add(dur.as_millis() as u64, Ordering::SeqCst);
        }
    }

    struct MaxJitter;
    impl Jitter for MaxJitter {
        fn next_below(&self, n: u64) -> u64 {
            n.saturating_sub(1)
        }
    }

    struct ZeroJitter;
    impl Jitter for ZeroJitter {
        fn next_below(&self, _n: u64) -> u64 {
            0
        }
    }

    fn client_with(cfg: ResilientConfig, jitter: Box<dyn Jitter>) -> ResilientClient {
        ResilientClient::with_sources(cfg, Box::new(TestClock::new()), jitter)
    }

    #[test]
    fn tripped_breaker_rejects_without_calling_through() {
        let cfg = ResilientConfig {
            max_attempts: 1,
            breaker_min_requests: 3,
            breaker_failure_ratio: 0.5,
            breaker_window: 10,
            breaker_open_ms: 5_000,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("auth".into());

        let calls = Cell::new(0u32);
        for _ in 0..3 {
            let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
                calls.set(calls.get() + 1);
                Err::<(), _>(CallError::Downstream {
                    message: "boom".into(),
                    retry_after_ms: None,
                })
            });
        }
        assert_eq!(client.breaker_state(&target), BreakerState::Open);
        let calls_before = calls.get();

        let res = client.call_op(&target, Idempotency::NonIdempotent, || {
            calls.set(calls.get() + 1);
            Ok::<(), CallError>(())
        });
        assert!(matches!(res, Err(CallError::BreakerOpen { .. })));
        assert_eq!(
            calls.get(),
            calls_before,
            "the downstream must NOT be invoked through a tripped breaker"
        );
    }

    #[test]
    fn idempotent_retry_does_not_pass_through_tripped_breaker() {
        let cfg = ResilientConfig {
            max_attempts: 5,
            breaker_min_requests: 1,
            breaker_failure_ratio: 1.0,
            breaker_window: 4,
            breaker_open_ms: 1_000_000,
            backoff_base_ms: 1,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("svc".into());

        let calls = Cell::new(0u32);
        let res = client.call_op(&target, Idempotency::Idempotent, || {
            calls.set(calls.get() + 1);
            Err::<(), _>(CallError::Downstream {
                message: "down".into(),
                retry_after_ms: None,
            })
        });
        assert_eq!(
            calls.get(),
            1,
            "retry must not pass through the tripped breaker"
        );
        assert!(res.is_err());
    }

    #[test]
    fn non_idempotent_call_is_never_retried() {
        let cfg = ResilientConfig {
            max_attempts: 5,
            breaker_min_requests: 100,
            backoff_base_ms: 1,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("svc".into());

        let calls = Cell::new(0u32);
        let res = client.call_op(&target, Idempotency::NonIdempotent, || {
            calls.set(calls.get() + 1);
            Err::<(), _>(CallError::Downstream {
                message: "down".into(),
                retry_after_ms: None,
            })
        });
        assert_eq!(
            calls.get(),
            1,
            "a NonIdempotent call must be attempted exactly once"
        );
        assert!(res.is_err());
    }

    #[test]
    fn idempotent_call_retries_to_max_attempts() {
        let cfg = ResilientConfig {
            max_attempts: 3,
            breaker_min_requests: 100,
            backoff_base_ms: 1,
            timeout_ms: 1_000_000,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("svc".into());

        let calls = Cell::new(0u32);
        let res = client.call_op(&target, Idempotency::Idempotent, || {
            calls.set(calls.get() + 1);
            Err::<(), _>(CallError::Downstream {
                message: "down".into(),
                retry_after_ms: None,
            })
        });
        assert_eq!(
            calls.get(),
            3,
            "an Idempotent call retries up to max_attempts"
        );
        assert!(res.is_err());
    }

    #[test]
    fn idempotent_retry_stops_on_success() {
        let cfg = ResilientConfig {
            max_attempts: 5,
            breaker_min_requests: 100,
            backoff_base_ms: 1,
            timeout_ms: 1_000_000,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("svc".into());

        let calls = Cell::new(0u32);
        let res = client.call_op(&target, Idempotency::Idempotent, || {
            calls.set(calls.get() + 1);
            if calls.get() < 3 {
                Err(CallError::Downstream {
                    message: "transient".into(),
                    retry_after_ms: None,
                })
            } else {
                Ok(42u32)
            }
        });
        assert_eq!(res, Ok(42));
        assert_eq!(calls.get(), 3, "retry stops as soon as the call succeeds");
    }

    #[test]
    fn saturated_bulkhead_fast_fails() {
        let cfg = ResilientConfig {
            bulkhead_max_concurrency: 1,
            max_attempts: 1,
            breaker_min_requests: 100,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("svc".into());

        let inner_result = Cell::new(None);
        let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
            let r = client.call_op(&target, Idempotency::NonIdempotent, || {
                Ok::<(), CallError>(())
            });
            inner_result.set(Some(r));
            Ok::<(), CallError>(())
        });
        assert_eq!(
            inner_result.into_inner(),
            Some(Err(CallError::BulkheadFull)),
            "a saturated bulkhead must fast-fail, not queue"
        );
        assert_eq!(client.bulkhead_rejections(), 1);
    }

    #[test]
    fn bulkhead_guard_releases_the_permit_on_drop_so_a_later_call_fits() {
        let cfg = ResilientConfig {
            bulkhead_max_concurrency: 1,
            max_attempts: 1,
            breaker_min_requests: 100,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("svc".into());

        let r1 = client.call_op(&target, Idempotency::NonIdempotent, || {
            Ok::<(), CallError>(())
        });
        assert_eq!(r1, Ok(()), "first call succeeds");
        let r2 = client.call_op(&target, Idempotency::NonIdempotent, || {
            Ok::<i32, CallError>(7)
        });
        assert_eq!(
            r2,
            Ok(7),
            "the second call fits - the permit was released on drop (no leak)"
        );
        assert_eq!(
            client.bulkhead_rejections(),
            0,
            "no bulkhead rejection - the permit was freed between calls"
        );
    }

    #[test]
    fn config_accessor_returns_the_configured_values_not_default() {
        let cfg = ResilientConfig {
            bulkhead_max_concurrency: 7,
            max_attempts: 9,
            backoff_base_ms: 123,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let read = client.config();
        assert_eq!(
            read.bulkhead_max_concurrency, 7,
            "config() returns the set cap"
        );
        assert_eq!(
            read.max_attempts, 9,
            "config() returns the set max_attempts"
        );
        assert_eq!(
            read.backoff_base_ms, 123,
            "config() returns the set backoff base"
        );
        assert_ne!(
            read.bulkhead_max_concurrency,
            ResilientConfig::default().bulkhead_max_concurrency
        );
    }

    #[test]
    fn full_jitter_backoff_within_base() {
        let cfg = ResilientConfig {
            backoff_base_ms: 100,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(MaxJitter));
        let b0 = client.full_jitter_backoff(0, &None);
        let b1 = client.full_jitter_backoff(1, &None);
        let b2 = client.full_jitter_backoff(2, &None);
        assert!(b0 <= 100, "attempt-0 jitter must stay within base ({b0})");
        assert!(b1 <= 200, "attempt-1 jitter must stay within base*2 ({b1})");
        assert!(b2 <= 400, "attempt-2 jitter must stay within base*4 ({b2})");
        let floored = client.full_jitter_backoff(
            0,
            &Some(CallError::Downstream {
                message: "x".into(),
                retry_after_ms: Some(5_000),
            }),
        );
        assert!(
            floored >= 5_000,
            "Retry-After is the backoff floor ({floored})"
        );
    }

    #[test]
    fn breaker_half_open_probe_recovers() {
        let cfg = ResilientConfig {
            max_attempts: 1,
            breaker_min_requests: 2,
            breaker_failure_ratio: 0.5,
            breaker_window: 4,
            breaker_open_ms: 1_000,
            ..ResilientConfig::default()
        };
        let clock = TestClock::new();
        let clock_handle = clock.clone();
        let client = ResilientClient::with_sources(cfg, Box::new(clock), Box::new(ZeroJitter));
        let target = Target("svc".into());

        for _ in 0..2 {
            let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
                Err::<(), _>(CallError::Downstream {
                    message: "boom".into(),
                    retry_after_ms: None,
                })
            });
        }
        assert_eq!(client.breaker_state(&target), BreakerState::Open);

        clock_handle.set(2_000);
        let res = client.call_op(&target, Idempotency::NonIdempotent, || {
            Ok::<(), CallError>(())
        });
        assert!(res.is_ok());
        assert_eq!(client.breaker_state(&target), BreakerState::Closed);
    }

    #[test]
    fn signals_export_breaker_and_bulkhead() {
        assert_eq!(BreakerState::Closed.signal_value(), 0);
        assert_eq!(BreakerState::HalfOpen.signal_value(), 1);
        assert_eq!(BreakerState::Open.signal_value(), 2);

        let client = ResilientClient::default();
        let target = Target("never-called".into());
        assert_eq!(client.breaker_state(&target), BreakerState::Closed);
        assert_eq!(client.bulkhead_rejections(), 0);
    }

    #[test]
    fn splitmix64_jitter_in_range_and_spreads() {
        let rng = SplitMix64::new(0xDEAD_BEEF);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1_000 {
            let v = rng.next_below(1_000);
            assert!(v < 1_000, "next_below(n) must be < n (got {v})");
            seen.insert(v);
        }
        assert!(
            seen.len() > 500,
            "jitter must spread across the range (only {} distinct values)",
            seen.len()
        );
        assert_eq!(rng.next_below(0), 0);
        assert_eq!(rng.next_below(1), 0);
        let a = SplitMix64::new(1);
        let b = SplitMix64::new(2);
        let sa: Vec<u64> = (0..16).map(|_| a.next_below(u64::MAX)).collect();
        let sb: Vec<u64> = (0..16).map(|_| b.next_below(u64::MAX)).collect();
        assert_ne!(sa, sb, "distinct seeds must yield distinct jitter streams");
    }

    #[test]
    fn splitmix64_matches_canonical_reference_vectors() {
        let rng = SplitMix64::new(0);
        let got: Vec<u64> = (0..3).map(|_| rng.next_below(u64::MAX)).collect();
        let expected: [u64; 3] = [
            0xE220_A839_7B1D_CDAF,
            0x6E78_9E6A_A1B9_65F4,
            0x06C4_5D18_8009_454F,
        ];
        assert_eq!(
            got,
            expected.to_vec(),
            "splitmix64 must match the canonical stream"
        );
    }

    #[test]
    fn breaker_window_is_bounded_and_ages_out() {
        let cfg = ResilientConfig {
            max_attempts: 1,
            breaker_min_requests: 4,
            breaker_failure_ratio: 0.75,
            breaker_window: 4,
            breaker_open_ms: 5_000,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("svc".into());

        for _ in 0..10 {
            let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
                Ok::<(), CallError>(())
            });
        }
        for _ in 0..2 {
            let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
                Err::<(), _>(CallError::Downstream {
                    message: "f".into(),
                    retry_after_ms: None,
                })
            });
        }
        assert_eq!(client.breaker_state(&target), BreakerState::Closed);

        for _ in 0..2 {
            let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
                Err::<(), _>(CallError::Downstream {
                    message: "f".into(),
                    retry_after_ms: None,
                })
            });
        }
        assert_eq!(client.breaker_state(&target), BreakerState::Open);
    }

    #[test]
    fn breaker_does_not_trip_below_min_requests() {
        let cfg = ResilientConfig {
            max_attempts: 1,
            breaker_min_requests: 5,
            breaker_failure_ratio: 0.5,
            breaker_window: 20,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("svc".into());
        for _ in 0..4 {
            let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
                Err::<(), _>(CallError::Downstream {
                    message: "f".into(),
                    retry_after_ms: None,
                })
            });
        }
        assert_eq!(client.breaker_state(&target), BreakerState::Closed);
        let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
            Err::<(), _>(CallError::Downstream {
                message: "f".into(),
                retry_after_ms: None,
            })
        });
        assert_eq!(client.breaker_state(&target), BreakerState::Open);
    }

    #[test]
    fn deadline_halts_retry_and_timeout_is_not_success() {
        let cfg = ResilientConfig {
            timeout_ms: 100,
            max_attempts: 3,
            breaker_min_requests: 100,
            backoff_base_ms: 1,
            ..ResilientConfig::default()
        };
        let clock = TestClock::new();
        let clock_handle = clock.clone();
        let client = ResilientClient::with_sources(cfg, Box::new(clock), Box::new(ZeroJitter));
        let target = Target("svc".into());

        let res: Result<()> = client.call_op(&target, Idempotency::Idempotent, || {
            clock_handle.set(200);
            Ok(())
        });
        assert_eq!(
            res,
            Err(CallError::Timeout),
            "an Ok that overran the deadline is a Timeout"
        );
    }

    #[test]
    fn backoff_is_between_attempts_and_widens() {
        #[derive(Clone)]
        struct RecordingClock {
            now: std::sync::Arc<AtomicU64>,
            sleeps: std::sync::Arc<Mutex<Vec<u64>>>,
        }
        impl TimeSource for RecordingClock {
            fn now_ms(&self) -> u64 {
                self.now.load(Ordering::SeqCst)
            }
            fn sleep(&self, dur: Duration) {
                let ms = dur.as_millis() as u64;
                self.sleeps.lock().unwrap().push(ms);
                self.now.fetch_add(ms, Ordering::SeqCst);
            }
        }

        let clock = RecordingClock {
            now: std::sync::Arc::new(AtomicU64::new(0)),
            sleeps: std::sync::Arc::new(Mutex::new(Vec::new())),
        };
        let sleeps = clock.sleeps.clone();
        let cfg = ResilientConfig {
            max_attempts: 4,
            breaker_min_requests: 100,
            backoff_base_ms: 10,
            timeout_ms: 10_000_000,
            ..ResilientConfig::default()
        };
        let client = ResilientClient::with_sources(cfg, Box::new(clock), Box::new(MaxJitter));
        let target = Target("svc".into());

        let calls = Cell::new(0u32);
        let _ = client.call_op(&target, Idempotency::Idempotent, || {
            calls.set(calls.get() + 1);
            Err::<(), _>(CallError::Downstream {
                message: "f".into(),
                retry_after_ms: None,
            })
        });
        let recorded = sleeps.lock().unwrap().clone();
        assert_eq!(
            recorded.len(),
            3,
            "exactly one backoff between each pair of attempts"
        );
        assert_eq!(recorded, vec![10, 20, 40]);
        assert_eq!(calls.get(), 4);
    }

    #[test]
    fn last_attempt_count_is_exact() {
        let cfg = ResilientConfig {
            max_attempts: 2,
            breaker_min_requests: 100,
            backoff_base_ms: 1,
            timeout_ms: 1_000_000,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("svc".into());
        let calls = Cell::new(0u32);
        let _ = client.call_op(&target, Idempotency::Idempotent, || {
            calls.set(calls.get() + 1);
            Err::<(), _>(CallError::Downstream {
                message: "f".into(),
                retry_after_ms: None,
            })
        });
        assert_eq!(calls.get(), 2, "max_attempts=2 means exactly 2 attempts");
    }

    #[test]
    fn call_signature_is_frozen_and_runs_primitives() {
        struct FakeResp(String);
        thread_local! {
            static SENDS: Cell<u32> = const { Cell::new(0) };
        }
        impl Transport for FakeResp {
            fn send(_target: &Target, req: &Req) -> Result<Self> {
                SENDS.with(|c| c.set(c.get() + 1));
                Ok(FakeResp(format!("ok:{}", req.0)))
            }
        }

        let client = client_with(
            ResilientConfig {
                breaker_min_requests: 100,
                ..ResilientConfig::default()
            },
            Box::new(ZeroJitter),
        );
        let _f: fn(&ResilientClient, Target, Req, Idempotency) -> Result<FakeResp> =
            ResilientClient::call::<FakeResp>;

        let resp: FakeResp = client
            .call(
                Target("svc".into()),
                Req("ping".into()),
                Idempotency::Idempotent,
            )
            .expect("happy-path call succeeds through the primitives");
        assert_eq!(resp.0, "ok:ping");
        assert_eq!(SENDS.with(|c| c.get()), 1, "exactly one downstream attempt");
    }

    #[test]
    fn parse_retry_after_maps_header_to_floor() {
        assert_eq!(parse_retry_after(None), RetryAfter::Absent);
        assert_eq!(RetryAfter::Absent.floor_ms(9_999), 0);
        assert!(!RetryAfter::Absent.is_present());

        assert_eq!(parse_retry_after(Some("120")), RetryAfter::DeltaMs(120_000));
        assert_eq!(parse_retry_after(Some("0")), RetryAfter::DeltaMs(0));
        assert_eq!(
            parse_retry_after(Some("  30 ")),
            RetryAfter::DeltaMs(30_000)
        );
        assert!(RetryAfter::DeltaMs(30_000).is_present());
        assert_eq!(RetryAfter::DeltaMs(30_000).floor_ms(9_999), 30_000);

        for bad in [
            "Wed, 21 Oct 2026 07:28:00 GMT",
            "-5",
            "+5",
            "12.5",
            "soon",
            "",
            "  ",
        ] {
            let ra = parse_retry_after(Some(bad));
            assert_eq!(
                ra,
                RetryAfter::Unparseable,
                "{bad:?} must be Unparseable, not honoured-as-zero"
            );
            assert!(
                ra.is_present(),
                "an Unparseable Retry-After is PRESENT (a non-zero floor)"
            );
            assert_eq!(
                ra.floor_ms(7_000),
                7_000,
                "Unparseable floors at the whole open window"
            );
        }
        assert_eq!(
            parse_retry_after(Some("99999999999999999999999")),
            RetryAfter::DeltaMs(u64::MAX)
        );
    }

    #[test]
    fn retry_after_is_the_backoff_floor_and_is_counted() {
        let cfg = ResilientConfig {
            max_attempts: 3,
            breaker_min_requests: 100,
            backoff_base_ms: 10,
            timeout_ms: 100_000_000,
            breaker_open_ms: 4_000,
            ..ResilientConfig::default()
        };
        #[derive(Clone)]
        struct RecordingClock {
            now: std::sync::Arc<AtomicU64>,
            sleeps: std::sync::Arc<Mutex<Vec<u64>>>,
        }
        impl TimeSource for RecordingClock {
            fn now_ms(&self) -> u64 {
                self.now.load(Ordering::SeqCst)
            }
            fn sleep(&self, dur: Duration) {
                let ms = dur.as_millis() as u64;
                self.sleeps.lock().unwrap().push(ms);
                self.now.fetch_add(ms, Ordering::SeqCst);
            }
        }
        let clock = RecordingClock {
            now: std::sync::Arc::new(AtomicU64::new(0)),
            sleeps: std::sync::Arc::new(Mutex::new(Vec::new())),
        };
        let sleeps = clock.sleeps.clone();
        let client = ResilientClient::with_sources(cfg, Box::new(clock), Box::new(ZeroJitter));
        let target = Target("svc".into());

        let _ = client.call_op(&target, Idempotency::Idempotent, || {
            Err::<(), _>(CallError::Downstream {
                message: "shed".into(),
                retry_after_ms: parse_retry_after(Some("5")).floor_ms(4_000).into(),
            })
        });
        let recorded = sleeps.lock().unwrap().clone();
        assert_eq!(recorded.len(), 2, "3 attempts → 2 backoffs");
        for s in &recorded {
            assert!(
                *s >= 5_000,
                "every backoff is floored at the Retry-After (5000ms), got {s}"
            );
        }
        assert_eq!(
            client.retry_after_honoured(),
            2,
            "each floored retry honours Retry-After once (2 backoffs here)"
        );
        assert_eq!(client.retry_through_tripped(), 0);
    }

    #[test]
    fn tripped_breaker_with_retry_after_fails_fast_no_amplification() {
        let cfg = ResilientConfig {
            max_attempts: 5,
            breaker_min_requests: 1,
            breaker_failure_ratio: 1.0,
            breaker_window: 4,
            breaker_open_ms: 1_000_000,
            backoff_base_ms: 1,
            timeout_ms: 100_000_000,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("svc".into());

        let calls = Cell::new(0u32);
        let res = client.call_op(&target, Idempotency::Idempotent, || {
            calls.set(calls.get() + 1);
            Err::<(), _>(CallError::Downstream {
                message: "overloaded".into(),
                retry_after_ms: Some(2_000),
            })
        });
        assert_eq!(
            calls.get(),
            1,
            "no retry passes through the tripped breaker"
        );
        assert!(matches!(res, Err(CallError::BreakerOpen { .. })) || res.is_err());
        assert_eq!(client.breaker_state(&target), BreakerState::Open);
        assert_eq!(
            client.retry_through_tripped(),
            0,
            "retry_through_tripped MUST be 0 - no retry-storm amplification"
        );
        assert_eq!(
            client.retry_admit_refusals(),
            1,
            "the open breaker refused the first would-be retry, then call_op returned"
        );
    }

    #[test]
    fn cdc_provider_trips_and_issues_retry_after_consumer_honours() {
        struct OverloadedDownstream;
        thread_local! {
            static HITS: Cell<u32> = const { Cell::new(0) };
        }
        impl Transport for OverloadedDownstream {
            fn send(_t: &Target, _r: &Req) -> Result<Self> {
                HITS.with(|c| c.set(c.get() + 1));
                let header = "3";
                let ra = parse_retry_after(Some(header));
                Err(CallError::Downstream {
                    message: "429 Too Many Requests".into(),
                    retry_after_ms: match ra {
                        RetryAfter::DeltaMs(ms) => Some(ms),
                        RetryAfter::Unparseable => Some(0),
                        RetryAfter::Absent => None,
                    },
                })
            }
        }

        let cfg = ResilientConfig {
            max_attempts: 4,
            breaker_min_requests: 2,
            breaker_failure_ratio: 1.0,
            breaker_window: 4,
            breaker_open_ms: 1_000_000,
            backoff_base_ms: 1,
            timeout_ms: 100_000_000,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));

        let res: Result<OverloadedDownstream> = client.call(
            Target("payments".into()),
            Req("charge".into()),
            Idempotency::Idempotent,
        );
        assert!(res.is_err(), "the overloaded downstream call fails");
        let hits = HITS.with(|c| c.get());
        assert!(
            hits <= 2,
            "the consumer must not amplify load past the breaker trip (hits={hits})"
        );
        assert!(
            client.retry_after_honoured() >= 1,
            "the consumer honoured the issued Retry-After"
        );
        assert_eq!(
            client.retry_through_tripped(),
            0,
            "no retry through the tripped breaker"
        );
    }

    #[test]
    fn first_attempt_breaker_refusal_is_not_a_retry_refusal() {
        let cfg = ResilientConfig {
            max_attempts: 3,
            breaker_min_requests: 1,
            breaker_failure_ratio: 1.0,
            breaker_window: 4,
            breaker_open_ms: 1_000_000,
            backoff_base_ms: 1,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("svc".into());

        let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
            Err::<(), _>(CallError::Downstream {
                message: "boom".into(),
                retry_after_ms: None,
            })
        });
        assert_eq!(client.breaker_state(&target), BreakerState::Open);
        assert_eq!(client.retry_admit_refusals(), 0);

        let res = client.call_op(&target, Idempotency::Idempotent, || Ok::<(), CallError>(()));
        assert!(matches!(res, Err(CallError::BreakerOpen { .. })));
        assert_eq!(
            client.retry_admit_refusals(),
            0,
            "a first-attempt fast-fail is not a retry refusal"
        );
        assert_eq!(client.retry_through_tripped(), 0);
    }

    #[test]
    fn retry_after_of_maps_every_error_variant() {
        assert_eq!(retry_after_of(&None), RetryAfter::Absent);
        assert_eq!(
            retry_after_of(&Some(CallError::Downstream {
                message: "x".into(),
                retry_after_ms: Some(7_000),
            })),
            RetryAfter::DeltaMs(7_000)
        );
        assert_eq!(
            retry_after_of(&Some(CallError::Downstream {
                message: "x".into(),
                retry_after_ms: None,
            })),
            RetryAfter::Absent
        );
        assert_eq!(
            retry_after_of(&Some(CallError::BreakerOpen { retry_after_ms: 9 })),
            RetryAfter::Absent
        );
        assert_eq!(
            retry_after_of(&Some(CallError::BulkheadFull)),
            RetryAfter::Absent
        );
        assert_eq!(
            retry_after_of(&Some(CallError::Timeout)),
            RetryAfter::Absent
        );
    }
}
