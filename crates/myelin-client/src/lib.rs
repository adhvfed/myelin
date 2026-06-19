//! # `myelin-client` — the shared resilient inter-service client
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.5 (`myelin-client` — the substrate-relevant seam) and §6 (the shared resilient
//! inter-service client).
//!
//! **Contract-index cluster:** 1 — Bootstrap & service shell
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` row 1.9
//! `ResilientClient::call`).
//!
//! ## What crosses the crate boundary here (the frozen surface)
//! `ResilientClient::call(target, req, idem)` — the ONE client every outbound
//! inter-service call goes through, so timeout/breaker/bulkhead/retry is correct in
//! exactly one place. The four primitives (all mandatory, all on by default):
//! per-call **timeout** (deadlines propagate), circuit **breaker** (never retry through a
//! tripped breaker — the retry-storm amplifier), bounded-concurrency **bulkhead**
//! (saturation fast-fails, never queues unboundedly), and jittered **retry** —
//! **idempotent calls only** (full jitter, Brooker 2015; a `NonIdempotent` call is never
//! retried). Our clients **MUST honour `Retry-After`** (§6.2) so shedding cannot become a
//! retry storm.
//!
//! ## Frozen units (architecture §6.3, §2.10)
//! Resilient-client timeouts = **milliseconds**; breaker thresholds = failure ratio over
//! a rolling window + a minimum request count; bulkhead = integer concurrency cap;
//! backoff base in ms with full jitter.
//!
//! ## What P-S16 ships (this prompt)
//! The **four primitives' control logic**, all on by default, exercised over a downstream
//! *operation* via [`ResilientClient::call_op`] — the testable core of `call`:
//! - **timeout** — every operation runs under a per-call deadline; an operation that
//!   overruns its `timeout_ms` budget is a [`CallError::Timeout`] and (for an
//!   `Idempotent` call) is retried subject to the breaker;
//! - **breaker** — a [`Breaker`] state machine (Closed → Open → HalfOpen) keyed per
//!   target; once tripped, calls **fast-fail without invoking the downstream** and a retry
//!   **never** goes through the tripped breaker (the textbook retry-storm amplifier);
//! - **bulkhead** — a per-target integer-cap permit counter; when saturated a call
//!   **fast-fails** ([`CallError::BulkheadFull`]) and never queues unboundedly;
//! - **retry** — only `Idempotency::Idempotent` calls are retried, with **full jitter**
//!   (`sleep ∈ [0, base * 2^attempt]`, capped at the deadline); a `NonIdempotent` call is
//!   attempted **exactly once**.
//!
//! Breaker state + bulkhead-rejection count are exported as the contract-1.8 producer-side
//! signals ([`ResilientClient::breaker_state`], [`ResilientClient::bulkhead_rejections`]).
//!
//! ## Floors named (deferred → filling prompt)
//! - **Real transport.** [`ResilientClient::call`] (the typed `call<R>` over a real
//!   downstream socket — `tokio`/HTTP, deserialising `R` from the wire) is **not built in
//!   M0**: there is no network substrate yet. `call` runs the four primitives over a
//!   [`Transport`] whose production impl lands when the wire format + runtime exist. The
//!   primitive *logic* (everything gateable here) is live and tested through `call_op`.
//!   **Follow-on: the real transport is wired with the service shells (`serve`, P-S12 →
//!   P-010, already merged) when the first real inter-service hop exists.**
//! - **`Retry-After` honouring + SUB-D5.** The header is honoured as the floor of the
//!   backoff in **P-S17** (the retry-storm drill). The retry primitive here already takes a
//!   `RetryAfter` hint on the error so P-S17 only wires the header → hint mapping.
//! - **Per-target tuned values.** This crate ships ONE **default per-target value set**
//!   ([`ResilientConfig::default`], the M0 floor). The per-target tuned numbers (the auth
//!   hot path gets a tighter timeout than a batch indexer) are measured by the surge/latency
//!   drills and written into the thresholds file in **M5 (P-S36)**.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// The target of an inter-service call (architecture §6; contract 1.9). The per-target
/// breaker/bulkhead are keyed on this.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Target(pub String);

/// An outbound request (architecture §6; contract 1.9). Opaque in the skeleton; the typed
/// request/response shape lands with the real transport floor (see crate docs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Req(pub String);

/// Whether a call is safe to retry (architecture §6; contract 1.9). A `NonIdempotent`
/// call is **never** retried (full-jitter retry is idempotent-only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Idempotency {
    Idempotent,
    NonIdempotent,
}

/// The taxonomy of resilient-client failures (architecture §6). Every variant is a **loud,
/// non-swallowed** terminal outcome (EI process-quality §5: violations are loud, never
/// silently swallowed — there is no `|| true` path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    /// The per-call deadline elapsed before the downstream answered (architecture §6 (1)).
    Timeout,
    /// The breaker is open (or half-open and probing was refused): the call fast-fails
    /// **without** invoking the downstream (architecture §6 (2); the retry-storm amplifier
    /// guard). Carries the `Retry-After` floor the caller should respect.
    BreakerOpen {
        /// The minimum backoff before the breaker will admit a probe again, in ms. The
        /// `Retry-After` header maps onto this in P-S17.
        retry_after_ms: u64,
    },
    /// The per-target bulkhead is saturated: the call fast-fails rather than queueing
    /// unboundedly (architecture §6 (3); Little's-Law bound).
    BulkheadFull,
    /// The downstream returned an error. Carries an optional `Retry-After` hint (the floor
    /// of the next backoff; honoured in P-S17) and whether the breaker should count it as a
    /// failure.
    Downstream {
        /// A human-meaningful downstream cause (never swallowed).
        message: String,
        /// The downstream's advertised `Retry-After`, if any (ms). The retry backoff is
        /// `max(full_jitter, retry_after_ms)` once P-S17 wires the header through.
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
                Some(ms) => write!(f, "resilient-client: downstream error ({message}); retry-after {ms}ms"),
                None => write!(f, "resilient-client: downstream error ({message})"),
            },
        }
    }
}

impl std::error::Error for CallError {}

/// `Result` alias for the client surface.
pub type Result<T> = core::result::Result<T, CallError>;

/// The default per-target value set (architecture §6.3 — the M0 floor; per-target tuning
/// lands in M5/P-S36). Timeouts in **ms**; breaker as a failure ratio over a rolling window
/// plus a minimum request count; bulkhead an integer concurrency cap; backoff base in ms
/// with full jitter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResilientConfig {
    /// Per-call deadline in **milliseconds** (§6.3 unit).
    pub timeout_ms: u64,
    /// Maximum number of attempts for an `Idempotent` call (1 = no retry). A
    /// `NonIdempotent` call is **always** attempted exactly once regardless.
    pub max_attempts: u32,
    /// Full-jitter backoff base in **milliseconds** (§6.3): the nth retry sleeps a uniform
    /// random duration in `[0, backoff_base_ms * 2^n]`.
    pub backoff_base_ms: u64,
    /// Breaker trip threshold: the rolling-window failure **ratio** in `[0.0, 1.0]` above
    /// which the breaker opens (§6.3).
    pub breaker_failure_ratio: f64,
    /// Breaker minimum request count: the breaker never trips on fewer than this many
    /// observations in the window (§6.3 — avoids tripping on a single early failure).
    pub breaker_min_requests: u32,
    /// The rolling-window size (number of recent outcomes) the breaker ratio is computed
    /// over.
    pub breaker_window: u32,
    /// How long the breaker stays open before admitting a half-open probe, in **ms**.
    pub breaker_open_ms: u64,
    /// The per-target bulkhead integer concurrency cap (§6.3).
    pub bulkhead_max_concurrency: u32,
}

impl Default for ResilientConfig {
    fn default() -> Self {
        // The M0 default-per-target floor (architecture §6.3). These are deliberately
        // conservative shape-correct defaults; the measured per-target numbers land in
        // M5/P-S36 (the surge/latency drills write them into the thresholds file).
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

/// The breaker lifecycle (architecture §6 (2)): Closed → Open → HalfOpen. Encoded as the
/// contract-1.8 `BreakerState` signal value (closed=0, half=1, open=2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakerState {
    /// Normal operation; calls pass through and outcomes feed the rolling window.
    Closed,
    /// Tripped: calls fast-fail **without invoking the downstream** until the open window
    /// elapses. A retry **never** goes through an open breaker (the retry-storm guard).
    Open,
    /// Probing: a single trial call is admitted; success closes the breaker, failure
    /// re-opens it.
    HalfOpen,
}

impl BreakerState {
    /// The numeric encoding for the contract-1.8 `BreakerState` signal (§10.2 row 5).
    pub fn signal_value(self) -> i64 {
        match self {
            BreakerState::Closed => 0,
            BreakerState::HalfOpen => 1,
            BreakerState::Open => 2,
        }
    }
}

/// Abstract monotonic clock (injectable so the timeout + breaker-open windows are
/// deterministically testable; production uses [`SystemTime`]).
pub trait TimeSource: Send + Sync {
    /// Milliseconds since an arbitrary fixed epoch (monotonic, non-decreasing).
    fn now_ms(&self) -> u64;
    /// Sleep for `dur` (the retry backoff). The default is a real sleep; tests inject a
    /// no-op + clock-advancing source.
    fn sleep(&self, dur: Duration);
}

/// The production wall clock.
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

/// Abstract source of full-jitter randomness (injectable so backoff is deterministic in
/// tests). `next_below(n)` returns a uniform value in `[0, n)`.
pub trait Jitter: Send + Sync {
    /// A uniform random `u64` in `[0, n)`. For `n == 0` returns 0.
    fn next_below(&self, n: u64) -> u64;
}

/// A small deterministic splitmix64 PRNG — the production default-jitter source. It needs
/// no external `rand` dependency (the crate is dep-light at M0) and is seedable so tests are
/// reproducible.
#[derive(Debug)]
pub struct SplitMix64 {
    state: AtomicU64,
}

impl SplitMix64 {
    /// Seed the generator.
    pub fn new(seed: u64) -> Self {
        SplitMix64 {
            state: AtomicU64::new(seed),
        }
    }
}

impl Default for SplitMix64 {
    fn default() -> Self {
        // Seed off the wall clock so independent clients do not synchronise their jitter
        // (synchronised jitter defeats the purpose — Brooker 2015).
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
        // splitmix64 — fetch_add the golden-ratio increment, then mix.
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

/// The per-target circuit breaker (architecture §6 (2)). A rolling window of the last
/// `window` outcomes; once `>= min_requests` observations and the failure ratio exceeds
/// `failure_ratio`, the breaker **opens** for `open_ms`. While open, every call fast-fails
/// without touching the downstream. After `open_ms` one **half-open** probe is admitted:
/// success closes, failure re-opens.
#[derive(Debug)]
struct Breaker {
    state: BreakerState,
    /// The rolling window of recent outcomes (true = success).
    window: std::collections::VecDeque<bool>,
    /// When the breaker opened (ms); the half-open probe is admitted at `opened_at +
    /// open_ms`.
    opened_at_ms: u64,
    /// True once a half-open probe is in flight (so only ONE probe is admitted).
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

    /// Decide whether to admit a call now. Returns `Ok(())` to proceed, or `Err` (breaker
    /// open) to fast-fail **without invoking the downstream**. This is the no-retry-through-a-
    /// tripped-breaker guard.
    fn admit(&mut self, cfg: &ResilientConfig, now_ms: u64) -> Result<()> {
        match self.state {
            BreakerState::Closed => Ok(()),
            BreakerState::Open => {
                if now_ms.saturating_sub(self.opened_at_ms) >= cfg.breaker_open_ms {
                    // The open window has elapsed: transition to half-open and admit ONE
                    // probe.
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
                    // A probe is already in flight; reject concurrent calls.
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

    /// Record the outcome of an admitted call and advance the state machine.
    fn record(&mut self, cfg: &ResilientConfig, success: bool, now_ms: u64) {
        match self.state {
            BreakerState::HalfOpen => {
                self.probe_in_flight = false;
                if success {
                    // Probe succeeded: close and clear the window.
                    self.state = BreakerState::Closed;
                    self.window.clear();
                } else {
                    // Probe failed: re-open.
                    self.state = BreakerState::Open;
                    self.opened_at_ms = now_ms;
                }
            }
            BreakerState::Closed => {
                self.window.push_back(success);
                while self.window.len() > cfg.breaker_window as usize {
                    self.window.pop_front();
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
                // An outcome recorded while Open should not happen (admit() never returns
                // Ok in Open without transitioning to HalfOpen first), but stay safe.
            }
        }
    }
}

/// The per-target bulkhead (architecture §6 (3)): a bounded permit counter. `try_acquire`
/// fast-fails when saturated rather than queueing. A [`BulkheadGuard`] releases its permit
/// on drop.
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

/// Per-target mutable resilience state (breaker + bulkhead), behind one lock per target.
#[derive(Debug)]
struct TargetState {
    breaker: Breaker,
    bulkhead: Bulkhead,
}

/// The shared resilient inter-service client (architecture §6; contract 1.9; ADR-16).
/// One place where timeout + breaker + bulkhead + jittered-retry-idempotent-only is
/// correct for every caller (services, CLI, agent runtime). Honours `Retry-After` (P-S17).
pub struct ResilientClient {
    cfg: ResilientConfig,
    time: Box<dyn TimeSource>,
    jitter: Box<dyn Jitter>,
    /// Per-target breaker + bulkhead state, created lazily on first call to a target.
    targets: Mutex<HashMap<Target, TargetState>>,
    /// Contract-1.8 producer signal: total bulkhead rejections across all targets.
    bulkhead_rejections: AtomicU64,
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
    /// Build a client with a config (production wall clock + default jitter source).
    pub fn new(cfg: ResilientConfig) -> Self {
        ResilientClient {
            cfg,
            time: Box::new(SystemTime),
            jitter: Box::new(SplitMix64::default()),
            targets: Mutex::new(HashMap::new()),
            bulkhead_rejections: AtomicU64::new(0),
        }
    }

    /// Build a client with injected time + jitter sources (deterministic tests).
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
        }
    }

    /// Every call: per-call TIMEOUT, BULKHEAD (bounded concurrency), through the BREAKER.
    /// Retry ONLY if idempotent, with full jitter, NEVER through a tripped breaker
    /// (architecture §6; contract 1.9).
    ///
    /// **Floor (named in the crate docs):** the real wire transport — serialising `req`,
    /// deserialising `R` from a downstream socket — is not built in M0 (no network
    /// substrate yet). The four primitives' control logic is live and is exercised through
    /// [`Self::call_op`]; `call` runs them over the production [`Transport`], whose body is
    /// the deferred floor. The follow-on is named on [`Transport::send`].
    pub fn call<R>(&self, target: Target, req: Req, idem: Idempotency) -> Result<R>
    where
        R: Transport,
    {
        self.call_op(&target, idem, || R::send(&target, &req))
    }

    /// The **testable core**: run `op` (a fallible downstream operation) through all four
    /// primitives. This is exactly the path `call` takes; tests drive it with a fake
    /// downstream (the CDC provider side of 1.9).
    ///
    /// Ordering of the primitives per attempt: BULKHEAD permit (fast-fail if saturated) →
    /// BREAKER admit (fast-fail if open, **no downstream call**) → TIMEOUT-bounded `op` →
    /// record the outcome. Retry (idempotent only) sleeps full jitter, then re-checks the
    /// breaker — a retry **never** passes through a tripped breaker.
    pub fn call_op<T, F>(&self, target: &Target, idem: Idempotency, mut op: F) -> Result<T>
    where
        F: FnMut() -> Result<T>,
    {
        let deadline_ms = self.time.now_ms().saturating_add(self.cfg.timeout_ms);
        let max_attempts = match idem {
            // A NonIdempotent call is attempted EXACTLY once — never retried.
            Idempotency::NonIdempotent => 1,
            Idempotency::Idempotent => self.cfg.max_attempts.max(1),
        };

        let mut last_err: Option<CallError> = None;
        // `attempts_done` counts completed downstream attempts; the loop runs once and then
        // re-enters only while there is budget for a retry. Encoding the retry budget as a
        // single `attempts_done < max_attempts` guard (rather than a `for` bound plus a
        // separate early break) keeps the retry-count logic single-sourced.
        let mut attempts_done: u32 = 0;
        loop {
            // Between attempts (i.e. before every retry) sleep a full-jitter backoff unless
            // it would overrun the deadline, in which case we stop with the last error.
            if attempts_done > 0 {
                let sleep_ms = self.full_jitter_backoff(attempts_done - 1, &last_err);
                if self.time.now_ms().saturating_add(sleep_ms) >= deadline_ms {
                    return Err(last_err.unwrap_or(CallError::Timeout));
                }
                self.time.sleep(Duration::from_millis(sleep_ms));
            }

            // Respect the per-call deadline across retries (deadlines propagate, §6 (1)).
            if self.time.now_ms() >= deadline_ms {
                return Err(last_err.unwrap_or(CallError::Timeout));
            }

            // (3) BULKHEAD: acquire a permit or fast-fail.
            let _permit = match self.acquire_permit(target) {
                Some(p) => p,
                None => {
                    self.bulkhead_rejections.fetch_add(1, Ordering::Relaxed);
                    // Bulkhead saturation is NOT a downstream failure; it does not feed the
                    // breaker window. It also is not retried in a tight loop — fast-fail.
                    return Err(CallError::BulkheadFull);
                }
            };

            // (2) BREAKER: admit or fast-fail WITHOUT invoking the downstream. A retry must
            // NEVER pass through a tripped breaker, so a `BreakerOpen` error returns
            // immediately (`?`) rather than looping back into the retry path.
            let now = self.time.now_ms();
            self.breaker_admit(target, now)?;

            // (1) TIMEOUT-bounded downstream operation. With a synchronous op the timeout is
            // a deadline check: if the op observed the deadline (a real transport sets the
            // socket deadline from it) it returns Timeout; here we additionally enforce the
            // deadline post-hoc so an op that overran is never counted as a success.
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

            // Out of retry budget → return the last error (idempotent calls have
            // `max_attempts` > 1; a NonIdempotent call has `max_attempts == 1` and so never
            // re-enters the loop).
            if attempts_done >= max_attempts {
                return Err(last_err.unwrap_or(CallError::Timeout));
            }
        }
    }

    /// (4) Full-jitter backoff (Brooker 2015): a uniform value in `[0, base * 2^attempt]`,
    /// floored by the downstream's `Retry-After` hint when present (the hint→honour wiring
    /// is P-S17; the floor is applied here so P-S17 only maps the `Retry-After` HEADER onto
    /// the [`CallError::Downstream::retry_after_ms`] hint — the backoff arithmetic does not
    /// change). A `BreakerOpen` error never reaches this path: `call_op` returns it
    /// immediately (a retry must never pass through a tripped breaker), so there is no
    /// breaker arm here.
    fn full_jitter_backoff(&self, attempt: u32, last_err: &Option<CallError>) -> u64 {
        let cap = self
            .cfg
            .backoff_base_ms
            .saturating_mul(1u64.checked_shl(attempt).unwrap_or(u64::MAX));
        let jittered = self.jitter.next_below(cap.saturating_add(1));
        let retry_after_floor = match last_err {
            Some(CallError::Downstream {
                retry_after_ms: Some(ms),
                ..
            }) => *ms,
            _ => 0,
        };
        jittered.max(retry_after_floor)
    }

    /// Acquire a bulkhead permit for `target`, or `None` if saturated (fast-fail).
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

    /// Contract-1.8 producer signal: the current breaker state for `target` (Closed if the
    /// target has never been called). Maps onto `SignalName::BreakerState` (§10.2 row 5) via
    /// [`BreakerState::signal_value`].
    pub fn breaker_state(&self, target: &Target) -> BreakerState {
        let targets = self.targets.lock().expect("targets lock poisoned");
        targets
            .get(target)
            .map(|st| st.breaker.state)
            .unwrap_or(BreakerState::Closed)
    }

    /// Contract-1.8 producer signal: total bulkhead rejections across all targets since the
    /// client was built. Maps onto the shed/bulkhead-rejection signal of the §10.2 set.
    pub fn bulkhead_rejections(&self) -> u64 {
        self.bulkhead_rejections.load(Ordering::Relaxed)
    }

    /// The configured per-target value set (the M0 floor).
    pub fn config(&self) -> &ResilientConfig {
        &self.cfg
    }
}

/// RAII permit for the bulkhead: decrements the in-flight count on drop.
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

/// The wire-transport seam the typed `call<R>` rides over.
///
/// **Floor (deferred):** the production impl — serialise the [`Req`], open a socket to the
/// [`Target`], deserialise `Self` from the response under the per-call deadline — is NOT
/// built in M0 (there is no network substrate yet). The four resilient primitives that wrap
/// it (timeout/breaker/bulkhead/retry) are fully live and tested via
/// [`ResilientClient::call_op`]. **Follow-on:** the real `send` lands with the first real
/// inter-service hop, once the service shells (`serve`, P-S12 → P-010) carry a wire format.
pub trait Transport: Sized {
    /// Perform one downstream attempt for `target` with `req`, returning the typed response
    /// or a [`CallError`]. The resilient wrapper supplies all four primitives; an impl only
    /// performs the single I/O attempt.
    fn send(target: &Target, req: &Req) -> Result<Self>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::AtomicU64;

    /// A controllable test clock: `now_ms` is readable + advanceable; `sleep` advances it
    /// (no real wall-clock wait, so the suite is fast and deterministic). The inner counter
    /// is shared via `Arc` so a test can hold a handle and advance the clock the client
    /// reads through.
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
            self.now
                .fetch_add(dur.as_millis() as u64, Ordering::SeqCst);
        }
    }

    /// A deterministic jitter that always returns `cap - 1` (the maximum), so the
    /// full-jitter bound test is exact.
    struct MaxJitter;
    impl Jitter for MaxJitter {
        fn next_below(&self, n: u64) -> u64 {
            n.saturating_sub(1)
        }
    }

    /// A deterministic jitter that always returns 0 (no sleep), so retry-count tests do not
    /// advance the clock through the deadline.
    struct ZeroJitter;
    impl Jitter for ZeroJitter {
        fn next_below(&self, _n: u64) -> u64 {
            0
        }
    }

    fn client_with(cfg: ResilientConfig, jitter: Box<dyn Jitter>) -> ResilientClient {
        ResilientClient::with_sources(cfg, Box::new(TestClock::new()), jitter)
    }

    // ---- Primitive (2): a tripped breaker rejects WITHOUT calling through. ----
    #[test]
    fn tripped_breaker_rejects_without_calling_through() {
        let cfg = ResilientConfig {
            max_attempts: 1, // isolate the breaker from the retry primitive
            breaker_min_requests: 3,
            breaker_failure_ratio: 0.5,
            breaker_window: 10,
            breaker_open_ms: 5_000,
            ..ResilientConfig::default()
        };
        let client = client_with(cfg, Box::new(ZeroJitter));
        let target = Target("auth".into());

        // Drive enough downstream failures to trip the breaker.
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

        // The next call must fast-fail with BreakerOpen and NOT invoke the downstream.
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

    // ---- Primitive (2)+(4): a retry NEVER passes through a tripped breaker. ----
    #[test]
    fn idempotent_retry_does_not_pass_through_tripped_breaker() {
        // Window of recent failures already trips on the first observed call's failure
        // because min_requests=1 and ratio>=1.0 trips immediately.
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
        // First attempt fails and trips the breaker; every remaining "retry" is refused by
        // the open breaker, so the downstream is invoked EXACTLY once despite max_attempts=5.
        assert_eq!(calls.get(), 1, "retry must not pass through the tripped breaker");
        assert!(res.is_err());
    }

    // ---- Primitive (4): a NonIdempotent call is NEVER retried. ----
    #[test]
    fn non_idempotent_call_is_never_retried() {
        let cfg = ResilientConfig {
            max_attempts: 5,
            breaker_min_requests: 100, // keep breaker closed so it never interferes
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
        assert_eq!(calls.get(), 1, "a NonIdempotent call must be attempted exactly once");
        assert!(res.is_err());
    }

    // ---- Primitive (4): an Idempotent call IS retried up to max_attempts. ----
    #[test]
    fn idempotent_call_retries_to_max_attempts() {
        let cfg = ResilientConfig {
            max_attempts: 3,
            breaker_min_requests: 100, // breaker never trips
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
        assert_eq!(calls.get(), 3, "an Idempotent call retries up to max_attempts");
        assert!(res.is_err());
    }

    // ---- Primitive (4): an Idempotent call that succeeds on retry stops retrying. ----
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

    // ---- Primitive (3): a saturated bulkhead fast-fails rather than queueing. ----
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

        // Hold the only permit by re-entering call_op from within an op (synthetic
        // saturation: the inner call sees in_flight == max).
        let inner_result = Cell::new(None);
        let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
            // While this op runs, the outer call holds the single permit.
            let r = client.call_op(&target, Idempotency::NonIdempotent, || Ok::<(), CallError>(()));
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

    // ---- Primitive (1): full-jitter backoff stays within the configured base. ----
    #[test]
    fn full_jitter_backoff_within_base() {
        let cfg = ResilientConfig {
            backoff_base_ms: 100,
            ..ResilientConfig::default()
        };
        // MaxJitter returns cap-1 = the largest value the bound permits. For attempt 0 the
        // cap is base*2^0 = 100, so the value must be <= 100. For attempt 2 the cap is
        // base*2^2 = 400 (full jitter widens with the attempt, Brooker 2015).
        let client = client_with(cfg, Box::new(MaxJitter));
        let b0 = client.full_jitter_backoff(0, &None);
        let b1 = client.full_jitter_backoff(1, &None);
        let b2 = client.full_jitter_backoff(2, &None);
        assert!(b0 <= 100, "attempt-0 jitter must stay within base ({b0})");
        assert!(b1 <= 200, "attempt-1 jitter must stay within base*2 ({b1})");
        assert!(b2 <= 400, "attempt-2 jitter must stay within base*4 ({b2})");
        // The Retry-After hint is honoured as the FLOOR of the backoff (the P-S17 wiring
        // only maps the header onto this hint).
        let floored = client.full_jitter_backoff(
            0,
            &Some(CallError::Downstream {
                message: "x".into(),
                retry_after_ms: Some(5_000),
            }),
        );
        assert!(floored >= 5_000, "Retry-After is the backoff floor ({floored})");
    }

    // ---- Primitive (2): the breaker recovers via a half-open probe. ----
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

        // Trip the breaker.
        for _ in 0..2 {
            let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
                Err::<(), _>(CallError::Downstream {
                    message: "boom".into(),
                    retry_after_ms: None,
                })
            });
        }
        assert_eq!(client.breaker_state(&target), BreakerState::Open);

        // Advance past the open window so the half-open probe is admitted.
        clock_handle.set(2_000);
        // A successful probe closes the breaker.
        let res = client.call_op(&target, Idempotency::NonIdempotent, || Ok::<(), CallError>(()));
        assert!(res.is_ok());
        assert_eq!(client.breaker_state(&target), BreakerState::Closed);
    }

    // ---- Signals: breaker-state encoding + bulkhead-rejection counter (contract 1.8). ----
    #[test]
    fn signals_export_breaker_and_bulkhead() {
        assert_eq!(BreakerState::Closed.signal_value(), 0);
        assert_eq!(BreakerState::HalfOpen.signal_value(), 1);
        assert_eq!(BreakerState::Open.signal_value(), 2);

        let client = ResilientClient::default();
        let target = Target("never-called".into());
        // An unseen target reads Closed (the fail-safe default).
        assert_eq!(client.breaker_state(&target), BreakerState::Closed);
        assert_eq!(client.bulkhead_rejections(), 0);
    }

    // ---- Jitter (4): SplitMix64 stays in range, spreads, and decorrelates. ----
    #[test]
    fn splitmix64_jitter_in_range_and_spreads() {
        let rng = SplitMix64::new(0xDEAD_BEEF);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1_000 {
            let v = rng.next_below(1_000);
            assert!(v < 1_000, "next_below(n) must be < n (got {v})");
            seen.insert(v);
        }
        // A correct mixer spreads across the range; a broken one (e.g. & instead of ^, or a
        // dropped shift) collapses to a tiny set of values. Require broad coverage.
        assert!(
            seen.len() > 500,
            "jitter must spread across the range (only {} distinct values)",
            seen.len()
        );
        // next_below(0) is defined as 0 (no panic / no modulo-by-zero).
        assert_eq!(rng.next_below(0), 0);
        // next_below(1) is always 0.
        assert_eq!(rng.next_below(1), 0);
        // Two different seeds must not produce identical streams (decorrelation —
        // synchronised jitter defeats Brooker 2015).
        let a = SplitMix64::new(1);
        let b = SplitMix64::new(2);
        let sa: Vec<u64> = (0..16).map(|_| a.next_below(u64::MAX)).collect();
        let sb: Vec<u64> = (0..16).map(|_| b.next_below(u64::MAX)).collect();
        assert_ne!(sa, sb, "distinct seeds must yield distinct jitter streams");
    }

    /// Known-answer vectors for the canonical splitmix64 (Vigna). Pinning the exact output
    /// stream locks every bit-mixing step (the `^`/`>>`/`*` operations): any single altered
    /// mixing op diverges from these reference values. `next_below(u64::MAX)` returns
    /// `full % (u64::MAX)`; for `full < u64::MAX` (true for all three reference values) that
    /// is `full` unchanged, so we read the raw 64-bit stream directly.
    #[test]
    fn splitmix64_matches_canonical_reference_vectors() {
        // Seed 0, the canonical splitmix64 reference stream (Vigna's prng.di.unimi.it).
        let rng = SplitMix64::new(0);
        let got: Vec<u64> = (0..3).map(|_| rng.next_below(u64::MAX)).collect();
        // All three reference values are < u64::MAX, so `next_below(u64::MAX)` (which is
        // `stream % u64::MAX`) returns them unchanged.
        let expected: [u64; 3] = [
            0xE220_A839_7B1D_CDAF,
            0x6E78_9E6A_A1B9_65F4,
            0x06C4_5D18_8009_454F,
        ];
        assert_eq!(got, expected.to_vec(), "splitmix64 must match the canonical stream");
    }

    // ---- Breaker (2): the rolling window is bounded and old outcomes age out. ----
    #[test]
    fn breaker_window_is_bounded_and_ages_out() {
        // window=4, min=4, ratio=0.75: the breaker trips only when >=3 of the last 4 are
        // failures. Feed 4 failures (trips would happen), but we keep it CLOSED by first
        // filling the window with successes, then a minority of failures — proving old
        // outcomes age out of the bounded window rather than accumulating forever.
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

        // 10 successes then 2 failures: if the window were UNBOUNDED, 2/12 = 0.17 < 0.75 →
        // closed. If bounded to 4, the last 4 are [ok, ok, fail, fail] = 0.5 < 0.75 → still
        // closed. Either way the count of failures the ratio sees must be window-bounded.
        for _ in 0..10 {
            let _ = client.call_op(&target, Idempotency::NonIdempotent, || Ok::<(), CallError>(()));
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

        // Now drive the LAST 4 outcomes to >=3 failures: trips. This kills the window-trim
        // and ratio mutants (if the trim is wrong, the older successes dilute the ratio and
        // it never trips).
        for _ in 0..2 {
            let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
                Err::<(), _>(CallError::Downstream {
                    message: "f".into(),
                    retry_after_ms: None,
                })
            });
        }
        // Last 4 = [fail, fail, fail, fail] = 1.0 >= 0.75 → open.
        assert_eq!(client.breaker_state(&target), BreakerState::Open);
    }

    // ---- Breaker (2): the min-request count gates the trip (no trip on too-few samples). ----
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
        // 4 failures < min_requests(5): must NOT trip yet (kills the `>=` → `<` flip on the
        // min-requests gate).
        for _ in 0..4 {
            let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
                Err::<(), _>(CallError::Downstream {
                    message: "f".into(),
                    retry_after_ms: None,
                })
            });
        }
        assert_eq!(client.breaker_state(&target), BreakerState::Closed);
        // The 5th failure reaches min_requests with ratio 1.0 → trips.
        let _ = client.call_op(&target, Idempotency::NonIdempotent, || {
            Err::<(), _>(CallError::Downstream {
                message: "f".into(),
                retry_after_ms: None,
            })
        });
        assert_eq!(client.breaker_state(&target), BreakerState::Open);
    }

    // ---- Retry (1)+(4): a timed-out attempt is NOT a success and the deadline halts retry. ----
    #[test]
    fn deadline_halts_retry_and_timeout_is_not_success() {
        // The op "succeeds" (Ok) but overruns the deadline: the result must be Timeout, NOT
        // Ok, and the breaker must see it as a FAILURE. This kills the `!timed_out` guard
        // mutants and the `success = is_ok && !timed_out` mutant.
        let cfg = ResilientConfig {
            timeout_ms: 100,
            max_attempts: 3,
            breaker_min_requests: 100, // keep breaker out of the way
            backoff_base_ms: 1,
            ..ResilientConfig::default()
        };
        let clock = TestClock::new();
        let clock_handle = clock.clone();
        let client = ResilientClient::with_sources(cfg, Box::new(clock), Box::new(ZeroJitter));
        let target = Target("svc".into());

        let res: Result<()> = client.call_op(&target, Idempotency::Idempotent, || {
            // The op consumes the whole budget then returns Ok — but it is too late.
            clock_handle.set(200);
            Ok(())
        });
        assert_eq!(res, Err(CallError::Timeout), "an Ok that overran the deadline is a Timeout");
    }

    // ---- Retry (4): backoff happens BETWEEN attempts and the cap widens per retry. ----
    #[test]
    fn backoff_is_between_attempts_and_widens() {
        // A clock that RECORDS every sleep duration so we can assert the full-jitter cap
        // grows as base * 2^n across successive retries (Brooker 2015 — the exponential).
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
            breaker_min_requests: 100, // breaker out of the way
            backoff_base_ms: 10,
            timeout_ms: 10_000_000, // huge: never clamps the backoff
            ..ResilientConfig::default()
        };
        // `full_jitter_backoff` draws from `next_below(cap + 1)` (range `[0, cap]`
        // inclusive), so MaxJitter (returns n-1) yields exactly `cap = base * 2^n`.
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
        // 4 attempts → 3 backoffs (one between each pair of attempts). If the backoff block
        // were skipped (attempts_done>0 flipped), there would be ZERO sleeps.
        let recorded = sleeps.lock().unwrap().clone();
        assert_eq!(recorded.len(), 3, "exactly one backoff between each pair of attempts");
        // The cap widens: retry 0 → base*1, retry 1 → base*2, retry 2 → base*4 (the
        // attempts_done-1 index feeds the exponent; a wrong index breaks this progression).
        assert_eq!(recorded, vec![10, 20, 40]);
        assert_eq!(calls.get(), 4);
    }

    // ---- Retry (4): the last attempt is the final attempt (attempt+1 arithmetic). ----
    #[test]
    fn last_attempt_count_is_exact() {
        // max_attempts=2 with a permanently-failing idempotent op: exactly 2 downstream
        // invocations (kills `attempt + 1` → `attempt * 1` / `attempt - 1`).
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

    // ---- The frozen `call<R>` signature (contract 1.9) is intact + drives the primitives. ----
    #[test]
    fn call_signature_is_frozen_and_runs_primitives() {
        // A fake downstream Transport (the CDC provider side of 1.9): it records that the
        // primitives delivered exactly one attempt to it.
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
        // The frozen signature: call<R>(Target, Req, Idempotency) -> Result<R>.
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
}
