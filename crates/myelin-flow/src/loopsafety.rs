use crate::FlowTelemetry;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const CEILING: u32 = 32;

pub const SHARED_ROOT_WINDOW_CAP: u32 = 64;

pub const ACTIVITY_POOL_CAP: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopVerdict {
    Admit,
    Drop,
    Park,
}

impl LoopVerdict {
    pub fn is_admit(&self) -> bool {
        matches!(self, LoopVerdict::Admit)
    }
    pub fn is_refused(&self) -> bool {
        !self.is_admit()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalReason {
    DepthCeiling,
    SharedRootTripwire,
    ActivityPoolFull,
}

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
    root_starts: HashMap<String, u32>,
    activities_in_flight: u32,
}

impl CausalGuard {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(GuardInner::default())),
            telemetry: None,
            ceiling: CEILING,
            shared_root_cap: SHARED_ROOT_WINDOW_CAP,
            pool_cap: ACTIVITY_POOL_CAP,
        }
    }

    pub fn with_caps(ceiling: u32, shared_root_cap: u32, pool_cap: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(GuardInner::default())),
            telemetry: None,
            ceiling,
            shared_root_cap,
            pool_cap,
        }
    }

    pub fn with_telemetry(mut self, telemetry: FlowTelemetry) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    pub fn ceiling(&self) -> u32 {
        self.ceiling
    }

    pub fn admit_child(
        &self,
        correlation_id: &str,
        parent_depth: u32,
    ) -> (LoopVerdict, Option<RefusalReason>) {
        let child_depth = parent_depth.saturating_add(1);

        if child_depth > self.ceiling {
            if let Some(t) = &self.telemetry {
                t.record_depth_ceiling_hit();
            }
            return (LoopVerdict::Drop, Some(RefusalReason::DepthCeiling));
        }

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
            inner
                .root_starts
                .insert(correlation_id.to_string(), seen + 1);
        }

        if let Some(t) = &self.telemetry {
            t.observe_causal_depth(child_depth, self.ceiling);
        }
        (LoopVerdict::Admit, None)
    }

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

    pub fn release_activity(&self) {
        let mut inner = self.lock();
        inner.activities_in_flight = inner.activities_in_flight.saturating_sub(1);
    }

    pub fn activities_in_flight(&self) -> u32 {
        self.lock().activities_in_flight
    }

    pub fn root_starts(&self, correlation_id: &str) -> u32 {
        self.lock()
            .root_starts
            .get(correlation_id)
            .copied()
            .unwrap_or(0)
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

    #[test]
    fn verdict_predicates_partition_admit_from_refused() {
        assert!(LoopVerdict::Admit.is_admit());
        assert!(
            !LoopVerdict::Admit.is_refused(),
            "an admit is NOT a refusal"
        );
        assert!(!LoopVerdict::Drop.is_admit());
        assert!(LoopVerdict::Drop.is_refused(), "a drop IS a refusal");
        assert!(!LoopVerdict::Park.is_admit());
        assert!(LoopVerdict::Park.is_refused(), "a park IS a refusal");
    }

    #[test]
    fn depth_ceiling_halts_self_feeding_loop_at_ceiling() {
        let telemetry = FlowTelemetry::new();
        let guard = CausalGuard::with_caps(4, 1_000, 1_000).with_telemetry(telemetry.clone());
        let root = "corr-loop";

        let mut depth = 0u32;
        let mut admitted = 0u32;
        let mut dropped = 0u32;
        for _ in 0..20 {
            let (verdict, reason) = guard.admit_child(root, depth);
            match verdict {
                LoopVerdict::Admit => {
                    admitted += 1;
                    depth += 1;
                }
                LoopVerdict::Drop => {
                    dropped += 1;
                    assert_eq!(reason, Some(RefusalReason::DepthCeiling));
                    break;
                }
                LoopVerdict::Park => panic!("the depth ceiling drops, it does not park"),
            }
        }

        assert_eq!(
            admitted, 4,
            "admitted exactly up to the ceiling (children 1..=4)"
        );
        assert_eq!(dropped, 1, "the next hop past the ceiling was dropped");
        assert!(
            telemetry.causal_depth_max() <= guard.ceiling(),
            "the causal-depth max never exceeds the ceiling (it was stopped AT it)"
        );
        assert_eq!(
            telemetry.causal_depth_max(),
            4,
            "the deepest admitted child was at the ceiling"
        );
        assert_eq!(
            telemetry.depth_ceiling_hits(),
            1,
            "the ceiling fired exactly once"
        );
        assert_eq!(
            telemetry.fork_count(),
            0,
            "NEVER forked - the headline invariant"
        );
    }

    #[test]
    fn shared_root_tripwire_detects_wf_event_wf_loop() {
        let telemetry = FlowTelemetry::new();
        let guard = CausalGuard::with_caps(1_000, 3, 1_000).with_telemetry(telemetry.clone());
        let root = "corr-shared";

        let mut admitted = 0u32;
        let mut tripped = 0u32;
        for _ in 0..10 {
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

        assert_eq!(
            admitted, 3,
            "the first 3 same-root starts were admitted (the window cap)"
        );
        assert_eq!(
            tripped, 7,
            "every same-root start past the cap tripped the tripwire"
        );
        assert_eq!(
            telemetry.depth_ceiling_hits(),
            0,
            "the depth ceiling NEVER fired (the loop stayed shallow)"
        );
        assert!(
            telemetry.shared_root_tripwire_firings() >= 1,
            "the tripwire fired"
        );
        assert_eq!(
            telemetry.shared_root_tripwire_firings(),
            7,
            "fired once per over-cap start"
        );
        assert_eq!(telemetry.fork_count(), 0, "NEVER forked");
    }

    #[test]
    fn bounded_activity_pool_caps_concurrency() {
        let telemetry = FlowTelemetry::new();
        let guard = CausalGuard::with_caps(1_000, 1_000, 2).with_telemetry(telemetry.clone());

        let (v1, _) = guard.admit_activity();
        let (v2, _) = guard.admit_activity();
        assert_eq!(v1, LoopVerdict::Admit);
        assert_eq!(v2, LoopVerdict::Admit);
        assert_eq!(guard.activities_in_flight(), 2, "the pool is at cap");

        let (v3, r3) = guard.admit_activity();
        assert_eq!(v3, LoopVerdict::Park, "over-cap → park, never fork");
        assert_eq!(r3, Some(RefusalReason::ActivityPoolFull));
        assert_eq!(telemetry.activity_pool_sheds(), 1, "one shed recorded");

        guard.release_activity();
        assert_eq!(guard.activities_in_flight(), 1);
        let (v4, _) = guard.admit_activity();
        assert_eq!(
            v4,
            LoopVerdict::Admit,
            "a freed slot admits the next activity"
        );
        assert_eq!(telemetry.fork_count(), 0, "NEVER forked");
    }

    #[test]
    fn distinct_roots_have_independent_tripwire_tallies() {
        let guard = CausalGuard::with_caps(1_000, 2, 1_000);
        assert!(guard.admit_child("A", 0).0.is_admit());
        assert!(guard.admit_child("A", 0).0.is_admit());
        assert!(
            guard.admit_child("A", 0).0.is_refused(),
            "A tripped at its cap"
        );
        assert!(
            guard.admit_child("B", 0).0.is_admit(),
            "B has its own window"
        );
        assert!(guard.admit_child("B", 0).0.is_admit());
        assert_eq!(guard.root_starts("A"), 2);
        assert_eq!(guard.root_starts("B"), 2);
    }
}
