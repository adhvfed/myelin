use myelin_events::{CorrelationId, EventEnvelope};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchAdmission {
    Admitted,
    Dropped,
}

impl DispatchAdmission {
    pub fn is_admitted(self) -> bool {
        matches!(self, DispatchAdmission::Admitted)
    }
}

#[derive(Clone, Debug)]
pub struct DispatchPool {
    capacity: u32,
    in_flight: u32,
    drops: u64,
}

impl DispatchPool {
    pub fn new(capacity: u32) -> DispatchPool {
        DispatchPool {
            capacity,
            in_flight: 0,
            drops: 0,
        }
    }

    pub fn try_dispatch(&mut self) -> DispatchAdmission {
        if self.in_flight < self.capacity {
            self.in_flight += 1;
            DispatchAdmission::Admitted
        } else {
            self.drops += 1;
            DispatchAdmission::Dropped
        }
    }

    pub fn complete(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    pub fn in_flight(&self) -> u32 {
        self.in_flight
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    pub fn dispatch_pool_drops(&self) -> u64 {
        self.drops
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepthVerdict {
    Admit,
    AdmitFlagged,
    Halt,
}

impl DepthVerdict {
    pub fn is_admitted(self) -> bool {
        matches!(self, DepthVerdict::Admit | DepthVerdict::AdmitFlagged)
    }

    pub fn is_halted(self) -> bool {
        matches!(self, DepthVerdict::Halt)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DepthCeiling {
    soft: u32,
    hard: u32,
    histogram: [u64; Self::HIST_BUCKETS],
    halts: u64,
}

impl DepthCeiling {
    pub const HIST_BUCKETS: usize = 32;

    pub const V1_SOFT: u32 = 12;

    pub const V1_HARD: u32 = 16;

    pub fn v1_floor() -> DepthCeiling {
        DepthCeiling::new(Self::V1_SOFT, Self::V1_HARD)
    }

    pub fn new(soft: u32, hard: u32) -> DepthCeiling {
        debug_assert!(soft <= hard, "the soft ceiling must be <= the hard ceiling");
        DepthCeiling {
            soft,
            hard,
            histogram: [0; Self::HIST_BUCKETS],
            halts: 0,
        }
    }

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

    pub fn soft(&self) -> u32 {
        self.soft
    }

    pub fn hard(&self) -> u32 {
        self.hard
    }

    pub fn histogram_bucket(&self, depth: u32) -> u64 {
        self.histogram[(depth as usize).min(Self::HIST_BUCKETS - 1)]
    }

    pub fn halts(&self) -> u64 {
        self.halts
    }

    pub fn max_observed_depth(&self) -> u32 {
        (0..Self::HIST_BUCKETS)
            .rev()
            .find(|&b| self.histogram[b] > 0)
            .map(|b| b as u32)
            .unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TripwireVerdict {
    Admit,
    Fired,
}

impl TripwireVerdict {
    pub fn is_fired(self) -> bool {
        matches!(self, TripwireVerdict::Fired)
    }
}

#[derive(Clone, Debug)]
pub struct SharedRootTripwire {
    window: usize,
    threshold: usize,
    recent: std::collections::VecDeque<CorrelationId>,
    quarantined: std::collections::HashSet<CorrelationId>,
    firings: u64,
}

impl SharedRootTripwire {
    pub const V1_WINDOW: usize = 64;

    pub const V1_THRESHOLD: usize = 16;

    pub fn v1_floor() -> SharedRootTripwire {
        SharedRootTripwire::new(Self::V1_WINDOW, Self::V1_THRESHOLD)
    }

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

    pub fn record(&mut self, envelope: &EventEnvelope) -> TripwireVerdict {
        let root = envelope.correlation_id.clone();

        if self.quarantined.contains(&root) {
            self.firings += 1;
            return TripwireVerdict::Fired;
        }

        self.recent.push_back(root.clone());
        while self.recent.len() > self.window {
            self.recent.pop_front();
        }

        let count = self.recent.iter().filter(|r| **r == root).count();
        if count >= self.threshold {
            self.quarantined.insert(root);
            self.firings += 1;
            TripwireVerdict::Fired
        } else {
            TripwireVerdict::Admit
        }
    }

    pub fn is_quarantined(&self, root: &CorrelationId) -> bool {
        self.quarantined.contains(root)
    }

    pub fn tripwire_fired(&self) -> u64 {
        self.firings
    }

    pub fn window(&self) -> usize {
        self.window
    }

    pub fn threshold(&self) -> usize {
        self.threshold
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PredicateVerdict {
    WithinBudget,
    OverBudget(BudgetBreach),
}

impl PredicateVerdict {
    pub fn is_within_budget(self) -> bool {
        matches!(self, PredicateVerdict::WithinBudget)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetBreach {
    Steps,
    Time,
}

#[derive(Clone, Copy, Debug)]
pub struct PredicateGuard {
    max_steps: u64,
    max_eval_micros: u64,
    rejections: u64,
}

impl PredicateGuard {
    pub const V1_MAX_STEPS: u64 = 256;

    pub const V1_MAX_EVAL_MICROS: u64 = 2_000;

    pub fn v1_floor() -> PredicateGuard {
        PredicateGuard::new(Self::V1_MAX_STEPS, Self::V1_MAX_EVAL_MICROS)
    }

    pub fn new(max_steps: u64, max_eval_micros: u64) -> PredicateGuard {
        PredicateGuard {
            max_steps,
            max_eval_micros,
            rejections: 0,
        }
    }

    pub fn admit_static(&mut self, steps: u64) -> PredicateVerdict {
        if steps > self.max_steps {
            self.rejections += 1;
            PredicateVerdict::OverBudget(BudgetBreach::Steps)
        } else {
            PredicateVerdict::WithinBudget
        }
    }

    pub fn check_runtime(&mut self, eval_micros: u64) -> PredicateVerdict {
        if eval_micros > self.max_eval_micros {
            self.rejections += 1;
            PredicateVerdict::OverBudget(BudgetBreach::Time)
        } else {
            PredicateVerdict::WithinBudget
        }
    }

    pub fn max_steps(&self) -> u64 {
        self.max_steps
    }

    pub fn max_eval_micros(&self) -> u64 {
        self.max_eval_micros
    }

    pub fn rejections(&self) -> u64 {
        self.rejections
    }
}

#[derive(Clone, Debug)]
pub struct AgentLoadGuard {
    pub pool: DispatchPool,
    pub depth: DepthCeiling,
    pub tripwire: SharedRootTripwire,
    pub predicate: PredicateGuard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardOutcome {
    Dispatch,
    HaltedByDepth,
    HaltedByTripwire,
    HaltedByPool,
}

impl AgentLoadGuard {
    pub fn v1_floor(pool_capacity: u32) -> AgentLoadGuard {
        AgentLoadGuard {
            pool: DispatchPool::new(pool_capacity),
            depth: DepthCeiling::v1_floor(),
            tripwire: SharedRootTripwire::v1_floor(),
            predicate: PredicateGuard::v1_floor(),
        }
    }

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

    pub fn complete(&mut self) {
        self.pool.complete();
    }

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentLoadSignals {
    pub dispatch_pool_drops: u64,
    pub causal_depth_halts: u64,
    pub max_observed_depth: u32,
    pub tripwire_fired: u64,
    pub predicate_rejections: u64,
}

impl AgentLoadSignals {
    pub fn causal_depth_firings(&self) -> u64 {
        self.causal_depth_halts + self.tripwire_fired
    }
}

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

    #[test]
    fn dispatch_pool_drops_over_cap_rather_than_forking() {
        let mut pool = DispatchPool::new(2);
        assert_eq!(pool.try_dispatch(), DispatchAdmission::Admitted);
        assert_eq!(pool.try_dispatch(), DispatchAdmission::Admitted);
        assert_eq!(
            pool.try_dispatch(),
            DispatchAdmission::Dropped,
            "an over-cap reaction is dropped, never forked into a new worker (§7.4)"
        );
        assert_eq!(pool.in_flight(), 2, "in-flight never exceeds the bound");
        assert_eq!(
            pool.dispatch_pool_drops(),
            1,
            "the drop is counted (contract-1.8)"
        );
        pool.complete();
        assert_eq!(
            pool.try_dispatch(),
            DispatchAdmission::Admitted,
            "a completed permit is reusable"
        );
    }

    #[test]
    fn dispatch_pool_complete_saturates_at_zero() {
        let mut pool = DispatchPool::new(1);
        pool.complete();
        assert_eq!(pool.in_flight(), 0, "a stray completion never wraps");
        assert_eq!(pool.try_dispatch(), DispatchAdmission::Admitted);
    }

    #[test]
    fn depth_ceiling_admits_flags_then_halts_at_the_hard_ceiling() {
        let mut ceiling = DepthCeiling::v1_floor();
        assert_eq!(ceiling.soft(), 12);
        assert_eq!(ceiling.hard(), 16);

        assert_eq!(ceiling.evaluate(&reaction(0, "r")), DepthVerdict::Admit);
        assert_eq!(ceiling.evaluate(&reaction(11, "r")), DepthVerdict::Admit);
        assert_eq!(
            ceiling.evaluate(&reaction(12, "r")),
            DepthVerdict::AdmitFlagged
        );
        assert_eq!(
            ceiling.evaluate(&reaction(15, "r")),
            DepthVerdict::AdmitFlagged
        );
        assert_eq!(ceiling.evaluate(&reaction(16, "r")), DepthVerdict::Halt);
        assert_eq!(ceiling.evaluate(&reaction(20, "r")), DepthVerdict::Halt);
        assert_eq!(ceiling.halts(), 2);
    }

    #[test]
    fn a_constructed_loop_is_halted_at_the_depth_ceiling_and_the_histogram_is_bounded() {
        let mut ceiling = DepthCeiling::new(12, 16);
        let mut halted = 0u64;
        for depth in 0..40u32 {
            if ceiling.evaluate(&reaction(depth, "loop-root")).is_halted() {
                halted += 1;
            }
        }
        assert_eq!(halted, 24);
        assert_eq!(ceiling.halts(), 24);
        assert!(
            ceiling.max_observed_depth() < DepthCeiling::HIST_BUCKETS as u32,
            "the depth histogram is bounded (no unbounded climb)"
        );
    }

    #[test]
    fn shared_root_tripwire_fires_when_too_many_reactions_share_one_root() {
        let mut tw = SharedRootTripwire::new(8, 4);
        let root = CorrelationId("hot-root".into());
        assert_eq!(tw.record(&reaction(1, "hot-root")), TripwireVerdict::Admit);
        assert_eq!(tw.record(&reaction(1, "hot-root")), TripwireVerdict::Admit);
        assert_eq!(tw.record(&reaction(1, "hot-root")), TripwireVerdict::Admit);
        assert_eq!(tw.record(&reaction(1, "hot-root")), TripwireVerdict::Fired);
        assert!(tw.is_quarantined(&root));
        assert_eq!(tw.record(&reaction(1, "hot-root")), TripwireVerdict::Fired);
        assert!(tw.tripwire_fired() >= 1);
    }

    #[test]
    fn shared_root_tripwire_does_not_fire_on_diverse_roots() {
        let mut tw = SharedRootTripwire::new(8, 4);
        for i in 0..8 {
            assert_eq!(
                tw.record(&reaction(1, &format!("root-{i}"))),
                TripwireVerdict::Admit,
                "diverse roots are normal traffic - the tripwire must not fire"
            );
        }
        assert_eq!(tw.tripwire_fired(), 0);
    }

    #[test]
    fn shared_root_tripwire_window_slides_so_old_reactions_age_out() {
        let mut tw = SharedRootTripwire::new(4, 3);
        for _ in 0..3 {
            assert_eq!(tw.record(&reaction(1, "hot")), TripwireVerdict::Admit);
            assert_eq!(tw.record(&reaction(1, "cold-a")), TripwireVerdict::Admit);
            assert_eq!(tw.record(&reaction(1, "cold-b")), TripwireVerdict::Admit);
        }
        assert_eq!(
            tw.tripwire_fired(),
            0,
            "interleaved roots age out of the window"
        );
    }

    #[test]
    fn predicate_guard_rejects_an_over_cost_matcher() {
        let mut guard = PredicateGuard::new(256, 2_000);
        assert_eq!(guard.admit_static(10), PredicateVerdict::WithinBudget);
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
        assert_eq!(guard.check_runtime(500), PredicateVerdict::WithinBudget);
        assert_eq!(
            guard.check_runtime(5_000),
            PredicateVerdict::OverBudget(BudgetBreach::Time),
            "a predicate that runs past the time ceiling is aborted (the runtime backstop)"
        );
    }

    #[test]
    fn composed_guard_halts_a_deep_chain_by_depth() {
        let mut guard = AgentLoadGuard::v1_floor(64);
        assert_eq!(
            guard.admit(&reaction(16, "deep")),
            GuardOutcome::HaltedByDepth
        );
        assert_eq!(
            guard.pool.in_flight(),
            0,
            "a depth-halted reaction takes no permit"
        );
        assert_eq!(guard.signals().causal_depth_halts, 1);
    }

    #[test]
    fn composed_guard_halts_a_wide_fanout_by_tripwire() {
        let mut guard = AgentLoadGuard {
            pool: DispatchPool::new(1000),
            depth: DepthCeiling::new(12, 16),
            tripwire: SharedRootTripwire::new(8, 4),
            predicate: PredicateGuard::v1_floor(),
        };
        for _ in 0..3 {
            assert_eq!(guard.admit(&reaction(2, "fan")), GuardOutcome::Dispatch);
        }
        assert_eq!(
            guard.admit(&reaction(2, "fan")),
            GuardOutcome::HaltedByTripwire
        );
        assert_eq!(guard.signals().tripwire_fired, 1);
        assert_eq!(
            guard.pool.in_flight(),
            3,
            "the tripped reaction takes no permit"
        );
    }

    #[test]
    fn composed_guard_halts_a_concurrency_surge_by_pool() {
        let mut guard = AgentLoadGuard {
            pool: DispatchPool::new(2),
            depth: DepthCeiling::new(12, 16),
            tripwire: SharedRootTripwire::new(64, 16),
            predicate: PredicateGuard::v1_floor(),
        };
        assert_eq!(guard.admit(&reaction(1, "a")), GuardOutcome::Dispatch);
        assert_eq!(guard.admit(&reaction(1, "b")), GuardOutcome::Dispatch);
        assert_eq!(guard.admit(&reaction(1, "c")), GuardOutcome::HaltedByPool);
        assert_eq!(guard.signals().dispatch_pool_drops, 1);
    }

    #[test]
    fn signals_combine_depth_halts_and_tripwire_firings_into_causal_depth_firings() {
        let mut guard = AgentLoadGuard::v1_floor(64);
        guard.admit(&reaction(16, "deep"));
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
