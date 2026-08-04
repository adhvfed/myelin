use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, ObjectId, ObjectType,
    Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    FragmentDef, NamespaceEngine, PermissionRule, StoreBackedCheck, TupleStore, Userset,
};
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

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn provider(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            tuples,
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("seed tuples");
    StoreBackedCheck::new(store)
}

#[test]
fn cdc_4_9_provider_admits_well_formed_rejects_malformed() {
    let mut eng = NamespaceEngine::new();

    let ok = FragmentDef {
        object_type: ObjectType("wiki_page".into()),
        relations: vec![RelName("reader".into()), RelName("editor".into())],
        permissions: vec![PermissionRule {
            permission: Permission("read".into()),
            rewrite: Userset::Union(vec![
                Userset::Relation(RelName("reader".into())),
                Userset::Relation(RelName("editor".into())),
            ]),
        }],
    };
    assert!(
        matches!(eng.admit(&ok), FragmentAdmit::Admitted { fragment_id } if fragment_id == "wiki_page"),
        "a well-formed fragment admits"
    );

    let bad = FragmentDef {
        object_type: ObjectType("ledger".into()),
        relations: vec![RelName("owner".into())],
        permissions: vec![PermissionRule {
            permission: Permission("read".into()),
            rewrite: Userset::Relation(RelName("auditor".into())),
        }],
    };
    assert!(
        matches!(eng.admit(&bad), FragmentAdmit::Rejected { .. }),
        "a fragment with an undeclared relation is rejected"
    );

    let id_minting = FragmentDef {
        object_type: ObjectType("wiki_page:home".into()),
        relations: vec![RelName("reader".into())],
        permissions: vec![],
    };
    assert!(
        matches!(eng.admit(&id_minting), FragmentAdmit::Rejected { .. }),
        "a fragment cannot mint object ids"
    );
}

#[test]
fn cdc_4_9_consumer_declares_a_fragment_and_gates_on_it() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("release:v2", "reader", "p:alice"),
            add("release:v2", "approver", "p:alice"),
            add("release:v2", "reader", "p:bob"),
        ],
    );

    let release_fragment = FragmentDef {
        object_type: ObjectType("release".into()),
        relations: vec![RelName("reader".into()), RelName("approver".into())],
        permissions: vec![PermissionRule {
            permission: Permission("ship".into()),
            rewrite: Userset::Intersect(vec![
                Userset::Relation(RelName("reader".into())),
                Userset::Relation(RelName("approver".into())),
            ]),
        }],
    };
    assert!(
        matches!(
            svc.admit_fragment_def(&release_fragment),
            FragmentAdmit::Admitted { .. }
        ),
        "the consumer's fragment admits into the cell schema"
    );

    let obj = ArtifactRef("release:v2".into());
    let ship = |actor: &Principal| -> bool {
        matches!(
            svc.check(actor, &Permission("ship".into()), &obj, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(
        ship(&subject("p:alice")),
        "reader ∩ approver ships (alice is both)"
    );
    assert!(
        !ship(&subject("p:bob")),
        "a reader-only does not ship (the intersect denies)"
    );
}

#[test]
fn cdc_4_9_abi_admit_fragment_validates_names_level() {
    let s = scope("acme");
    let svc = provider(&s, &[]);

    let ok = myelin_identity::NamespaceFragment {
        object_type: ObjectType("channel".into()),
        relations: vec![RelName("member".into()), RelName("read".into())],
        permissions: vec![Permission("read".into())],
    };
    assert!(
        matches!(svc.admit_fragment(&ok), Ok(FragmentAdmit::Admitted { .. })),
        "a names-only fragment with a declared-relation permission admits"
    );

    let bad = myelin_identity::NamespaceFragment {
        object_type: ObjectType("vault".into()),
        relations: vec![RelName("owner".into())],
        permissions: vec![Permission("decrypt".into())],
    };
    assert!(
        matches!(svc.admit_fragment(&bad), Ok(FragmentAdmit::Rejected { .. })),
        "a names-only permission that names no declared relation is rejected"
    );
}

#[test]
fn cdc_4_9_core_hierarchy_inheritance_resolves() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("project:web", "parent_team", "team:eng#view"),
            add("team:eng", "member", "p:alice"),
        ],
    );
    let obj = ArtifactRef("project:web".into());
    assert_eq!(
        svc.check(&subject("p:alice"), &Permission("view".into()), &obj, &at_latest(), None),
        Ok(Decision::Allow),
        "a project reader via team membership inherits view (parent_team->view, the four operators)"
    );
    assert_eq!(
        svc.check(
            &subject("p:bob"),
            &Permission("view".into()),
            &obj,
            &at_latest(),
            None
        ),
        Ok(Decision::Deny),
        "a non-member does not inherit project view (fail-closed)"
    );
}
