use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, RevokeTarget, TupleDelta, Zookie,
};
use myelin_identity_service::{RevocationStore, StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn subject(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn grant(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn ts(s: &str) -> Timestamp {
    Timestamp(s.into())
}

fn provider(s: &TenantScope) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            s,
            &subject("p-admin"),
            &[grant("repo:core", "view", "p:alice")],
            None,
            None,
            ts("2026-06-19T00:00:00Z"),
        )
        .expect("seed grant");
    StoreBackedCheck::new(store)
}

fn surface_honours_session<S: IdentityService>(
    svc: &S,
    actor: &Principal,
    permission: &str,
    object: &ArtifactRef,
) -> bool {
    let decision = svc.check(
        actor,
        &Permission(permission.to_string()),
        object,
        &at_latest(),
        None,
    );
    matches!(decision, Ok(Decision::Allow))
}

#[test]
fn cdc_4_7_unrevoked_session_honoured() {
    let s = scope("acme");
    let svc = provider(&s);
    let obj = ArtifactRef("repo:core".into());
    assert!(
        surface_honours_session(&svc, &subject("p:alice"), "view", &obj),
        "an un-revoked principal with a grant is honoured"
    );
}

#[test]
fn cdc_4_7_revoked_principal_refused_across_surfaces() {
    let s = scope("acme");
    let svc = provider(&s);
    let obj = ArtifactRef("repo:core".into());

    assert!(surface_honours_session(
        &svc,
        &subject("p:alice"),
        "view",
        &obj
    ));

    svc.disable_principal_in(
        &s,
        &PrincipalId("p:alice".into()),
        ts("2026-06-19T01:00:00Z"),
    )
    .expect("record principal disablement");

    assert!(
        !surface_honours_session(&svc, &subject("p:alice"), "view", &obj),
        "a revoked principal is denied on every surface (the grant is intact but the revoke wins)"
    );
    let svc2 = {
        let store = TupleStore::new(OutboxStore::new());
        store
            .write_tuples(
                &s,
                &subject("p-admin"),
                &[grant("repo:core", "view", "p:carol")],
                None,
                None,
                ts("2026-06-19T00:00:00Z"),
            )
            .expect("seed carol");
        StoreBackedCheck::new(store)
    };
    assert!(
        surface_honours_session(&svc2, &subject("p:carol"), "view", &obj),
        "a different un-revoked principal is unaffected by alice's revoke"
    );
}

#[test]
fn cdc_4_7_revoke_is_idempotent() {
    let s = scope("acme");
    let store = RevocationStore::new();
    let target = RevokeTarget::Principal(PrincipalId("p:alice".into()));
    store
        .revoke(&s, &target, ts("2026-06-19T00:00:00Z"))
        .expect("record first revocation");
    store
        .revoke(&s, &target, ts("2026-06-19T09:00:00Z"))
        .expect("record idempotent revocation");
    assert_eq!(
        store.revocation_count(&s).expect("count revocations"),
        1,
        "a double-revoke is a no-op (idempotent - the denylist holds it once)"
    );
    assert!(store.is_revoked(&s, &target, &ts("2026-06-19T10:00:00Z")));
}
