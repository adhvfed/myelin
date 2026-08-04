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
fn git_d8_cross_tenant_repo_access_reads_zero() {
    let mut signals = SignalSource::new();

    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &[add("repo:core", "admin", "p:alice")],
            None,
            None,
            Timestamp("2026-06-20T00:00:00Z".into()),
        )
        .expect("seed acme grant");

    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_git_fragment() {
        assert!(matches!(
            admit,
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
    }
    let spoofed_repo = ArtifactRef("repo:core".into());

    let alice = principal("acme", "p:alice");
    assert_eq!(
        svc.check(
            &alice,
            &Permission("pull".into()),
            &spoofed_repo,
            &at_latest(),
            None
        ),
        Ok(Decision::Allow),
        "the legitimate acme admin pulls (Id resolves within acme's partition)"
    );

    let mut cross_tenant_reads: i64 = 0;
    const BATCH: usize = 64;
    for i in 0..BATCH {
        let mut attacker = principal("evil-corp", &format!("p:mallory-{i}"));
        attacker.principal_id = PrincipalId("p:alice".into());
        attacker.tenant = TenantId("evil-corp".into());
        let decision = svc.check(
            &attacker,
            &Permission("pull".into()),
            &spoofed_repo,
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
        "0 cross-tenant repo reads on a spoofed token-tenant ≠ path-tenant request (GIT-D8)"
    );

    println!(
        "[P-247 DRILL GREEN 2026-06-21] GIT-D8 cross-tenant repo access: \
         victim=acme attacker=evil-corp batch={BATCH} spoofed pull attempts on repo:core (Id's \
         compiled Git fragment, pull = reader∪writer∪admin∪parent_project->view) → \
         CrossTenantCount=0 (tenant-from-token, never the URL path - no cross-tenant query path, \
         identity §6 / ID-3)"
    );
}

#[test]
fn git_d8_approve_untrusted_ci_does_not_cross_tenant() {
    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &[add("repo:core", "approve_untrusted_ci", "p:maintainer")],
            None,
            None,
            Timestamp("2026-06-20T00:00:00Z".into()),
        )
        .expect("seed acme endorsement");
    let svc = StoreBackedCheck::new(store);
    for _ in svc.admit_git_fragment() {}
    let repo = ArtifactRef("repo:core".into());

    assert_eq!(
        svc.check(
            &principal("acme", "p:maintainer"),
            &Permission("approve_untrusted_ci".into()),
            &repo,
            &at_latest(),
            None,
        ),
        Ok(Decision::Allow),
        "acme's maintainer endorses within acme's partition"
    );
    assert_eq!(
        svc.check(
            &principal("evil-corp", "p:maintainer"),
            &Permission("approve_untrusted_ci".into()),
            &repo,
            &at_latest(),
            None,
        ),
        Ok(Decision::Deny),
        "a cross-tenant principal cannot read the acme endorsement (X-1 gate is scope-local)"
    );
}
