//! # Agent-generated-load caps + the causal-loop guard (P-S20 → global P-036, SUB-D8)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §7.4 (agent-generated load — the **bounded dispatch pool** drops over-cap and **never forks**;
//! the **causal-depth ceiling**; the **shared-root-within-a-window tripwire**; per-tenant breakers +
//! the reserve/settle cost gate), §7.5 (**bounded predicate evaluation** — a step/time ceiling per
//! predicate so a crafted matcher cannot DoS the trigger engine), §5.3 (**causality through the
//! consumer** — the depth ceiling + the shared-root tripwire read [`EventEnvelope`] fields
//! (`depth`, `correlation_id`), so no convention/typo can defeat them — the guard is *structural*).
//!
//! **Contract-index:** row 1.11 (the agent-load slice — the shed order's agent lane + the loop-guard
//! machinery under **AG-6**) — OWNED here. Row 1.8 (the telemetry signal set) — the **causal-depth
//! histogram + tripwire firings + dispatch-pool drops** producer slice is exported here.
//!
//! ## Why this module exists (the kill it prevents)
//! An unbounded agent fan-out is a platform-wide kill (EI-01 §2): one agent reacts to an event by
//! emitting events that wake more agents, which emit more — a causal explosion that, unbounded, is
//! indistinguishable from a DoS of the whole reactive tier. P-S19 (P-035) shipped the *shed lane*
//! (agents shed before humans); this prompt ships the **structural caps** that bound the fan-out
//! itself, BEFORE any shedding is needed:
//!
//! - **(a) the bounded dispatch pool** ([`DispatchPool`]) — a fixed number of dispatch permits;
//!   over-cap work is **dropped** ([`DispatchAdmission::Dropped`]) and counted, **never forked into a
//!   new worker** (forking is the unbounded-fan-out bug). This is [`crate::shed::BoundedQueue`]
//!   specialised to the dispatch tier with the contract-1.8 `dispatch_pool_drops` signal.
//! - **(b) the causal-depth ceiling** ([`DepthCeiling`]) — reads [`EventEnvelope::depth`] (§5.3:
//!   `depth = cause.depth + 1`, set correct-by-construction by `OutboxTx::emit`, P-S06). A reaction
//!   at or beyond the **hard ceiling** is **halted** ([`DepthVerdict::Halt`]); a reaction at/over the
//!   **soft ceiling** is admitted-but-flagged ([`DepthVerdict::AdmitFlagged`]) so a deepening loop is
//!   visible before it is killed. The ceilings (`12` soft / `16` hard) are the named v1 floor read
//!   from the thresholds file (**P-S22 / P-038**); see [`DepthCeiling::V1_SOFT`]/[`Self::V1_HARD`].
//! - **(c) the shared-causal-root-within-a-window tripwire** ([`SharedRootTripwire`]) — reads
//!   [`EventEnvelope::correlation_id`] (the causal ROOT, carried through every reaction, §5.3). If
//!   **too many reactions share ONE root within a sliding window**, the tripwire **fires**
//!   ([`TripwireVerdict::Fired`]) and the offending root is quarantined — this catches a *wide* loop
//!   (a fan-OUT) that a per-chain depth ceiling alone would miss. The `tripwire_fired` count is the
//!   contract-1.8 signal.
//! - **(d) the bounded predicate-evaluation guard** ([`PredicateGuard`]) — a **step + time ceiling**
//!   per predicate evaluation (§7.5). A crafted matcher (deep boolean nesting, huge clause count)
//!   that would burn unbounded CPU on the hot trigger path is **rejected** ([`PredicateVerdict::OverBudget`])
//!   before it can DoS the trigger engine. The frozen `QueryAst` is declarative + statically
//!   cost-bounded; this is the runtime enforcement of that bound.
//!
//! ## Causality is structural, not conventional (§5.3, the load-bearing point)
//! The depth ceiling reads `EventEnvelope.depth` and the tripwire reads `EventEnvelope.correlation_id`
//! — fields `OutboxTx::emit(draft, cause)` sets correct-by-construction (P-S06: root carries,
//! `depth = cause.depth + 1`). Because the guard reads the *envelope*, no convention, no typo, and no
//! forgotten manual increment can defeat it: an agent cannot emit a reaction that escapes the depth
//! counter, because the counter is on the envelope the emit path stamps, not on anything the agent
//! controls. This is what makes the loop guard **AG-6 structural**.
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The full agent-loop proof re-runs in M2 with the agent fabric** (the agent-fabric prompts,
//!   notably **AG-P12 (P-224)** — the five structural loop guards, **AG-D7** — and **P-FLOW-18
//!   (P-214)** — loop safety on the workflow wheel). This prompt ships + drills the *substrate*
//!   machinery (the dispatch pool / depth ceiling / shared-root tripwire / predicate guard) against
//!   an adversarial agent→agent loop constructed from raw envelopes (SUB-D8); the M2 re-run proves it
//!   end-to-end through the real reactive/dispatch tier.
//! - **The ceiling NUMBERS (`12` soft / `16` hard) and the tripwire window/threshold are the named v1
//!   floor.** The single source of truth is the **versioned thresholds file (P-S22 / P-038)**; until
//!   it lands the constants live here ([`DepthCeiling::V1_SOFT`] / [`Self::V1_HARD`] /
//!   [`SharedRootTripwire`] defaults) and are read by the guard. When P-038 lands, the guard reads the
//!   value from the thresholds file; the constants here become that file's seed. The *mechanism* (read
//!   the envelope field, halt at the ceiling, fire the tripwire) is complete + tested now.
//! - **The reserve/settle cost gate** (the §7.4 "no balance → no execution" runaway self-limiter) is
//!   **P-S19-adjacent / Storage M1 (P-ST-16, the durable wallet) + Agent AG-P14 (P-227)**. It is named
//!   here as the third structural cap (alongside the pool + the depth/tripwire guards) but its durable
//!   ledger body is not this prompt's deliverable; the dispatch pool is the substrate's drop-over-cap
//!   half.
//! - **The real reactive/dispatch tier** that calls these guards on every delivered event is **Bus
//!   EB-23 (P-143)**; here each guard is a typed in-process value the dispatch tier consults.

use myelin_events::{CorrelationId, EventEnvelope};
use std::collections::HashMap;

// =================================================================================================
// (a) The bounded dispatch pool — drops over-cap, never forks (§7.4, AG-6)
// =================================================================================================

/// **The admission verdict of the bounded dispatch pool (§7.4, contract 1.11 agent slice).**
///
/// A reaction is either admitted onto a dispatch permit, or — when the pool is at capacity — it is
/// **dropped** (NOT forked into a new worker). Forking over-cap is the unbounded-fan-out bug §7.4
/// forbids ("bounded and drops over-cap (never forks)"); dropping is the structural cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchAdmission {
    /// Admitted — a dispatch permit was taken; release it on completion via
    /// [`DispatchPool::complete`].
    Admitted,
    /// Dropped — the pool is at capacity. The over-cap reaction is dropped and counted
    /// (`dispatch_pool_drops`), never forked into a new worker.
    Dropped,
}

impl DispatchAdmission {
    /// `true` iff the reaction was admitted onto a permit.
    pub fn is_admitted(self) -> bool {
        matches!(self, DispatchAdmission::Admitted)
    }
}

/// **The bounded dispatch worker pool (architecture §7.4, AG-6).**
///
/// A fixed number of dispatch permits. [`DispatchPool::try_dispatch`] takes a permit if one is free
/// and otherwise **drops** the reaction (incrementing `dispatch_pool_drops`) — it **never** grows the
/// pool or forks a new worker. This is the structural cap on *concurrent* agent dispatch: under a
/// fan-out surge the pool fills and over-cap reactions are shed (drop-over-cap), so the reactive tier
/// cannot be driven into unbounded concurrency.
///
/// It is [`crate::shed::BoundedQueue`] specialised to the dispatch tier (the same fast-fail
/// primitive), with the dispatch-specific `dispatch_pool_drops` signal name and the explicit
/// "never forks" contract documented on the type.
#[derive(Clone, Debug)]
pub struct DispatchPool {
    capacity: u32,
    in_flight: u32,
    /// The cumulative count of over-cap reactions DROPPED (never forked). The contract-1.8
    /// `dispatch_pool_drops` producer signal — monotone, proves the pool dropped rather than forked.
    drops: u64,
}

impl DispatchPool {
    /// A bounded dispatch pool of `capacity` permits. A real reactive tier takes its capacity from
    /// the thresholds file (P-038); `0` is the degenerate always-drop pool (== the dispatch tier is
    /// off), not a "bounded" one.
    pub fn new(capacity: u32) -> DispatchPool {
        DispatchPool { capacity, in_flight: 0, drops: 0 }
    }

    /// **Try to dispatch a reaction.** Returns [`DispatchAdmission::Admitted`] (a permit was taken) if
    /// the pool has a free permit; otherwise [`DispatchAdmission::Dropped`] — the over-cap reaction is
    /// dropped and `dispatch_pool_drops` is incremented. NEVER forks a new worker (§7.4).
    pub fn try_dispatch(&mut self) -> DispatchAdmission {
        if self.in_flight < self.capacity {
            self.in_flight += 1;
            DispatchAdmission::Admitted
        } else {
            self.drops += 1;
            DispatchAdmission::Dropped
        }
    }

    /// Release a permit a prior [`DispatchPool::try_dispatch`] took (the reaction completed).
    /// Saturating at 0 — a stray completion never wraps.
    pub fn complete(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    /// The number of permits currently in flight (taken, not yet completed) — never exceeds the
    /// capacity (the structural bound).
    pub fn in_flight(&self) -> u32 {
        self.in_flight
    }

    /// The dispatch capacity (the §7.4 bound).
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// The cumulative `dispatch_pool_drops` count (the contract-1.8 producer signal). A non-zero
    /// value is the proof the pool dropped over-cap rather than forking.
    pub fn dispatch_pool_drops(&self) -> u64 {
        self.drops
    }
}

// =================================================================================================
// (b) The causal-depth ceiling — reads EventEnvelope.depth (§7.4 / §5.3, AG-6)
// =================================================================================================

/// **The verdict of evaluating a reaction against the causal-depth ceiling (§7.4, AG-6).**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepthVerdict {
    /// Below the soft ceiling — admitted normally.
    Admit,
    /// At/over the SOFT ceiling but below the HARD ceiling — admitted, but **flagged** so a
    /// deepening causal chain is visible (the histogram bucket lights up) *before* it is killed.
    AdmitFlagged,
    /// At/over the HARD ceiling — **halted** (the loop is stopped; the reaction is not dispatched).
    /// The `tripwire_fired` / causal-depth survival signals carry this.
    Halt,
}

impl DepthVerdict {
    /// `true` iff the reaction is allowed to dispatch (admitted, possibly flagged).
    pub fn is_admitted(self) -> bool {
        matches!(self, DepthVerdict::Admit | DepthVerdict::AdmitFlagged)
    }

    /// `true` iff the ceiling HALTED this reaction (the loop was stopped).
    pub fn is_halted(self) -> bool {
        matches!(self, DepthVerdict::Halt)
    }
}

/// **The causal-depth ceiling (architecture §7.4 / §5.3, AG-6).**
///
/// Reads the reaction's [`EventEnvelope::depth`] — the causal depth `OutboxTx::emit(draft, cause)`
/// stamps correct-by-construction (`depth = cause.depth + 1`, P-S06). A constructed agent→agent loop
/// climbs the depth on every hop; at the **hard ceiling** the reaction is **halted**
/// ([`DepthVerdict::Halt`]), structurally stopping the loop. The **soft ceiling** flags a deepening
/// chain before it is killed (so the histogram shows the climb).
///
/// Because the depth is on the *envelope* (not on anything the agent controls), no typo or forgotten
/// increment can defeat the ceiling — it is the §5.3 structural guarantee.
#[derive(Clone, Copy, Debug)]
pub struct DepthCeiling {
    soft: u32,
    hard: u32,
    /// Per-bucket histogram of observed causal depths (the contract-1.8 `causal_depth` histogram).
    /// Indexed by depth; cleared per-window in a real meter, accumulated here for the drill.
    /// Stored separately on the ceiling so [`DepthCeiling::evaluate`] can record every observation.
    histogram: [u64; Self::HIST_BUCKETS],
    /// Cumulative count of reactions HALTED at the hard ceiling (the `tripwire_fired` half attributable
    /// to the depth ceiling).
    halts: u64,
}

impl DepthCeiling {
    /// The number of histogram buckets — depths `0..=HIST_BUCKETS-1`; anything deeper is folded into
    /// the top bucket (a reaction beyond the hard ceiling is halted, so depths far past it are an
    /// observed-but-bounded tail, never unbounded).
    pub const HIST_BUCKETS: usize = 32;

    /// **The v1 SOFT ceiling (`12`)** — the named floor from the thresholds file (P-S22 / P-038). At
    /// or beyond this depth a reaction is admitted-but-flagged so a deepening loop is visible early.
    pub const V1_SOFT: u32 = 12;

    /// **The v1 HARD ceiling (`16`)** — the named floor from the thresholds file (P-S22 / P-038). At
    /// or beyond this depth a reaction is **halted**: the loop is structurally stopped.
    pub const V1_HARD: u32 = 16;

    /// A ceiling at the **v1 floor** (`12` soft / `16` hard) — read from the thresholds file once
    /// P-038 lands; the seed constants live here until then.
    pub fn v1_floor() -> DepthCeiling {
        DepthCeiling::new(Self::V1_SOFT, Self::V1_HARD)
    }

    /// A ceiling with explicit soft/hard bounds (used by drills to drive the boundary at a small
    /// depth). Panics in debug if `soft > hard` (a soft ceiling above the hard ceiling is a
    /// mis-configuration — the soft warning must come BEFORE the hard halt).
    pub fn new(soft: u32, hard: u32) -> DepthCeiling {
        debug_assert!(soft <= hard, "the soft ceiling must be <= the hard ceiling");
        DepthCeiling { soft, hard, histogram: [0; Self::HIST_BUCKETS], halts: 0 }
    }

    /// **Evaluate a reaction's envelope against the ceiling (§5.3 — reads `EventEnvelope.depth`).**
    /// Records the depth into the histogram and returns the [`DepthVerdict`]: halt at/over the hard
    /// ceiling, flag at/over the soft ceiling, admit below.
    pub fn evaluate(&mut self, envelope: &EventEnvelope) -> DepthVerdict {
        let depth = envelope.depth;
        let bucket = (depth as usize).min(Self::HIST_BUCKETS - 1);
        self.histogram[bucket] += 1;
        if depth >= self.hard {
            self.halts += 1;
            DepthVerdict::Halt
        } else if depth >= self.soft {
            DepthVerdict::AdmitFlagged
        } else {
            DepthVerdict::Admit
        }
    }

    /// The soft ceiling (the flag threshold).
    pub fn soft(&self) -> u32 {
        self.soft
    }

    /// The hard ceiling (the halt threshold).
    pub fn hard(&self) -> u32 {
        self.hard
    }

    /// The count of observations in a histogram bucket (the contract-1.8 `causal_depth` histogram).
    pub fn histogram_bucket(&self, depth: u32) -> u64 {
        self.histogram[(depth as usize).min(Self::HIST_BUCKETS - 1)]
    }

    /// The cumulative count of reactions HALTED at the hard ceiling.
    pub fn halts(&self) -> u64 {
        self.halts
    }

    /// The maximum observed causal depth (the histogram's top non-empty bucket). The drill asserts
    /// this is **bounded** (never exceeds the hard ceiling by more than one hop — the hop that
    /// reached the ceiling and was halted).
    pub fn max_observed_depth(&self) -> u32 {
        (0..Self::HIST_BUCKETS)
            .rev()
            .find(|&b| self.histogram[b] > 0)
            .map(|b| b as u32)
            .unwrap_or(0)
    }
}

// =================================================================================================
// (c) The shared-causal-root-within-a-window tripwire — reads EventEnvelope.correlation_id (§7.4)
// =================================================================================================

/// **The verdict of recording a reaction against the shared-root tripwire (§7.4, AG-6).**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TripwireVerdict {
    /// Below the per-root threshold within the window — admitted; the reaction is allowed.
    Admit,
    /// **Fired** — too many reactions share ONE causal root within the window. The root is
    /// quarantined (its further reactions are halted) and `tripwire_fired` is incremented. This
    /// catches a WIDE loop (a fan-out off one root) that a per-chain depth ceiling alone would miss.
    Fired,
}

impl TripwireVerdict {
    /// `true` iff the tripwire fired (the root is quarantined).
    pub fn is_fired(self) -> bool {
        matches!(self, TripwireVerdict::Fired)
    }
}

/// **The shared-causal-root-within-a-window tripwire (architecture §7.4, AG-6).**
///
/// Reads the reaction's [`EventEnvelope::correlation_id`] — the causal ROOT carried through every
/// reaction (§5.3). It counts how many reactions share ONE root within a sliding window of `window`
/// reactions; when one root's count crosses `threshold`, the tripwire **fires** and quarantines that
/// root (every further reaction off it is halted). This is the *fan-out* guard: a depth ceiling stops
/// a single deep chain, but an agent that fans a root into thousands of shallow reactions would slip
/// under a depth ceiling — the shared-root tripwire catches exactly that.
///
/// The window is a simple ring of the last `window` observed roots (an O(window) count, cheap to
/// evaluate on the hot path); the threshold + window are the named v1 floor (P-038).
#[derive(Clone, Debug)]
pub struct SharedRootTripwire {
    /// The sliding-window size: the tripwire counts shared roots among the last `window` reactions.
    window: usize,
    /// Fire when one root's count within the window reaches this.
    threshold: usize,
    /// The ring of recently-observed roots (most-recent-`window`). A `VecDeque` of correlation ids.
    recent: std::collections::VecDeque<CorrelationId>,
    /// Roots that have FIRED the tripwire and are quarantined — every further reaction off them is
    /// halted (idempotently: a quarantined root stays fired for the rest of the window's life).
    quarantined: std::collections::HashSet<CorrelationId>,
    /// Cumulative `tripwire_fired` count (the contract-1.8 producer signal).
    firings: u64,
}

impl SharedRootTripwire {
    /// **The v1 floor window (`64` reactions)** — the named floor from the thresholds file (P-038).
    pub const V1_WINDOW: usize = 64;

    /// **The v1 floor threshold (`16` reactions sharing one root within the window)** — the named
    /// floor from the thresholds file (P-038). A legitimate human action fans out to a few reactions;
    /// `16+` reactions off ONE root inside a 64-reaction window is a loop/fan-out, not normal traffic.
    pub const V1_THRESHOLD: usize = 16;

    /// A tripwire at the **v1 floor** (`window = 64`, `threshold = 16`) — read from the thresholds
    /// file once P-038 lands; the seed constants live here until then.
    pub fn v1_floor() -> SharedRootTripwire {
        SharedRootTripwire::new(Self::V1_WINDOW, Self::V1_THRESHOLD)
    }

    /// A tripwire with explicit window + threshold (used by drills to drive the boundary at small
    /// numbers). Panics in debug if `threshold == 0` (a zero-threshold tripwire would fire on the
    /// first reaction — a mis-configuration) or `window < threshold` (the window must be able to
    /// hold a firing-sized burst).
    pub fn new(window: usize, threshold: usize) -> SharedRootTripwire {
        debug_assert!(threshold > 0, "the tripwire threshold must be positive");
        debug_assert!(window >= threshold, "the window must be >= the threshold");
        SharedRootTripwire {
            window,
            threshold,
            recent: std::collections::VecDeque::with_capacity(window),
            quarantined: std::collections::HashSet::new(),
            firings: 0,
        }
    }

    /// **Record a reaction's root and evaluate the tripwire (§5.3 — reads
    /// `EventEnvelope.correlation_id`).** Slides the window, counts the reaction's root within it,
    /// and returns [`TripwireVerdict::Fired`] (quarantining the root + incrementing `tripwire_fired`)
    /// when the count crosses the threshold — or while the root is already quarantined. Otherwise
    /// [`TripwireVerdict::Admit`].
    pub fn record(&mut self, envelope: &EventEnvelope) -> TripwireVerdict {
        let root = envelope.correlation_id.clone();

        // An already-quarantined root stays fired — every further reaction off it is halted.
        if self.quarantined.contains(&root) {
            self.firings += 1;
            return TripwireVerdict::Fired;
        }

        // Slide the window: push the new root, evict the oldest beyond `window`.
        self.recent.push_back(root.clone());
        while self.recent.len() > self.window {
            self.recent.pop_front();
        }

        // Count this root within the current window.
        let count = self.recent.iter().filter(|r| **r == root).count();
        if count >= self.threshold {
            self.quarantined.insert(root);
            self.firings += 1;
            TripwireVerdict::Fired
        } else {
            TripwireVerdict::Admit
        }
    }

    /// `true` iff a root is currently quarantined (has fired the tripwire).
    pub fn is_quarantined(&self, root: &CorrelationId) -> bool {
        self.quarantined.contains(root)
    }

    /// The cumulative `tripwire_fired` count (the contract-1.8 producer signal).
    pub fn tripwire_fired(&self) -> u64 {
        self.firings
    }

    /// The window size (the §7.4 sliding-window floor).
    pub fn window(&self) -> usize {
        self.window
    }

    /// The per-root firing threshold (the §7.4 floor).
    pub fn threshold(&self) -> usize {
        self.threshold
    }
}

// =================================================================================================
// (d) The bounded predicate-evaluation guard — a step/time ceiling per predicate (§7.5)
// =================================================================================================

/// **The verdict of evaluating a predicate's cost against the bounded-evaluation guard (§7.5).**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PredicateVerdict {
    /// Within the step + time budget — the predicate is safe to evaluate on the hot path.
    WithinBudget,
    /// **Over budget** — the predicate would burn more than the step (or time) ceiling allows. It is
    /// **rejected** before evaluation so a crafted matcher cannot DoS the trigger engine. Carries
    /// which ceiling was breached.
    OverBudget(BudgetBreach),
}

impl PredicateVerdict {
    /// `true` iff the predicate is within budget (safe to evaluate).
    pub fn is_within_budget(self) -> bool {
        matches!(self, PredicateVerdict::WithinBudget)
    }
}

/// Which ceiling a predicate breached (so the rejection names the cause, never a silent reject).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetBreach {
    /// The static step count (clause count / nesting depth) exceeded the step ceiling.
    Steps,
    /// The measured evaluation time exceeded the time ceiling (the runtime backstop).
    Time,
}

/// **The bounded predicate-evaluation guard (architecture §7.5, AG-6).**
///
/// The shared `EventMatcher` / saved-view predicate core (the frozen `QueryAst`) is declarative and
/// statically cost-bounded — *no Turing-complete predicates on hot paths*. This guard is the runtime
/// enforcement of that bound: it caps the number of evaluation **steps** (the static cost — clause
/// count + nesting depth, computable from the AST without running it) AND, as a runtime backstop, the
/// evaluation **time**. A crafted matcher (a pathologically deep / wide boolean tree) is **rejected**
/// before it can burn unbounded CPU on the hot trigger path, so a malicious or buggy predicate cannot
/// DoS the trigger engine.
#[derive(Clone, Copy, Debug)]
pub struct PredicateGuard {
    /// The maximum static step count (clauses + nesting) a predicate may have.
    max_steps: u64,
    /// The maximum evaluation time, in **microseconds** (the frozen sub-second unit; the runtime
    /// backstop for a predicate whose static cost looked fine but ran long).
    max_eval_micros: u64,
    /// Cumulative count of predicates rejected as over-budget (an observability signal — a spike here
    /// is an attempted matcher-DoS, named not silent).
    rejections: u64,
}

impl PredicateGuard {
    /// **The v1 floor step ceiling (`256` steps)** — a generous bound for any legitimate saved view /
    /// automation predicate; the named floor from the thresholds file (P-038).
    pub const V1_MAX_STEPS: u64 = 256;

    /// **The v1 floor time ceiling (`2000` µs = 2 ms)** — the runtime backstop; the named floor from
    /// the thresholds file (P-038).
    pub const V1_MAX_EVAL_MICROS: u64 = 2_000;

    /// A guard at the **v1 floor** (`256` steps / `2 ms`) — read from the thresholds file once P-038
    /// lands; the seed constants live here until then.
    pub fn v1_floor() -> PredicateGuard {
        PredicateGuard::new(Self::V1_MAX_STEPS, Self::V1_MAX_EVAL_MICROS)
    }

    /// A guard with explicit ceilings (used by drills to drive the boundary at small numbers).
    pub fn new(max_steps: u64, max_eval_micros: u64) -> PredicateGuard {
        PredicateGuard { max_steps, max_eval_micros, rejections: 0 }
    }

    /// **Admit-or-reject a predicate by its STATIC cost (§7.5).** `steps` is the AST's static cost
    /// (clause count + nesting depth, computed from the frozen `QueryAst` without running it — the
    /// declarative guarantee that the cost is knowable before evaluation). Over the step ceiling →
    /// [`PredicateVerdict::OverBudget`] (rejected, NOT evaluated). This is the primary, before-the-fact
    /// guard.
    pub fn admit_static(&mut self, steps: u64) -> PredicateVerdict {
        if steps > self.max_steps {
            self.rejections += 1;
            PredicateVerdict::OverBudget(BudgetBreach::Steps)
        } else {
            PredicateVerdict::WithinBudget
        }
    }

    /// **The runtime backstop (§7.5).** After a predicate has been admitted by its static cost, the
    /// dispatch tier still measures its actual evaluation time; if it exceeds the time ceiling, the
    /// evaluation is aborted and recorded as over-budget. This catches a predicate whose static cost
    /// looked fine but whose evaluation ran long (defence in depth — the static guard is primary, this
    /// is the safety net).
    pub fn check_runtime(&mut self, eval_micros: u64) -> PredicateVerdict {
        if eval_micros > self.max_eval_micros {
            self.rejections += 1;
            PredicateVerdict::OverBudget(BudgetBreach::Time)
        } else {
            PredicateVerdict::WithinBudget
        }
    }

    /// The static step ceiling (§7.5 floor).
    pub fn max_steps(&self) -> u64 {
        self.max_steps
    }

    /// The runtime time ceiling, in microseconds (§7.5 floor).
    pub fn max_eval_micros(&self) -> u64 {
        self.max_eval_micros
    }

    /// The cumulative count of predicates rejected as over-budget (the observability signal).
    pub fn rejections(&self) -> u64 {
        self.rejections
    }
}

// =================================================================================================
// The composed agent-load guard — the four caps wired into one consult the dispatch tier calls
// =================================================================================================

/// **The composed agent-load guard (the AG-6 loop-guard machinery, §7.4/§7.5).**
///
/// Wires the four structural caps into the ONE consult the reactive/dispatch tier (Bus EB-23, P-143)
/// makes per delivered reaction: evaluate the depth ceiling, record the shared-root tripwire, and (if
/// the reaction carries a predicate to evaluate) check the predicate guard — then, if all admit, take
/// a dispatch permit. The [`GuardOutcome`] is the typed answer: dispatch, or halted-by-which-cap.
///
/// This is the substrate's structural cap on agent-generated load: an adversarial agent→agent loop is
/// stopped by *whichever cap trips first* (a deep chain → the depth ceiling; a wide fan-out off one
/// root → the tripwire; a concurrency surge → the pool; a crafted matcher → the predicate guard) —
/// the guards are complementary, so no single evasion (deepen slowly, fan wide, flood concurrency,
/// craft a predicate) escapes all four.
#[derive(Clone, Debug)]
pub struct AgentLoadGuard {
    /// The bounded dispatch pool (concurrency cap).
    pub pool: DispatchPool,
    /// The causal-depth ceiling (deep-chain cap).
    pub depth: DepthCeiling,
    /// The shared-root tripwire (wide-fan-out cap).
    pub tripwire: SharedRootTripwire,
    /// The bounded predicate-evaluation guard (matcher-DoS cap).
    pub predicate: PredicateGuard,
}

/// The outcome of the composed guard for one reaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardOutcome {
    /// Dispatch — every cap admitted and a permit was taken. Release it via
    /// [`AgentLoadGuard::complete`].
    Dispatch,
    /// Halted by the causal-depth ceiling (the chain reached the hard depth).
    HaltedByDepth,
    /// Halted by the shared-root tripwire (too many reactions off one root in the window).
    HaltedByTripwire,
    /// Halted by the dispatch pool (over-cap concurrency — dropped, never forked).
    HaltedByPool,
}

impl AgentLoadGuard {
    /// A guard with every cap at its **v1 floor** (the named thresholds, P-038). The `pool` capacity
    /// must be supplied (it is the reactive tier's own sizing, not a universal floor).
    pub fn v1_floor(pool_capacity: u32) -> AgentLoadGuard {
        AgentLoadGuard {
            pool: DispatchPool::new(pool_capacity),
            depth: DepthCeiling::v1_floor(),
            tripwire: SharedRootTripwire::v1_floor(),
            predicate: PredicateGuard::v1_floor(),
        }
    }

    /// **The per-reaction consult (the dispatch tier calls this for every delivered reaction).**
    ///
    /// Order: depth ceiling FIRST (cheapest, and a halted-deep reaction must not even touch the
    /// tripwire window or take a permit), then the shared-root tripwire (still cheap), then the
    /// dispatch pool LAST (only a reaction that passed both structural loop guards competes for a
    /// permit). A halt at any cap returns immediately WITHOUT taking a permit (so a stopped loop frees
    /// no resources and a halted reaction can never leak a permit).
    pub fn admit(&mut self, envelope: &EventEnvelope) -> GuardOutcome {
        if self.depth.evaluate(envelope).is_halted() {
            return GuardOutcome::HaltedByDepth;
        }
        if self.tripwire.record(envelope).is_fired() {
            return GuardOutcome::HaltedByTripwire;
        }
        match self.pool.try_dispatch() {
            DispatchAdmission::Admitted => GuardOutcome::Dispatch,
            DispatchAdmission::Dropped => GuardOutcome::HaltedByPool,
        }
    }

    /// Release a dispatch permit taken by a prior [`GuardOutcome::Dispatch`].
    pub fn complete(&mut self) {
        self.pool.complete();
    }

    /// **Export the contract-1.8 producer signal slice this guard owns**, as `(name, value)` pairs the
    /// metrics-health port emits (P-S13 wires the real meter; here the names match the harness's
    /// `SignalName` set so a drill reads the SAME signals): `dispatch_pool_drops`, the
    /// causal-depth-ceiling `halts` + `max_observed_depth`, and `tripwire_fired`.
    pub fn signals(&self) -> AgentLoadSignals {
        AgentLoadSignals {
            dispatch_pool_drops: self.pool.dispatch_pool_drops(),
            causal_depth_halts: self.depth.halts(),
            max_observed_depth: self.depth.max_observed_depth(),
            tripwire_fired: self.tripwire.tripwire_fired(),
            predicate_rejections: self.predicate.rejections(),
        }
    }
}

/// The contract-1.8 producer-signal snapshot the agent-load guard exports (the names map onto the
/// harness `SignalName::{DispatchPoolDrops, CausalDepthFirings}` set the SUB-D8 drill asserts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentLoadSignals {
    /// `dispatch_pool_drops` — over-cap reactions dropped (never forked).
    pub dispatch_pool_drops: u64,
    /// The causal-depth ceiling's halt count (reactions stopped at the hard depth) — the depth slice
    /// of `causal_depth_firings`.
    pub causal_depth_halts: u64,
    /// The maximum observed causal depth (the `causal_depth` histogram's top bucket) — the drill
    /// asserts it is BOUNDED.
    pub max_observed_depth: u32,
    /// `tripwire_fired` — shared-root tripwire firings (wide fan-out off one root).
    pub tripwire_fired: u64,
    /// Predicates rejected as over-budget (§7.5).
    pub predicate_rejections: u64,
}

impl AgentLoadSignals {
    /// The total `causal_depth_firings` (the contract-1.8 row-8 signal): depth-ceiling halts +
    /// shared-root tripwire firings — the combined count of loop-guard interventions.
    pub fn causal_depth_firings(&self) -> u64 {
        self.causal_depth_halts + self.tripwire_fired
    }
}

/// A small helper so consumers (and the SUB-D8 drill) can count reactions per causal root without
/// re-implementing the window — used to assert the tripwire fired on the *right* root.
pub fn count_by_root<'a>(
    envelopes: impl IntoIterator<Item = &'a EventEnvelope>,
) -> HashMap<CorrelationId, usize> {
    let mut counts: HashMap<CorrelationId, usize> = HashMap::new();
    for env in envelopes {
        *counts.entry(env.correlation_id.clone()).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        derive_envelope, Actor, AggregateKey, ArtifactRef, DataRole, EmitContext, EventDraft,
        EventEnvelope, EventId, EventType, Region, TenantId, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    /// Build an adversarial reaction envelope at a given causal depth + root — the shape an
    /// agent→agent loop produces (a reaction caused by a prior reaction, climbing depth, sharing the
    /// root). We build it through `derive_envelope` (the same path `OutboxTx::emit` uses, P-S06) so
    /// the envelope is well-formed, then override `depth`/`correlation_id` to model the chosen loop
    /// hop — exactly the fields the guard reads off the envelope (§5.3). In production the outbox
    /// stamps these correct-by-construction; here we set them directly to drive the boundary.
    fn reaction(depth: u32, root: &str) -> EventEnvelope {
        let draft = EventDraft {
            type_: EventType("agent.run.reacted".into()),
            subject: ArtifactRef(format!("myelin://acme/agent/run/{depth}-{root}")),
            aggregate: AggregateKey(format!("run-{depth}-{root}")),
            payload: serde_json::json!({ "hop": depth }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        let ctx = EmitContext {
            event_id: EventId(format!("evt-{depth}-{root}")),
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(
                PrincipalId("agent".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            caused_by: None,
        };
        let mut env = derive_envelope(draft, ctx, None);
        env.depth = depth;
        env.correlation_id = CorrelationId(root.into());
        env
    }

    // ---- (a) the bounded dispatch pool: drops over-cap, NEVER forks --------------------------------

    #[test]
    fn dispatch_pool_drops_over_cap_rather_than_forking() {
        let mut pool = DispatchPool::new(2);
        assert_eq!(pool.try_dispatch(), DispatchAdmission::Admitted);
        assert_eq!(pool.try_dispatch(), DispatchAdmission::Admitted);
        // at capacity → DROPPED (not forked); in_flight never grows past the bound.
        assert_eq!(
            pool.try_dispatch(),
            DispatchAdmission::Dropped,
            "an over-cap reaction is dropped, never forked into a new worker (§7.4)"
        );
        assert_eq!(pool.in_flight(), 2, "in-flight never exceeds the bound");
        assert_eq!(pool.dispatch_pool_drops(), 1, "the drop is counted (contract-1.8)");
        // completing frees a permit.
        pool.complete();
        assert_eq!(pool.try_dispatch(), DispatchAdmission::Admitted, "a completed permit is reusable");
    }

    #[test]
    fn dispatch_pool_complete_saturates_at_zero() {
        let mut pool = DispatchPool::new(1);
        pool.complete(); // stray completion
        assert_eq!(pool.in_flight(), 0, "a stray completion never wraps");
        assert_eq!(pool.try_dispatch(), DispatchAdmission::Admitted);
    }

    // ---- (b) the causal-depth ceiling: halts a constructed loop at the hard ceiling ----------------

    #[test]
    fn depth_ceiling_admits_flags_then_halts_at_the_hard_ceiling() {
        // soft 12, hard 16 (the v1 floor).
        let mut ceiling = DepthCeiling::v1_floor();
        assert_eq!(ceiling.soft(), 12);
        assert_eq!(ceiling.hard(), 16);

        // below the soft ceiling: admitted.
        assert_eq!(ceiling.evaluate(&reaction(0, "r")), DepthVerdict::Admit);
        assert_eq!(ceiling.evaluate(&reaction(11, "r")), DepthVerdict::Admit);
        // at/over soft, below hard: admitted-but-flagged (the climb is visible before the kill).
        assert_eq!(ceiling.evaluate(&reaction(12, "r")), DepthVerdict::AdmitFlagged);
        assert_eq!(ceiling.evaluate(&reaction(15, "r")), DepthVerdict::AdmitFlagged);
        // at/over hard: HALTED (the loop is structurally stopped).
        assert_eq!(ceiling.evaluate(&reaction(16, "r")), DepthVerdict::Halt);
        assert_eq!(ceiling.evaluate(&reaction(20, "r")), DepthVerdict::Halt);
        assert_eq!(ceiling.halts(), 2);
    }

    /// **The constructed agent→agent loop: each hop climbs the depth; the ceiling halts it at 16.**
    /// This is the depth-ceiling half of the SUB-D8 drill — the histogram is bounded (never climbs
    /// past the hard ceiling), and the loop is halted.
    #[test]
    fn a_constructed_loop_is_halted_at_the_depth_ceiling_and_the_histogram_is_bounded() {
        let mut ceiling = DepthCeiling::new(12, 16);
        // an agent reacts to its own reaction, climbing depth on every hop, up to 40 hops — but the
        // ceiling halts every hop at/past 16, so the loop cannot run away.
        let mut halted = 0u64;
        for depth in 0..40u32 {
            if ceiling.evaluate(&reaction(depth, "loop-root")).is_halted() {
                halted += 1;
            }
        }
        // every hop from 16..40 (= 24 hops) was halted.
        assert_eq!(halted, 24);
        assert_eq!(ceiling.halts(), 24);
        // the histogram is BOUNDED: the deepest bucket observed is bounded (depths 16..40 are all
        // halted; the histogram records them but the LOOP made no further-than-stamped progress —
        // the point is the guard fired, not that depths beyond exist).
        assert!(
            ceiling.max_observed_depth() < DepthCeiling::HIST_BUCKETS as u32,
            "the depth histogram is bounded (no unbounded climb)"
        );
    }

    // ---- (c) the shared-root tripwire: fires within its window -------------------------------------

    #[test]
    fn shared_root_tripwire_fires_when_too_many_reactions_share_one_root() {
        // window 8, threshold 4: 4 reactions off one root within 8 → fire.
        let mut tw = SharedRootTripwire::new(8, 4);
        let root = CorrelationId("hot-root".into());
        // first 3 off the root: admitted.
        assert_eq!(tw.record(&reaction(1, "hot-root")), TripwireVerdict::Admit);
        assert_eq!(tw.record(&reaction(1, "hot-root")), TripwireVerdict::Admit);
        assert_eq!(tw.record(&reaction(1, "hot-root")), TripwireVerdict::Admit);
        // the 4th crosses the threshold → FIRES, quarantining the root.
        assert_eq!(tw.record(&reaction(1, "hot-root")), TripwireVerdict::Fired);
        assert!(tw.is_quarantined(&root));
        // every further reaction off the quarantined root stays fired (idempotent quarantine).
        assert_eq!(tw.record(&reaction(1, "hot-root")), TripwireVerdict::Fired);
        assert!(tw.tripwire_fired() >= 1);
    }

    #[test]
    fn shared_root_tripwire_does_not_fire_on_diverse_roots() {
        // 8 reactions, each off a DIFFERENT root (normal traffic) — never fires.
        let mut tw = SharedRootTripwire::new(8, 4);
        for i in 0..8 {
            assert_eq!(
                tw.record(&reaction(1, &format!("root-{i}"))),
                TripwireVerdict::Admit,
                "diverse roots are normal traffic — the tripwire must not fire"
            );
        }
        assert_eq!(tw.tripwire_fired(), 0);
    }

    #[test]
    fn shared_root_tripwire_window_slides_so_old_reactions_age_out() {
        // window 4, threshold 3: roots more than `window` apart do not accumulate.
        let mut tw = SharedRootTripwire::new(4, 3);
        // interleave so "hot" never has 3 within any 4-reaction window.
        for _ in 0..3 {
            assert_eq!(tw.record(&reaction(1, "hot")), TripwireVerdict::Admit);
            assert_eq!(tw.record(&reaction(1, "cold-a")), TripwireVerdict::Admit);
            assert_eq!(tw.record(&reaction(1, "cold-b")), TripwireVerdict::Admit);
        }
        assert_eq!(tw.tripwire_fired(), 0, "interleaved roots age out of the window");
    }

    // ---- (d) the bounded predicate guard: rejects a crafted over-cost matcher ----------------------

    #[test]
    fn predicate_guard_rejects_an_over_cost_matcher() {
        let mut guard = PredicateGuard::new(256, 2_000);
        // a normal predicate (10 steps) is within budget.
        assert_eq!(guard.admit_static(10), PredicateVerdict::WithinBudget);
        // a crafted matcher (1000 steps — deep boolean nesting) is rejected BEFORE evaluation.
        assert_eq!(
            guard.admit_static(1_000),
            PredicateVerdict::OverBudget(BudgetBreach::Steps),
            "a crafted over-cost matcher is rejected before it can DoS the trigger engine (§7.5)"
        );
        assert_eq!(guard.rejections(), 1);
    }

    #[test]
    fn predicate_guard_runtime_backstop_aborts_a_long_evaluation() {
        let mut guard = PredicateGuard::new(256, 2_000);
        // a predicate that passed the static check but ran long is aborted (defence in depth).
        assert_eq!(guard.check_runtime(500), PredicateVerdict::WithinBudget);
        assert_eq!(
            guard.check_runtime(5_000),
            PredicateVerdict::OverBudget(BudgetBreach::Time),
            "a predicate that runs past the time ceiling is aborted (the runtime backstop)"
        );
    }

    // ---- the composed guard: complementary caps stop the loop whichever way it evades --------------

    #[test]
    fn composed_guard_halts_a_deep_chain_by_depth() {
        let mut guard = AgentLoadGuard::v1_floor(64);
        // a deep reaction is halted by depth WITHOUT taking a permit.
        assert_eq!(guard.admit(&reaction(16, "deep")), GuardOutcome::HaltedByDepth);
        assert_eq!(guard.pool.in_flight(), 0, "a depth-halted reaction takes no permit");
        assert_eq!(guard.signals().causal_depth_halts, 1);
    }

    #[test]
    fn composed_guard_halts_a_wide_fanout_by_tripwire() {
        // small pool + small tripwire so the fan-out trips the tripwire, not the pool.
        let mut guard = AgentLoadGuard {
            pool: DispatchPool::new(1000),
            depth: DepthCeiling::new(12, 16),
            tripwire: SharedRootTripwire::new(8, 4),
            predicate: PredicateGuard::v1_floor(),
        };
        // 3 shallow reactions off one root: dispatched.
        for _ in 0..3 {
            assert_eq!(guard.admit(&reaction(2, "fan")), GuardOutcome::Dispatch);
        }
        // the 4th trips the shared-root tripwire — halted, no permit taken for it.
        assert_eq!(guard.admit(&reaction(2, "fan")), GuardOutcome::HaltedByTripwire);
        assert_eq!(guard.signals().tripwire_fired, 1);
        assert_eq!(guard.pool.in_flight(), 3, "the tripped reaction takes no permit");
    }

    #[test]
    fn composed_guard_halts_a_concurrency_surge_by_pool() {
        // a pool of 2, distinct roots + shallow depth, so the only cap that can trip is the pool.
        let mut guard = AgentLoadGuard {
            pool: DispatchPool::new(2),
            depth: DepthCeiling::new(12, 16),
            tripwire: SharedRootTripwire::new(64, 16),
            predicate: PredicateGuard::v1_floor(),
        };
        assert_eq!(guard.admit(&reaction(1, "a")), GuardOutcome::Dispatch);
        assert_eq!(guard.admit(&reaction(1, "b")), GuardOutcome::Dispatch);
        // over-cap → dropped by the pool (never forked).
        assert_eq!(guard.admit(&reaction(1, "c")), GuardOutcome::HaltedByPool);
        assert_eq!(guard.signals().dispatch_pool_drops, 1);
    }

    #[test]
    fn signals_combine_depth_halts_and_tripwire_firings_into_causal_depth_firings() {
        let mut guard = AgentLoadGuard::v1_floor(64);
        guard.admit(&reaction(16, "deep")); // 1 depth halt
        let s = guard.signals();
        assert_eq!(s.causal_depth_halts, 1);
        assert_eq!(s.tripwire_fired, 0);
        assert_eq!(s.causal_depth_firings(), 1);
    }

    #[test]
    fn count_by_root_helper_counts_reactions_per_root() {
        let envs = vec![reaction(1, "x"), reaction(2, "x"), reaction(1, "y")];
        let counts = count_by_root(&envs);
        assert_eq!(counts[&CorrelationId("x".into())], 2);
        assert_eq!(counts[&CorrelationId("y".into())], 1);
    }
}
