use crate::ci_guard::{plan_ci_gated_transition, LinkedPrCheck};
use crate::refs_glue::IssueLifecycleRel;
use crate::workflow::{TransitionPlan, Workflow};
use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use std::collections::BTreeSet;
use std::sync::Mutex;

pub const GIT_BRANCH_CREATED: &str = "git.branch.created";
pub const GIT_PR_OPENED: &str = "git.pr.opened";
pub const GIT_PR_MERGED: &str = "git.pr.merged";
pub const CI_CHECK_UPDATED: &str = "ci.check.updated";
pub const CHAT_MESSAGE_CREATED: &str = "chat.message.created";
pub const IDENTITY_MEMBER_ADDED: &str = "identity.member.added";
pub const IDENTITY_MEMBER_DEACTIVATED: &str = "identity.member.deactivated";
pub const IDENTITY_MEMBER_ERASED: &str = "identity.member.erased";

pub const REFLEX_SUBJECTS: &[&str] = &[
    GIT_BRANCH_CREATED,
    GIT_PR_OPENED,
    GIT_PR_MERGED,
    CI_CHECK_UPDATED,
    CHAT_MESSAGE_CREATED,
    IDENTITY_MEMBER_ADDED,
    IDENTITY_MEMBER_DEACTIVATED,
    IDENTITY_MEMBER_ERASED,
];

pub fn reflex_subjects() -> &'static [SubjectPattern] {
    use std::sync::OnceLock;
    static SUBJECTS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    SUBJECTS
        .get_or_init(|| {
            REFLEX_SUBJECTS
                .iter()
                .map(|s| SubjectPattern((*s).to_string()))
                .collect()
        })
        .as_slice()
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReflexEffect {
    Link {
        issue: String,
        artifact: String,
        rel: IssueLifecycleRel,
        transition: Option<TransitionPlan>,
    },
    CreateIssueFromChat {
        source_message: String,
        title: String,
    },
    TransitionBlocked {
        issue: String,
        reason: String,
    },
    AnonymiseActor {
        actor_pseudonym: String,
        is_erasure: bool,
    },
    GuardFeed {
        issue: String,
        check: LinkedPrCheck,
    },
    NoOp,
}

pub mod payload_key {
    pub const ISSUE_REF: &str = "issue_ref";
    pub const ARTIFACT_REF: &str = "artifact_ref";
    pub const MESSAGE_REF: &str = "message_ref";
    pub const TITLE: &str = "title";
    pub const CREATE_ISSUE: &str = "create_issue";
    pub const ACTOR_PSEUDONYM: &str = "actor_pseudonym";
    pub const CHECK_STATE: &str = "state";
    pub const TRUST_TIER: &str = "trust_tier";
    pub const ENDORSED: &str = "endorsed";
}

fn payload_str<'a>(ev: &'a EventEnvelope, key: &str) -> Option<&'a str> {
    ev.payload.get(key).and_then(|v| v.as_str())
}

fn payload_bool(ev: &EventEnvelope, key: &str) -> bool {
    ev.payload
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

pub const AUTO_STATE_IN_PROGRESS: &str = "In Progress";
pub const AUTO_STATE_DONE: &str = "Done";

pub fn plan_branch_created(ev: &EventEnvelope, wf: &Workflow, current_state: &str) -> ReflexEffect {
    let (Some(issue), Some(branch)) = (
        payload_str(ev, payload_key::ISSUE_REF),
        payload_str(ev, payload_key::ARTIFACT_REF),
    ) else {
        return ReflexEffect::NoOp;
    };
    let transition = wf
        .plan_transition(current_state, AUTO_STATE_IN_PROGRESS, &Default::default())
        .ok();
    ReflexEffect::Link {
        issue: issue.to_string(),
        artifact: branch.to_string(),
        rel: IssueLifecycleRel::Relates,
        transition,
    }
}

pub fn plan_pr_opened(ev: &EventEnvelope) -> ReflexEffect {
    let (Some(issue), Some(pr)) = (
        payload_str(ev, payload_key::ISSUE_REF),
        payload_str(ev, payload_key::ARTIFACT_REF),
    ) else {
        return ReflexEffect::NoOp;
    };
    ReflexEffect::Link {
        issue: issue.to_string(),
        artifact: pr.to_string(),
        rel: IssueLifecycleRel::Closes,
        transition: None,
    }
}

pub fn plan_pr_merged(ev: &EventEnvelope, wf: &Workflow, current_state: &str) -> Vec<ReflexEffect> {
    let (Some(issue), Some(pr)) = (
        payload_str(ev, payload_key::ISSUE_REF),
        payload_str(ev, payload_key::ARTIFACT_REF),
    ) else {
        return vec![ReflexEffect::NoOp];
    };
    let check = linked_pr_from_payload(ev)
        .unwrap_or_else(|| LinkedPrCheck::trusted(crate::ci_guard::CHECK_STATE_NEUTRAL));
    let plan = plan_ci_gated_transition(
        wf,
        current_state,
        AUTO_STATE_DONE,
        Default::default(),
        &check,
    );
    let mut effects = Vec::with_capacity(2);
    match plan {
        Ok(transition) => {
            effects.push(ReflexEffect::Link {
                issue: issue.to_string(),
                artifact: pr.to_string(),
                rel: IssueLifecycleRel::Closes,
                transition: Some(transition),
            });
        }
        Err(block) => {
            effects.push(ReflexEffect::Link {
                issue: issue.to_string(),
                artifact: pr.to_string(),
                rel: IssueLifecycleRel::Closes,
                transition: None,
            });
            effects.push(ReflexEffect::TransitionBlocked {
                issue: issue.to_string(),
                reason: block.reason(),
            });
        }
    }
    effects
}

pub fn plan_check_updated(ev: &EventEnvelope) -> ReflexEffect {
    let Some(issue) = payload_str(ev, payload_key::ISSUE_REF) else {
        return ReflexEffect::NoOp;
    };
    let Some(check) = linked_pr_from_payload(ev) else {
        return ReflexEffect::NoOp;
    };
    ReflexEffect::GuardFeed {
        issue: issue.to_string(),
        check,
    }
}

pub fn linked_pr_from_payload(ev: &EventEnvelope) -> Option<LinkedPrCheck> {
    let state = payload_str(ev, payload_key::CHECK_STATE)?;
    let trust_tier = payload_str(ev, payload_key::TRUST_TIER)?;
    let endorsed = payload_bool(ev, payload_key::ENDORSED);
    Some(LinkedPrCheck {
        state: state.to_string(),
        trust_tier: trust_tier.to_string(),
        endorsed,
    })
}

pub fn plan_chat_message_created(ev: &EventEnvelope) -> ReflexEffect {
    if !payload_bool(ev, payload_key::CREATE_ISSUE) {
        return ReflexEffect::NoOp;
    }
    let (Some(message), Some(title)) = (
        payload_str(ev, payload_key::MESSAGE_REF),
        payload_str(ev, payload_key::TITLE),
    ) else {
        return ReflexEffect::NoOp;
    };
    ReflexEffect::CreateIssueFromChat {
        source_message: message.to_string(),
        title: title.to_string(),
    }
}

pub fn plan_member_event(ev: &EventEnvelope, is_erasure: bool) -> ReflexEffect {
    let Some(actor) = payload_str(ev, payload_key::ACTOR_PSEUDONYM) else {
        return ReflexEffect::NoOp;
    };
    ReflexEffect::AnonymiseActor {
        actor_pseudonym: actor.to_string(),
        is_erasure,
    }
}

pub fn plan_reflex(ev: &EventEnvelope, wf: &Workflow, current_state: &str) -> Vec<ReflexEffect> {
    match ev.type_.0.as_str() {
        GIT_BRANCH_CREATED => vec![plan_branch_created(ev, wf, current_state)],
        GIT_PR_OPENED => vec![plan_pr_opened(ev)],
        GIT_PR_MERGED => plan_pr_merged(ev, wf, current_state),
        CI_CHECK_UPDATED => vec![plan_check_updated(ev)],
        CHAT_MESSAGE_CREATED => vec![plan_chat_message_created(ev)],
        IDENTITY_MEMBER_ADDED | IDENTITY_MEMBER_DEACTIVATED => {
            vec![plan_member_event(ev, false)]
        }
        IDENTITY_MEMBER_ERASED => vec![plan_member_event(ev, true)],
        _ => vec![ReflexEffect::NoOp],
    }
}

pub struct ReflexConsumer {
    state: Mutex<ReflexState>,
}

struct ReflexState {
    workflow: Workflow,
    current_state: std::collections::BTreeMap<String, String>,
    seen_events: BTreeSet<String>,
    staged: Vec<ReflexEffect>,
}

impl ReflexConsumer {
    pub fn new(workflow: Workflow) -> ReflexConsumer {
        ReflexConsumer {
            state: Mutex::new(ReflexState {
                workflow,
                current_state: std::collections::BTreeMap::new(),
                seen_events: BTreeSet::new(),
                staged: Vec::new(),
            }),
        }
    }

    pub fn set_state(&self, issue: &str, state: &str) {
        let mut s = self.state.lock().expect("reflex state lock");
        s.current_state.insert(issue.to_string(), state.to_string());
    }

    fn state_of(state: &ReflexState, issue: &str) -> String {
        state
            .current_state
            .get(issue)
            .cloned()
            .or_else(|| state.workflow.states.first().map(|s| s.name.clone()))
            .unwrap_or_default()
    }

    pub fn staged(&self) -> Vec<ReflexEffect> {
        self.state.lock().expect("reflex state lock").staged.clone()
    }

    pub fn staged_count(&self) -> usize {
        self.state.lock().expect("reflex state lock").staged.len()
    }
}

impl EventHandler for ReflexConsumer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        reflex_subjects()
    }

    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        let mut state = self.state.lock().expect("reflex state lock");
        if !state.seen_events.insert(ev.event_id.0.clone()) {
            return HandleOutcome::Done;
        }
        if ev.type_.0.is_empty() {
            return HandleOutcome::NonRetryable(Reason(
                "reflex: event carries no type - cannot route the reflex".into(),
            ));
        }
        let issue = payload_str(ev, payload_key::ISSUE_REF)
            .unwrap_or_default()
            .to_string();
        let current = Self::state_of(&state, &issue);
        let effects = plan_reflex(ev, &state.workflow, &current);
        for effect in effects {
            if effect != ReflexEffect::NoOp {
                state.staged.push(effect);
            }
        }
        HandleOutcome::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{StateCategory, WorkflowState, WorkflowTransition};
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventId, EventType, Timestamp,
        Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn ev(type_: &str, payload: serde_json::Value) -> EventEnvelope {
        ev_with_id("e-1", type_, payload)
    }

    fn ev_with_id(id: &str, type_: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            subject: ArtifactRef("myelin://acme/issue/issue/ENG-1".into()),
            aggregate: AggregateKey("issue:ENG-1".into()),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
            payload,
        }
    }

    fn dev_workflow() -> Workflow {
        Workflow {
            states: vec![
                WorkflowState {
                    name: "Todo".into(),
                    category: StateCategory::Unstarted,
                },
                WorkflowState {
                    name: AUTO_STATE_IN_PROGRESS.into(),
                    category: StateCategory::Started,
                },
                WorkflowState {
                    name: AUTO_STATE_DONE.into(),
                    category: StateCategory::Completed,
                },
            ],
            transitions: vec![
                WorkflowTransition {
                    from: "Todo".into(),
                    to: AUTO_STATE_IN_PROGRESS.into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
                WorkflowTransition {
                    from: AUTO_STATE_IN_PROGRESS.into(),
                    to: AUTO_STATE_DONE.into(),
                    guards: vec![crate::ci_guard::ci_done_guard()],
                    required_fields: vec![],
                    post_actions: vec![],
                },
            ],
        }
    }

    #[test]
    fn every_reflex_subject_is_grammatical_and_wildcard_free() {
        for &subj in REFLEX_SUBJECTS {
            assert!(
                myelin_events::validate_event_type(subj).is_ok(),
                "reflex subject `{subj}` is UNGRAMMATICAL: {:?}",
                myelin_events::validate_event_type(subj)
            );
            assert!(
                !subj.contains('*') && !subj.contains('>'),
                "no `*`/`>`: {subj}"
            );
        }
        let subjects: Vec<&str> = REFLEX_SUBJECTS.to_vec();
        let sub = myelin_events::Subscription::bind(
            myelin_events::ConsumerName("issue-reflexes".into()),
            &subjects,
            myelin_events::PrefetchBound::DEFAULT,
        );
        assert!(sub.is_ok(), "the reflex whitelist binds: {sub:?}");
    }

    #[test]
    fn no_reflex_subject_is_issue_originated() {
        for &subj in REFLEX_SUBJECTS {
            assert!(
                !subj.starts_with("issue."),
                "reflex subject `{subj}` must be FOREIGN (consumed, not originated)"
            );
        }
    }

    #[test]
    fn branch_created_links_and_auto_advances_through_the_fsm() {
        let wf = dev_workflow();
        let e = ev(
            GIT_BRANCH_CREATED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/branch/eng-1-fix",
            }),
        );
        let effect = plan_branch_created(&e, &wf, "Todo");
        match effect {
            ReflexEffect::Link {
                issue,
                artifact,
                rel,
                transition,
            } => {
                assert_eq!(issue, "myelin://acme/issue/issue/ENG-1");
                assert_eq!(artifact, "myelin://acme/git/branch/eng-1-fix");
                assert_eq!(rel, IssueLifecycleRel::Relates);
                let plan = transition.expect("Todo → In Progress is permitted");
                assert_eq!(plan.to, AUTO_STATE_IN_PROGRESS);
                assert_eq!(plan.to_category, StateCategory::Started);
            }
            other => panic!("expected a Link effect, got {other:?}"),
        }
    }

    #[test]
    fn branch_created_links_but_does_not_transition_when_not_permitted() {
        let wf = dev_workflow();
        let e = ev(
            GIT_BRANCH_CREATED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/branch/eng-1-fix",
            }),
        );
        let effect = plan_branch_created(&e, &wf, "Done");
        match effect {
            ReflexEffect::Link { transition, .. } => {
                assert!(
                    transition.is_none(),
                    "no FSM edge → no auto-transition (no bypass)"
                );
            }
            other => panic!("expected a Link effect, got {other:?}"),
        }
    }

    #[test]
    fn branch_created_with_no_issue_ref_is_a_noop() {
        let wf = dev_workflow();
        let e = ev(
            GIT_BRANCH_CREATED,
            serde_json::json!({ "artifact_ref": "x" }),
        );
        assert_eq!(plan_branch_created(&e, &wf, "Todo"), ReflexEffect::NoOp);
    }

    #[test]
    fn pr_opened_links_closes_without_transition() {
        let e = ev(
            GIT_PR_OPENED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/pr/42",
            }),
        );
        match plan_pr_opened(&e) {
            ReflexEffect::Link {
                rel, transition, ..
            } => {
                assert_eq!(rel, IssueLifecycleRel::Closes);
                assert!(transition.is_none(), "opening a PR does not auto-close");
            }
            other => panic!("expected a Link, got {other:?}"),
        }
    }

    #[test]
    fn pr_merged_trusted_green_auto_closes_through_the_ci_gated_fsm() {
        let wf = dev_workflow();
        let e = ev(
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/pr/42",
                "state": "success",
                "trust_tier": "trusted",
            }),
        );
        let effects = plan_pr_merged(&e, &wf, AUTO_STATE_IN_PROGRESS);
        assert_eq!(
            effects.len(),
            1,
            "a permitted close is one Link effect (no block)"
        );
        match &effects[0] {
            ReflexEffect::Link {
                rel, transition, ..
            } => {
                assert_eq!(*rel, IssueLifecycleRel::Closes);
                let plan = transition
                    .as_ref()
                    .expect("a trusted green merge auto-closes");
                assert_eq!(plan.to, AUTO_STATE_DONE);
                assert_eq!(plan.to_category, StateCategory::Completed);
            }
            other => panic!("expected a Link, got {other:?}"),
        }
    }

    #[test]
    fn pr_merged_ci_red_links_but_blocks_the_auto_close() {
        let wf = dev_workflow();
        let e = ev(
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/pr/42",
                "state": "failure",
                "trust_tier": "trusted",
            }),
        );
        let effects = plan_pr_merged(&e, &wf, AUTO_STATE_IN_PROGRESS);
        assert_eq!(
            effects.len(),
            2,
            "a blocked close = the link + the loud block"
        );
        match &effects[0] {
            ReflexEffect::Link { transition, .. } => {
                assert!(transition.is_none(), "a CI-red merge does NOT auto-close");
            }
            other => panic!("expected a Link, got {other:?}"),
        }
        match &effects[1] {
            ReflexEffect::TransitionBlocked { reason, .. } => {
                assert!(
                    reason.contains("CI is not green"),
                    "the block names the guard: {reason}"
                );
            }
            other => panic!("expected a TransitionBlocked, got {other:?}"),
        }
    }

    #[test]
    fn pr_merged_unendorsed_fork_success_blocks_the_auto_close() {
        let wf = dev_workflow();
        let e = ev(
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/pr/42",
                "state": "success",
                "trust_tier": "untrusted_fork",
                "endorsed": false,
            }),
        );
        let effects = plan_pr_merged(&e, &wf, AUTO_STATE_IN_PROGRESS);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, ReflexEffect::TransitionBlocked { .. })),
            "an un-endorsed fork success blocks the auto-close (poisoned-Done defence)"
        );
    }

    #[test]
    fn pr_merged_with_no_check_posture_fails_closed() {
        let wf = dev_workflow();
        let e = ev(
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/pr/42",
            }),
        );
        let effects = plan_pr_merged(&e, &wf, AUTO_STATE_IN_PROGRESS);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, ReflexEffect::TransitionBlocked { .. })),
            "a merge with no readable check posture fails closed (no auto-close)"
        );
    }

    #[test]
    fn check_updated_feeds_the_guard_off_the_fact() {
        let e = ev(
            CI_CHECK_UPDATED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "state": "success",
                "trust_tier": "untrusted_fork",
                "endorsed": true,
            }),
        );
        match plan_check_updated(&e) {
            ReflexEffect::GuardFeed { issue, check } => {
                assert_eq!(issue, "myelin://acme/issue/issue/ENG-1");
                assert_eq!(check.trust_tier, "untrusted_fork");
                assert!(check.endorsed);
                assert!(
                    check.is_acceptable(),
                    "an endorsed fork success is acceptable"
                );
            }
            other => panic!("expected a GuardFeed, got {other:?}"),
        }
    }

    #[test]
    fn chat_create_issue_message_creates_an_issue_with_a_relates_edge() {
        let e = ev(
            CHAT_MESSAGE_CREATED,
            serde_json::json!({
                "create_issue": true,
                "message_ref": "myelin://acme/chat/message/m-7",
                "title": "Investigate the flaky test",
            }),
        );
        match plan_chat_message_created(&e) {
            ReflexEffect::CreateIssueFromChat {
                source_message,
                title,
            } => {
                assert_eq!(source_message, "myelin://acme/chat/message/m-7");
                assert_eq!(title, "Investigate the flaky test");
            }
            other => panic!("expected CreateIssueFromChat, got {other:?}"),
        }
    }

    #[test]
    fn chat_non_create_issue_message_is_a_noop() {
        let e = ev(
            CHAT_MESSAGE_CREATED,
            serde_json::json!({ "message_ref": "myelin://acme/chat/message/m-7" }),
        );
        assert_eq!(plan_chat_message_created(&e), ReflexEffect::NoOp);
    }

    #[test]
    fn member_erased_anonymises_the_actor() {
        let e = ev(
            IDENTITY_MEMBER_ERASED,
            serde_json::json!({ "actor_pseudonym": "8a2f@acme.noreply" }),
        );
        match plan_member_event(&e, true) {
            ReflexEffect::AnonymiseActor {
                actor_pseudonym,
                is_erasure,
            } => {
                assert_eq!(actor_pseudonym, "8a2f@acme.noreply");
                assert!(is_erasure, "an erased member is the §7 erasure lever");
            }
            other => panic!("expected AnonymiseActor, got {other:?}"),
        }
    }

    #[test]
    fn member_deactivated_reassigns_without_erasure() {
        let e = ev(
            IDENTITY_MEMBER_DEACTIVATED,
            serde_json::json!({ "actor_pseudonym": "8a2f@acme.noreply" }),
        );
        match plan_member_event(&e, false) {
            ReflexEffect::AnonymiseActor { is_erasure, .. } => {
                assert!(!is_erasure, "a deactivate is a reassign, not an erasure");
            }
            other => panic!("expected AnonymiseActor, got {other:?}"),
        }
    }

    #[test]
    fn replayed_merge_produces_zero_duplicate_effects() {
        let consumer = ReflexConsumer::new(dev_workflow());
        consumer.set_state("myelin://acme/issue/issue/ENG-1", AUTO_STATE_IN_PROGRESS);
        let e = ev_with_id(
            "merge-1",
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "myelin://acme/git/pr/42",
                "state": "success",
                "trust_tier": "trusted",
            }),
        );
        assert_eq!(
            consumer.handle(&e, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        let after_first = consumer.staged_count();
        assert_eq!(after_first, 1, "the merge staged one Link (the auto-close)");
        assert_eq!(
            consumer.handle(&e, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(
            consumer.staged_count(),
            after_first,
            "a replayed merge produces 0 duplicate staged effects (idempotent on event_id)"
        );
    }

    #[test]
    fn chained_branch_then_merge_is_idempotent_on_replay() {
        let consumer = ReflexConsumer::new(dev_workflow());
        let issue = "myelin://acme/issue/issue/ENG-1";
        consumer.set_state(issue, "Todo");
        let branch = ev_with_id(
            "branch-1",
            GIT_BRANCH_CREATED,
            serde_json::json!({ "issue_ref": issue, "artifact_ref": "myelin://acme/git/branch/b" }),
        );
        consumer.handle(&branch, &mut myelin_events::HandlerTx::none());
        consumer.set_state(issue, AUTO_STATE_IN_PROGRESS);
        let merge = ev_with_id(
            "merge-1",
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": issue,
                "artifact_ref": "myelin://acme/git/pr/42",
                "state": "success",
                "trust_tier": "trusted",
            }),
        );
        consumer.handle(&merge, &mut myelin_events::HandlerTx::none());
        let after = consumer.staged_count();
        assert_eq!(
            after, 2,
            "the branch link + the merge close are two staged effects"
        );
        consumer.handle(&branch, &mut myelin_events::HandlerTx::none());
        consumer.handle(&merge, &mut myelin_events::HandlerTx::none());
        assert_eq!(
            consumer.staged_count(),
            after,
            "replaying the chain produces 0 duplicate staged effects"
        );
        let staged = consumer.staged();
        let advanced = staged.iter().any(|e| {
            matches!(
                e,
                ReflexEffect::Link { transition: Some(p), .. } if p.to == AUTO_STATE_IN_PROGRESS
            )
        });
        let closed = staged.iter().any(|e| {
            matches!(
                e,
                ReflexEffect::Link { transition: Some(p), .. } if p.to == AUTO_STATE_DONE
            )
        });
        assert!(advanced, "the branch auto-advanced through the FSM");
        assert!(closed, "the merge auto-closed through the CI-gated FSM");
    }

    #[test]
    fn consumer_never_bypasses_the_workflow_guard() {
        let consumer = ReflexConsumer::new(dev_workflow());
        let issue = "myelin://acme/issue/issue/ENG-1";
        consumer.set_state(issue, AUTO_STATE_IN_PROGRESS);
        let merge = ev_with_id(
            "merge-red",
            GIT_PR_MERGED,
            serde_json::json!({
                "issue_ref": issue,
                "artifact_ref": "myelin://acme/git/pr/42",
                "state": "failure",
                "trust_tier": "trusted",
            }),
        );
        consumer.handle(&merge, &mut myelin_events::HandlerTx::none());
        let staged = consumer.staged();
        assert!(staged.iter().any(|e| matches!(
            e,
            ReflexEffect::Link {
                rel: IssueLifecycleRel::Closes,
                transition: None,
                ..
            }
        )));
        assert!(staged
            .iter()
            .any(|e| matches!(e, ReflexEffect::TransitionBlocked { .. })));
        assert!(
            !staged.iter().any(|e| matches!(
                e,
                ReflexEffect::Link { transition: Some(p), .. } if p.to == AUTO_STATE_DONE
            )),
            "0 governance bypass: a CI-red merge never auto-closes to Done"
        );
    }

    #[test]
    fn plan_reflex_routes_every_whitelisted_type() {
        let wf = dev_workflow();
        for &subj in REFLEX_SUBJECTS {
            let payload = serde_json::json!({
                "issue_ref": "myelin://acme/issue/issue/ENG-1",
                "artifact_ref": "x",
                "actor_pseudonym": "8a2f@acme.noreply",
                "create_issue": true,
                "message_ref": "m",
                "title": "t",
                "state": "success",
                "trust_tier": "trusted",
            });
            let e = ev(subj, payload);
            let effects = plan_reflex(&e, &wf, "Todo");
            assert!(
                !effects.is_empty(),
                "every type routes to ≥1 effect: {subj}"
            );
            assert!(
                !effects.iter().all(|e| *e == ReflexEffect::NoOp),
                "a well-formed `{subj}` event is not a no-op"
            );
        }
    }
}
