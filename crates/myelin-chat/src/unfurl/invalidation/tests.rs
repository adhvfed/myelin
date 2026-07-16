//! Unit tests for the CHAT-P14 invalidation/erasure/anchor core (the mandatory-core no-recoverable-PII
//! + bus-bust precision + dangle-free properties).

use super::anchor::{is_dangle_free, message_sub_anchor, resolve_message_anchor, MessageLifecycle};
use super::*;

use myelin_events::firehose::FirehoseScope;
use myelin_events::{
    Actor, AggregateKey, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    HandleOutcome, Timestamp, Visibility,
};
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_refs::ArtifactRef as RefsArtifactRef;
use myelin_tenancy::{ArtifactRef as TenancyRef, Region, TenantId};

use super::super::Projection;

const ISSUE_REF: &str = "myelin://acme/issues/issue/ENG-412";
const SECRET_TITLE: &str = "Acme acquires Initech for $400M";
const ERASED_TITLE: &str = "[erased]";

fn principal(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    p.region = Region("fr-par".into());
    p
}

/// Build an envelope of `type_` whose subject is `subject_ref`.
fn envelope(type_: &str, subject_ref: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J-evt".into()),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(principal("p:author")),
        subject: TenancyRef(subject_ref.into()),
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

fn issue_ref() -> RefsArtifactRef {
    RefsArtifactRef(ISSUE_REF.into())
}

fn strong_at() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

/// A programmable resolver: returns a fixed outcome and counts calls.
struct FixedResolver {
    outcome: std::sync::Mutex<LadderOutcome>,
    calls: std::sync::Mutex<usize>,
}
impl FixedResolver {
    fn new(o: LadderOutcome) -> FixedResolver {
        FixedResolver {
            outcome: std::sync::Mutex::new(o),
            calls: std::sync::Mutex::new(0),
        }
    }
    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}
impl RefsResolvePort for FixedResolver {
    fn resolve(
        &self,
        _t: &TenantId,
        _r: &Region,
        _ref_: &RefsArtifactRef,
        _v: &Principal,
        _at: &Consistency,
    ) -> LadderOutcome {
        *self.calls.lock().unwrap() += 1;
        self.outcome.lock().unwrap().clone()
    }
}

/// A test [`CardUpdatePush`] (the gateway firehose seam, CHAT-P10) — assigns monotonic seqs and records
/// the pushed ref so a test asserts the bust frame carries the REF (a pointer), never a rendered title.
/// `Arc`-cloneable so the test keeps a handle after the invalidator takes ownership.
#[derive(Default, Clone)]
struct SeqPush {
    next: std::sync::Arc<std::sync::Mutex<u64>>,
    pushed: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}
impl SeqPush {
    fn pushed(&self) -> Vec<String> {
        self.pushed.lock().unwrap().clone()
    }
}
impl CardUpdatePush for SeqPush {
    fn push_card_update(&self, _scope: &FirehoseScope, invalidated: &RefsArtifactRef) -> u64 {
        self.pushed.lock().unwrap().push(invalidated.0.clone());
        let mut n = self.next.lock().unwrap();
        *n += 1;
        *n
    }
}

// ───────────────────────────── the bus-bust match precision (§4.4) ────────────────────────────────

/// **The mandatory-core match: `*.updated`/`*.erased`/`ci.check.updated`/`*.revoked` bust; everything
/// else does NOT.** A survived mutant flipping any of these is a stale/leaky card.
#[test]
fn invalidates_card_matches_exactly_the_updated_erased_revoked_set() {
    // matching → bust.
    assert!(invalidates_card("issue.issue.updated"));
    assert!(invalidates_card("git.pr.updated"));
    assert!(invalidates_card("knowledge.page.updated"));
    assert!(invalidates_card("ci.check.updated"));
    assert!(invalidates_card("chat.message.erased"));
    assert!(invalidates_card("issue.issue.erased"));
    assert!(invalidates_card("identity.permission.revoked"));
    // NON-matching → no bust (precision: a create / member_added / added is NOT a card change).
    assert!(!invalidates_card("issue.issue.created"));
    assert!(!invalidates_card("chat.message.created"));
    assert!(!invalidates_card("chat.channel.member_added"));
    assert!(!invalidates_card("git.pr.opened"));
    // ungrammatical → fail-closed-to-no-bust.
    assert!(!invalidates_card("nodots"));
    assert!(!invalidates_card(""));
}

/// **The consumer subjects are a `*`-free whitelist (contract 2.4).** `into_consumer` binds without
/// rejecting (the subjects carry no `*`), and the consumer name is durable.
#[test]
fn invalidator_subjects_are_a_wildcard_free_whitelist() {
    for s in UNFURL_INVALIDATION_SUBJECTS {
        assert!(!s.contains('*'), "no `*` in the whitelist: {s}");
        assert!(!s.is_empty());
    }
    let cache = UnfurlCache::new();
    let consumer =
        UnfurlInvalidator::new(cache).into_consumer("chat.unfurl.invalidation", DedupLedger::new());
    assert_eq!(consumer.name().0, "chat.unfurl.invalidation");
}

/// **A matching event busts the ONE shared entry; a non-matching event leaves it.** The consumer and
/// the cache share the SAME `Arc`-backed entries (never a second cache).
#[test]
fn matching_event_busts_the_shared_entry_nonmatching_leaves_it() {
    let cache = UnfurlCache::new();
    cache.put(
        &issue_ref(),
        Projection {
            title: SECRET_TITLE.into(),
            state: "open".into(),
            icon: "issue".into(),
            sub_anchor: None,
        },
    );
    let invalidator = UnfurlInvalidator::new(cache.clone());

    // a NON-matching event (created) → no bust, the entry survives.
    let created = envelope("issue.issue.created", ISSUE_REF);
    assert!(!invalidator.invalidate(&created));
    assert!(cache.contains(&issue_ref()), "created does not bust");

    // a matching event (updated) → bust, the entry is gone.
    let updated = envelope("issue.issue.updated", ISSUE_REF);
    assert!(invalidator.invalidate(&updated));
    assert!(!cache.contains(&issue_ref()), "updated busts the entry");

    // busting an already-absent entry is idempotent (a no-op, still `Done`).
    assert!(!invalidator.invalidate(&updated));
}

/// **The handler is idempotent (contract 2.5): re-handling the same event is a no-op, always `Done`.**
#[test]
fn handler_is_idempotent_and_always_done() {
    let cache = UnfurlCache::new();
    cache.put(
        &issue_ref(),
        Projection {
            title: SECRET_TITLE.into(),
            state: "open".into(),
            icon: "issue".into(),
            sub_anchor: None,
        },
    );
    let invalidator = UnfurlInvalidator::new(cache.clone());
    let ev = envelope("issue.issue.updated", ISSUE_REF);

    assert_eq!(invalidator.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
    assert!(!cache.contains(&issue_ref()));
    // re-handle (redelivery) — still Done, still busted, no error.
    assert_eq!(invalidator.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
}

/// **A `#sub`-anchored subject busts the ROOT entry (§4.2 — the cache keys the root projection).** An
/// `issue.updated` whose subject carries a `#sub` still busts the root-keyed entry.
#[test]
fn sub_anchored_subject_busts_the_root_entry() {
    let cache = UnfurlCache::new();
    // the cache is keyed by the ROOT (no #sub).
    cache.put(
        &issue_ref(),
        Projection {
            title: SECRET_TITLE.into(),
            state: "open".into(),
            icon: "issue".into(),
            sub_anchor: None,
        },
    );
    let invalidator = UnfurlInvalidator::new(cache.clone());
    // the event subject carries a #sub anchor.
    let ev = envelope("issue.issue.updated", &format!("{ISSUE_REF}#comment-7"));
    assert!(invalidator.invalidate(&ev), "busts the root-keyed entry");
    assert!(!cache.contains(&issue_ref()));
}

// ───────────────────────────── the live firehose push (§4.4; CHAT-D7) ─────────────────────────────

/// **CHAT-D7: a matching event busts AND pushes a live card-update frame on the channel scope.** The
/// frame is a references-not-payloads pointer (the ref, never a title); the scope is `channel:<id>`,
/// never `*`.
#[test]
fn live_invalidate_busts_and_pushes_a_leak_free_frame() {
    let cache = UnfurlCache::new();
    cache.put(
        &issue_ref(),
        Projection {
            title: SECRET_TITLE.into(),
            state: "open".into(),
            icon: "issue".into(),
            sub_anchor: None,
        },
    );
    let push = SeqPush::default();
    let live = UnfurlInvalidator::new(cache.clone()).with_push(push.clone());
    let scope = FirehoseScope::parse("channel:eng-incidents").unwrap();

    let ev = envelope("issue.issue.updated", ISSUE_REF);
    let (busted, seq) = live.invalidate_and_push(&ev, &scope);
    assert!(busted, "the shared entry was busted");
    assert_eq!(seq, Some(1), "a live frame was pushed (seq 1)");
    assert!(!cache.contains(&issue_ref()), "the entry is gone");
    // the pushed frame carries the REF pointer, never the rendered title (leak-free bust frame).
    let pushed = push.pushed();
    assert_eq!(
        pushed,
        vec![ISSUE_REF.to_string()],
        "the bust frame carries the ref, not a title"
    );
    assert!(
        !pushed.iter().any(|f| f.contains(SECRET_TITLE)),
        "0 title in the live bust frame (references-not-payloads)"
    );

    // a non-matching event → no bust, no push.
    let created = envelope("issue.issue.created", ISSUE_REF);
    let (busted2, seq2) = live.invalidate_and_push(&created, &scope);
    assert!(!busted2);
    assert_eq!(seq2, None);
}

// ───────────────────────────── erasure-safe re-render (§6; CHAT-D6) ────────────────────────────────

/// **CHAT-D6 (mandatory-core): erase a third party in a card → bust → re-resolve live → Tombstone, 0
/// recoverable PII, no durable snapshot.** The cache held the third party's name in the title; the
/// `*.erased` bust drops it (the ONLY place rendered content lived); the re-resolve returns Erased → a
/// tombstone, and the cache holds NOTHING (nothing to recover).
#[test]
fn chat_d6_erase_third_party_rerenders_tombstone_zero_recoverable_pii() {
    let cache = UnfurlCache::new();
    // 1. the card was cached LIVE with the third party's name in the title.
    cache.put(
        &issue_ref(),
        Projection {
            title: SECRET_TITLE.into(),
            state: "open".into(),
            icon: "issue".into(),
            sub_anchor: None,
        },
    );
    assert!(cache.contains(&issue_ref()));

    // 2. the `*.erased` event busts the shared entry — the rendered content is DROPPED (no snapshot).
    let invalidator = UnfurlInvalidator::new(cache.clone());
    let erased_ev = envelope("issue.issue.erased", ISSUE_REF);
    assert!(invalidator.invalidate(&erased_ev), "the *.erased busts");
    assert!(
        !cache.contains(&issue_ref()),
        "no durable snapshot — the cache is the only place rendered content lived (§4.5)"
    );

    // 3. the re-resolve returns Erased (the third party shredded) → a tombstone, 0 recoverable PII.
    let resolver = FixedResolver::new(LadderOutcome::Erased(Tombstone {
        root: issue_ref(),
        reason: TombstoneReason::Erased,
    }));
    let card = erasure_safe_rerender(
        &cache,
        &resolver,
        &TenantId("acme".into()),
        &Region("fr-par".into()),
        &issue_ref(),
        &principal("p:viewer"),
        &strong_at(),
    );
    assert!(card.is_tombstone(), "the re-render is a tombstone");
    assert_eq!(card.exposed_title(), None, "0 title in the tombstone");

    // 0 recoverable PII: the cache holds NOTHING for the ref (nothing to recover); the secret is
    // nowhere in the re-rendered card's debug form.
    assert!(
        !cache.contains(&issue_ref()),
        "an erased outcome is NOT re-cached as content (0 recoverable PII)"
    );
    let debug = format!("{card:?}");
    assert!(
        !debug.contains(SECRET_TITLE) && !debug.contains("Initech"),
        "0 recoverable PII in the card; debug = {debug}"
    );
    assert_eq!(
        resolver.calls(),
        1,
        "the re-render re-resolved live (1 call)"
    );
}

/// **The cache re-resolves live — never a stale title.** After a bust, a re-render that resolves Live
/// returns the FRESH projection, never the previously-cached one.
#[test]
fn rerender_uses_fresh_projection_never_the_stale_cached_one() {
    let cache = UnfurlCache::new();
    cache.put(
        &issue_ref(),
        Projection {
            title: "STALE".into(),
            state: "open".into(),
            icon: "issue".into(),
            sub_anchor: None,
        },
    );
    let invalidator = UnfurlInvalidator::new(cache.clone());
    invalidator.invalidate(&envelope("issue.issue.updated", ISSUE_REF));

    let resolver = FixedResolver::new(LadderOutcome::Live(Projection {
        title: "FRESH".into(),
        state: "closed".into(),
        icon: "issue".into(),
        sub_anchor: None,
    }));
    let card = erasure_safe_rerender(
        &cache,
        &resolver,
        &TenantId("acme".into()),
        &Region("fr-par".into()),
        &issue_ref(),
        &principal("p:viewer"),
        &strong_at(),
    );
    assert_eq!(
        card.exposed_title(),
        Some("FRESH"),
        "the fresh title, not STALE"
    );
}

// ───────────────────────────── `#sub` anchor stability (§2; CHAT-D18) ──────────────────────────────

/// **CHAT-D18: an edited referenced message keeps its `message-<id>` anchor stable/live.** Any number of
/// edits keeps the embed LIVE with the SAME `#sub` (the id is immutable); the anchor never dangles.
#[test]
fn chat_d18_edit_keeps_message_anchor_stable_live() {
    let embed = RefsArtifactRef("myelin://acme/chat/channel/eng-incidents#message-01J0MSG".into());
    let anchor = message_sub_anchor(&embed);
    assert_eq!(anchor.as_deref(), Some("message-01J0MSG"));

    // edited any number of times → still Live, SAME anchor (the id is immutable across edits).
    let outcome = resolve_message_anchor(&embed, MessageLifecycle::Live, "the live preview");
    match &outcome {
        LadderOutcome::Live(p) => {
            assert_eq!(
                p.sub_anchor.as_deref(),
                Some("message-01J0MSG"),
                "stable anchor"
            );
            assert_eq!(p.title, "the live preview");
        }
        other => panic!("an edited message stays live, got {other:?}"),
    }
    assert!(is_dangle_free(&outcome), "a live anchor never dangles");
}

/// **CHAT-D18: deleting the referenced message degrades the embed to a Tombstone carrying the ROOT,
/// never dangles.** The tombstone carries the `#sub`-stripped channel; the dangling-anchor signal is 0.
#[test]
fn chat_d18_delete_degrades_to_root_tombstone_zero_dangle() {
    let embed = RefsArtifactRef("myelin://acme/chat/channel/eng-incidents#message-01J0MSG".into());
    let outcome = resolve_message_anchor(&embed, MessageLifecycle::Deleted, "ignored");
    match &outcome {
        LadderOutcome::Gone(t) => {
            assert_eq!(
                t.root.0, "myelin://acme/chat/channel/eng-incidents",
                "the tombstone carries the ROOT channel (never dangles)"
            );
            assert!(!t.root.0.contains('#'), "the root is `#sub`-stripped");
            assert_eq!(t.reason, TombstoneReason::Gone);
        }
        other => panic!("a deleted message degrades to a Gone tombstone, got {other:?}"),
    }
    assert!(is_dangle_free(&outcome), "0 dangling anchor on delete");
}

/// **CHAT-D18: erasing the referenced message → a Tombstone, `[erased]`, carrying the root.** Still
/// dangle-free.
#[test]
fn chat_d18_erase_degrades_to_erased_tombstone_zero_dangle() {
    let embed = RefsArtifactRef("myelin://acme/chat/channel/eng-incidents#message-01J0MSG".into());
    let outcome = resolve_message_anchor(&embed, MessageLifecycle::Erased, ERASED_TITLE);
    match &outcome {
        LadderOutcome::Erased(t) => {
            assert_eq!(t.root.0, "myelin://acme/chat/channel/eng-incidents");
            assert_eq!(t.reason, TombstoneReason::Erased);
        }
        other => panic!("an erased message degrades to an Erased tombstone, got {other:?}"),
    }
    assert!(is_dangle_free(&outcome), "0 dangling anchor on erase");
}

/// **The cache-TTL tunable is NAMED (R-C4, the backstop).** A floor assertion: the default is a named,
/// positive backstop value (the precise path is the bus-bust; the TTL is the staleness ceiling).
#[test]
fn cache_ttl_tunable_is_named_and_positive() {
    // the named backstop is a positive, finite duration (a missed event cannot pin a stale card past
    // it); the precise path is the bus-bust, the TTL is only the staleness ceiling (R-C4).
    let ttl = std::time::Duration::from_secs(DEFAULT_CACHE_TTL_SECONDS);
    assert!(
        !ttl.is_zero(),
        "the TTL backstop is a named, positive tunable"
    );
}
