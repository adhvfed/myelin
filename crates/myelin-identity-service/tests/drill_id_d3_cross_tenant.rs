use myelin_events::{OutboxStore, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn principal(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn scope_of(p: &Principal) -> TenantScope {
    TenantScope::from_verified_token(p, p.region.clone())
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

#[test]
fn id_d3_cross_tenant_path_spoof_reads_zero() {
    let mut signals = SignalSource::new();

    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &[
                add("project:web", "parent_team", "team:eng#view"),
                add("team:eng", "member", "p:alice"),
            ],
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("seed acme grant");

    let svc = StoreBackedCheck::new(store);
    let spoofed_object = ArtifactRef("project:web".into());

    let alice = principal("acme", "p:alice");
    assert_eq!(
        svc.check(
            &alice,
            &Permission("view".into()),
            &spoofed_object,
            &at_latest(),
            None
        ),
        Ok(Decision::Allow),
        "the legitimate acme principal inherits view (the engine resolves within acme's partition)"
    );

    let mut cross_tenant_reads: i64 = 0;
    const BATCH: usize = 64;
    for i in 0..BATCH {
        let mallory = principal("evil-corp", &format!("p:mallory-{i}"));
        let mallory_as_alice = {
            let mut m = mallory.clone();
            m.principal_id = PrincipalId("p:alice".into());
            m.tenant = TenantId("evil-corp".into());
            m
        };
        let decision = svc.check(
            &mallory_as_alice,
            &Permission("view".into()),
            &spoofed_object,
            &at_latest(),
            None,
        );
        if decision == Ok(Decision::Allow) {
            cross_tenant_reads += 1;
        }
    }

    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_reads);

    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    assert_eq!(
        cross_tenant_reads, 0,
        "0 cross-tenant tuples readable through the engine on a spoofed path (ID-D3)"
    );

    println!(
        "[P-068 DRILL GREEN 2026-06-19] ID-D3 cross-tenant path-spoof: \
         victim=acme attacker=evil-corp batch={BATCH} spoof attempts on project:web (view, \
         parent_team->view inheritance) → CrossTenantCount=0 (the engine resolves only the \
         verified (tenant, region) partition - no cross-tenant query path, identity §6)"
    );
}

#[test]
fn id_d3_inheritance_edge_does_not_cross_tenant() {
    let acme = scope_of(&principal("acme", "p-admin"));
    let evil = scope_of(&principal("evil-corp", "p-admin"));

    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &[
                add("project:web", "parent_team", "team:eng#view"),
                add("team:eng", "member", "p:alice"),
            ],
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("acme chain");
    let _ = evil;

    let svc = StoreBackedCheck::new(store);
    let obj = ArtifactRef("project:web".into());

    let mallory = principal("evil-corp", "p:alice");
    assert_eq!(
        svc.check(
            &mallory,
            &Permission("view".into()),
            &obj,
            &at_latest(),
            None
        ),
        Ok(Decision::Deny),
        "a cross-tenant principal does not inherit through acme's tuple-to-userset edge"
    );
    let alice = principal("acme", "p:alice");
    assert_eq!(
        svc.check(&alice, &Permission("view".into()), &obj, &at_latest(), None),
        Ok(Decision::Allow),
        "acme's principal still inherits within its own partition"
    );
}
