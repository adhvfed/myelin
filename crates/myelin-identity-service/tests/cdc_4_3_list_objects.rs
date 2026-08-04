use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    ColRef, Consistency, ConsistencyMode, IdentityService, ListObjectsResult, ObjectId, ObjectType,
    Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta,
    Zookie,
};
use myelin_identity_service::{
    namespace::{FragmentDef, PermissionRule, Userset},
    ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

fn admin(tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn scope(tenant: &str) -> TenantScope {
    TenantScope::from_verified_token(&admin(tenant), Region("eu-west".into()))
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

fn provider(scope: &TenantScope, grants: &[TupleDelta]) -> StoreBackedCheck {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    store
        .write_tuples(
            scope,
            &admin(&scope.tenant().0),
            grants,
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }

    let svc = StoreBackedCheck::with_index(store, index);
    let _ = svc.admit_fragment_def(&FragmentDef {
        object_type: ObjectType("repo".into()),
        relations: vec![RelName("reader".into()), RelName("writer".into())],
        permissions: vec![PermissionRule {
            permission: Permission("read".into()),
            rewrite: Userset::Union(vec![
                Userset::Relation(RelName("reader".into())),
                Userset::Relation(RelName("writer".into())),
            ]),
        }],
    });
    svc
}

fn list_consumer_renders(result: &ListObjectsResult, ty: &str) -> Vec<String> {
    match result {
        ListObjectsResult::Ids { ids, .. } => {
            let mut out: Vec<String> = ids.iter().map(|o| o.0.clone()).collect();
            out.sort();
            out
        }
        ListObjectsResult::Filter { set_expr, .. } => {
            match set_expr {
                SetExpr::InRelation { via_column, .. } => {
                    assert_eq!(
                        via_column,
                        &ColRef { table: ty.to_string(), column: "id".to_string() },
                        "the Filter push-down names the consumer's own id column (§7.3)"
                    );
                }
                other => panic!("the Filter is the InRelation push-down shape the consumer conjoins, got {other:?}"),
            }
            vec!["<pushed-down-filter>".to_string()]
        }
    }
}

#[test]
fn cdc_4_3_ids_path_renders_exactly_the_reachable_set() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            grant("repo:core", "reader", "p:alice"),
            grant("repo:web", "writer", "p:alice"),
            grant("repo:secret", "reader", "p:bob"),
        ],
    );
    let result = svc
        .list_objects(
            &subject("p:alice"),
            &Permission("read".into()),
            &ObjectType("repo".into()),
            &at_latest(),
        )
        .expect("list_objects returns a result");
    let rendered = list_consumer_renders(&result, "repo");
    assert_eq!(
        rendered,
        vec!["repo:core".to_string(), "repo:web".to_string()],
        "the consumer renders exactly alice's two readable repos (leak-free - bob's repo is absent)"
    );
}

#[test]
fn cdc_4_3_filter_path_is_conjoined_not_post_filtered() {
    let filter = ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: ColRef {
                table: "repo".into(),
                column: "id".into(),
            },
        },
        zookie: Zookie("zk-00000000000000000001".into()),
    };
    let rendered = list_consumer_renders(&filter, "repo");
    assert_eq!(
        rendered,
        vec!["<pushed-down-filter>".to_string()],
        "the consumer conjoins the push-down (no post-filter)"
    );
}

#[test]
fn cdc_4_3_no_grant_renders_empty() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("repo:core", "reader", "p:alice")]);
    let result = svc
        .list_objects(
            &subject("p:nobody"),
            &Permission("read".into()),
            &ObjectType("repo".into()),
            &at_latest(),
        )
        .expect("list_objects returns a result");
    assert!(
        list_consumer_renders(&result, "repo").is_empty(),
        "a subject with no grant renders nothing (leak-free - never a permissive set)"
    );
}

#[test]
fn cdc_4_3_no_cross_tenant_list() {
    let acme = scope("acme");
    let svc = provider(&acme, &[grant("repo:core", "reader", "p:alice")]);
    let mut alice_globex = subject("p:alice");
    alice_globex.tenant = TenantId("globex".into());
    let result = svc
        .list_objects(
            &alice_globex,
            &Permission("read".into()),
            &ObjectType("repo".into()),
            &at_latest(),
        )
        .expect("list_objects returns a result");
    assert!(
        list_consumer_renders(&result, "repo").is_empty(),
        "a grant in acme does not list under globex (0 cross-tenant rows)"
    );
}
