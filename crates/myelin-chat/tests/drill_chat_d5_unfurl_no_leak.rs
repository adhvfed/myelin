//! # CHAT-D5 — the unfurl no-leak drill: a confidential artifact → tombstone, title NEVER present
//! (the 4-step ladder step 1) (CHAT-P13 / P-407, M4-C4)
//!
//! **Drill catalogue** row **CHAT-D5** (F1): "Notify/unfurl a confidential artifact to a viewer
//! lacking access → tombstone rendered, title NEVER present — the 4-step ladder step 1." **Threshold:
//! the leaked-title signal = 0.**
//!
//! This is the **UNFURL-SERVICE** seam of CHAT-D5 (the per-ref cache + per-viewer `check` gate, the
//! no-leak FLOOR). It is COMPLEMENTARY to `drill_chat_d5_humanise_leak.rs` (the Notif `humanise` seam,
//! NOTIF-P22): the humanise drill proves the @mention/notification string is leak-free; THIS drill
//! proves the in-message UNFURL CARD is leak-free — the per-viewer gate runs BEFORE the shared cache /
//! the resolver is touched, so a denied viewer's title is never even fetched. Both seams resolve
//! per-viewer through the SAME chokepoint posture (contract 5.2; EI-01 §7 — chat never re-implements
//! permission-aware resolution).
//!
//! The drill proves, against the **REAL** Identity `check` engine (the admitted Chat fragment), the
//! three quantified GATE assertions from the prompt:
//! 1. **CHAT-D5 — 0 title leak.** A confidential channel referenced in a message renders a TOMBSTONE
//!    for a viewer lacking access; the secret title is absent from EVERY field of the card.
//! 2. **One-cache-entry-per-ref.** N viewers resolving the SAME confidential ref → exactly ONE cache
//!    entry (never per `(ref, viewer)`); 0 viewer-content baked into the cache.
//! 3. **The chained no-leak (EI-01 §4).** resolve as member → revoke the member tuple → re-resolve →
//!    tombstone, 0 leak — even though the shared cache STILL holds the projection (the gate is the
//!    per-viewer chokepoint, not the cache).
//!
//! Architecture: `chat/architecture/02-internals-and-algorithms.md` §4.2 (split the cache by
//! viewer-varying vs viewer-independent — ONE entry per ref, gated per viewer) + `05-hard-problems.md`
//! §4 (the no-leak subtlety that separates a real implementation from a demo). The Refs `resolve`
//! chokepoint (5.2; REF-P10 / CHAT-P15) is the NAMED FLOOR; a deterministic synthetic resolver models
//! its EXACT `Projection | Tombstone` contract so the leak PROPERTY is proven structurally.

use myelin_chat::unfurl::{
    Card, LadderOutcome, Projection, RefsResolvePort, UnfurlCandidate, UnfurlService,
};
use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ObjectId, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple,
    TupleDelta,
};
use myelin_identity_service::{chat_fragment_defs, StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
/// The secret channel title a denied viewer must NEVER see (the leak-test payload — if it appears in
/// any card field for a denied viewer, the leaked-title signal is non-zero and the drill is RED).
const SECRET_TITLE: &str = "#board-leadership-comp";

fn principal(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(TENANT.into()),
    );
    p.region = Region(REGION.into());
    p
}

fn confidential_ref() -> myelin_refs::ArtifactRef {
    myelin_refs::ArtifactRef("myelin://acme/chat/channel/board-secret".into())
}

fn candidate() -> UnfurlCandidate {
    UnfurlCandidate {
        ref_: confidential_ref(),
        channel_id: Some("board-secret".into()),
    }
}

/// The synthetic Refs resolve chokepoint (5.2; REF-P10 floor) — ALWAYS returns the live projection
/// carrying the SECRET_TITLE, so ANY leak of the title to a denied viewer is observable. The gate is
/// what keeps it from a denied viewer (the resolver itself is "naive" here — defence in depth: even if
/// the resolver leaked, the gate runs first).
struct LeakyResolver;
impl RefsResolvePort for LeakyResolver {
    fn resolve(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        _ref_: &myelin_refs::ArtifactRef,
        _viewer: &Principal,
        _at: &Consistency,
    ) -> LadderOutcome {
        LadderOutcome::Live(Projection {
            title: SECRET_TITLE.into(),
            state: "active".into(),
            icon: "channel".into(),
            sub_anchor: None,
        })
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

/// Assert no field of a card carries any fragment of the secret title (the leaked-title signal == 0).
fn assert_zero_title_leak(card: &Card) {
    assert_eq!(card.exposed_title(), None, "the card exposes NO title");
    // be paranoid: the tombstone's debug form must not carry the secret either.
    let debug = format!("{card:?}");
    assert!(
        !debug.contains(SECRET_TITLE) && !debug.contains("leadership") && !debug.contains("comp"),
        "the leaked-title signal must be 0; card debug = {debug}"
    );
}

/// **CHAT-D5 (the GATE): a confidential unfurl → tombstone for a denied viewer; 0 title leak.** The
/// non-member's card is a tombstone; the secret title is absent from every field. Threshold 0.
#[test]
fn chat_d5_confidential_unfurl_tombstones_zero_title_leak() {
    // alice is a member of channel:board-secret; mallory is NOT.
    let (svc_check, _store) = check_engine_with(&[member("channel:board-secret", "p:alice")]);
    let svc = UnfurlService::new(svc_check, LeakyResolver);

    // the intruder (a non-member) unfurls the confidential channel → TOMBSTONE, 0 leak.
    let card = svc.resolve_one(&candidate(), &principal("p:mallory"));
    assert!(card.is_tombstone(), "a denied viewer sees a tombstone");
    assert_zero_title_leak(&card);

    // the complement (not vacuously blank): the MEMBER does see the title.
    let allowed = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert_eq!(
        allowed.exposed_title(),
        Some(SECRET_TITLE),
        "the member DOES see the title (the gate is not vacuously denying everyone)"
    );
}

/// **The one-cache-entry-per-ref invariant (the GATE): N viewers → ONE cache entry, never per
/// (ref, viewer).** Three members resolve the SAME confidential ref; the cache holds exactly one
/// (viewer-independent) entry — 0 per-viewer cache entries, 0 viewer-content baked in.
#[test]
fn chat_d5_one_cache_entry_per_ref_no_viewer_baked_in() {
    let (svc_check, _store) = check_engine_with(&[
        member("channel:board-secret", "p:alice"),
        member("channel:board-secret", "p:bob"),
        member("channel:board-secret", "p:carol"),
    ]);
    let svc = UnfurlService::new(svc_check, LeakyResolver);

    for who in ["p:alice", "p:bob", "p:carol"] {
        let card = svc.resolve_one(&candidate(), &principal(who));
        assert_eq!(
            card.exposed_title(),
            Some(SECRET_TITLE),
            "{who} sees the shared content"
        );
    }
    // exactly ONE entry for the ref — never three (keyed by ref, not (ref, viewer)).
    assert_eq!(
        svc.cache().entry_count(),
        1,
        "exactly ONE cache entry per ref (0 per-viewer entries)"
    );
}

/// **The chained no-leak (EI-01 §4): member → revoke → re-resolve → tombstone, 0 leak.** The SAME
/// viewer who saw the title sees a tombstone after the member tuple is removed — the per-viewer gate
/// denies BEFORE the cache (which still holds the shared projection) is read, so 0 title leak.
#[test]
fn chat_d5_chained_member_revoke_tombstone_zero_leak() {
    let (svc_check, store) = check_engine_with(&[member("channel:board-secret", "p:dave")]);
    let svc = UnfurlService::new(svc_check, LeakyResolver);

    // member resolve → sees the title; the shared cache fills.
    let before = svc.resolve_one(&candidate(), &principal("p:dave"));
    assert_eq!(before.exposed_title(), Some(SECRET_TITLE));
    assert_eq!(svc.cache().entry_count(), 1);

    // REVOKE: remove dave's member tuple (the new-enemy case).
    let scope = TenantScope::from_verified_token(&principal("p-admin"), Region(REGION.into()));
    store
        .write_tuples(
            &scope,
            &principal("p-admin"),
            &[TupleDelta::Remove(RelationTuple {
                object: ObjectId("channel:board-secret".into()),
                relation: RelName("member".into()),
                subject: PrincipalId("p:dave".into()),
                caveat: None,
            })],
            None,
            None,
            Timestamp("2026-06-24T00:01:00Z".into()),
        )
        .expect("revoke");

    // re-resolve → TOMBSTONE, 0 leak — even though the cache STILL holds the projection.
    let after = svc.resolve_one(&candidate(), &principal("p:dave"));
    assert!(after.is_tombstone(), "post-revoke the card is a tombstone");
    assert_zero_title_leak(&after);
    assert_eq!(
        svc.cache().entry_count(),
        1,
        "still ONE shared cache entry — the no-leak is the GATE, not a per-viewer cache"
    );
}
