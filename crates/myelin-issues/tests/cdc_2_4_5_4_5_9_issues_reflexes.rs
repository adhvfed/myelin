use myelin_events::{
    consume, Actor, AggregateKey, ArtifactRef, ConsumerName, ConsumerSpec, CorrelationId, DataRole,
    DedupLedger, Delivered, EventEnvelope, EventHandler, EventId, EventType, HandleOutcome,
    Message, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::reflexes::{
    AUTO_STATE_DONE, AUTO_STATE_IN_PROGRESS, GIT_BRANCH_CREATED, GIT_PR_MERGED, REFLEX_SUBJECTS,
};
use myelin_issues::workflow::{StateCategory, Workflow, WorkflowState, WorkflowTransition};
use myelin_issues::{IssueLifecycleRel, ReflexConsumer, ReflexEffect};
use myelin_tenancy::{Region, TenantId};

const ISSUE: &str = "myelin://acme/issue/issue/ENG-1";

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn ev(event_id: &str, type_: &str, payload: serde_json::Value) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        subject: ArtifactRef(ISSUE.into()),
        aggregate: AggregateKey("issue:ENG-1".into()),
        causation_id: None,
        correlation_id: CorrelationId(event_id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
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
                guards: vec![myelin_issues::ci_guard::ci_done_guard()],
                required_fields: vec![],
                post_actions: vec![],
            },
        ],
    }
}

fn merge_event(id: &str, ci_state: &str) -> EventEnvelope {
    ev(
        id,
        GIT_PR_MERGED,
        serde_json::json!({
            "issue_ref": ISSUE,
            "artifact_ref": "myelin://acme/git/pr/42",
            "state": ci_state,
            "trust_tier": "trusted",
        }),
    )
}

#[test]
fn producer_reflex_consumer_binds_a_star_free_foreign_whitelist() {
    let consumer = ReflexConsumer::new(dev_workflow());
    let subjects = consumer.subjects();
    assert_eq!(
        subjects.len(),
        REFLEX_SUBJECTS.len(),
        "the consumer binds exactly the frozen reflex whitelist"
    );
    for s in subjects {
        assert!(
            s.0 != "*" && s.0 != ">",
            "NEVER a wildcard (BUS-3): {}",
            s.0
        );
        assert!(
            REFLEX_SUBJECTS.contains(&s.0.as_str()),
            "subject `{}` is on the frozen whitelist",
            s.0
        );
        assert!(
            !s.0.starts_with("issue."),
            "reflex subject `{}` must be FOREIGN (consumed)",
            s.0
        );
    }
    let outcome = consumer.handle(
        &merge_event("p-1", "success"),
        &mut myelin_events::HandlerTx::none(),
    );
    assert_eq!(outcome, HandleOutcome::Done);
}

#[test]
fn consumer_bus_admits_and_drives_the_reflex_consumer() {
    let consumer = ReflexConsumer::new(dev_workflow());
    consumer.set_state(ISSUE, AUTO_STATE_IN_PROGRESS);
    let spec = ConsumerSpec::new(ConsumerName("issues.reflexes".into()), REFLEX_SUBJECTS);
    let runtime =
        consume(spec, consumer, DedupLedger::new()).expect("the *-free reflex whitelist must bind");

    let msg = Message {
        subject: GIT_PR_MERGED.into(),
        envelope: merge_event("c-1", "success"),
    };
    assert_eq!(
        runtime.deliver(&msg),
        Delivered::Acked,
        "a well-formed git.pr.merged acks (Done)"
    );
    assert_eq!(
        runtime.handler().staged_count(),
        1,
        "one staged effect (the auto-close link)"
    );
    assert!(
        runtime.handler().staged().iter().any(|e| matches!(
            e,
            ReflexEffect::Link { transition: Some(p), .. } if p.to == AUTO_STATE_DONE
        )),
        "the merge auto-closed to Done through the FSM interpreter"
    );
    assert_eq!(runtime.lag(), 0, "no backlog after a clean ack");
}

#[test]
fn replayed_merge_is_deduplicated_by_the_runtime_zero_duplicate() {
    let consumer = ReflexConsumer::new(dev_workflow());
    consumer.set_state(ISSUE, AUTO_STATE_IN_PROGRESS);
    let spec = ConsumerSpec::new(ConsumerName("issues.reflexes".into()), REFLEX_SUBJECTS);
    let runtime = consume(spec, consumer, DedupLedger::new()).expect("binds");

    let msg = Message {
        subject: GIT_PR_MERGED.into(),
        envelope: merge_event("merge-1", "success"),
    };
    assert_eq!(runtime.deliver(&msg), Delivered::Acked);
    let after_first = runtime.handler().staged_count();
    assert_eq!(after_first, 1, "the merge staged one effect");
    assert_eq!(
        runtime.deliver(&msg),
        Delivered::Deduplicated,
        "a redelivered event_id is deduplicated by the runtime ledger"
    );
    assert_eq!(
        runtime.handler().staged_count(),
        after_first,
        "0 duplicate staged effects on replay (idempotent on event_id)"
    );
}

#[test]
fn chained_branch_then_merge_e2e_is_idempotent_over_the_runtime() {
    let consumer = ReflexConsumer::new(dev_workflow());
    consumer.set_state(ISSUE, "Todo");
    let spec = ConsumerSpec::new(ConsumerName("issues.reflexes".into()), REFLEX_SUBJECTS);
    let runtime = consume(spec, consumer, DedupLedger::new()).expect("binds");

    let branch = Message {
        subject: GIT_BRANCH_CREATED.into(),
        envelope: ev(
            "branch-1",
            GIT_BRANCH_CREATED,
            serde_json::json!({ "issue_ref": ISSUE, "artifact_ref": "myelin://acme/git/branch/b" }),
        ),
    };
    assert_eq!(runtime.deliver(&branch), Delivered::Acked);
    runtime.handler().set_state(ISSUE, AUTO_STATE_IN_PROGRESS);
    let merge = Message {
        subject: GIT_PR_MERGED.into(),
        envelope: merge_event("merge-1", "success"),
    };
    assert_eq!(runtime.deliver(&merge), Delivered::Acked);
    let after = runtime.handler().staged_count();
    assert_eq!(
        after, 2,
        "the branch link + the merge close are two staged effects"
    );

    assert_eq!(runtime.deliver(&branch), Delivered::Deduplicated);
    assert_eq!(runtime.deliver(&merge), Delivered::Deduplicated);
    assert_eq!(
        runtime.handler().staged_count(),
        after,
        "replaying the chain produces 0 duplicate staged effects"
    );

    let staged = runtime.handler().staged();
    assert!(
        staged.iter().any(|e| matches!(
            e,
            ReflexEffect::Link { transition: Some(p), .. } if p.to == AUTO_STATE_IN_PROGRESS
        )),
        "the branch auto-advanced through the FSM"
    );
    assert!(
        staged.iter().any(|e| matches!(
            e,
            ReflexEffect::Link { transition: Some(p), .. } if p.to == AUTO_STATE_DONE
        )),
        "the merge auto-closed through the CI-gated FSM"
    );
}

#[test]
fn ci_red_merge_blocks_the_auto_close_no_governance_bypass() {
    let consumer = ReflexConsumer::new(dev_workflow());
    consumer.set_state(ISSUE, AUTO_STATE_IN_PROGRESS);
    let spec = ConsumerSpec::new(ConsumerName("issues.reflexes".into()), REFLEX_SUBJECTS);
    let runtime = consume(spec, consumer, DedupLedger::new()).expect("binds");

    let msg = Message {
        subject: GIT_PR_MERGED.into(),
        envelope: merge_event("merge-red", "failure"),
    };
    assert_eq!(runtime.deliver(&msg), Delivered::Acked);
    let staged = runtime.handler().staged();
    assert!(
        staged.iter().any(|e| matches!(
            e,
            ReflexEffect::Link {
                rel: IssueLifecycleRel::Closes,
                transition: None,
                ..
            }
        )),
        "the closes link lands even when the auto-close is blocked"
    );
    assert!(
        staged
            .iter()
            .any(|e| matches!(e, ReflexEffect::TransitionBlocked { .. })),
        "a CI-red merge surfaces a loud TransitionBlocked"
    );
    assert!(
        !staged.iter().any(|e| matches!(
            e,
            ReflexEffect::Link { transition: Some(p), .. } if p.to == AUTO_STATE_DONE
        )),
        "0 governance bypass: a CI-red merge never auto-closes to Done over the runtime"
    );
}

#[test]
fn a_wildcard_reflex_subscription_is_rejected() {
    let consumer = ReflexConsumer::new(dev_workflow());
    let spec = ConsumerSpec::new(ConsumerName("issues.reflexes.bad".into()), &["*"]);
    assert!(
        consume(spec, consumer, DedupLedger::new()).is_err(),
        "a `*` subscription must be rejected at registration (BUS-3)"
    );
}
