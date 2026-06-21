//! # `loopsafety` — the §6.2 loop-safety enforcement (P-FLOW-18, FLOW-D7)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/durable-workflow.md`
//! §6.2 (loop safety: causal-depth ceiling + shared-root tripwire + bounded activity pool — *an
//! adversarial workflow→event→workflow loop is dropped/parked, NEVER forked*) + §3.1 (the causality
//! columns `correlation_id`/`causation_id`/`caused_by`/`depth` on `workflow_run`) + §5.4 (the
//! causal-depth-histogram telemetry). Phase-3 §6.2 (AG-6: *a human cannot typo into a loop*).
//!
//! **Contract:** 9.2 `WfCtx` — the loop-safety enforcement HALF (no new owned row; this HARDENS the
//! surface against self-feeding loops). 1.8 — the causal-depth-histogram telemetry leg (added to
//! [`crate::FlowTelemetry`]).
//!
//! ## The three mechanisms (§6.2)
//!
//! 1. **Causal-depth ceiling.** Every `workflow_run` carries an inherited `depth` (§3.1). A workflow
//!    that would START a child run (or dispatch a job that begets a child workflow) at `depth + 1`
//!    past the [`CEILING`] is REFUSED — the in-engine ceiling. The child is **dropped/parked, never
//!    forked**. *A runaway self-spawning chain stops at the ceiling.*
//!
//! 2. **Shared-root tripwire.** A workflow→event→workflow loop re-enters the SAME `correlation_id`
//!    root over and over (the depth alone climbs slowly if each hop fans wide). The tripwire counts
//!    how many starts have shared one root within a sliding window; past [`SHARED_ROOT_WINDOW_CAP`]
//!    the breaker TRIPS — further same-root starts are dropped/parked. *A loop that stays shallow but
//!    wide is caught by the root, not the depth.*
//!
//! 3. **Bounded activity pool.** The concurrent-activity count is BOUNDED ([`ACTIVITY_POOL_CAP`]);
//!    a would-be activity over the cap is SHED/PARKED, never forked into an unbounded fan-out (X-3:
//!    *a mention storm cannot fan out unboundedly*).
//!
//! ## The ONE invariant: drop/park, NEVER fork
//!
//! Every refusal is a [`LoopVerdict::Drop`] or [`LoopVerdict::Park`] — there is **no `Fork` variant**.
//! The FLOW-D7 green artifact is the causal-depth signal staying `<=` the ceiling PLUS a **0-fork
//! counter** ([`crate::FlowTelemetry::fork_count`] `== 0`). A mutant that forks instead of
//! dropping/parking, or that lets the depth ceiling be exceeded, MUST be caught by the mutation floor.

use crate::FlowTelemetry;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// **The causal-depth ceiling (§6.2, AG-6).** A workflow at this depth may NOT start a child (which
/// would be `depth + 1`). The in-engine loop bound: a self-spawning chain stops here. Chosen so a
/// legitimate deep automation (an agent run that schedules a CI pipeline that fans a few jobs) is well
/// within bound, while a runaway self-feeding loop hits it fast. The bus's dispatch-tier ceiling
/// (event-bus §4.7) is the cross-subsystem mirror; this is the in-engine one the `workflow_run.depth`
/// reads.
pub const CEILING: u32 = 32;

/// **The shared-root window cap (§6.2).** The maximum number of workflow STARTS that may share one
/// `correlation_id` root within the tripwire's sliding window before the breaker trips. A
/// workflow→event→workflow loop re-enters the same root each hop; past this many same-root starts the
/// tripwire fires (the loop is caught by its ROOT even if each hop stays shallow). A legitimate fan
/// (one trigger → a handful of child workflows under one root) is well under cap.
pub const SHARED_ROOT_WINDOW_CAP: u32 = 64;

/// **The bounded activity pool cap (§6.2 / X-3).** The maximum number of CONCURRENT (admitted-not-yet-
/// released) activities the pool admits at once. A would-be activity over this cap is SHED/PARKED, not
/// forked — the fan-out is bounded (a mention storm cannot fan unboundedly). Per-engine (a real
/// deployment tunes it per cell); the in-isolation drill exercises the cap directly.
pub const ACTIVITY_POOL_CAP: u32 = 256;

/// **The loop-safety verdict — drop/park, NEVER fork (§6.2).** Every refusal stops the runaway: a
/// [`Drop`](Self::Drop) sheds the would-be hop outright; a [`Park`](Self::Park) holds it (the run
/// stays `waiting`, to be retried when pressure relents). There is deliberately **no `Fork` variant**
/// — the whole posture is that a self-feeding loop is stopped, never multiplied. The 0-fork counter is
/// the structural proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopVerdict {
    /// the hop is ADMITTED — within the depth ceiling, under the shared-root window, and under the
    /// activity-pool cap. The child run / activity may proceed at the carried `depth`.
    Admit,
    /// the hop is DROPPED — the depth ceiling was hit OR the shared-root tripwire fired. The runaway
    /// chain is shed outright (never forked); the telemetry records the ceiling-hit / tripwire-firing.
    Drop,
    /// the hop is PARKED — the bounded activity pool is at cap. The would-be activity is held (shed/
    /// parked, never forked); it is admitted later when an in-flight activity releases a slot.
    Park,
}

impl LoopVerdict {
    /// `true` iff the hop was admitted (not dropped/parked).
    pub fn is_admit(&self) -> bool {
        matches!(self, LoopVerdict::Admit)
    }
    /// `true` iff the hop was refused (dropped OR parked) — the loop was stopped.
    pub fn is_refused(&self) -> bool {
        !self.is_admit()
    }
}

/// Why a hop was refused — the machine reason the audit records (no PII). Surfaced so a refusal is
/// observable, never a silent drop (EI-02 §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalReason {
    /// the causal-depth ceiling was hit (`depth + 1 > CEILING`).
    DepthCeiling,
    /// the shared-root tripwire fired (too many starts shared one correlation root in the window).
    SharedRootTripwire,
    /// the bounded activity pool is at cap (over-cap → shed/park).
    ActivityPoolFull,
}

/// **The §6.2 loop-safety guard — the three mechanisms over one shared state.** A cloneable handle
/// (an `Arc<Mutex<…>>`) the engine consults on each would-be CHILD start (the depth ceiling + the
/// shared-root tripwire) and each would-be ACTIVITY admit (the bounded pool). Every refusal is a
/// drop/park; nothing ever forks. The verdicts feed the [`FlowTelemetry`] loop-safety signals
/// (causal-depth histogram, depth-ceiling hits, tripwire firings, pool sheds, the 0-fork counter).
#[derive(Clone)]
pub struct CausalGuard {
    inner: Arc<Mutex<GuardInner>>,
    telemetry: Option<FlowTelemetry>,
    ceiling: u32,
    shared_root_cap: u32,
    pool_cap: u32,
}

#[derive(Default)]
struct GuardInner {
    /// per-`correlation_id` count of starts seen within the window (the shared-root tripwire's
    /// sliding tally — the engine's window is the run-of-the-loop; a real cell ages it out).
    root_starts: HashMap<String, u32>,
    /// the count of currently-admitted (not-yet-released) activities — the bounded-pool gauge.
    activities_in_flight: u32,
}

impl CausalGuard {
    /// A fresh guard at the default ceilings ([`CEILING`] / [`SHARED_ROOT_WINDOW_CAP`] /
    /// [`ACTIVITY_POOL_CAP`]), no telemetry wired.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(GuardInner::default())),
            telemetry: None,
            ceiling: CEILING,
            shared_root_cap: SHARED_ROOT_WINDOW_CAP,
            pool_cap: ACTIVITY_POOL_CAP,
        }
    }

    /// A guard at EXPLICIT caps — the in-isolation drill drives small caps so the adversarial loop
    /// hits them fast (a depth ceiling of 4, a pool cap of 2) without spawning thousands of hops.
    pub fn with_caps(ceiling: u32, shared_root_cap: u32, pool_cap: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(GuardInner::default())),
            telemetry: None,
            ceiling,
            shared_root_cap,
            pool_cap,
        }
    }

    /// Wire the [`FlowTelemetry`] the guard feeds (the causal-depth histogram + the loop-safety
    /// counters). Builder-style.
    pub fn with_telemetry(mut self, telemetry: FlowTelemetry) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    /// the configured causal-depth ceiling.
    pub fn ceiling(&self) -> u32 {
        self.ceiling
    }

    /// **Admit (or refuse) a would-be CHILD start at `correlation_id` root, parent depth `parent_depth`
    /// (§6.2).** The child would carry `parent_depth + 1` (§3.1). Two mechanisms gate it, in order:
    ///
    /// 1. **the causal-depth ceiling** — if `parent_depth + 1 > ceiling`, the chain has run too deep:
    ///    [`LoopVerdict::Drop`] (`DepthCeiling`), `depth_ceiling_hits += 1`. NEVER forked.
    /// 2. **the shared-root tripwire** — if this root has been started past `shared_root_cap` times in
    ///    the window, a wide same-root loop is in progress: [`LoopVerdict::Drop`]
    ///    (`SharedRootTripwire`), `shared_root_tripwire_firings += 1`. NEVER forked.
    ///
    /// On ADMIT it observes the child depth into the causal-depth histogram (§5.4) and bumps the
    /// root's start tally. Returns the verdict + (on refusal) the machine reason.
    pub fn admit_child(
        &self,
        correlation_id: &str,
        parent_depth: u32,
    ) -> (LoopVerdict, Option<RefusalReason>) {
        let child_depth = parent_depth.saturating_add(1);

        // (1) the causal-depth ceiling — the in-engine loop bound (§3.1 `depth`). A child past the
        //     ceiling is DROPPED, never forked. Checked FIRST so a deep chain is stopped before the
        //     histogram/root-tally even count it (the histogram observes admitted depths only).
        if child_depth > self.ceiling {
            if let Some(t) = &self.telemetry {
                t.record_depth_ceiling_hit();
            }
            return (LoopVerdict::Drop, Some(RefusalReason::DepthCeiling));
        }

        // (2) the shared-root tripwire — a wide same-root loop (§6.2). Read the current tally BEFORE
        //     incrementing: if admitting this start would push the root past the cap, the breaker
        //     trips and the start is DROPPED (never forked).
        {
            let mut inner = self.lock();
            let seen = inner.root_starts.get(correlation_id).copied().unwrap_or(0);
            if seen >= self.shared_root_cap {
                drop(inner);
                if let Some(t) = &self.telemetry {
                    t.record_shared_root_tripwire_firing();
                }
                return (LoopVerdict::Drop, Some(RefusalReason::SharedRootTripwire));
            }
            inner.root_starts.insert(correlation_id.to_string(), seen + 1);
        }

        // ADMIT: observe the admitted child depth into the §5.4 histogram + advance the max.
        if let Some(t) = &self.telemetry {
            t.observe_causal_depth(child_depth, self.ceiling);
        }
        (LoopVerdict::Admit, None)
    }

    /// **Admit (or refuse) a would-be ACTIVITY into the bounded pool (§6.2 / X-3).** If the pool is at
    /// cap the activity is SHED/PARKED ([`LoopVerdict::Park`], `ActivityPoolFull`), `activity_pool_sheds
    /// += 1` — over-cap fan-out is bounded, never forked. On ADMIT the in-flight count rises by one;
    /// the caller MUST [`release_activity`](Self::release_activity) when the activity terminates so the
    /// slot frees. Returns the verdict + (on refusal) the reason.
    pub fn admit_activity(&self) -> (LoopVerdict, Option<RefusalReason>) {
        let mut inner = self.lock();
        if inner.activities_in_flight >= self.pool_cap {
            drop(inner);
            if let Some(t) = &self.telemetry {
                t.record_activity_pool_shed();
            }
            return (LoopVerdict::Park, Some(RefusalReason::ActivityPoolFull));
        }
        inner.activities_in_flight += 1;
        (LoopVerdict::Admit, None)
    }

    /// Release one admitted activity (it terminated) — frees a pool slot so a parked activity can be
    /// admitted. Saturating (never underflows below 0).
    pub fn release_activity(&self) {
        let mut inner = self.lock();
        inner.activities_in_flight = inner.activities_in_flight.saturating_sub(1);
    }

    /// The current concurrent-activity count (the bounded-pool gauge) — for the drill's pool assertion.
    pub fn activities_in_flight(&self) -> u32 {
        self.lock().activities_in_flight
    }

    /// The number of starts seen for `correlation_id` in the window (the shared-root tripwire tally) —
    /// for the drill's tripwire assertion.
    pub fn root_starts(&self, correlation_id: &str) -> u32 {
        self.lock().root_starts.get(correlation_id).copied().unwrap_or(0)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GuardInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for CausalGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The verdict predicates are exact** — `is_admit`/`is_refused` partition the three variants:
    /// only `Admit` is an admit; both `Drop` and `Park` are refusals (the loop was stopped).
    #[test]
    fn verdict_predicates_partition_admit_from_refused() {
        assert!(LoopVerdict::Admit.is_admit());
        assert!(!LoopVerdict::Admit.is_refused(), "an admit is NOT a refusal");
        assert!(!LoopVerdict::Drop.is_admit());
        assert!(LoopVerdict::Drop.is_refused(), "a drop IS a refusal");
        assert!(!LoopVerdict::Park.is_admit());
        assert!(LoopVerdict::Park.is_refused(), "a park IS a refusal");
    }

    /// **The depth ceiling halts a self-feeding loop AT the ceiling (§6.2).** An adversarial chain
    /// that keeps starting a child at `depth + 1` is admitted up to the ceiling, then the next hop is
    /// DROPPED — the depth never exceeds the ceiling, and the run is dropped (never forked).
    #[test]
    fn depth_ceiling_halts_self_feeding_loop_at_ceiling() {
        let telemetry = FlowTelemetry::new();
        let guard = CausalGuard::with_caps(4, 1_000, 1_000).with_telemetry(telemetry.clone());
        let root = "corr-loop";

        // Walk the chain: parent depth 0 → child 1 → ... admitted while child_depth <= 4.
        let mut depth = 0u32;
        let mut admitted = 0u32;
        let mut dropped = 0u32;
        for _ in 0..20 {
            let (verdict, reason) = guard.admit_child(root, depth);
            match verdict {
                LoopVerdict::Admit => {
                    admitted += 1;
                    depth += 1; // the loop self-feeds: the child becomes the next parent.
                }
                LoopVerdict::Drop => {
                    dropped += 1;
                    assert_eq!(reason, Some(RefusalReason::DepthCeiling));
                    break;
                }
                LoopVerdict::Park => panic!("the depth ceiling drops, it does not park"),
            }
        }

        // parent 0→child 1, …, parent 3→child 4 are admitted (4 hops); parent 4→child 5 > ceiling 4
        // is DROPPED. The depth never exceeds the ceiling.
        assert_eq!(admitted, 4, "admitted exactly up to the ceiling (children 1..=4)");
        assert_eq!(dropped, 1, "the next hop past the ceiling was dropped");
        assert!(
            telemetry.causal_depth_max() <= guard.ceiling(),
            "the causal-depth max never exceeds the ceiling (it was stopped AT it)"
        );
        assert_eq!(telemetry.causal_depth_max(), 4, "the deepest admitted child was at the ceiling");
        assert_eq!(telemetry.depth_ceiling_hits(), 1, "the ceiling fired exactly once");
        assert_eq!(telemetry.fork_count(), 0, "NEVER forked — the headline invariant");
    }

    /// **The shared-root tripwire detects a workflow→event→workflow loop (§6.2).** A loop that stays
    /// SHALLOW (each hop at depth 0/1) but re-enters the SAME correlation root over and over is caught
    /// by the ROOT, not the depth: past the window cap the tripwire fires (drop, never fork).
    #[test]
    fn shared_root_tripwire_detects_wf_event_wf_loop() {
        let telemetry = FlowTelemetry::new();
        // a generous depth ceiling so depth NEVER fires — only the root tripwire can stop this loop.
        let guard = CausalGuard::with_caps(1_000, 3, 1_000).with_telemetry(telemetry.clone());
        let root = "corr-shared";

        let mut admitted = 0u32;
        let mut tripped = 0u32;
        for _ in 0..10 {
            // every hop shares ONE root at a shallow depth (1) — the depth ceiling cannot catch it.
            let (verdict, reason) = guard.admit_child(root, 1);
            match verdict {
                LoopVerdict::Admit => admitted += 1,
                LoopVerdict::Drop => {
                    tripped += 1;
                    assert_eq!(reason, Some(RefusalReason::SharedRootTripwire));
                }
                LoopVerdict::Park => panic!("the tripwire drops, it does not park"),
            }
        }

        assert_eq!(admitted, 3, "the first 3 same-root starts were admitted (the window cap)");
        assert_eq!(tripped, 7, "every same-root start past the cap tripped the tripwire");
        assert_eq!(telemetry.depth_ceiling_hits(), 0, "the depth ceiling NEVER fired (the loop stayed shallow)");
        assert!(telemetry.shared_root_tripwire_firings() >= 1, "the tripwire fired");
        assert_eq!(telemetry.shared_root_tripwire_firings(), 7, "fired once per over-cap start");
        assert_eq!(telemetry.fork_count(), 0, "NEVER forked");
    }

    /// **The bounded activity pool caps concurrent activities (§6.2 / X-3).** Admitting up to the cap
    /// succeeds; the next is SHED/PARKED (never forked). Releasing one frees a slot for a parked one.
    #[test]
    fn bounded_activity_pool_caps_concurrency() {
        let telemetry = FlowTelemetry::new();
        let guard = CausalGuard::with_caps(1_000, 1_000, 2).with_telemetry(telemetry.clone());

        let (v1, _) = guard.admit_activity();
        let (v2, _) = guard.admit_activity();
        assert_eq!(v1, LoopVerdict::Admit);
        assert_eq!(v2, LoopVerdict::Admit);
        assert_eq!(guard.activities_in_flight(), 2, "the pool is at cap");

        // the 3rd over-cap activity is SHED/PARKED, never forked.
        let (v3, r3) = guard.admit_activity();
        assert_eq!(v3, LoopVerdict::Park, "over-cap → park, never fork");
        assert_eq!(r3, Some(RefusalReason::ActivityPoolFull));
        assert_eq!(telemetry.activity_pool_sheds(), 1, "one shed recorded");

        // release one → a slot frees → the next admits.
        guard.release_activity();
        assert_eq!(guard.activities_in_flight(), 1);
        let (v4, _) = guard.admit_activity();
        assert_eq!(v4, LoopVerdict::Admit, "a freed slot admits the next activity");
        assert_eq!(telemetry.fork_count(), 0, "NEVER forked");
    }

    /// **Distinct correlation roots do NOT share the tripwire tally.** Two independent automations
    /// under DIFFERENT roots each get their own window — one busy root does not starve another.
    #[test]
    fn distinct_roots_have_independent_tripwire_tallies() {
        let guard = CausalGuard::with_caps(1_000, 2, 1_000);
        // root A: 2 admits then trip.
        assert!(guard.admit_child("A", 0).0.is_admit());
        assert!(guard.admit_child("A", 0).0.is_admit());
        assert!(guard.admit_child("A", 0).0.is_refused(), "A tripped at its cap");
        // root B is untouched by A's tripwire.
        assert!(guard.admit_child("B", 0).0.is_admit(), "B has its own window");
        assert!(guard.admit_child("B", 0).0.is_admit());
        assert_eq!(guard.root_starts("A"), 2);
        assert_eq!(guard.root_starts("B"), 2);
    }
}
