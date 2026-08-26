use crate::effect_api::{EffectCost, PlannedEffect};
use myelin_agent::{EffectResult, GateId};
use myelin_identity::{Consistency, Permission, PrincipalId, SubjectTree};
use myelin_tenancy::ArtifactRef;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HitlGateState {
    Waiting,
    Approved,
    Rejected,
    Expired,
}

impl HitlGateState {
    pub fn as_str(self) -> &'static str {
        match self {
            HitlGateState::Waiting => "waiting",
            HitlGateState::Approved => "approved",
            HitlGateState::Rejected => "rejected",
            HitlGateState::Expired => "expired",
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, HitlGateState::Waiting)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskSummary {
    pub template_key: String,
    pub args: Vec<(String, ArtifactRef)>,
}

impl RiskSummary {
    pub fn for_action(template_key: impl Into<String>, object: &ArtifactRef) -> RiskSummary {
        RiskSummary {
            template_key: template_key.into(),
            args: vec![("object".to_string(), object.clone())],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Halted {
    Rejected(String),
    Expired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidTransition {
    pub already: HitlGateState,
}

impl core::fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "hitl_gate is already terminal ({}) - a decided gate does not re-transition",
            self.already.as_str()
        )
    }
}

impl std::error::Error for InvalidTransition {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitlGate {
    pub gate_id: GateId,
    pub run_id: String,
    pub tool_name: String,
    pub object: ArtifactRef,
    pub risk_summary: RiskSummary,
    pub cost_estimate: u64,
    pub approver_filter: Vec<PrincipalId>,
    pub state: HitlGateState,
    pub card_ref: String,
}

impl HitlGate {
    pub fn open(
        gate_id: GateId,
        run_id: impl Into<String>,
        plan: &PlannedEffect,
        risk_summary: RiskSummary,
        approver_filter: Vec<PrincipalId>,
        card_ref: impl Into<String>,
    ) -> HitlGate {
        HitlGate {
            gate_id,
            run_id: run_id.into(),
            tool_name: plan.tool.0.clone(),
            object: plan.object.clone(),
            risk_summary,
            cost_estimate: live_cost_estimate(&plan.cost),
            approver_filter,
            state: HitlGateState::Waiting,
            card_ref: card_ref.into(),
        }
    }

    pub fn approve(&mut self) -> Result<(), InvalidTransition> {
        if self.state.is_terminal() {
            return Err(InvalidTransition {
                already: self.state,
            });
        }
        self.state = HitlGateState::Approved;
        Ok(())
    }

    pub fn reject(&mut self, reason: impl Into<String>) -> Result<Halted, InvalidTransition> {
        if self.state.is_terminal() {
            return Err(InvalidTransition {
                already: self.state,
            });
        }
        self.state = HitlGateState::Rejected;
        Ok(Halted::Rejected(reason.into()))
    }

    pub fn expire(&mut self) -> Result<Halted, InvalidTransition> {
        if self.state.is_terminal() {
            return Err(InvalidTransition {
                already: self.state,
            });
        }
        self.state = HitlGateState::Expired;
        Ok(Halted::Expired)
    }

    pub fn is_approved(&self) -> bool {
        matches!(self.state, HitlGateState::Approved)
    }
}

pub fn live_cost_estimate(cost: &EffectCost) -> u64 {
    cost.total()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitlCard {
    pub gate_id: GateId,
    pub action_tool: String,
    pub action_object: ArtifactRef,
    pub risk_summary: RiskSummary,
    pub cost_estimate: u64,
    pub approvers: Vec<PrincipalId>,
    pub card_ref: String,
}

pub fn surface_card(gate: &HitlGate) -> HitlCard {
    HitlCard {
        gate_id: gate.gate_id.clone(),
        action_tool: gate.tool_name.clone(),
        action_object: gate.object.clone(),
        risk_summary: gate.risk_summary.clone(),
        cost_estimate: gate.cost_estimate,
        approvers: gate.approver_filter.clone(),
        card_ref: gate.card_ref.clone(),
    }
}

pub trait ApproverSet {
    fn list_subjects(
        &self,
        object: &ArtifactRef,
        approve_perm: &Permission,
        at: &Consistency,
    ) -> SubjectTree;
}

pub fn derive_approver_set<A: ApproverSet>(
    approvers: &A,
    object: &ArtifactRef,
    approve_perm: &Permission,
    at: &Consistency,
) -> Vec<PrincipalId> {
    approvers.list_subjects(object, approve_perm, at).members
}

pub trait HitlWait {
    fn park_and_wait(&self, gate: &HitlGate) -> WaitDecision;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitDecision {
    Approve,
    Reject(String),
    Expired,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApprovedTools(pub std::collections::BTreeSet<String>);

impl ApprovedTools {
    pub fn new() -> ApprovedTools {
        ApprovedTools::default()
    }

    pub fn admit(&mut self, gate: &HitlGate) -> bool {
        if !gate.is_approved() {
            return false;
        }
        self.0.insert(crate::effect_api::effect_gate_key_str(
            &gate.tool_name,
            &gate.object.0,
        ));
        true
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.contains(key)
    }

    pub fn contains_effect(&self, tool: &str, object: &str) -> bool {
        self.0
            .contains(&crate::effect_api::effect_gate_key_str(tool, object))
    }

    pub fn as_set(&self) -> std::collections::BTreeSet<String> {
        self.0.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HitlOutcome {
    Approved(HitlGate),
    Halted(Halted),
}

pub(crate) fn resolve_decision(
    gate: &mut HitlGate,
    decision: WaitDecision,
    approved: &mut ApprovedTools,
) -> Result<(), Halted> {
    match decision {
        WaitDecision::Approve => {
            gate.approve().expect("a freshly-opened gate is Waiting");
            approved.admit(gate);
            Ok(())
        }
        WaitDecision::Reject(reason) => {
            let halted = gate
                .reject(reason)
                .expect("a freshly-opened gate is Waiting");
            Err(halted)
        }
        WaitDecision::Expired => {
            let halted = gate.expire().expect("a freshly-opened gate is Waiting");
            Err(halted)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_hitl_loop<W: HitlWait>(
    gate_id: GateId,
    run_id: &str,
    plan: &PlannedEffect,
    risk_summary: RiskSummary,
    approver_filter: Vec<PrincipalId>,
    card_ref: &str,
    wait: &W,
    approved: &mut ApprovedTools,
) -> HitlOutcome {
    let mut gate = HitlGate::open(
        gate_id,
        run_id,
        plan,
        risk_summary,
        approver_filter,
        card_ref,
    );

    let _card = surface_card(&gate);
    let decision = wait.park_and_wait(&gate);

    match resolve_decision(&mut gate, decision, approved) {
        Ok(()) => HitlOutcome::Approved(gate),
        Err(halted) => HitlOutcome::Halted(halted),
    }
}

pub fn gate_id_of(result: &EffectResult) -> Option<GateId> {
    match result {
        EffectResult::Gated(g) => Some(g.clone()),
        EffectResult::Applied(_)
        | EffectResult::AppliedResource { .. }
        | EffectResult::Denied(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{ConsistencyMode, Zookie};

    fn plan(tool: &str) -> PlannedEffect {
        PlannedEffect {
            tool: myelin_agent::ToolName(tool.into()),
            object: ArtifactRef("myelin://acme/git/pr/42".into()),
            input_json: r#"{"pr":42}"#.into(),
            field: None,
            transition: None,
            cost: EffectCost {
                unit: "git.merge",
                wholesale: 30,
                markup: 20,
            },
        }
    }

    fn risk() -> RiskSummary {
        RiskSummary::for_action(
            "agent.hitl.merge_pr",
            &ArtifactRef("myelin://acme/git/pr/42".into()),
        )
    }

    fn approvers() -> Vec<PrincipalId> {
        vec![
            PrincipalId("psn:lead".into()),
            PrincipalId("psn:maintainer".into()),
        ]
    }

    fn open_waiting() -> HitlGate {
        HitlGate::open(
            GateId("gate:git.merge:myelin://acme/git/pr/42".into()),
            "R1",
            &plan("git.merge"),
            risk(),
            approvers(),
            "card:R1:0",
        )
    }

    #[test]
    fn open_gate_is_waiting_and_carries_the_card_fields() {
        let g = open_waiting();
        assert_eq!(g.state, HitlGateState::Waiting);
        assert!(!g.state.is_terminal());
        assert_eq!(
            g.cost_estimate, 50,
            "the card shows the LIVE cost the reserve would debit"
        );
        assert_eq!(g.tool_name, "git.merge");
        assert_eq!(g.object, ArtifactRef("myelin://acme/git/pr/42".into()));
        assert_eq!(g.risk_summary.template_key, "agent.hitl.merge_pr");
        assert_eq!(
            g.approver_filter.len(),
            2,
            "the approver set = list_subjects(object, approve_perm)"
        );
        assert_eq!(g.card_ref, "card:R1:0");
    }

    #[test]
    fn waiting_approves_to_approved() {
        let mut g = open_waiting();
        assert!(!g.is_approved());
        g.approve().expect("waiting → approved");
        assert_eq!(g.state, HitlGateState::Approved);
        assert!(g.is_approved());
        assert!(g.state.is_terminal());
    }

    #[test]
    fn waiting_rejects_and_settles_halted_rejected() {
        let mut g = open_waiting();
        let halted = g.reject("not safe to merge").expect("waiting → rejected");
        assert_eq!(halted, Halted::Rejected("not safe to merge".into()));
        assert_eq!(g.state, HitlGateState::Rejected);
        assert!(
            !g.is_approved(),
            "a rejected gate never approves the tool (0 mutation, AG-8)"
        );
    }

    #[test]
    fn waiting_expires_to_expired() {
        let mut g = open_waiting();
        let halted = g.expire().expect("waiting → expired");
        assert_eq!(halted, Halted::Expired);
        assert_eq!(g.state, HitlGateState::Expired);
        assert!(!g.is_approved());
    }

    #[test]
    fn a_terminal_gate_refuses_re_transition() {
        let mut g = open_waiting();
        g.approve().unwrap();
        assert_eq!(
            g.approve(),
            Err(InvalidTransition {
                already: HitlGateState::Approved
            })
        );
        assert_eq!(
            g.reject("late"),
            Err(InvalidTransition {
                already: HitlGateState::Approved
            })
        );

        let mut r = open_waiting();
        r.reject("no").unwrap();
        assert_eq!(
            r.approve(),
            Err(InvalidTransition {
                already: HitlGateState::Rejected
            })
        );
        assert_eq!(
            r.expire(),
            Err(InvalidTransition {
                already: HitlGateState::Rejected
            })
        );
    }

    #[test]
    fn state_tokens_are_frozen() {
        assert_eq!(HitlGateState::Waiting.as_str(), "waiting");
        assert_eq!(HitlGateState::Approved.as_str(), "approved");
        assert_eq!(HitlGateState::Rejected.as_str(), "rejected");
        assert_eq!(HitlGateState::Expired.as_str(), "expired");
    }

    #[test]
    fn surface_card_shows_action_risk_cost_and_approvers() {
        let g = open_waiting();
        let card = surface_card(&g);
        assert_eq!(
            card.action_tool, "git.merge",
            "the card shows the pending ACTION"
        );
        assert_eq!(
            card.action_object,
            ArtifactRef("myelin://acme/git/pr/42".into())
        );
        assert_eq!(
            card.risk_summary.template_key, "agent.hitl.merge_pr",
            "the card shows the RISK slot"
        );
        assert_eq!(
            card.cost_estimate, 50,
            "the card shows the LIVE COST estimate"
        );
        assert_eq!(card.approvers.len(), 2, "the card shows the APPROVER set");
        assert_eq!(g.state, HitlGateState::Waiting);
    }

    struct FakeSubjects {
        members: Vec<PrincipalId>,
    }
    impl ApproverSet for FakeSubjects {
        fn list_subjects(
            &self,
            object: &ArtifactRef,
            approve_perm: &Permission,
            at: &Consistency,
        ) -> SubjectTree {
            SubjectTree {
                object: myelin_identity::ObjectId(object.0.clone()),
                relation: myelin_identity::RelName(approve_perm.0.clone()),
                members: self.members.clone(),
                zookie: at.at_least.clone(),
            }
        }
    }

    #[test]
    fn approver_set_is_list_subjects_members() {
        let subjects = FakeSubjects {
            members: approvers(),
        };
        let at = Consistency {
            at_least: Zookie("z-7".into()),
            mode: ConsistencyMode::Strong,
        };
        let set = derive_approver_set(
            &subjects,
            &ArtifactRef("myelin://acme/git/pr/42".into()),
            &Permission("git.approve".into()),
            &at,
        );
        assert_eq!(
            set,
            approvers(),
            "the approver_filter is list_subjects(object, approve_perm).members"
        );
    }

    #[test]
    fn approved_set_admits_only_approved_gates_idempotently() {
        const PR42: &str = "myelin://acme/git/pr/42";
        let mut approved = ApprovedTools::new();
        assert!(!approved.contains_effect("git.merge", PR42));

        let waiting = open_waiting();
        assert!(!approved.admit(&waiting), "a Waiting gate threads nothing");
        assert!(!approved.contains_effect("git.merge", PR42));

        let mut g = open_waiting();
        g.approve().unwrap();
        assert!(
            approved.admit(&g),
            "an Approved gate threads its per-effect key into approved"
        );
        assert!(approved.contains_effect("git.merge", PR42));
        assert!(
            !approved.contains("git.merge"),
            "a bare tool name is never an approval key (Defect B)"
        );
        assert!(
            !approved.contains_effect("git.merge", "myelin://acme/git/pr/41"),
            "an approval never transfers to a sibling object sharing the tool name"
        );
        assert!(approved.admit(&g));
        assert_eq!(
            approved.as_set().len(),
            1,
            "a double-click is one approval (one entry)"
        );

        let mut r = open_waiting();
        r.tool_name = "git.force_push".into();
        r.reject("no").unwrap();
        assert!(
            !approved.admit(&r),
            "a Rejected gate NEVER approves the effect (AG-8)"
        );
        assert!(!approved.contains_effect("git.force_push", PR42));
    }

    struct ApproveWait;
    impl HitlWait for ApproveWait {
        fn park_and_wait(&self, _gate: &HitlGate) -> WaitDecision {
            WaitDecision::Approve
        }
    }
    struct RejectWait(String);
    impl HitlWait for RejectWait {
        fn park_and_wait(&self, _gate: &HitlGate) -> WaitDecision {
            WaitDecision::Reject(self.0.clone())
        }
    }

    #[test]
    fn loop_approve_admits_tool_for_the_re_run() {
        let mut approved = ApprovedTools::new();
        let outcome = run_hitl_loop(
            GateId("gate:git.merge:pr42".into()),
            "R1",
            &plan("git.merge"),
            risk(),
            approvers(),
            "card:R1:0",
            &ApproveWait,
            &mut approved,
        );
        match outcome {
            HitlOutcome::Approved(g) => {
                assert_eq!(g.state, HitlGateState::Approved);
            }
            other => panic!("expected Approved, got {other:?}"),
        }
        assert!(
            approved.contains_effect("git.merge", "myelin://acme/git/pr/42"),
            "the approved effect's key is now in the run's approved set"
        );
    }

    #[test]
    fn loop_reject_halts_and_never_admits() {
        let mut approved = ApprovedTools::new();
        let outcome = run_hitl_loop(
            GateId("gate:git.merge:pr42".into()),
            "R1",
            &plan("git.merge"),
            risk(),
            approvers(),
            "card:R1:0",
            &RejectWait("not safe".into()),
            &mut approved,
        );
        assert_eq!(
            outcome,
            HitlOutcome::Halted(Halted::Rejected("not safe".into()))
        );
        assert!(
            !approved.contains_effect("git.merge", "myelin://acme/git/pr/42"),
            "a rejected gate makes 0 mutation (the effect stays unapproved, AG-8)"
        );
    }

    #[test]
    fn gate_id_of_only_for_gated() {
        assert_eq!(
            gate_id_of(&EffectResult::Gated(GateId("g".into()))),
            Some(GateId("g".into()))
        );
        assert_eq!(
            gate_id_of(&EffectResult::Applied(myelin_agent::EventId("e".into()))),
            None
        );
        assert_eq!(gate_id_of(&EffectResult::Denied("nope".into())), None);
    }
}
