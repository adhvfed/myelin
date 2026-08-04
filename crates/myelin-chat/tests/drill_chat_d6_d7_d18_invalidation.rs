use std::sync::Mutex;

use myelin_chat::unfurl::invalidation::anchor::{
    is_dangle_free, resolve_message_anchor, MessageLifecycle,
};
use myelin_chat::unfurl::invalidation::{erasure_safe_rerender, CardUpdatePush, UnfurlInvalidator};
use myelin_chat::unfurl::{
    Card, Card as UnfurlCard, LadderOutcome, Projection, RefsResolvePort, Tombstone,
    TombstoneReason, UnfurlCandidate, UnfurlService,
};
use myelin_events::firehose::FirehoseScope;
use myelin_events::taxonomy::new_tokens::CI_CHECK_UPDATED;
use myelin_events::{
    Actor, AggregateKey, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    OutboxStore, Timestamp, Visibility,
};
use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, Principal, PrincipalId, PrincipalKind, RelName,
    RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{chat_fragment_defs, StoreBackedCheck, TupleStore};
use myelin_refs::ArtifactRef as RefsRef;
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
const THIRD_PARTY: &str = "Dana Quartz";
const CARD_TITLE: &str = "Issue assigned to Dana Quartz";

fn principal(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(TENANT.into()),
    );
    p.region = Region(REGION.into());
    p
}

fn card_ref() -> RefsRef {
    RefsRef("myelin://acme/issues/issue/ENG-77".into())
}

fn candidate() -> UnfurlCandidate {
    UnfurlCandidate {
        ref_: card_ref(),
        channel_id: Some("eng".into()),
    }
}

fn strong_at() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

struct ProgResolver {
    outcome: Mutex<LadderOutcome>,
    calls: Mutex<usize>,
}
impl ProgResolver {
    fn new(o: LadderOutcome) -> ProgResolver {
        ProgResolver {
            outcome: Mutex::new(o),
            calls: Mutex::new(0),
        }
    }
    fn set(&self, o: LadderOutcome) {
        *self.outcome.lock().unwrap() = o;
    }
    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}
impl RefsResolvePort for ProgResolver {
    fn resolve(
        &self,
        _t: &TenantId,
        _r: &Region,
        _ref_: &RefsRef,
        _v: &Principal,
        _at: &Consistency,
    ) -> LadderOutcome {
        *self.calls.lock().unwrap() += 1;
        self.outcome.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct RecordingPush {
    next: Mutex<u64>,
}
impl CardUpdatePush for RecordingPush {
    fn push_card_update(&self, _scope: &FirehoseScope, _invalidated: &RefsRef) -> u64 {
        let mut n = self.next.lock().unwrap();
        *n += 1;
        *n
    }
}

fn check_engine_with(grants: &[TupleDelta]) -> (StoreBackedCheck, TupleStore) {
    let store = TupleStore::new(OutboxStore::new());
    let svc = StoreBackedCheck::new(store.clone());
    for def in chat_fragment_defs() {
        assert!(matches!(
            svc.admit_fragment_def(&def),
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
    }
    let scope = TenantScope::from_verified_token(&principal("p-admin"), Region(REGION.into()));
    if !grants.is_empty() {
        store
            .write_tuples(
                &scope,
                &principal("p-admin"),
                grants,
                None,
                None,
                Timestamp("2026-06-24T00:00:00Z".into()),
            )
            .expect("seed grants");
    }
    (svc, store)
}

fn member(object: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName("member".into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn producer_event(token: &str, subject_ref: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J-evt".into()),
        type_: EventType(token.into()),
        schema_ver: 1,
        tenant: TenantId(TENANT.into()),
        region: Region(REGION.into()),
        actor: Actor(principal("p:sys")),
        subject: myelin_tenancy::ArtifactRef(subject_ref.into()),
        aggregate: AggregateKey("agg:01J".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J-corr".into()),
        caused_by: Some(CausedBy("session:1".into())),
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

fn assert_zero_recoverable_pii(card: &Card) {
    assert_eq!(card.exposed_title(), None, "a tombstone exposes NO title");
    let debug = format!("{card:?}");
    assert!(
        !debug.contains(THIRD_PARTY) && !debug.contains("Dana") && !debug.contains("Quartz"),
        "0 recoverable PII; card debug = {debug}"
    );
}

#[test]
fn chat_d6_erase_third_party_in_card_rerenders_tombstone_zero_recoverable_pii() {
    let (svc_check, _store) = check_engine_with(&[member("channel:eng", "p:alice")]);
    let resolver = ProgResolver::new(LadderOutcome::Live(Projection {
        title: CARD_TITLE.into(),
        state: "open".into(),
        icon: "issue".into(),
        sub_anchor: None,
    }));
    let svc = UnfurlService::new(svc_check, resolver);
    let cache = svc.cache().clone();
    let invalidator = UnfurlInvalidator::new(cache.clone());

    let live = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert_eq!(
        live.exposed_title(),
        Some(CARD_TITLE),
        "the live card shows the name"
    );
    assert!(cache.contains(&card_ref()), "the card is cached live");

    let erased_ev = producer_event("issues.issue.erased", &card_ref().0);
    assert!(
        invalidator.invalidate(&erased_ev),
        "the *.erased busts the shared entry"
    );
    assert!(
        !cache.contains(&card_ref()),
        "0 durable snapshot - the rendered content is dropped (erasure is free, §6)"
    );

    svc.resolver().set(LadderOutcome::Erased(Tombstone {
        root: card_ref(),
        reason: TombstoneReason::Erased,
    }));

    let rerendered = erasure_safe_rerender(
        &cache,
        svc.resolver(),
        &TenantId(TENANT.into()),
        &Region(REGION.into()),
        &card_ref(),
        &principal("p:alice"),
        &strong_at(),
    );
    assert!(rerendered.is_tombstone(), "the re-render is a tombstone");
    assert_zero_recoverable_pii(&rerendered);
    assert!(
        !cache.contains(&card_ref()),
        "nothing left in the cache to recover"
    );

    let via_service = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert!(via_service.is_tombstone());
    assert_zero_recoverable_pii(&via_service);
}

#[test]
fn chat_d7_check_updated_busts_and_pushes_live_card_update() {
    let (svc_check, _store) = check_engine_with(&[member("channel:eng", "p:alice")]);
    let resolver = ProgResolver::new(LadderOutcome::Live(Projection {
        title: "CI: 3/4 checks passing".into(),
        state: "open".into(),
        icon: "pr".into(),
        sub_anchor: None,
    }));
    let svc = UnfurlService::new(svc_check, resolver);
    let cache = svc.cache().clone();

    let live = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert!(matches!(live, UnfurlCard::Live { .. }));
    assert!(cache.contains(&card_ref()));
    let calls_before = svc.resolver().calls();

    let live_invalidator =
        UnfurlInvalidator::new(cache.clone()).with_push(RecordingPush::default());
    let scope = FirehoseScope::parse("channel:eng").expect("bounded channel scope, never *");

    let ev = producer_event(CI_CHECK_UPDATED, &card_ref().0);
    let (busted, frame_seq) = live_invalidator.invalidate_and_push(&ev, &scope);
    assert!(
        busted,
        "the cache-bust signal is non-zero (it busted the shared entry)"
    );
    assert_eq!(
        frame_seq,
        Some(1),
        "a live card-update frame was delivered (within budget)"
    );
    assert!(!cache.contains(&card_ref()), "the shared entry is busted");

    svc.resolver().set(LadderOutcome::Live(Projection {
        title: "CI: 4/4 checks passing".into(),
        state: "open".into(),
        icon: "pr".into(),
        sub_anchor: None,
    }));
    let fresh = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert_eq!(
        fresh.exposed_title(),
        Some("CI: 4/4 checks passing"),
        "re-resolved fresh"
    );
    assert!(
        svc.resolver().calls() > calls_before,
        "the bust forced a live re-resolve (the update is live, not stale)"
    );
}

#[test]
fn chat_d18_edit_stable_delete_degrades_to_root_tombstone_zero_dangle() {
    let embed = RefsRef("myelin://acme/chat/channel/eng#message-01J0MSGID".into());

    let edited = resolve_message_anchor(&embed, MessageLifecycle::Live, "the (edited) preview");
    match &edited {
        LadderOutcome::Live(p) => {
            assert_eq!(
                p.sub_anchor.as_deref(),
                Some("message-01J0MSGID"),
                "the message-<id> anchor is STABLE across edits"
            );
        }
        other => panic!("an edited message stays live, got {other:?}"),
    }
    assert!(is_dangle_free(&edited), "0 dangling anchor while live");

    let deleted = resolve_message_anchor(&embed, MessageLifecycle::Deleted, "ignored");
    match &deleted {
        LadderOutcome::Gone(t) => {
            assert_eq!(
                t.root.0, "myelin://acme/chat/channel/eng",
                "the tombstone carries the root channel"
            );
            assert!(
                !t.root.0.contains('#'),
                "the root is #sub-stripped (no dangling anchor)"
            );
        }
        other => panic!("a deleted message degrades to a Gone tombstone, got {other:?}"),
    }
    assert!(
        is_dangle_free(&deleted),
        "the dangling-anchor signal is 0 on delete"
    );
}
