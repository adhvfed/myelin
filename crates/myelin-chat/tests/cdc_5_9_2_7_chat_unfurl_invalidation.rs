use myelin_chat::unfurl::invalidation::{invalidates_card, UnfurlInvalidator};
use myelin_chat::unfurl::{Projection, UnfurlCache};
use myelin_events::taxonomy::new_tokens::CI_CHECK_UPDATED;
use myelin_events::{
    Actor, AggregateKey, CausedBy, CorrelationId, DataRole, DedupLedger, EventEnvelope,
    EventHandler, EventId, EventType, HandleOutcome, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef as RefsRef;
use myelin_tenancy::{ArtifactRef as TenancyRef, Region, TenantId};

const PR_REF: &str = "myelin://acme/git/pr/88";
const ISSUE_REF: &str = "myelin://acme/issues/issue/ENG-9";

fn principal(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    p.region = Region("fr-par".into());
    p
}

fn producer_event(token: &str, subject_ref: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J-evt".into()),
        type_: EventType(token.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(principal("p:ci")),
        subject: TenancyRef(subject_ref.into()),
        aggregate: AggregateKey("agg:01J".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J-corr".into()),
        caused_by: Some(CausedBy("session:1".into())),
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

fn cached(cache: &UnfurlCache, ref_: &str, title: &str) {
    cache.put(
        &RefsRef(ref_.into()),
        Projection {
            title: title.into(),
            state: "open".into(),
            icon: "pr".into(),
            sub_anchor: None,
        },
    );
}

#[test]
fn cdc_5_9_ci_check_updated_busts_the_chat_unfurl_card() {
    let cache = UnfurlCache::new();
    cached(&cache, PR_REF, "Fix the flaky test");
    let invalidator = UnfurlInvalidator::new(cache.clone());

    assert_eq!(CI_CHECK_UPDATED, "ci.check.updated");
    assert!(
        invalidates_card(CI_CHECK_UPDATED),
        "ci.check.updated invalidates a card"
    );

    let ev = producer_event(CI_CHECK_UPDATED, PR_REF);
    assert_eq!(invalidator.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
    assert!(
        !cache.contains(&RefsRef(PR_REF.into())),
        "the ci.check.updated busted the chat unfurl card (it re-resolves the fresh check state)"
    );
}

#[test]
fn cdc_2_7_erased_busts_the_chat_unfurl_card() {
    let cache = UnfurlCache::new();
    cached(&cache, ISSUE_REF, "A third party's name in the title");
    let invalidator = UnfurlInvalidator::new(cache.clone());

    assert!(
        invalidates_card("issue.issue.erased"),
        "*.erased invalidates a card"
    );

    let ev = producer_event("issue.issue.erased", ISSUE_REF);
    assert_eq!(invalidator.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
    assert!(
        !cache.contains(&RefsRef(ISSUE_REF.into())),
        "the *.erased busted the chat unfurl card (no durable snapshot; re-resolves to a tombstone)"
    );
}

#[test]
fn cdc_5_9_2_7_consumer_is_idempotent_on_redelivery() {
    let cache = UnfurlCache::new();
    cached(&cache, PR_REF, "Fix the flaky test");
    let invalidator = UnfurlInvalidator::new(cache.clone());

    let ev = producer_event(CI_CHECK_UPDATED, PR_REF);
    assert_eq!(invalidator.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
    assert_eq!(invalidator.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
    assert!(!cache.contains(&RefsRef(PR_REF.into())));
}

#[test]
fn cdc_5_9_2_7_consumer_subjects_are_bounded() {
    let cache = UnfurlCache::new();
    let invalidator = UnfurlInvalidator::new(cache.clone());
    for s in EventHandler::subjects(&invalidator) {
        assert!(!s.0.contains('*') && !s.0.is_empty());
    }
    let consumer = invalidator.into_consumer("chat.unfurl.invalidation", DedupLedger::new());
    assert_eq!(consumer.name().0, "chat.unfurl.invalidation");
}
