use crate::PublishDraft;
use myelin_events::{
    derive_envelope, Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EmitContext,
    EventDraft, EventEnvelope, EventId, EventType, PiiKeyRef, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId};
use myelin_tenancy::TenantId;
use std::collections::HashMap;

pub const CAUSAL_DEPTH_CEILING: u32 = 12;

pub const SHARED_ROOT_TRIPWIRE_K: u32 = 64;

pub const DISPATCH_INFLIGHT_CAP: u32 = 32;

pub const SHED_RETRY_AFTER_SECONDS: u32 = 30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchRequest {
    pub event: EventEnvelope,
    pub agent: PrincipalId,
    pub run_ref: String,
    pub trigger: TriggerKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalBinding {
    pub agent: PrincipalId,
    pub run_ref: String,
    pub trigger: TriggerKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerKind {
    Mention,
    Automation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    Delivered {
        action: Box<EventEnvelope>,
    },
    NotifiedOnly,
    SelfGuardDropped,
    ReferenceGateDropped,
    DepthCeilingParked {
        depth: u32,
    },
    BreakerShed {
        shed: ShedSignal,
    },
    OverCapShed {
        shed: ShedSignal,
    },
    NoBalanceRefused,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShedSignal {
    pub status: u16,
    pub retry_after_seconds: u32,
    pub reason: ShedReason,
}

impl ShedSignal {
    fn lane_shed(reason: ShedReason) -> ShedSignal {
        ShedSignal {
            status: 429,
            retry_after_seconds: SHED_RETRY_AFTER_SECONDS,
            reason,
        }
    }

    pub fn signal_subject(&self) -> &'static str {
        match self.reason {
            ShedReason::OverCap => "signal.dispatch.over_cap",
            ShedReason::BreakerOpen => "signal.dispatch.breaker_open",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShedReason {
    OverCap,
    BreakerOpen,
}

pub trait DispatchTarget {
    fn deliver(&self, action: &EventEnvelope) -> Result<(), DispatchError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchError(pub String);

#[derive(Debug, Default)]
pub struct RecordingTarget {
    delivered: std::cell::RefCell<Vec<EventEnvelope>>,
}

impl RecordingTarget {
    pub fn new() -> RecordingTarget {
        RecordingTarget::default()
    }

    pub fn delivered(&self) -> Vec<EventEnvelope> {
        self.delivered.borrow().clone()
    }

    pub fn delivered_count(&self) -> usize {
        self.delivered.borrow().len()
    }
}

impl DispatchTarget for RecordingTarget {
    fn deliver(&self, action: &EventEnvelope) -> Result<(), DispatchError> {
        self.delivered.borrow_mut().push(action.clone());
        Ok(())
    }
}

pub trait CostGate {
    fn reserve(&self, tenant: &TenantId, run_ref: &str) -> Option<Reservation>;

    fn in_flight(&self, tenant: &TenantId) -> u32;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    pub run_ref: String,
    pub reserved_units: u64,
}

#[derive(Debug)]
pub struct InMemoryCostGate {
    balance: std::cell::RefCell<HashMap<TenantId, u64>>,
    in_flight: std::cell::RefCell<HashMap<TenantId, std::collections::BTreeSet<String>>>,
    cost_per_run: u64,
}

impl InMemoryCostGate {
    pub fn new(cost_per_run: u64) -> InMemoryCostGate {
        InMemoryCostGate {
            balance: std::cell::RefCell::new(HashMap::new()),
            in_flight: std::cell::RefCell::new(HashMap::new()),
            cost_per_run,
        }
    }

    pub fn credit(&self, tenant: &TenantId, units: u64) {
        let mut b = self.balance.borrow_mut();
        *b.entry(tenant.clone()).or_insert(0) += units;
    }
}

impl CostGate for InMemoryCostGate {
    fn reserve(&self, tenant: &TenantId, run_ref: &str) -> Option<Reservation> {
        {
            let inflight = self.in_flight.borrow();
            if inflight.get(tenant).is_some_and(|s| s.contains(run_ref)) {
                return Some(Reservation {
                    run_ref: run_ref.to_string(),
                    reserved_units: self.cost_per_run,
                });
            }
        }
        let mut bal = self.balance.borrow_mut();
        let remaining = bal.entry(tenant.clone()).or_insert(0);
        if *remaining < self.cost_per_run {
            return None;
        }
        *remaining -= self.cost_per_run;
        self.in_flight
            .borrow_mut()
            .entry(tenant.clone())
            .or_default()
            .insert(run_ref.to_string());
        Some(Reservation {
            run_ref: run_ref.to_string(),
            reserved_units: self.cost_per_run,
        })
    }

    fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.in_flight
            .borrow()
            .get(tenant)
            .map(|s| s.len() as u32)
            .unwrap_or(0)
    }
}

#[derive(Debug, Default)]
pub struct DispatchBreaker {
    root_counts: HashMap<(TenantId, CorrelationId), u32>,
    open: std::collections::HashSet<TenantId>,
    tripwire_k: u32,
}

impl DispatchBreaker {
    pub fn new() -> DispatchBreaker {
        DispatchBreaker {
            tripwire_k: SHARED_ROOT_TRIPWIRE_K,
            ..Default::default()
        }
    }

    pub fn with_tripwire(k: u32) -> DispatchBreaker {
        DispatchBreaker {
            tripwire_k: k,
            ..Default::default()
        }
    }

    pub fn is_open(&self, tenant: &TenantId) -> bool {
        self.open.contains(tenant)
    }

    fn record_and_check(&mut self, tenant: &TenantId, root: &CorrelationId) -> bool {
        if self.open.contains(tenant) {
            return true;
        }
        let count = self
            .root_counts
            .entry((tenant.clone(), root.clone()))
            .or_insert(0);
        *count += 1;
        if *count > self.tripwire_k {
            self.open.insert(tenant.clone());
            return true;
        }
        false
    }

    pub fn root_count(&self, tenant: &TenantId, root: &CorrelationId) -> u32 {
        self.root_counts
            .get(&(tenant.clone(), root.clone()))
            .copied()
            .unwrap_or(0)
    }

    pub fn reset(&mut self, tenant: &TenantId) {
        self.open.remove(tenant);
        self.root_counts.retain(|(t, _), _| t != tenant);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchTelemetry {
    pub delivered: u64,
    pub notified_only: u64,
    pub self_guard_dropped: u64,
    pub reference_gate_dropped: u64,
    pub depth_ceiling_parked: u64,
    pub over_cap_shed: u64,
    pub breaker_shed: u64,
    pub no_balance_refused: u64,
    pub max_dispatched_depth: u32,
    pub tripwire_firings: u64,
}

impl DispatchTelemetry {
    fn record(&mut self, disp: &Disposition, tripwire_fired: bool) {
        match disp {
            Disposition::Delivered { action } => {
                self.delivered += 1;
                self.max_dispatched_depth = self.max_dispatched_depth.max(action.depth);
            }
            Disposition::NotifiedOnly => self.notified_only += 1,
            Disposition::SelfGuardDropped => self.self_guard_dropped += 1,
            Disposition::ReferenceGateDropped => self.reference_gate_dropped += 1,
            Disposition::DepthCeilingParked { .. } => self.depth_ceiling_parked += 1,
            Disposition::OverCapShed { .. } => self.over_cap_shed += 1,
            Disposition::BreakerShed { .. } => self.breaker_shed += 1,
            Disposition::NoBalanceRefused => self.no_balance_refused += 1,
        }
        if tripwire_fired {
            self.tripwire_firings += 1;
        }
    }
}

#[derive(Debug)]
pub struct DispatchTier<T: DispatchTarget, G: CostGate> {
    target: T,
    cost_gate: G,
    breaker: DispatchBreaker,
    telemetry: DispatchTelemetry,
    depth_ceiling: u32,
    inflight_cap: u32,
}

struct Decision {
    disposition: Disposition,
    tripwire_fired: bool,
}

impl<T: DispatchTarget, G: CostGate> DispatchTier<T, G> {
    pub fn new(target: T, cost_gate: G) -> DispatchTier<T, G> {
        DispatchTier {
            target,
            cost_gate,
            breaker: DispatchBreaker::new(),
            telemetry: DispatchTelemetry::default(),
            depth_ceiling: CAUSAL_DEPTH_CEILING,
            inflight_cap: DISPATCH_INFLIGHT_CAP,
        }
    }

    pub fn with_limits(
        target: T,
        cost_gate: G,
        depth_ceiling: u32,
        tripwire_k: u32,
        inflight_cap: u32,
    ) -> DispatchTier<T, G> {
        DispatchTier {
            target,
            cost_gate,
            breaker: DispatchBreaker::with_tripwire(tripwire_k),
            telemetry: DispatchTelemetry::default(),
            depth_ceiling,
            inflight_cap,
        }
    }

    pub fn telemetry(&self) -> &DispatchTelemetry {
        &self.telemetry
    }

    pub fn breaker_open(&self, tenant: &TenantId) -> bool {
        self.breaker.is_open(tenant)
    }

    pub fn root_count(&self, tenant: &TenantId, root: &CorrelationId) -> u32 {
        self.breaker.root_count(tenant, root)
    }

    pub fn target(&self) -> &T {
        &self.target
    }

    pub fn reset_breaker(&mut self, tenant: &TenantId) {
        self.breaker.reset(tenant);
    }

    pub fn dispatch(
        &mut self,
        req: &DispatchRequest,
        mint_event_id: impl FnOnce() -> EventId,
        now: &Timestamp,
    ) -> Disposition {
        let Decision {
            disposition,
            tripwire_fired,
        } = self.decide(req, mint_event_id, now);
        self.telemetry.record(&disposition, tripwire_fired);
        disposition
    }

    pub fn dispatch_for_signal(
        &mut self,
        draft: &PublishDraft,
        origin: &EventEnvelope,
        binding: SignalBinding,
        mint_event_id: impl FnOnce() -> EventId,
        now: &Timestamp,
    ) -> Disposition {
        if draft.signal.state == crate::SignalState::Resolved {
            let disp = Disposition::NotifiedOnly;
            self.telemetry.record(&disp, false);
            return disp;
        }
        let req = DispatchRequest {
            event: origin.clone(),
            agent: binding.agent,
            run_ref: binding.run_ref,
            trigger: binding.trigger,
        };
        self.dispatch(&req, mint_event_id, now)
    }

    fn decide(
        &mut self,
        req: &DispatchRequest,
        mint_event_id: impl FnOnce() -> EventId,
        now: &Timestamp,
    ) -> Decision {
        let ev = &req.event;
        let no_trip = |disposition| Decision {
            disposition,
            tripwire_fired: false,
        };

        if ev.actor.0.principal_id == req.agent {
            return no_trip(Disposition::SelfGuardDropped);
        }

        if !is_artifact_ref(&ev.subject) {
            return no_trip(Disposition::ReferenceGateDropped);
        }

        if req.trigger == TriggerKind::Mention {
            return no_trip(Disposition::NotifiedOnly);
        }

        if ev.depth >= self.depth_ceiling {
            return no_trip(Disposition::DepthCeilingParked { depth: ev.depth });
        }

        if self.breaker.is_open(&ev.tenant) {
            return no_trip(Disposition::BreakerShed {
                shed: ShedSignal::lane_shed(ShedReason::BreakerOpen),
            });
        }
        if self.cost_gate.in_flight(&ev.tenant) >= self.inflight_cap {
            return no_trip(Disposition::OverCapShed {
                shed: ShedSignal::lane_shed(ShedReason::OverCap),
            });
        }

        if self.cost_gate.reserve(&ev.tenant, &req.run_ref).is_none() {
            return no_trip(Disposition::NoBalanceRefused);
        }

        let was_open_before = self.breaker.is_open(&ev.tenant);
        self.breaker
            .record_and_check(&ev.tenant, &ev.correlation_id);
        let tripwire_fired = !was_open_before && self.breaker.is_open(&ev.tenant);

        let action = derive_dispatched_action(ev, &req.run_ref, mint_event_id(), now);

        match self.target.deliver(&action) {
            Ok(()) => Decision {
                disposition: Disposition::Delivered {
                    action: Box::new(action),
                },
                tripwire_fired,
            },
            Err(_e) => Decision {
                disposition: Disposition::BreakerShed {
                    shed: ShedSignal::lane_shed(ShedReason::BreakerOpen),
                },
                tripwire_fired,
            },
        }
    }
}

fn is_artifact_ref(subject: &ArtifactRef) -> bool {
    subject.0.starts_with("myelin://") && subject.0.len() > "myelin://".len()
}

fn derive_dispatched_action(
    trigger: &EventEnvelope,
    run_ref: &str,
    event_id: EventId,
    now: &Timestamp,
) -> EventEnvelope {
    let draft = EventDraft {
        type_: EventType("agent.run.dispatched".into()),
        subject: ArtifactRef(format!(
            "myelin://{}/agent/run/{}",
            trigger.tenant.0, run_ref
        )),
        aggregate: AggregateKey(format!("agent_run:{run_ref}")),
        payload: serde_json::json!({
            "trigger_event": trigger.event_id.0,
            "run_ref": run_ref,
        }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None::<PiiKeyRef>,
    };
    let ctx = EmitContext {
        event_id,
        tenant: trigger.tenant.clone(),
        region: trigger.region.clone(),
        actor: Actor(dispatched_actor(&trigger.actor.0)),
        schema_ver: trigger.schema_ver,
        occurred_at: now.clone(),
        recorded_at: now.clone(),
        caused_by: trigger.caused_by.clone(),
    };
    derive_envelope(draft, ctx, Some(trigger))
}

fn dispatched_actor(trigger_actor: &Principal) -> Principal {
    trigger_actor.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{CausedBy, DataRole as EvDataRole};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::Region;

    fn principal(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("t1".into()),
        )
    }

    fn event(actor_id: &str, subject: &str, depth: u32, correlation: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("ev:{actor_id}:{subject}:{depth}:{correlation}")),
            type_: EventType("chat.message.created".into()),
            schema_ver: 1,
            tenant: TenantId("t1".into()),
            region: Region("t1-home".into()),
            actor: Actor(principal(actor_id)),
            subject: ArtifactRef(subject.into()),
            aggregate: AggregateKey("agg:1".into()),
            causation_id: None,
            correlation_id: CorrelationId(correlation.into()),
            caused_by: Some(CausedBy("session:human".into())),
            depth,
            contains_personal_data: false,
            data_role: EvDataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            payload: serde_json::json!({}),
        }
    }

    fn now() -> Timestamp {
        Timestamp("2026-06-20T00:00:01Z".into())
    }

    fn minter(id: &str) -> impl FnOnce() -> EventId {
        let id = id.to_string();
        move || EventId(id)
    }

    fn tier_with_balance(balance: u64) -> DispatchTier<RecordingTarget, InMemoryCostGate> {
        let gate = InMemoryCostGate::new(1);
        gate.credit(&TenantId("t1".into()), balance);
        DispatchTier::new(RecordingTarget::new(), gate)
    }

    fn auto_req(ev: EventEnvelope, agent: &str, run_ref: &str) -> DispatchRequest {
        DispatchRequest {
            event: ev,
            agent: PrincipalId(agent.into()),
            run_ref: run_ref.into(),
            trigger: TriggerKind::Automation,
        }
    }

    #[test]
    fn nested_causality_dispatched_action_is_parent_plus_one_correlation_carried() {
        let mut tier = tier_with_balance(10);
        let ev = event("human", "myelin://t1/chat/message/1", 3, "root-A");
        let disp = tier.dispatch(
            &auto_req(ev.clone(), "agentX", "run-1"),
            minter("act-1"),
            &now(),
        );
        match disp {
            Disposition::Delivered { action } => {
                assert_eq!(action.causation_id, Some(ev.event_id.clone()));
                assert_eq!(action.correlation_id, ev.correlation_id);
                assert_eq!(action.depth, ev.depth + 1, "depth = parent + 1");
                assert_eq!(action.caused_by, ev.caused_by);
            }
            other => panic!("expected Delivered, got {other:?}"),
        }
        assert_eq!(tier.telemetry().delivered, 1);
        assert_eq!(tier.telemetry().max_dispatched_depth, 4);
        assert_eq!(tier.target().delivered_count(), 1);
    }

    #[test]
    fn nested_causality_is_structural_no_flat_field_to_author() {
        let mut tier = tier_with_balance(10);
        let ev1 = event("human", "myelin://t1/chat/message/1", 3, "root-A");
        let d1 = tier.dispatch(&auto_req(ev1, "agentX", "run-1"), minter("act-1"), &now());
        let action1 = match d1 {
            Disposition::Delivered { action } => action,
            o => panic!("{o:?}"),
        };
        let mut ev2 = *action1;
        ev2.actor = Actor(principal("human"));
        ev2.subject = ArtifactRef("myelin://t1/chat/message/2".into());
        let d2 = tier.dispatch(&auto_req(ev2, "agentX", "run-2"), minter("act-2"), &now());
        match d2 {
            Disposition::Delivered { action } => {
                assert_eq!(action.depth, 5, "two hops from depth 3 = depth 5 (nested)");
                assert_eq!(action.correlation_id, CorrelationId("root-A".into()));
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn self_guard_drops_the_agents_own_event() {
        let mut tier = tier_with_balance(10);
        let ev = event("agentX", "myelin://t1/chat/message/1", 1, "root-A");
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-1"), minter("act-1"), &now());
        assert_eq!(disp, Disposition::SelfGuardDropped);
        assert_eq!(tier.telemetry().self_guard_dropped, 1);
        assert_eq!(
            tier.target().delivered_count(),
            0,
            "0 dispatch on a self-event"
        );
    }

    #[test]
    fn reference_gate_admits_artifact_ref_node() {
        let mut tier = tier_with_balance(10);
        let ev = event("human", "myelin://t1/chat/message/1", 1, "root-A");
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-1"), minter("act-1"), &now());
        assert!(matches!(disp, Disposition::Delivered { .. }));
    }

    #[test]
    fn reference_gate_drops_raw_text_trigger() {
        let mut tier = tier_with_balance(10);
        let ev = event("human", "please do the thing @agentX", 1, "root-A");
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-1"), minter("act-1"), &now());
        assert_eq!(disp, Disposition::ReferenceGateDropped);
        assert_eq!(tier.telemetry().reference_gate_dropped, 1);
        assert_eq!(tier.target().delivered_count(), 0);
    }

    #[test]
    fn depth_ceiling_parks_at_twelve() {
        let mut tier = tier_with_balance(10);
        let ev = event(
            "human",
            "myelin://t1/chat/message/1",
            CAUSAL_DEPTH_CEILING,
            "root-A",
        );
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-1"), minter("act-1"), &now());
        assert_eq!(
            disp,
            Disposition::DepthCeilingParked {
                depth: CAUSAL_DEPTH_CEILING
            }
        );
        assert_eq!(tier.telemetry().depth_ceiling_parked, 1);
        assert_eq!(
            tier.target().delivered_count(),
            0,
            "the chain halts ≤ ceiling"
        );
    }

    #[test]
    fn depth_below_ceiling_dispatches() {
        let mut tier = tier_with_balance(10);
        let ev = event(
            "human",
            "myelin://t1/chat/message/1",
            CAUSAL_DEPTH_CEILING - 1,
            "root-A",
        );
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-1"), minter("act-1"), &now());
        assert!(matches!(disp, Disposition::Delivered { .. }));
    }

    #[test]
    fn shared_root_tripwire_trips_the_per_tenant_breaker() {
        let gate = InMemoryCostGate::new(0);
        let mut tier = DispatchTier::with_limits(RecordingTarget::new(), gate, 100, 3, 1000);
        let root = "root-storm";
        let t1 = TenantId("t1".into());
        for i in 0..4 {
            let ev = event("human", &format!("myelin://t1/chat/message/{i}"), 1, root);
            let _ = tier.dispatch(
                &auto_req(ev, "agentX", &format!("run-{i}")),
                minter(&format!("a{i}")),
                &now(),
            );
        }
        assert!(
            tier.breaker_open(&t1),
            "the breaker tripped on the over-K root"
        );
        assert!(tier.root_count(&t1, &CorrelationId(root.into())) > SHARED_ROOT_TRIPWIRE_K.min(3));
        assert_eq!(
            tier.telemetry().tripwire_firings,
            1,
            "exactly one trip recorded"
        );
        let ev = event("human", "myelin://t1/chat/message/99", 1, root);
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-99"), minter("a99"), &now());
        assert!(matches!(
            disp,
            Disposition::BreakerShed { shed } if shed.status == 429 && shed.reason == ShedReason::BreakerOpen
        ));
        assert_eq!(tier.telemetry().breaker_shed, 1);
    }

    #[test]
    fn explicit_first_mention_notifies_zero_auto_spawn() {
        let mut tier = tier_with_balance(10);
        let ev = event("human", "myelin://t1/chat/message/1", 1, "root-A");
        let req = DispatchRequest {
            event: ev,
            agent: PrincipalId("agentX".into()),
            run_ref: "run-1".into(),
            trigger: TriggerKind::Mention,
        };
        let disp = tier.dispatch(&req, minter("act-1"), &now());
        assert_eq!(disp, Disposition::NotifiedOnly);
        assert_eq!(tier.telemetry().notified_only, 1);
        assert_eq!(
            tier.target().delivered_count(),
            0,
            "a mention auto-spawns 0 runs (CHAT-1)"
        );
        assert_eq!(tier.telemetry().delivered, 0);
    }

    #[test]
    fn reserve_settle_blocks_a_no_balance_run() {
        let mut tier = tier_with_balance(0);
        let ev = event("human", "myelin://t1/chat/message/1", 1, "root-A");
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-1"), minter("act-1"), &now());
        assert_eq!(disp, Disposition::NoBalanceRefused);
        assert_eq!(tier.telemetry().no_balance_refused, 1);
        assert_eq!(
            tier.target().delivered_count(),
            0,
            "no balance → no execution (11.7)"
        );
    }

    #[test]
    fn reserve_is_idempotent_on_run_ref_no_double_charge() {
        let gate = InMemoryCostGate::new(1);
        let t1 = TenantId("t1".into());
        gate.credit(&t1, 1);
        assert!(gate.reserve(&t1, "run-1").is_some());
        assert!(
            gate.reserve(&t1, "run-1").is_some(),
            "redelivery re-reserves, no double-charge"
        );
        assert!(
            gate.reserve(&t1, "run-2").is_none(),
            "balance exhausted → refused"
        );
    }

    #[test]
    fn over_cap_sheds_with_429_retry_after() {
        let gate = InMemoryCostGate::new(1);
        let t1 = TenantId("t1".into());
        gate.credit(&t1, 100);
        let mut tier = DispatchTier::with_limits(RecordingTarget::new(), gate, 100, 1000, 1);
        let ev1 = event("human", "myelin://t1/chat/message/1", 1, "root-A");
        let d1 = tier.dispatch(&auto_req(ev1, "agentX", "run-1"), minter("a1"), &now());
        assert!(matches!(d1, Disposition::Delivered { .. }));
        let ev2 = event("human", "myelin://t1/chat/message/2", 1, "root-B");
        let d2 = tier.dispatch(&auto_req(ev2, "agentX", "run-2"), minter("a2"), &now());
        match d2 {
            Disposition::OverCapShed { shed } => {
                assert_eq!(shed.status, 429);
                assert_eq!(shed.retry_after_seconds, SHED_RETRY_AFTER_SECONDS);
                assert_eq!(shed.reason, ShedReason::OverCap);
                assert_eq!(shed.signal_subject(), "signal.dispatch.over_cap");
            }
            o => panic!("expected OverCapShed, got {o:?}"),
        }
        assert_eq!(tier.telemetry().over_cap_shed, 1);
    }

    #[test]
    fn dispatch_for_signal_resolved_does_not_spawn_a_run() {
        use crate::{DedupKey, RuleId, Severity, Signal, SignalState};
        let mut tier = tier_with_balance(10);
        let resolved = Signal {
            rule_id: RuleId("r".into()),
            tenant: TenantId("t1".into()),
            severity: Severity::Error,
            dedup_key: DedupKey("k".into()),
            subject: ArtifactRef("myelin://t1/ci/run/1".into()),
            count: 1,
            state: SignalState::Resolved,
            first_seen: "2026-06-20T00:00:00Z".into(),
            last_seen: "2026-06-20T00:00:00Z".into(),
        };
        let draft = PublishDraft {
            subject: "sig.t1.error.r".into(),
            signal: resolved,
            kind: crate::PublishKind::Resolved,
        };
        let origin = event("human", "myelin://t1/ci/run/1", 1, "root-A");
        let disp = tier.dispatch_for_signal(
            &draft,
            &origin,
            SignalBinding {
                agent: PrincipalId("agentX".into()),
                run_ref: "run-1".into(),
                trigger: TriggerKind::Automation,
            },
            minter("a1"),
            &now(),
        );
        assert_eq!(
            disp,
            Disposition::NotifiedOnly,
            "a resolving Signal closes, never dispatches"
        );
        assert_eq!(tier.target().delivered_count(), 0);
    }
}
