use myelin_identity::{Consistency, ObjectId, Permission, PrincipalId, RewriteTrace, SubjectTree};
use serde::{Deserialize, Serialize};

use crate::sla_calendar::{business_fire_at, Calendar, CalendarError};
use crate::workflow::Workflow;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceView {
    WorkflowEditor,
    SlaPolicyEditor,
    TeamProjectSettings,
    AutomationTriggerBuilder,
    ImportWizard,
    AuditChangeHistory,
}

impl GovernanceView {
    pub fn all() -> [GovernanceView; 6] {
        [
            GovernanceView::WorkflowEditor,
            GovernanceView::SlaPolicyEditor,
            GovernanceView::TeamProjectSettings,
            GovernanceView::AutomationTriggerBuilder,
            GovernanceView::ImportWizard,
            GovernanceView::AuditChangeHistory,
        ]
    }

    pub fn screen_id(&self) -> &'static str {
        match self {
            GovernanceView::WorkflowEditor => "S13",
            GovernanceView::SlaPolicyEditor => "S14",
            GovernanceView::TeamProjectSettings => "S15",
            GovernanceView::AutomationTriggerBuilder => "S16",
            GovernanceView::ImportWizard => "S17",
            GovernanceView::AuditChangeHistory => "S18",
        }
    }

    pub fn view_model(&self) -> GovernanceViewModel {
        match self {
            GovernanceView::WorkflowEditor => GovernanceViewModel {
                view: *self,
                backing_engine: "crate::workflow::Workflow (ISS-P12 FSM) + crate::schemes (ISS-P11)",
                guard_language: GuardLanguage::FrozenQueryAst,
                inline_validation: &[
                    "unreachable-state (a state with no inbound transition from the initial state)",
                    "missing-category-mapping (every state maps to a fixed category - the invariant)",
                ],
            },
            GovernanceView::SlaPolicyEditor => GovernanceViewModel {
                view: *self,
                backing_engine: "crate::sla_calendar::{SlaEngine, business_fire_at, Calendar} (ISS-P26)",
                guard_language: GuardLanguage::None,
                inline_validation: &[
                    "budget exceeds the calendar's reachable working windows (a misconfigured SLA)",
                    "escalation chain references an unknown on-call team",
                ],
            },
            GovernanceView::TeamProjectSettings => GovernanceViewModel {
                view: *self,
                backing_engine: "myelin_identity::IdentityService::{list_subjects, explain} (contract 4.4)",
                guard_language: GuardLanguage::None,
                inline_validation: &[
                    "prefix collides with an existing project key",
                    "scheme assignment references an undefined scheme",
                ],
            },
            GovernanceView::AutomationTriggerBuilder => GovernanceViewModel {
                view: *self,
                backing_engine: "crate::trigger::{IssueTriggerEngine, ArmableCondition} (ISS-P25)",
                guard_language: GuardLanguage::FrozenQueryAst,
                inline_validation: &[
                    "the agent-handler picker references an undeclared ToolDef",
                    "a side-effecting handler without a HITL gate (requires_approval default)",
                ],
            },
            GovernanceView::ImportWizard => GovernanceViewModel {
                view: *self,
                backing_engine: "crate::import (ISS-P28 import engine + ADF map, contract 13.2)",
                guard_language: GuardLanguage::None,
                inline_validation: &[
                    "an unmappable permission scheme (named dropped in the reconciliation report)",
                ],
            },
            GovernanceView::AuditChangeHistory => GovernanceViewModel {
                view: *self,
                backing_engine: "contract 10.6 (the tamper-evident audit log - Issues contributes attribution)",
                guard_language: GuardLanguage::None,
                inline_validation: &[
                    "no inline validation (a read-only timeline; the log is append-only upstream)",
                ],
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernanceViewModel {
    pub view: GovernanceView,
    pub backing_engine: &'static str,
    pub guard_language: GuardLanguage,
    pub inline_validation: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardLanguage {
    FrozenQueryAst,
    None,
}

pub trait PermissionResolver {
    fn list_subjects(
        &self,
        object: &ObjectId,
        permission: &Permission,
        at: &Consistency,
    ) -> SubjectTree;

    fn explain(
        &self,
        subject: &PrincipalId,
        permission: &Permission,
        object: &ObjectId,
        at: &Consistency,
    ) -> RewriteTrace;
}

pub struct PermissionInspector<R: PermissionResolver> {
    resolver: R,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectorAnswer {
    pub object: ObjectId,
    pub permission: Permission,
    pub members: Vec<PrincipalId>,
}

impl<R: PermissionResolver> PermissionInspector<R> {
    pub fn new(resolver: R) -> PermissionInspector<R> {
        PermissionInspector { resolver }
    }

    pub fn who_can(
        &self,
        object: &ObjectId,
        permission: &Permission,
        at: &Consistency,
    ) -> InspectorAnswer {
        let tree = self.resolver.list_subjects(object, permission, at);
        InspectorAnswer {
            object: tree.object,
            permission: Permission(tree.relation.0),
            members: tree.members,
        }
    }

    pub fn why(
        &self,
        subject: &PrincipalId,
        permission: &Permission,
        object: &ObjectId,
        at: &Consistency,
    ) -> RewriteTrace {
        self.resolver.explain(subject, permission, object, at)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreachSimulation {
    pub start: i64,
    pub budget_secs: i64,
    pub fire_at: i64,
}

pub fn simulate_breach(
    start: i64,
    budget_secs: i64,
    cal: &Calendar,
) -> Result<BreachSimulation, CalendarError> {
    let fire_at = business_fire_at(start, budget_secs, cal)?;
    Ok(BreachSimulation {
        start,
        budget_secs,
        fire_at,
    })
}

pub fn workflow_unreachable_states(wf: &Workflow) -> Vec<String> {
    if wf.states.is_empty() {
        return Vec::new();
    }
    let initial = wf.states[0].name.clone();

    let mut reachable: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut frontier: Vec<String> = vec![initial.clone()];
    reachable.insert(initial);
    while let Some(state) = frontier.pop() {
        for t in &wf.transitions {
            if t.from == state && !reachable.contains(&t.to) {
                reachable.insert(t.to.clone());
                frontier.push(t.to.clone());
            }
        }
    }

    let mut unreachable: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for s in &wf.states {
        if !reachable.contains(&s.name) {
            unreachable.insert(s.name.clone());
        }
    }
    unreachable.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sla_calendar::Calendar;
    use crate::workflow::{StateCategory, Workflow, WorkflowState, WorkflowTransition};

    #[test]
    fn governance_catalogue_covers_s13_through_s18() {
        let ids: Vec<&str> = GovernanceView::all()
            .iter()
            .map(|v| v.screen_id())
            .collect();
        assert_eq!(ids, vec!["S13", "S14", "S15", "S16", "S17", "S18"]);
    }

    #[test]
    fn every_view_declares_a_real_backing_engine() {
        for v in GovernanceView::all() {
            let vm = v.view_model();
            assert_eq!(vm.view, v);
            assert!(
                !vm.backing_engine.is_empty(),
                "{} must name its real backing engine (no parallel calc)",
                v.screen_id()
            );
        }
    }

    #[test]
    fn guard_carrying_screens_use_frozen_queryast() {
        assert_eq!(
            GovernanceView::WorkflowEditor.view_model().guard_language,
            GuardLanguage::FrozenQueryAst
        );
        assert_eq!(
            GovernanceView::AutomationTriggerBuilder
                .view_model()
                .guard_language,
            GuardLanguage::FrozenQueryAst
        );
    }

    #[test]
    fn breach_simulation_uses_real_sla_engine() {
        let cal = Calendar::business_hours_fixed("biz-utc", 0);
        let start = 1_718_870_400;
        let budget = 4 * 3600;

        let sim = simulate_breach(start, budget, &cal).expect("simulation");
        let engine_fire_at = business_fire_at(start, budget, &cal).expect("engine fire_at");

        assert_eq!(
            sim.fire_at, engine_fire_at,
            "the breach-simulation fire_at must equal the REAL SLA engine's business_fire_at (no parallel calc)"
        );
        assert!(
            sim.fire_at >= start + budget,
            "the business-calendar fire_at is at or after the wall-clock instant (non-working time is skipped)"
        );
    }

    #[test]
    fn workflow_editor_flags_unreachable_state() {
        let wf = Workflow {
            states: vec![
                WorkflowState {
                    name: "Todo".into(),
                    category: StateCategory::Unstarted,
                },
                WorkflowState {
                    name: "In Progress".into(),
                    category: StateCategory::Started,
                },
                WorkflowState {
                    name: "Done".into(),
                    category: StateCategory::Completed,
                },
                WorkflowState {
                    name: "Blocked".into(),
                    category: StateCategory::Started,
                },
            ],
            transitions: vec![
                WorkflowTransition {
                    from: "Todo".into(),
                    to: "In Progress".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
                WorkflowTransition {
                    from: "In Progress".into(),
                    to: "Done".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
            ],
        };
        let unreachable = workflow_unreachable_states(&wf);
        assert_eq!(
            unreachable,
            vec!["Blocked".to_string()],
            "the orphaned Blocked state is flagged unreachable; Todo/In Progress/Done are not"
        );
    }

    #[test]
    fn fully_connected_workflow_has_no_unreachable_states() {
        let wf = Workflow {
            states: vec![
                WorkflowState {
                    name: "Todo".into(),
                    category: StateCategory::Unstarted,
                },
                WorkflowState {
                    name: "In Progress".into(),
                    category: StateCategory::Started,
                },
                WorkflowState {
                    name: "Done".into(),
                    category: StateCategory::Completed,
                },
                WorkflowState {
                    name: "Cancelled".into(),
                    category: StateCategory::Cancelled,
                },
            ],
            transitions: vec![
                WorkflowTransition {
                    from: "Todo".into(),
                    to: "In Progress".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
                WorkflowTransition {
                    from: "In Progress".into(),
                    to: "Done".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
                WorkflowTransition {
                    from: "Todo".into(),
                    to: "Cancelled".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
            ],
        };
        assert!(
            workflow_unreachable_states(&wf).is_empty(),
            "a fully-connected workflow has no unreachable states"
        );
    }

    #[test]
    fn inspector_renders_exactly_the_resolver_answer() {
        use myelin_identity::{ConsistencyMode, RelName, Zookie};

        struct FakeResolver;
        impl PermissionResolver for FakeResolver {
            fn list_subjects(
                &self,
                object: &ObjectId,
                permission: &Permission,
                _at: &Consistency,
            ) -> SubjectTree {
                SubjectTree {
                    object: object.clone(),
                    relation: RelName(permission.0.clone()),
                    members: vec![PrincipalId("p:alice".into()), PrincipalId("p:bob".into())],
                    zookie: Zookie("z42".into()),
                }
            }
            fn explain(
                &self,
                subject: &PrincipalId,
                permission: &Permission,
                object: &ObjectId,
                _at: &Consistency,
            ) -> RewriteTrace {
                RewriteTrace {
                    steps: vec![
                        format!("expand {}#{} for {}", object.0, permission.0, subject.0),
                        "ALLOW - p:alice is in the expanded subject set (2 member(s))".into(),
                    ],
                }
            }
        }

        let inspector = PermissionInspector::new(FakeResolver);
        let at = Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        };
        let object = ObjectId("issue:PROJ-1".into());
        let perm = Permission("approve".into());

        let answer = inspector.who_can(&object, &perm, &at);
        assert_eq!(
            answer.members,
            vec![PrincipalId("p:alice".into()), PrincipalId("p:bob".into())],
            "the inspector renders exactly the resolver's SubjectTree members (0 private recompute)"
        );
        assert_eq!(answer.object, object);
        assert_eq!(answer.permission, perm);

        let trace = inspector.why(&PrincipalId("p:alice".into()), &perm, &object, &at);
        assert_eq!(trace.steps.len(), 2);
        assert!(
            trace.steps.last().unwrap().starts_with("ALLOW"),
            "the inspector shows Identity's verdict verbatim"
        );
    }
}
