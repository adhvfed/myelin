//! # The CDC pair for contracts 5.9 + 2.7 — the Chat unfurl-invalidation consumer (CHAT-P14 / P-408)
//!
//! **Contracts:**
//! - `contract-index.md` row **5.9** (the Git↔CI `CheckStatus` seam — `ci.check.updated` via outbox,
//!   X-1). CHAT-P14 CONSUMES `ci.check.updated` for **unfurl invalidation only** (it does NOT gate
//!   merges — that is Git): a `ci.check.updated` busts the shared per-ref projection cache so a chat
//!   card showing the artifact re-resolves the fresh check state.
//! - `contract-index.md` row **2.7** (crypto-shred / tombstone on the log — `*.erased` tombstones,
//!   the bus is a holder). CHAT-P14 CONSUMES `*.erased` for invalidation: an `*.erased` busts the
//!   shared entry so an erased third party in a card re-resolves to a tombstone on next render (0
//!   recoverable PII — the erasure-safe property, CHAT-D6).
//!
//! Owning architecture: chat `03-events-contracts-and-glue.md` §1.3 (the unfurl-invalidation consumer
//! matches the `*.updated`/`*.erased`/`ci.check.updated` set, whitelisted-subject never `*`, idempotent
//! via `consumer_dedup`) + `02-internals-and-algorithms.md` §4.4 (the bus-driven invalidation).
//!
//! ## The seam this pair pins (chat CONSUMES the frozen producer events; the producers own them)
//! - **PROVIDER (the producers — CI for `ci.check.updated` / any owner for `*.erased`)** emit the
//!   frozen pointer events over the one [`myelin_events::EventEnvelope`]. This pair MODELS the producer
//!   side: a real envelope of the frozen token + subject ref (CI's `ci.check.updated`; an owner's
//!   `*.erased`), never re-defining the producer's shape.
//! - **CONSUMER (chat — [`myelin_chat::unfurl::invalidation`])** runs the ONE frozen consumer template
//!   (contract 2.4) matching the whitelisted set + busting the shared cache entry, idempotently. Chat
//!   does NOT author a second invalidation language — it consumes the frozen tokens by name.

use myelin_chat::unfurl::invalidation::{invalidates_card, UnfurlInvalidator};
use myelin_chat::unfurl::{Projection, UnfurlCache};
use myelin_events::taxonomy::new_tokens::CI_CHECK_UPDATED;
use myelin_events::{
    Actor, AggregateKey, CausedBy, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId,
    EventType, HandleOutcome, Timestamp, Visibility,
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

/// **PROVIDER side** — model a producer's frozen pointer event over the one envelope: token + subject.
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

/// **5.9 CDC: a `ci.check.updated` busts the chat unfurl card (the CONSUMER leg).** The provider emits
/// the frozen X-1 token; chat's consumer busts the shared per-ref cache so the card re-resolves the
/// fresh check state. (Chat consumes the check event for INVALIDATION ONLY — the merge gate is Git.)
#[test]
fn cdc_5_9_ci_check_updated_busts_the_chat_unfurl_card() {
    // CONSUMER: chat's invalidator over the shared cache.
    let cache = UnfurlCache::new();
    cached(&cache, PR_REF, "Fix the flaky test");
    let invalidator = UnfurlInvalidator::new(cache.clone());

    // the producer's frozen token is the one X-1 `ci.check.updated` (no second token language).
    assert_eq!(CI_CHECK_UPDATED, "ci.check.updated");
    assert!(
        invalidates_card(CI_CHECK_UPDATED),
        "ci.check.updated invalidates a card"
    );

    // PROVIDER: a `ci.check.updated` on the PR ref → CONSUMER busts the shared entry, returns Done.
    let ev = producer_event(CI_CHECK_UPDATED, PR_REF);
    assert_eq!(invalidator.handle(&ev), HandleOutcome::Done);
    assert!(
        !cache.contains(&RefsRef(PR_REF.into())),
        "the ci.check.updated busted the chat unfurl card (it re-resolves the fresh check state)"
    );
}

/// **2.7 CDC: an `*.erased` busts the chat unfurl card (the CONSUMER leg).** The provider emits the
/// cross-cutting erasure tombstone; chat's consumer busts the shared entry so the erased artifact
/// re-resolves to a tombstone on next render (0 recoverable PII — the erasure-safe property).
#[test]
fn cdc_2_7_erased_busts_the_chat_unfurl_card() {
    let cache = UnfurlCache::new();
    cached(&cache, ISSUE_REF, "A third party's name in the title");
    let invalidator = UnfurlInvalidator::new(cache.clone());

    // the `*.erased` token is the cross-cutting erasure tombstone (contract 2.7).
    assert!(
        invalidates_card("issue.issue.erased"),
        "*.erased invalidates a card"
    );

    // PROVIDER: an `issue.issue.erased` on the issue ref → CONSUMER busts the shared entry.
    let ev = producer_event("issue.issue.erased", ISSUE_REF);
    assert_eq!(invalidator.handle(&ev), HandleOutcome::Done);
    assert!(
        !cache.contains(&RefsRef(ISSUE_REF.into())),
        "the *.erased busted the chat unfurl card (no durable snapshot; re-resolves to a tombstone)"
    );
}

/// **The consumer is idempotent (contract 2.5): a redelivered erase/check event is a no-op, Done.**
#[test]
fn cdc_5_9_2_7_consumer_is_idempotent_on_redelivery() {
    let cache = UnfurlCache::new();
    cached(&cache, PR_REF, "Fix the flaky test");
    let invalidator = UnfurlInvalidator::new(cache.clone());

    let ev = producer_event(CI_CHECK_UPDATED, PR_REF);
    assert_eq!(invalidator.handle(&ev), HandleOutcome::Done);
    // redelivery (at-least-once): still Done, still busted, never an error.
    assert_eq!(invalidator.handle(&ev), HandleOutcome::Done);
    assert!(!cache.contains(&RefsRef(PR_REF.into())));
}

/// **The consumer subjects are a `*`-free whitelist (contract 2.4) — the producer/consumer seam is
/// bounded.** The provider's tokens are the frozen set; the consumer's subjects carry no `*`.
#[test]
fn cdc_5_9_2_7_consumer_subjects_are_bounded() {
    let cache = UnfurlCache::new();
    let invalidator = UnfurlInvalidator::new(cache.clone());
    // the handler whitelist carries no `*` (an over-broad subscription is unconstructable).
    for s in EventHandler::subjects(&invalidator) {
        assert!(!s.0.contains('*') && !s.0.is_empty());
    }
    // and it binds into the ONE frozen consumer runtime (the `*`-rejection passes by construction).
    let consumer = invalidator.into_consumer("chat.unfurl.invalidation");
    assert_eq!(consumer.name().0, "chat.unfurl.invalidation");
}
