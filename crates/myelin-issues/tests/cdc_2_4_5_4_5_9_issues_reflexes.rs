//! # The CDC pair for the cross-subsystem reflex consumers (ISS-P28 / P-395, M4)
//!
//! **Contract-index rows consumed:** 2.4 (the `EventHandler` consumer template — a `*`-free
//! `subjects()` whitelist + idempotent `handle`), 5.4 (the link/relates edges the reflexes stage), 5.9
//! (the `ci.check.updated` → CI-red Done guard feed). The reflexes are the differentiator (VISION §2 —
//! work flows between tools) shipped as CONSUMERS off the bus (EI-01 §7), never bespoke cross-subsystem
//! calls — Issues imports NO producer crate (the acyclic-producer invariant, EI-02 §3): it whitelists
//! the foreign subjects (`git.*` / `ci.check.updated` / `chat.message.created` / `identity.member.*`)
//! as validated tokens.
//!
//! The **PRODUCER** side is **Issues authoring an [`EventHandler`]** ([`ReflexConsumer`]) whose
//! `subjects()` whitelist is the FROZEN `*`-free `REFLEX_SUBJECTS` and whose `handle` is idempotent on
//! `event_id`. The **CONSUMER** side is **the Bus runtime admitting + driving it**
//! ([`myelin_events::consume`] → a live [`myelin_events::Consumer`]) — the only honest "accepted": the
//! whitelist binds (no wildcard rejection) and the seven delivery rules drive `handle` to a terminal
//! [`Delivered`]. Pinning both sides here fails CI in the same job on a drift (Issues widens to `*` or
//! authors a non-idempotent handle; the Bus renames the template surface).
//!
//! This file ALSO carries the **chained-mutation e2e drill** (the ISS-P28 GATE artifacts proven over
//! the LIVE Bus runtime, not just the in-module planner): a `git.pr.merged` → `closes` link +
//! workflow-permitting auto-close, REPLAYED, asserts **0 duplicate** (the runtime dedups the redelivery
//! → `Delivered::Deduplicated`, and the handler's staged-effect count is unchanged); and a CI-red merge
//! is **blocked** through the FSM interpreter (the **0-governance-bypass** artifact — no Done is ever
//! staged for a guarded merge the human path could not green).

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

/// The 3-state CI-gated dev workflow the auto-transitions run through (the SAME guard the human path
/// uses — ISS-P27): Todo →(branch)→ In Progress →(merge, CI-red Done guard)→ Done.
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

// =================================================================================================
// PRODUCER side — Issues authors a `*`-free, idempotent EventHandler (contract 2.4).
// =================================================================================================

/// **PRODUCER: the reflex consumer binds the FROZEN `*`-free `REFLEX_SUBJECTS` whitelist (never `*`).**
/// Pins the 2.4 promise — the handler whitelists exactly the foreign reflex subjects (no wildcard) and
/// each is grammatical/foreign.
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
        // each is a FOREIGN subject — Issues consumes, never originates it (acyclic-producer, EI-02 §3).
        assert!(
            !s.0.starts_with("issue."),
            "reflex subject `{}` must be FOREIGN (consumed)",
            s.0
        );
    }
    // the handle returns a terminal outcome for a well-formed reflex event.
    let outcome = consumer.handle(&merge_event("p-1", "success"));
    assert_eq!(outcome, HandleOutcome::Done);
}

// =================================================================================================
// CONSUMER side — the Bus runtime ADMITS + DRIVES the reflex consumer (the honest "accepted").
// =================================================================================================

/// **CONSUMER: the Bus runtime admits the reflex consumer (no wildcard rejection) + drives it to a
/// terminal `Acked`.** The `*`-free whitelist binds into a live [`Consumer`]; a `git.pr.merged`
/// delivers through the seven rules and acks (the handler staged the `closes` link + the auto-close).
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
    // the handler staged the auto-close (a trusted-green merge → Done through the CI-gated FSM).
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

/// **GATE (over the LIVE runtime): a replayed `git.pr.merged` is deduplicated by the Bus runtime → 0
/// duplicate staged effects.** The first delivery acks (staging one effect); the SAME `event_id`
/// redelivered hits the dedup ledger (`Delivered::Deduplicated`) — the handler is SKIPped and the
/// staged-effect count is UNCHANGED. This is the ISS-P28 0-duplicate-on-replay artifact proven through
/// the real consumer runtime (the dedup ledger rule 1), not just the in-module within-handler dedup.
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
    // REPLAY the same event_id: the runtime dedups (rule 1) → 0 duplicate.
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

/// **GATE (the chained-mutation e2e over the runtime): branch → link + auto-advance, merge → link +
/// auto-close, REPLAY both → 0 duplicate.** The full differentiator flow (a branch advances the issue,
/// a merge closes it) driven through the Bus consumer; replaying the chain produces 0 duplicate
/// links/transitions and every auto-transition went through the FSM interpreter (the staged Links carry
/// the FIXED categories — no bypass).
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
    // a real consumer advances the projection off its own auto-transition; the drill seeds it.
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

    // REPLAY both: the runtime dedups each → 0 duplicate.
    assert_eq!(runtime.deliver(&branch), Delivered::Deduplicated);
    assert_eq!(runtime.deliver(&merge), Delivered::Deduplicated);
    assert_eq!(
        runtime.handler().staged_count(),
        after,
        "replaying the chain produces 0 duplicate staged effects"
    );

    // and both auto-transitions went through the FSM interpreter (the FIXED categories prove it).
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

/// **GATE (0-governance-bypass over the runtime): a CI-red merge LINKS but the auto-close is BLOCKED
/// through the FSM interpreter — no Done is ever staged.** The `closes` link still lands (the PR↔issue
/// link is a fact); the auto-close is blocked by the CI-red Done guard (5.9) → a loud
/// `TransitionBlocked`, never a silent allow. A reflex can never green a Done the human path could not.
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
    // the `closes` link landed (with NO transition).
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
    // the block is LOUD.
    assert!(
        staged
            .iter()
            .any(|e| matches!(e, ReflexEffect::TransitionBlocked { .. })),
        "a CI-red merge surfaces a loud TransitionBlocked"
    );
    // 0 governance bypass: NO Done transition was ever staged.
    assert!(
        !staged.iter().any(|e| matches!(
            e,
            ReflexEffect::Link { transition: Some(p), .. } if p.to == AUTO_STATE_DONE
        )),
        "0 governance bypass: a CI-red merge never auto-closes to Done over the runtime"
    );
}

/// A `*` subscription is REJECTED at bind (the structural BUS-3 guard) — the reflex consumer CANNOT be
/// widened to a wildcard. The provider never asks for one; this pins the runtime would refuse if it did.
#[test]
fn a_wildcard_reflex_subscription_is_rejected() {
    let consumer = ReflexConsumer::new(dev_workflow());
    let spec = ConsumerSpec::new(ConsumerName("issues.reflexes.bad".into()), &["*"]);
    assert!(
        consume(spec, consumer, DedupLedger::new()).is_err(),
        "a `*` subscription must be rejected at registration (BUS-3)"
    );
}
