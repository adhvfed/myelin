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

#[test]
fn invalidates_card_matches_exactly_the_updated_erased_revoked_set() {
    assert!(invalidates_card("issue.issue.updated"));
    assert!(invalidates_card("git.pr.updated"));
    assert!(invalidates_card("knowledge.page.updated"));
    assert!(invalidates_card("ci.check.updated"));
    assert!(invalidates_card("chat.message.erased"));
    assert!(invalidates_card("issue.issue.erased"));
    assert!(invalidates_card("identity.permission.revoked"));
    assert!(!invalidates_card("issue.issue.created"));
    assert!(!invalidates_card("chat.message.created"));
    assert!(!invalidates_card("chat.channel.member_added"));
    assert!(!invalidates_card("git.pr.opened"));
    assert!(!invalidates_card("nodots"));
    assert!(!invalidates_card(""));
}

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

    let created = envelope("issue.issue.created", ISSUE_REF);
    assert!(!invalidator.invalidate(&created));
    assert!(cache.contains(&issue_ref()), "created does not bust");

    let updated = envelope("issue.issue.updated", ISSUE_REF);
    assert!(invalidator.invalidate(&updated));
    assert!(!cache.contains(&issue_ref()), "updated busts the entry");

    assert!(!invalidator.invalidate(&updated));
}

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
    assert_eq!(invalidator.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
}

#[test]
fn sub_anchored_subject_busts_the_root_entry() {
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
    let ev = envelope("issue.issue.updated", &format!("{ISSUE_REF}#comment-7"));
    assert!(invalidator.invalidate(&ev), "busts the root-keyed entry");
    assert!(!cache.contains(&issue_ref()));
}

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

    let created = envelope("issue.issue.created", ISSUE_REF);
    let (busted2, seq2) = live.invalidate_and_push(&created, &scope);
    assert!(!busted2);
    assert_eq!(seq2, None);
}

#[test]
fn chat_d6_erase_third_party_rerenders_tombstone_zero_recoverable_pii() {
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
    assert!(cache.contains(&issue_ref()));

    let invalidator = UnfurlInvalidator::new(cache.clone());
    let erased_ev = envelope("issue.issue.erased", ISSUE_REF);
    assert!(invalidator.invalidate(&erased_ev), "the *.erased busts");
    assert!(
        !cache.contains(&issue_ref()),
        "no durable snapshot - the cache is the only place rendered content lived (§4.5)"
    );

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

#[test]
fn chat_d18_edit_keeps_message_anchor_stable_live() {
    let embed = RefsArtifactRef("myelin://acme/chat/channel/eng-incidents#message-01J0MSG".into());
    let anchor = message_sub_anchor(&embed);
    assert_eq!(anchor.as_deref(), Some("message-01J0MSG"));

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

#[test]
fn cache_ttl_tunable_is_named_and_positive() {
    let ttl = std::time::Duration::from_secs(DEFAULT_CACHE_TTL_SECONDS);
    assert!(
        !ttl.is_zero(),
        "the TTL backstop is a named, positive tunable"
    );
}
