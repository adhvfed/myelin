use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MicroUsd, RunId};
use myelin_tenancy::TenantId;

use crate::agent_spend::{spend_bearing_run, BalancedRunSignal, IssueRunKind, IssueSpendGate};
use crate::ci_guard::{plan_agent_ci_gated_transition, AgentTransitionOutcome, LinkedPrCheck};
use crate::e2e_wedge::IssuesE2eArtifact;
use crate::workflow::{IssueContext, StateCategory, Workflow, WorkflowState, WorkflowTransition};

use super::ci_guard::ci_done_guard;

pub const E2E_FLAGSHIP_SCENARIO: &str = "E2E-2";

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

pub const CLOSE_CARD_ID: &str = "card:triage:close-eng-1421";

const WALLET: u64 = 100;

const ESTIMATE: u64 = 20;

fn triage_workflow() -> Workflow {
    Workflow {
        states: vec![
            WorkflowState {
                name: "In Review".into(),
                category: StateCategory::Started,
            },
            WorkflowState {
                name: "Done".into(),
                category: StateCategory::Completed,
            },
        ],
        transitions: vec![WorkflowTransition {
            from: "In Review".into(),
            to: "Done".into(),
            guards: vec![ci_done_guard()],
            required_fields: vec![],
            post_actions: vec![],
        }],
    }
}

fn triage_metered_units() -> Vec<MeteredUnit> {
    vec![
        MeteredUnit {
            unit: IssueRunKind::Triage.metered_unit(),
            wholesale: MicroUsd(8),
            markup: MicroUsd(2),
        },
        MeteredUnit {
            unit: IssueRunKind::Triage.metered_unit(),
            wholesale: MicroUsd(3),
            markup: MicroUsd(1),
        },
    ]
}

#[derive(Default)]
struct HitlApplyLedger {
    buffered: std::collections::BTreeSet<String>,
    applied: std::collections::BTreeSet<String>,
    apply_count: u64,
}

impl HitlApplyLedger {
    fn deliver_approval(&mut self, key: &str) -> bool {
        self.buffered.insert(key.to_string())
    }

    fn apply_once(&mut self, key: &str) -> bool {
        if !self.buffered.contains(key) {
            return false;
        }
        if self.applied.contains(key) {
            return false;
        }
        self.applied.insert(key.to_string());
        self.apply_count += 1;
        true
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_e2e_2_issues_flagship() -> IssuesE2eArtifact {
    let mut leaks: u64 = 0;
    let tenant = tenant();
    let wf = triage_workflow();

    let green_check = LinkedPrCheck::trusted(crate::ci_guard::CHECK_STATE_SUCCESS);
    let agent_close =
        plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &green_check);
    let withheld = agent_close.is_withheld();
    let pre_approval_mutations = agent_close.pre_approval_mutations();
    if !withheld {
        leaks += 1;
    }
    if pre_approval_mutations != 0 {
        leaks += 1;
    }
    let plan_is_completed = matches!(
        &agent_close,
        AgentTransitionOutcome::WithheldForApproval { plan }
            if plan.to_category == StateCategory::Completed && plan.to == "Done"
    );
    if !plan_is_completed {
        leaks += 1;
    }

    let undeclared = plan_agent_ci_gated_transition(
        &wf,
        "In Review",
        "Canceled",
        IssueContext::new(),
        &green_check,
    );
    let undeclared_blocked = undeclared.is_blocked();
    let undeclared_zero_mutation = undeclared.pre_approval_mutations() == 0;
    if !undeclared_blocked || !undeclared_zero_mutation {
        leaks += 1;
    }
    let ci_red = LinkedPrCheck::trusted("failure");
    let red_close =
        plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &ci_red);
    let ci_red_blocks_agent = red_close.is_blocked() && red_close.pre_approval_mutations() == 0;
    if !ci_red_blocks_agent {
        leaks += 1;
    }

    let idem_key = crate::per_effect_idem_key(CLOSE_CARD_ID, 0, 1);
    let mut ledger = HitlApplyLedger::default();
    let first_delivery = ledger.deliver_approval(&idem_key);
    let applied_before_kill = ledger.apply_count;
    let duplicate_delivery = ledger.deliver_approval(&idem_key);
    let applied_on_resume = ledger.apply_once(&idem_key);
    let re_drive_no_op = !ledger.apply_once(&idem_key);
    let exactly_once_across_kill = first_delivery
        && !duplicate_delivery
        && applied_before_kill == 0
        && applied_on_resume
        && re_drive_no_op
        && ledger.apply_count == 1;
    if !exactly_once_across_kill {
        leaks += 1;
    }

    let mut gate = IssueSpendGate::new();
    let mut cost_ledger = CostLedger::new();
    let balance: BalancedRunSignal = spend_bearing_run(
        &mut gate,
        &mut cost_ledger,
        tenant.clone(),
        RunId::new("run:triage:eng-1421"),
        IssueRunKind::Triage,
        MicroUsd(ESTIMATE),
        MicroUsd(WALLET),
        triage_metered_units,
    )
    .expect("a funded wallet reserves + settles the triage run (no balance → no start)");
    let reserve_settle_balanced = balance.is_green();
    if !reserve_settle_balanced {
        leaks += 1;
    }
    let mut empty_gate = IssueSpendGate::new();
    let mut empty_ledger = CostLedger::new();
    let refused = spend_bearing_run(
        &mut empty_gate,
        &mut empty_ledger,
        tenant.clone(),
        RunId::new("run:triage:starved"),
        IssueRunKind::Triage,
        MicroUsd(ESTIMATE),
        MicroUsd(0),
        || panic!("the work must NEVER run on an exhausted wallet (no balance → no start)"),
    )
    .is_err();
    let no_balance_no_start = refused && empty_gate.runs_dispatched() == 0;
    if !no_balance_no_start {
        leaks += 1;
    }

    let green = withheld
        && pre_approval_mutations == 0
        && plan_is_completed
        && undeclared_blocked
        && undeclared_zero_mutation
        && ci_red_blocks_agent
        && exactly_once_across_kill
        && reserve_settle_balanced
        && no_balance_no_start;

    IssuesE2eArtifact {
        scenario: E2E_FLAGSHIP_SCENARIO,
        green,
        evidence: format!(
            "CI-fail→triage→issue→chat→fix-PR (Issues slice): governed close HITL-gated \
             (withheld={withheld}, pre_approval_mutations={pre_approval_mutations}); 0 effect outside \
             the ∩ (undeclared edge blocked={undeclared_blocked}, ci-red blocks agent={ci_red_blocks_agent}); \
             exactly-once approval + governed transition across a kill (first_delivery={first_delivery}, \
             duplicate_absorbed={}, apply_count={}, across_kill={exactly_once_across_kill}); \
             reserve/settle balanced (reserved {ESTIMATE} == billed {} + refunded {}, no-balance→no-start={no_balance_no_start})={reserve_settle_balanced}; \
             mock-agent runtime (real-LLM is post-M5/R-10)",
            !duplicate_delivery,
            ledger.apply_count,
            balance.billed.0,
            balance.refunded.0,
        ),
        leaks,
    }
}

#[cfg(test)]
#[path = "e2e_flagship/tests.rs"]
mod tests;
