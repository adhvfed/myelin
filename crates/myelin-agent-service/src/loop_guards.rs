use myelin_content::InlineNode;
use myelin_events::{Actor, EventEnvelope};
use myelin_flow::{CausalGuard, FlowTelemetry, LoopVerdict, RefusalReason};
use myelin_identity::PrincipalId;
use std::collections::BTreeSet;

pub const AGENT_CEILING: u32 = 12;

pub const AGENT_SHARED_ROOT_CAP: u32 = 64;

pub const AGENT_DISPATCH_POOL_CAP: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardVerdict {
    Admit,
    Drop(GuardRefusal),
    Park(GuardRefusal),
}

impl GuardVerdict {
    pub fn is_admit(&self) -> bool {
        matches!(self, GuardVerdict::Admit)
    }
    pub fn is_refused(&self) -> bool {
        !self.is_admit()
    }
    pub fn refusal(&self) -> Option<GuardRefusal> {
        match self {
            GuardVerdict::Admit => None,
            GuardVerdict::Drop(r) | GuardVerdict::Park(r) => Some(*r),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardRefusal {
    SelfTrigger,
    RawTextNotAReference,
    DepthCeiling,
    SharedRootTripwire,
    DispatchPoolFull,
}

impl From<RefusalReason> for GuardRefusal {
    fn from(r: RefusalReason) -> Self {
        match r {
            RefusalReason::DepthCeiling => GuardRefusal::DepthCeiling,
            RefusalReason::SharedRootTripwire => GuardRefusal::SharedRootTripwire,
            RefusalReason::ActivityPoolFull => GuardRefusal::DispatchPoolFull,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SelfGuard {
    agent: PrincipalId,
}

impl SelfGuard {
    pub fn new(agent: PrincipalId) -> SelfGuard {
        SelfGuard { agent }
    }

    pub fn agent(&self) -> &PrincipalId {
        &self.agent
    }

    pub fn admit(&self, actor: &Actor) -> GuardVerdict {
        if actor.0.principal_id == self.agent {
            GuardVerdict::Drop(GuardRefusal::SelfTrigger)
        } else {
            GuardVerdict::Admit
        }
    }

    pub fn admit_envelope(&self, ev: &EventEnvelope) -> GuardVerdict {
        self.admit(&ev.actor)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ReferenceGate;

impl ReferenceGate {
    pub fn new() -> ReferenceGate {
        ReferenceGate
    }

    pub fn admit_node(&self, node: &InlineNode) -> GuardVerdict {
        match node {
            InlineNode::ArtifactRefNode(_) => GuardVerdict::Admit,
            InlineNode::Mention(_) | InlineNode::Embed(_) => {
                GuardVerdict::Drop(GuardRefusal::RawTextNotAReference)
            }
        }
    }

    pub fn admit_raw_text(&self, _text: &str) -> GuardVerdict {
        GuardVerdict::Drop(GuardRefusal::RawTextNotAReference)
    }
}

#[derive(Clone, Debug, Default)]
pub struct IdempotentToolLedger {
    applied: BTreeSet<(String, String)>,
}

impl IdempotentToolLedger {
    pub fn new() -> IdempotentToolLedger {
        IdempotentToolLedger::default()
    }

    pub fn key(run: &str, effect_id: &str) -> (String, String) {
        (run.to_string(), effect_id.to_string())
    }

    pub fn record(&mut self, run: &str, effect_id: &str) -> bool {
        self.applied.insert(Self::key(run, effect_id))
    }

    pub fn contains(&self, run: &str, effect_id: &str) -> bool {
        self.applied.contains(&Self::key(run, effect_id))
    }

    pub fn applies(&self) -> usize {
        self.applied.len()
    }
}

#[derive(Clone)]
pub struct AgentLoopGuards {
    self_guard: SelfGuard,
    reference_gate: ReferenceGate,
    causal: CausalGuard,
}

impl AgentLoopGuards {
    pub fn new(agent: PrincipalId) -> AgentLoopGuards {
        AgentLoopGuards {
            self_guard: SelfGuard::new(agent),
            reference_gate: ReferenceGate::new(),
            causal: CausalGuard::with_caps(
                AGENT_CEILING,
                AGENT_SHARED_ROOT_CAP,
                AGENT_DISPATCH_POOL_CAP,
            ),
        }
    }

    pub fn with_caps(
        agent: PrincipalId,
        ceiling: u32,
        shared_root_cap: u32,
        pool_cap: u32,
    ) -> AgentLoopGuards {
        AgentLoopGuards {
            self_guard: SelfGuard::new(agent),
            reference_gate: ReferenceGate::new(),
            causal: CausalGuard::with_caps(ceiling, shared_root_cap, pool_cap),
        }
    }

    pub fn with_telemetry(mut self, telemetry: FlowTelemetry) -> AgentLoopGuards {
        self.causal = self.causal.with_telemetry(telemetry);
        self
    }

    pub fn ceiling(&self) -> u32 {
        self.causal.ceiling()
    }

    pub fn self_guard(&self) -> &SelfGuard {
        &self.self_guard
    }

    pub fn reference_gate(&self) -> &ReferenceGate {
        &self.reference_gate
    }

    pub fn admit_dispatch(
        &self,
        actor: &Actor,
        re_trigger: &InlineNode,
        correlation_id: &str,
        parent_depth: u32,
    ) -> GuardVerdict {
        let v = self.self_guard.admit(actor);
        if v.is_refused() {
            return v;
        }
        let v = self.reference_gate.admit_node(re_trigger);
        if v.is_refused() {
            return v;
        }
        let (verdict, reason) = self.causal.admit_child(correlation_id, parent_depth);
        match verdict {
            LoopVerdict::Admit => GuardVerdict::Admit,
            LoopVerdict::Drop => {
                GuardVerdict::Drop(reason.expect("a drop carries a reason").into())
            }
            LoopVerdict::Park => {
                GuardVerdict::Park(reason.expect("a park carries a reason").into())
            }
        }
    }

    pub fn admit_dispatch_pool(&self) -> GuardVerdict {
        let (verdict, reason) = self.causal.admit_activity();
        match verdict {
            LoopVerdict::Admit => GuardVerdict::Admit,
            LoopVerdict::Drop => {
                GuardVerdict::Drop(reason.expect("a drop carries a reason").into())
            }
            LoopVerdict::Park => {
                GuardVerdict::Park(reason.expect("a park carries a reason").into())
            }
        }
    }

    pub fn release_dispatch(&self) {
        self.causal.release_activity();
    }

    pub fn dispatches_in_flight(&self) -> u32 {
        self.causal.activities_in_flight()
    }

    pub fn root_dispatches(&self, correlation_id: &str) -> u32 {
        self.causal.root_starts(correlation_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::InlineNode;
    use myelin_events::ArtifactRef;
    use myelin_identity::{Principal, PrincipalKind, RuntimeRef};
    use myelin_tenancy::TenantId;

    fn agent_principal(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt".into()),
                on_behalf_of: None,
            },
            TenantId("acme".into()),
        )
    }

    fn human_principal(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn artifact_ref_node() -> InlineNode {
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()))
    }

    #[test]
    fn self_guard_drops_own_emission_admits_others() {
        let guard = SelfGuard::new(PrincipalId("agent-alice".into()));

        let own = Actor(agent_principal("agent-alice"));
        let v = guard.admit(&own);
        assert_eq!(v, GuardVerdict::Drop(GuardRefusal::SelfTrigger));
        assert!(v.is_refused(), "a self-trigger is refused");

        let human = Actor(human_principal("user-bob"));
        assert_eq!(guard.admit(&human), GuardVerdict::Admit);

        let other = Actor(agent_principal("agent-carol"));
        assert_eq!(guard.admit(&other), GuardVerdict::Admit);
    }

    #[test]
    fn reference_gate_admits_only_artifact_ref_node_never_raw_text() {
        let gate = ReferenceGate::new();

        assert_eq!(gate.admit_node(&artifact_ref_node()), GuardVerdict::Admit);

        for raw in [
            "@agent-alice please re-run this",
            "myelin://acme/issues/issue/PROJ-1",
            "",
        ] {
            let v = gate.admit_raw_text(raw);
            assert_eq!(
                v,
                GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
                "raw text {raw:?} must NEVER re-trigger",
            );
        }

        let mention = InlineNode::Mention(agent_principal("agent-alice"));
        assert_eq!(
            gate.admit_node(&mention),
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
            "a mention is explicit-dispatch, not a loop re-trigger",
        );
        let embed = InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/42".into()));
        assert_eq!(
            gate.admit_node(&embed),
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
        );
    }

    #[test]
    fn idempotent_tool_ledger_dedups_on_run_effect_id() {
        let mut ledger = IdempotentToolLedger::new();

        assert!(ledger.record("run-1", "eff-1"), "first apply records");
        assert!(ledger.contains("run-1", "eff-1"));

        assert!(
            !ledger.record("run-1", "eff-1"),
            "a re-apply under the same key is a NO-OP",
        );

        assert!(ledger.record("run-1", "eff-2"), "a distinct effect applies");
        assert!(ledger.record("run-2", "eff-1"), "a distinct run applies");

        assert_eq!(ledger.applies(), 3, "exactly 3 distinct effects applied");
    }

    #[test]
    fn composed_gate_halts_self_feeding_loop_at_ceiling() {
        let telemetry = FlowTelemetry::new();
        let guards =
            AgentLoopGuards::with_caps(PrincipalId("agent-alice".into()), 12, 10_000, 10_000)
                .with_telemetry(telemetry.clone());
        let other = Actor(human_principal("user-bob"));
        let node = artifact_ref_node();
        let root = "corr-loop";

        let mut depth = 0u32;
        let mut admitted = 0u32;
        let mut dropped = 0u32;
        for _ in 0..50 {
            let v = guards.admit_dispatch(&other, &node, root, depth);
            match v {
                GuardVerdict::Admit => {
                    admitted += 1;
                    depth += 1;
                }
                GuardVerdict::Drop(r) => {
                    dropped += 1;
                    assert_eq!(r, GuardRefusal::DepthCeiling);
                    break;
                }
                GuardVerdict::Park(_) => panic!("the depth ceiling drops, it does not park"),
            }
        }

        assert_eq!(
            admitted, 12,
            "admitted exactly up to the ceiling (children 1..=12)"
        );
        assert_eq!(dropped, 1, "the hop past the ceiling was dropped");
        assert!(
            telemetry.causal_depth_max() <= guards.ceiling(),
            "the causal-depth max never exceeds the ceiling - halted AT it",
        );
        assert_eq!(
            telemetry.causal_depth_max(),
            12,
            "deepest admitted child at the ceiling"
        );
        assert_eq!(telemetry.depth_ceiling_hits(), 1, "the ceiling fired once");
        assert_eq!(
            telemetry.fork_count(),
            0,
            "NEVER forked - the headline invariant"
        );
    }

    #[test]
    fn composed_gate_trips_shared_root_breaker_on_wide_loop() {
        let telemetry = FlowTelemetry::new();
        let guards =
            AgentLoopGuards::with_caps(PrincipalId("agent-alice".into()), 10_000, 3, 10_000)
                .with_telemetry(telemetry.clone());
        let other = Actor(human_principal("user-bob"));
        let node = artifact_ref_node();
        let root = "corr-shared";

        let mut admitted = 0u32;
        let mut tripped = 0u32;
        for _ in 0..10 {
            let v = guards.admit_dispatch(&other, &node, root, 1);
            match v {
                GuardVerdict::Admit => admitted += 1,
                GuardVerdict::Drop(r) => {
                    tripped += 1;
                    assert_eq!(r, GuardRefusal::SharedRootTripwire);
                }
                GuardVerdict::Park(_) => panic!("the tripwire drops, it does not park"),
            }
        }

        assert_eq!(
            admitted, 3,
            "the first 3 same-root dispatches admitted (the window cap)"
        );
        assert_eq!(
            tripped, 7,
            "every same-root dispatch past the cap tripped the breaker"
        );
        assert_eq!(
            telemetry.depth_ceiling_hits(),
            0,
            "depth NEVER fired (the loop stayed shallow)"
        );
        assert!(
            telemetry.shared_root_tripwire_firings() >= 1,
            "the breaker tripped"
        );
        assert_eq!(telemetry.fork_count(), 0, "NEVER forked");
    }

    #[test]
    fn composed_gate_bounds_dispatch_pool_never_forks() {
        let telemetry = FlowTelemetry::new();
        let guards =
            AgentLoopGuards::with_caps(PrincipalId("agent-alice".into()), 10_000, 10_000, 2)
                .with_telemetry(telemetry.clone());

        assert_eq!(guards.admit_dispatch_pool(), GuardVerdict::Admit);
        assert_eq!(guards.admit_dispatch_pool(), GuardVerdict::Admit);
        assert_eq!(guards.dispatches_in_flight(), 2, "the pool is at cap");

        let v = guards.admit_dispatch_pool();
        assert_eq!(v, GuardVerdict::Park(GuardRefusal::DispatchPoolFull));
        assert_eq!(telemetry.activity_pool_sheds(), 1, "one shed recorded");

        guards.release_dispatch();
        assert_eq!(guards.dispatches_in_flight(), 1);
        assert_eq!(
            guards.admit_dispatch_pool(),
            GuardVerdict::Admit,
            "a freed slot admits"
        );
        assert_eq!(telemetry.fork_count(), 0, "NEVER forked");
    }

    #[test]
    fn self_guard_preempts_in_composed_gate() {
        let telemetry = FlowTelemetry::new();
        let guards = AgentLoopGuards::with_caps(PrincipalId("agent-alice".into()), 12, 64, 256)
            .with_telemetry(telemetry.clone());
        let own = Actor(agent_principal("agent-alice"));
        let node = artifact_ref_node();

        let v = guards.admit_dispatch(&own, &node, "corr", 0);
        assert_eq!(
            v,
            GuardVerdict::Drop(GuardRefusal::SelfTrigger),
            "self-guard pre-empts"
        );
        assert_eq!(
            telemetry.causal_depth_max(),
            0,
            "depth never observed (self-guard pre-empted)"
        );
        assert_eq!(telemetry.fork_count(), 0);
    }

    #[test]
    fn reference_gate_preempts_depth_in_composed_gate() {
        let guards = AgentLoopGuards::with_caps(PrincipalId("agent-alice".into()), 12, 64, 256);
        let other = Actor(human_principal("user-bob"));
        let mention = InlineNode::Mention(human_principal("user-bob"));

        let v = guards.admit_dispatch(&other, &mention, "corr", 0);
        assert_eq!(
            v,
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
            "the reference gate pre-empts: a non-ref re-trigger is dropped",
        );
    }

    #[test]
    fn verdict_predicates_and_refusal_surface() {
        assert!(GuardVerdict::Admit.is_admit());
        assert!(GuardVerdict::Admit.refusal().is_none());
        let d = GuardVerdict::Drop(GuardRefusal::SelfTrigger);
        assert!(d.is_refused());
        assert_eq!(d.refusal(), Some(GuardRefusal::SelfTrigger));
        let p = GuardVerdict::Park(GuardRefusal::DispatchPoolFull);
        assert!(p.is_refused());
        assert_eq!(p.refusal(), Some(GuardRefusal::DispatchPoolFull));
    }

    #[test]
    fn default_agent_ceiling_is_twelve() {
        let guards = AgentLoopGuards::new(PrincipalId("agent-alice".into()));
        assert_eq!(
            guards.ceiling(),
            12,
            "the agent-lane ceiling default is 12 (AG-D7)"
        );
        assert_eq!(AGENT_CEILING, 12);
    }
}
