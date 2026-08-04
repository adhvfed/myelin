use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, Literal, ObjectId,
    Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    knowledge_fragment, ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("p-admin".into()),
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
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            tuples,
            None,
            None,
            Timestamp("2026-06-21T00:00:00Z".into()),
        )
        .expect("seed tuples");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    let svc = StoreBackedCheck::with_index(store, index);
    for admit in svc.admit_knowledge_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "Id's compiled Knowledge fragment admits: {admit:?}"
        );
    }
    svc
}

#[test]
fn cdc_4_9_id_compiled_knowledge_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);
    let ns = svc.namespace();
    for ty in ["space", "page", "block", "database_row"] {
        assert!(
            ns.object_types().contains(&ty.to_string()),
            "`{ty}` admitted into the cell schema"
        );
    }
    assert!(ns.resolve_permission("page", "read").is_some());
    assert!(ns.resolve_permission("database_row", "read").is_some());
}

#[test]
fn cdc_4_9_page_tree_inheritance_resolves() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("page:child", "parent_page", "page:parent#read"),
            add("page:parent", "direct_reader", "p:alice"),
            add("page:child", "direct_reader", "p:bob"),
        ],
    );
    let child = ArtifactRef("page:child".into());
    let can_read = |actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission("read".into()),
                &child,
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can_read(&subject("p:alice")),
        "a parent-page reader inherits the child (parent_page->read)"
    );
    assert!(
        can_read(&subject("p:bob")),
        "a direct reader of the child reads it"
    );
    assert!(
        !can_read(&subject("p:carol")),
        "an outsider cannot read (fail-closed)"
    );
}

#[test]
fn cdc_4_9_direct_block_override_narrows_inherited_access() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("page:child", "parent_page", "page:parent#read"),
            add("page:parent", "direct_reader", "p:alice"),
            add("page:child", "direct_block", "p:alice"),
            add("page:parent", "direct_reader", "p:bob"),
        ],
    );
    let parent = ArtifactRef("page:parent".into());
    let child = ArtifactRef("page:child".into());
    let can_read = |actor: &Principal, obj: &ArtifactRef| {
        matches!(
            svc.check(actor, &Permission("read".into()), obj, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can_read(&subject("p:alice"), &parent),
        "alice reads the parent (direct_reader)"
    );
    assert!(
        !can_read(&subject("p:alice"), &child),
        "the - direct_block override narrows alice's inherited access (she does NOT read the sub-page)"
    );
    assert!(
        can_read(&subject("p:bob"), &child),
        "an un-blocked inheriting reader still reads the child"
    );
}

#[test]
fn cdc_4_9_row_level_acl_conjoins_via_list_objects() {
    use myelin_identity::{ListObjectsResult, ObjectType};
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("database_row:r1", "direct_reader", "p:viewer"),
            add("database_row:r2", "direct_reader", "p:viewer"),
            add("database_row:r-secret", "direct_reader", "p:other"),
        ],
    );
    let result = svc
        .list_objects(
            &subject("p:viewer"),
            &Permission("read".into()),
            &ObjectType("database_row".into()),
            &at_latest(),
        )
        .expect("list_objects over the row ACL");
    let ids = match result {
        ListObjectsResult::Ids { ids, .. } => ids,
        ListObjectsResult::Filter { .. } => panic!("a small visible set materialises as Ids"),
    };
    assert_eq!(ids.len(), 2, "exactly the viewer's two readable rows");
    assert!(
        !ids.iter().any(|o| o.0 == "database_row:r-secret"),
        "0 leak: the row granted to someone else NEVER appears in the viewer's list"
    );
}

#[test]
fn cdc_4_9_field_caveat_hides_a_column() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[add("database_row:emp-1", "direct_reader", "p:viewer")],
    );
    let row = ArtifactRef("database_row:emp-1".into());

    assert_eq!(
        svc.check(
            &subject("p:viewer"),
            &Permission("read".into()),
            &row,
            &at_latest(),
            None
        ),
        Ok(Decision::Allow),
        "the viewer reads the row (the row-level ACL); the field caveat gates a column on top"
    );

    let cleared = knowledge_fragment::field_view_caveat(
        "database_row:emp-1",
        "salary",
        "ge",
        "clearance",
        Literal::Int(3),
        &[("clearance", Literal::Int(5))],
    );
    assert_eq!(
        svc.check(
            &subject("p:viewer"),
            &Permission("view_field".into()),
            &row,
            &at_latest(),
            Some(&cleared)
        ),
        Ok(Decision::Allow),
        "a cleared viewer sees the salary column"
    );

    let under = knowledge_fragment::field_view_caveat(
        "database_row:emp-1",
        "salary",
        "ge",
        "clearance",
        Literal::Int(3),
        &[("clearance", Literal::Int(1))],
    );
    assert_eq!(
        svc.check(
            &subject("p:viewer"),
            &Permission("view_field".into()),
            &row,
            &at_latest(),
            Some(&under)
        ),
        Ok(Decision::Deny),
        "an under-cleared viewer's salary column is redacted (Deny) - absent, not a post-filter"
    );

    let missing = knowledge_fragment::field_view_caveat(
        "database_row:emp-1",
        "salary",
        "ge",
        "clearance",
        Literal::Int(3),
        &[],
    );
    assert_eq!(
        svc.check(
            &subject("p:viewer"),
            &Permission("view_field".into()),
            &row,
            &at_latest(),
            Some(&missing)
        ),
        Ok(Decision::Conditional),
        "a field caveat needing missing context is Conditional, never a silent allow (§8.6)"
    );
}
