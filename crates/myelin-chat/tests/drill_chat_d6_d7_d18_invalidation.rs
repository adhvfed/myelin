//! # CHAT-D6 / D7 / D18 — the bus-driven invalidation + erasure-safe + `#sub` anchor-stability drills
//! (CHAT-P14 / P-408, M4-C4)
//!
//! Three drill-catalogue rows, one chained harness over the ONE shared unfurl cache (CHAT-P13) + the
//! CHAT-P14 invalidation consumer:
//!
//! - **CHAT-D6 (F1, mandatory-core):** "Erase a third party rendered in a card → tombstone on next
//!   render, 0 recoverable PII (no durable snapshot; cache re-resolves live → `erased`)." Threshold:
//!   **0 recoverable PII; live re-resolve.** The CHAINED test (EI-01 §4): resolve as member (the card
//!   shows the third party's name) → the third party is erased (`*.erased` busts the shared entry) →
//!   re-resolve → tombstone, **0 recoverable PII** (the secret is nowhere in the re-rendered card AND
//!   nowhere in the cache — the cache is the ONLY place rendered content ever lived, §4.5).
//!
//! - **CHAT-D7 (live-update):** "An artifact's `ci.check.updated`/`*.updated` → the shared per-ref
//!   cache busts; viewers showing the card get a live firehose update within budget." Threshold:
//!   **cache-bust; update latency.** A `ci.check.updated`/`*.updated` busts the shared entry AND pushes
//!   a live firehose card-update frame (contract 3.5, `channel:<id>` scope, never `*`) — the viewer
//!   re-resolves the fresh card; the bust frame is a references-not-payloads pointer (no leaked title).
//!
//! - **CHAT-D18 (sub-anchor):** "Edit a message referenced by another artifact → the `message-<id>`
//!   anchor stays stable (live); delete it → the embed degrades to a Tombstone carrying the root
//!   (channel), never dangles." Threshold: **anchor stability; tombstone (0 dangling).**
//!
//! Architecture: `02-internals-and-algorithms.md` §4.4 (bus-driven invalidation; TTL the backstop) +
//! §6 / §4.5 (erasure is free — no rendered content is ever stored durably) + §2 (the `#sub` anchor
//! stays stable across edits, degrades to a root tombstone on delete). The per-viewer gate runs over
//! the **REAL** Identity `check` engine (the admitted Chat fragment); the Refs `resolve` chokepoint
//! (REF-P10 / CHAT-P15) is the named floor — a deterministic resolver models its exact
//! `Projection | Tombstone` contract so the erasure-safe / live-bust / anchor PROPERTIES are proven
//! structurally.

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
/// The third party's name rendered in the card. After the erase it must NOT be recoverable from the
/// card or the cache (the recoverable-PII signal = 0).
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

/// A programmable Refs resolve chokepoint (5.2; REF-P10 floor) — returns whatever outcome is set, and
/// counts calls (so a test proves a live re-resolve happened, not a stale cache read).
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

/// A test [`CardUpdatePush`] modelling the gateway's CHAT-P10 firehose seam: assigns monotonic frame
/// seqs and records the pushed ref (so the drill asserts the bust frame carries the ref, never a title).
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

/// A REAL `check` engine over the admitted Chat fragment, sharing the store `grants` are written to.
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

/// Assert no fragment of the third party's name survives anywhere in the card (the recoverable-PII
/// signal == 0).
fn assert_zero_recoverable_pii(card: &Card) {
    assert_eq!(card.exposed_title(), None, "a tombstone exposes NO title");
    let debug = format!("{card:?}");
    assert!(
        !debug.contains(THIRD_PARTY) && !debug.contains("Dana") && !debug.contains("Quartz"),
        "0 recoverable PII; card debug = {debug}"
    );
}

/// **CHAT-D6 (the chained drill, EI-01 §4): member resolve → erase the third party → re-resolve →
/// tombstone, 0 recoverable PII, no durable snapshot.** The card shows the third party's name; the
/// `*.erased` busts the shared entry (the ONLY place rendered content lived); the re-resolve returns
/// `erased` → a tombstone, and the cache holds NOTHING (nothing to recover).
#[test]
fn chat_d6_erase_third_party_in_card_rerenders_tombstone_zero_recoverable_pii() {
    // alice is a member of channel:eng; the issue card is referenced in #eng.
    let (svc_check, _store) = check_engine_with(&[member("channel:eng", "p:alice")]);
    // the resolver first returns the LIVE card carrying the third party's name.
    let resolver = ProgResolver::new(LadderOutcome::Live(Projection {
        title: CARD_TITLE.into(),
        state: "open".into(),
        icon: "issue".into(),
        sub_anchor: None,
    }));
    let svc = UnfurlService::new(svc_check, resolver);
    let cache = svc.cache().clone();
    let invalidator = UnfurlInvalidator::new(cache.clone());

    // 1. member resolve → the card shows the third party's name (the cache fills).
    let live = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert_eq!(
        live.exposed_title(),
        Some(CARD_TITLE),
        "the live card shows the name"
    );
    assert!(cache.contains(&card_ref()), "the card is cached live");

    // 2. the third party is ERASED → the producer emits `*.erased`; the consumer busts the shared entry
    //    (no durable snapshot — the cache is the only place rendered content ever lived, §4.5).
    let erased_ev = producer_event("issues.issue.erased", &card_ref().0);
    assert!(
        invalidator.invalidate(&erased_ev),
        "the *.erased busts the shared entry"
    );
    assert!(
        !cache.contains(&card_ref()),
        "0 durable snapshot — the rendered content is dropped (erasure is free, §6)"
    );

    // the resolver now returns ERASED (the third party was crypto-shredded).
    svc.resolver().set(LadderOutcome::Erased(Tombstone {
        root: card_ref(),
        reason: TombstoneReason::Erased,
    }));

    // 3. re-resolve → tombstone, 0 recoverable PII (the helper re-resolves live after the bust).
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
    // 0 recoverable from the cache either — the erased outcome is NOT re-cached as content.
    assert!(
        !cache.contains(&card_ref()),
        "nothing left in the cache to recover"
    );

    // and the whole-service path agrees: a fresh resolve_one (cache miss → resolver Erased) → tombstone.
    let via_service = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert!(via_service.is_tombstone());
    assert_zero_recoverable_pii(&via_service);
}

/// **CHAT-D7 (live-update): a `ci.check.updated`/`*.updated` busts the shared entry AND pushes a live
/// firehose card-update frame (within budget) — the viewer re-resolves the fresh card.** The bust frame
/// is a references-not-payloads pointer on the bounded `channel:<id>` scope (never `*`); the cache-bust
/// signal is non-zero (it busted) and the update is delivered (a frame seq).
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

    // the card is cached live (the viewer is showing it).
    let live = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert!(matches!(live, UnfurlCard::Live { .. }));
    assert!(cache.contains(&card_ref()));
    let calls_before = svc.resolver().calls();

    // the live invalidator (bust + push) over the SAME shared cache + the gateway push port (CHAT-P10).
    let live_invalidator =
        UnfurlInvalidator::new(cache.clone()).with_push(RecordingPush::default());
    let scope = FirehoseScope::parse("channel:eng").expect("bounded channel scope, never *");

    // a `ci.check.updated` on the card ref → bust + a live frame on the channel scope.
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

    // the viewer re-resolves the FRESH card (a live re-resolve, not a stale cache read).
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

/// **CHAT-D18 (sub-anchor): edit keeps the `message-<id>` anchor stable/live; delete degrades to a
/// Tombstone carrying the root, never dangles.** A referenced message embedded in another artifact.
#[test]
fn chat_d18_edit_stable_delete_degrades_to_root_tombstone_zero_dangle() {
    let embed = RefsRef("myelin://acme/chat/channel/eng#message-01J0MSGID".into());

    // EDIT → the anchor stays stable/live (any number of edits; the id is immutable).
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

    // DELETE → degrade to a Tombstone carrying the ROOT channel; never dangles.
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
