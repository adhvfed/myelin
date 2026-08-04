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

fn assert_zero_title_leak(card: &Card) {
    assert_eq!(card.exposed_title(), None, "the card exposes NO title");
    let debug = format!("{card:?}");
    assert!(
        !debug.contains(SECRET_TITLE) && !debug.contains("leadership") && !debug.contains("comp"),
        "the leaked-title signal must be 0; card debug = {debug}"
    );
}

#[test]
fn chat_d5_confidential_unfurl_tombstones_zero_title_leak() {
    let (svc_check, _store) = check_engine_with(&[member("channel:board-secret", "p:alice")]);
    let svc = UnfurlService::new(svc_check, LeakyResolver);

    let card = svc.resolve_one(&candidate(), &principal("p:mallory"));
    assert!(card.is_tombstone(), "a denied viewer sees a tombstone");
    assert_zero_title_leak(&card);

    let allowed = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert_eq!(
        allowed.exposed_title(),
        Some(SECRET_TITLE),
        "the member DOES see the title (the gate is not vacuously denying everyone)"
    );
}

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
    assert_eq!(
        svc.cache().entry_count(),
        1,
        "exactly ONE cache entry per ref (0 per-viewer entries)"
    );
}

#[test]
fn chat_d5_chained_member_revoke_tombstone_zero_leak() {
    let (svc_check, store) = check_engine_with(&[member("channel:board-secret", "p:dave")]);
    let svc = UnfurlService::new(svc_check, LeakyResolver);

    let before = svc.resolve_one(&candidate(), &principal("p:dave"));
    assert_eq!(before.exposed_title(), Some(SECRET_TITLE));
    assert_eq!(svc.cache().entry_count(), 1);

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

    let after = svc.resolve_one(&candidate(), &principal("p:dave"));
    assert!(after.is_tombstone(), "post-revoke the card is a tombstone");
    assert_zero_title_leak(&after);
    assert_eq!(
        svc.cache().entry_count(),
        1,
        "still ONE shared cache entry - the no-leak is the GATE, not a per-viewer cache"
    );
}
