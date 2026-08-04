use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ListObjectsResult, ObjectId,
    ObjectType, Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr,
    TupleDelta, Zookie,
};
use myelin_identity_service::{FragmentDef, PermissionRule, StoreBackedCheck, TupleStore, Userset};
use myelin_issues::planner::{compose_board_query, lower_over_issue_id, AuthzVisibleIndex};
use myelin_issues::rebac_fragment::object_types;
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
            Timestamp("2026-06-20T00:00:00Z".into()),
        )
        .expect("seed tuples");
    StoreBackedCheck::new(store)
}

fn issue_view_def() -> FragmentDef {
    let rel = |n: &str| Userset::Relation(RelName(n.into()));
    let ttu = |t: &str, c: &str| Userset::TupleToUserset {
        tupleset: RelName(t.into()),
        computed: RelName(c.into()),
    };
    FragmentDef {
        object_type: ObjectType(object_types::ISSUE.into()),
        relations: vec![
            RelName("parent_project".into()),
            RelName("assignee".into()),
            RelName("watcher".into()),
            RelName("confidential".into()),
            RelName("confidential_grant".into()),
        ],
        permissions: vec![PermissionRule {
            permission: Permission("view".into()),
            rewrite: Userset::Union(vec![
                Userset::Exclusion {
                    base: Box::new(ttu("parent_project", "view")),
                    subtracted: Box::new(rel("confidential")),
                },
                rel("confidential_grant"),
            ]),
        }],
    }
}

#[test]
fn cdc_4_3_confidential_issue_absent_through_the_planner() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("project:web", "reader", "p:carol"),
            add("project:web", "reader", "p:dave"),
            add("issue:open", "parent_project", "project:web#view"),
            add("issue:secret", "parent_project", "project:web#view"),
            add("issue:secret", "confidential", "p:dave"),
            add("issue:secret", "confidential_grant", "p:carol"),
        ],
    );
    let _ = svc.admit_fragment_def(&issue_view_def());

    let view = Permission("view".into());
    let universe = ["issue:open", "issue:secret"];

    let reachable = |actor: &Principal| -> Vec<ObjectId> {
        universe
            .iter()
            .filter(|id| {
                matches!(
                    svc.check(
                        actor,
                        &view,
                        &ArtifactRef((**id).into()),
                        &at_latest(),
                        None
                    ),
                    Ok(Decision::Allow)
                )
            })
            .map(|id| ObjectId((*id).into()))
            .collect()
    };

    let dave_set = reachable(&subject("p:dave"));
    assert!(
        dave_set.contains(&ObjectId("issue:open".into())),
        "dave reads the non-confidential issue (inherited parent_project->view)"
    );
    assert!(
        !dave_set.contains(&ObjectId("issue:secret".into())),
        "PROVIDER: the confidential issue is ABSENT from dave's reachable set (the - confidential \
         set-difference; 0 leak at the source)"
    );

    let idx = AuthzVisibleIndex::new();
    let universe_ids: Vec<ObjectId> = universe.iter().map(|id| ObjectId((*id).into())).collect();
    let dave_lowered = lower_over_issue_id(&SetExpr::Ids(dave_set), &subject("p:dave"));
    let dave_visible = idx.evaluate(
        &TenantId("acme".into()),
        &Region("eu-west".into()),
        &subject("p:dave"),
        &dave_lowered,
        &universe_ids,
    );
    assert_eq!(
        dave_visible,
        vec![ObjectId("issue:open".into())],
        "CONSUMER: the planner's lowered board excludes the confidential issue - 0 leak end-to-end"
    );

    let carol_set = reachable(&subject("p:carol"));
    assert!(
        carol_set.contains(&ObjectId("issue:secret".into())),
        "carol (grantee) reaches the confidential issue at the provider (the + grant arm)"
    );
    let carol_lowered = lower_over_issue_id(&SetExpr::Ids(carol_set), &subject("p:carol"));
    let carol_visible = idx.evaluate(
        &TenantId("acme".into()),
        &Region("eu-west".into()),
        &subject("p:carol"),
        &carol_lowered,
        &universe_ids,
    );
    assert!(
        carol_visible.contains(&ObjectId("issue:secret".into())),
        "CONSUMER: the grantee's lowered board includes the confidential issue (the + grant arm)"
    );
}

#[test]
fn cdc_4_3_composed_board_is_one_query_over_the_real_set() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("project:web", "reader", "p:carol"),
            add("issue:open", "parent_project", "project:web#view"),
        ],
    );
    let _ = svc.admit_fragment_def(&issue_view_def());

    let result = svc
        .list_objects(
            &subject("p:carol"),
            &Permission("view".into()),
            &ObjectType(object_types::ISSUE.into()),
            &at_latest(),
        )
        .expect("live list_objects");
    let set_expr = match result {
        ListObjectsResult::Ids { ids, .. } => SetExpr::Ids(ids),
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
    };
    let q = compose_board_query(
        &set_expr,
        &subject("p:carol"),
        &TenantId("acme".into()),
        &Region("eu-west".into()),
    );
    assert_eq!(
        q.statement_count(),
        1,
        "one query - the conjoin is the planner's job, no N+1"
    );
    assert!(q
        .sql
        .contains("WHERE issue.tenant_id = :tenant AND issue.region = :region"));
    let acl_pos = q.sql.find("AND (").unwrap();
    let order_pos = q.sql.find("ORDER BY issue.rank").unwrap();
    assert!(
        acl_pos < order_pos,
        "the ACL pre-filter precedes the ORDER BY (never a post-filter)"
    );
}
