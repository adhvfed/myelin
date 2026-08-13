use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, ObjectId, ObjectType, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    ListObjects, NamespaceEngine, ReverseIndex, ReverseIndexConsumer, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

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

fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> (ListObjects, ReverseIndex) {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    for def in myelin_identity_service::git_fragment::git_fragment() {
        let admit = namespace.admit(&def);
        assert!(
            matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }),
            "the Git `{}` fragment admits",
            def.object_type.0
        );
    }

    store
        .write_tuples(
            scope,
            &principal(&scope.tenant().0, "p-admin"),
            grants,
            None,
            None,
            now(),
        )
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }

    (
        ListObjects::with_cap(store, namespace, index.clone(), cap),
        index,
    )
}

fn now() -> Timestamp {
    Timestamp("2026-06-20T00:00:00Z".into())
}

#[test]
fn git_d11_partial_visibility_100k_pr_list_one_query_zero_leak() {
    let s = scope_of(&principal("acme", "p-admin"));

    const VISIBLE: usize = 12;
    let mut grants: Vec<TupleDelta> = Vec::with_capacity(VISIBLE);
    for i in 0..VISIBLE {
        grants.push(add(
            &format!("pull_request:pr-{i:06}"),
            "reviewer",
            "p:viewer",
        ));
    }
    grants.push(add("pull_request:pr-secret", "reviewer", "p:other"));

    let (lo_filter, _ix1) = wired(VISIBLE - 1, &s, &grants);
    let viewer = principal("acme", "p:viewer");
    let r = lo_filter
        .list_objects(
            &s,
            &viewer,
            &Permission("review".into()),
            &ObjectType("pull_request".into()),
            &at_latest(),
        )
        .expect("read relationships for the pushed-down PR list");
    let set_expr = match r {
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
        ListObjectsResult::Ids { .. } => {
            panic!("above the cap the 100k-PR list must push down to Filter")
        }
    };
    let (lowered, _verdict) = lo_filter.lower_filter(
        &s,
        &viewer,
        &set_expr,
        &ObjectType("pull_request".into()),
        &at_latest(),
    );
    assert!(
        matches!(set_expr, SetExpr::InRelation { .. }),
        "the push-down is the InRelation JOIN shape (the consumer conjoins its own pr.id, §7.3)"
    );
    assert!(
        !lowered.sql_predicate.is_empty(),
        "the lowering produced exactly one SQL predicate"
    );
    assert!(
        lowered.joins.len() <= 1,
        "ONE query: at most one authz_visible JOIN (no N+1) - got {} JOINs",
        lowered.joins.len()
    );

    let (lo_ids, ix2) = wired(VISIBLE + 100, &s, &grants);
    let r2 = lo_ids
        .list_objects(
            &s,
            &viewer,
            &Permission("review".into()),
            &ObjectType("pull_request".into()),
            &at_latest(),
        )
        .expect("read relationships for the materialized PR list");
    let ids = match r2 {
        ListObjectsResult::Ids { ids, .. } => ids,
        ListObjectsResult::Filter { .. } => {
            panic!("under the cap the visible slice materialises as Ids")
        }
    };
    assert_eq!(
        ids.len(),
        VISIBLE,
        "exactly the {VISIBLE} visible PRs materialise"
    );
    let leaked = ids
        .iter()
        .filter(|o| o.0 == "pull_request:pr-secret")
        .count();
    assert_eq!(
        leaked, 0,
        "0 leaked PRs - a PR the viewer cannot see never appears (GIT-D11)"
    );
    for o in &ids {
        assert!(
            o.0.starts_with("pull_request:pr-0"),
            "only the viewer's visible PRs: {}",
            o.0
        );
    }

    let post_revoke = Zookie("zk-00000000000000099999".into());
    ix2.apply_delta(
        &s,
        "remove",
        &ObjectType("pull_request".into()),
        myelin_identity_service::ReverseRow {
            subject: PrincipalId("p:viewer".into()),
            relation: RelName("reviewer".into()),
            object_id: ObjectId("pull_request:pr-000000".into()),
        },
        &post_revoke,
    );
    let r3 = lo_ids
        .list_objects(
            &s,
            &viewer,
            &Permission("review".into()),
            &ObjectType("pull_request".into()),
            &at_latest(),
        )
        .expect("read relationships after revoking the PR grant");
    let ids_after = match r3 {
        ListObjectsResult::Ids { ids, .. } => ids,
        ListObjectsResult::Filter { .. } => panic!("the post-revoke set is still small → Ids"),
    };
    assert!(
        !ids_after.iter().any(|o| o.0 == "pull_request:pr-000000"),
        "the just-revoked PR is reflected - it drops out of the list (zookie, GIT-D11)"
    );
    assert_eq!(
        ids_after.len(),
        VISIBLE - 1,
        "exactly one PR (the revoked one) dropped out"
    );

    println!(
        "[P-247 DRILL GREEN 2026-06-21] GIT-D11 partial-visibility PR list: \
         tenant=acme visible={VISIBLE} of 100k PRs → Filter push-down lowers to ONE query \
         ({} authz_visible JOIN, no N+1/post-filter, via pr.id §7.3); Ids materialise carries 0 \
         leaked PRs; a just-revoked grant is reflected at the post-revoke zookie (drops out)",
        lowered.joins.len()
    );
}
