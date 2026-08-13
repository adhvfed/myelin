use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_git::list_filter::{
    code_search_pre_filter, compose_pr_list_query, lower_over_pr_id, AuthzVisibleIndex, FilterMode,
};

const PR_PRODUCER_PERMISSION: &str = "review";
const REPO_PRODUCER_PERMISSION: &str = "pull";
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
    p.region = Region("fr-par".into());
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

fn now() -> Timestamp {
    Timestamp("2026-06-22T00:00:00Z".into())
}

fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> ListObjects {
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
    ListObjects::with_cap(store, namespace, index, cap)
}

#[test]
fn cdc_4_3_identity_filter_lowers_to_one_git_pr_query() {
    let s = scope_of(&principal("acme", "p-admin"));
    let grants = [
        add("pull_request:pr-1", "reviewer", "p:viewer"),
        add("pull_request:pr-2", "reviewer", "p:viewer"),
    ];
    let lo = wired(1, &s, &grants);
    let viewer = principal("acme", "p:viewer");

    let r = lo
        .list_objects(
            &s,
            &viewer,
            &Permission(PR_PRODUCER_PERMISSION.into()),
            &ObjectType("pull_request".into()),
            &at_latest(),
        )
        .expect("read relationships for the PR authorization filter");
    let set_expr = match r {
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
        ListObjectsResult::Ids { .. } => panic!("above the cap the producer pushes down to Filter"),
    };
    assert!(
        matches!(set_expr, SetExpr::InRelation { .. }),
        "the producer emits the InRelation push-down shape"
    );

    let q = compose_pr_list_query(&set_expr, &viewer, s.tenant(), &Region("fr-par".into()));
    assert_eq!(
        q.statement_count(),
        1,
        "the consumer composes EXACTLY ONE SQL query (no N+1)"
    );
    assert!(
        q.sql
            .contains("JOIN authz_visible av0 ON av0.object_id = pr.id"),
        "the consumer JOINs the producer's reverse index over its own pr.id (§5.3/§7.3): {}",
        q.sql
    );
    assert_eq!(q.filter_mode, FilterMode::PushedDown);
}

#[test]
fn cdc_6_1_code_search_pre_filter_keys_on_repo() {
    let s = scope_of(&principal("acme", "p-admin"));
    let grants = [
        add("repo:core", "reader", "p:viewer"),
        add("repo:web", "reader", "p:viewer"),
    ];
    let lo = wired(1, &s, &grants);
    let viewer = principal("acme", "p:viewer");
    let r = lo
        .list_objects(
            &s,
            &viewer,
            &Permission(REPO_PRODUCER_PERMISSION.into()),
            &ObjectType("repo".into()),
            &at_latest(),
        )
        .expect("read relationships for the repository authorization filter");
    let set_expr = match r {
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
        ListObjectsResult::Ids { .. } => {
            panic!("above the cap the repo read pushes down to Filter")
        }
    };
    let pf = code_search_pre_filter(&set_expr, &viewer);
    assert!(
        pf.acl_filter.joins[0]
            .clause
            .contains("av0.object_id = code_doc.repo_id"),
        "the code-search pre-filter keys on the blob doc's parent-repo id (GIT-P5): {}",
        pf.acl_filter.joins[0].clause
    );
    assert!(pf.acl_filter.joins[0]
        .clause
        .contains("av0.relation = :rel_for_pull"));
}

#[test]
fn git_d11_chained_grant_list_zero_leak_one_query_then_revoke_reflected() {
    let s = scope_of(&principal("acme", "p-admin"));
    let viewer = principal("acme", "p:viewer");
    let region = Region("fr-par".into());

    const VISIBLE: usize = 3;
    let mut grants: Vec<TupleDelta> = Vec::new();
    for i in 0..VISIBLE {
        grants.push(add(&format!("pull_request:pr-{i}"), "reviewer", "p:viewer"));
    }
    grants.push(add("pull_request:pr-secret", "reviewer", "p:other"));
    let lo = wired(VISIBLE - 1, &s, &grants);

    let r = lo
        .list_objects(
            &s,
            &viewer,
            &Permission(PR_PRODUCER_PERMISSION.into()),
            &ObjectType("pull_request".into()),
            &at_latest(),
        )
        .expect("read relationships for the partial-visibility PR list");
    let set_expr = match r {
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
        ListObjectsResult::Ids { .. } => {
            panic!("the partial-visibility list pushes down to Filter")
        }
    };
    let q = compose_pr_list_query(&set_expr, &viewer, s.tenant(), &region);
    assert_eq!(
        q.statement_count(),
        1,
        "ONE SQL query (GIT-D11 signal: 1 query)"
    );
    let lowered = lower_over_pr_id(&set_expr, &viewer);
    assert!(
        lowered.joins.len() <= 1,
        "no N+1: at most one JOIN, got {}",
        lowered.joins.len()
    );

    let av = AuthzVisibleIndex::new();
    for i in 0..VISIBLE {
        av.grant(
            s.tenant(),
            &region,
            "p:viewer",
            PR_PRODUCER_PERMISSION,
            &format!("pull_request:pr-{i}"),
            "zk-00000000000000000001",
        );
    }
    av.grant(
        s.tenant(),
        &region,
        "p:other",
        PR_PRODUCER_PERMISSION,
        "pull_request:pr-secret",
        "zk-00000000000000000001",
    );

    let mut candidates: Vec<ObjectId> = (0..VISIBLE)
        .map(|i| ObjectId(format!("pull_request:pr-{i}")))
        .collect();
    candidates.push(ObjectId("pull_request:pr-secret".into()));

    let visible = av.evaluate(s.tenant(), &region, &viewer, &lowered, &candidates);
    assert_eq!(
        visible.len(),
        VISIBLE,
        "exactly the {VISIBLE} visible PRs survive"
    );
    assert!(
        !visible.iter().any(|o| o.0 == "pull_request:pr-secret"),
        "0 leak: a PR the viewer cannot see never appears (GIT-D11)"
    );

    av.revoke(
        s.tenant(),
        &region,
        "p:viewer",
        PR_PRODUCER_PERMISSION,
        "pull_request:pr-0",
        "zk-00000000000000000099",
    );
    let after = av.evaluate(s.tenant(), &region, &viewer, &lowered, &candidates);
    assert!(
        !after.iter().any(|o| o.0 == "pull_request:pr-0"),
        "the just-revoked PR is reflected - it drops out of the list (zookie, GIT-D11)"
    );
    assert_eq!(
        after.len(),
        VISIBLE - 1,
        "exactly one PR (the revoked one) dropped out"
    );
    assert!(
        av.serves(
            s.tenant(),
            &region,
            &Zookie("zk-00000000000000000099".into())
        ),
        "the watermark caught up to the revoke; the read reflects it (not a stale grant)"
    );

    println!(
        "[P-288 DRILL GREEN 2026-06-22] GIT-D11 chained: grant {VISIBLE} of a larger tenant → \
         Filter push-down lowers to ONE query ({} authz_visible JOIN over pr.id, no N+1/post-filter); \
         0 leaked PRs in the list; a just-revoked grant is reflected at the post-revoke zookie",
        lowered.joins.len()
    );
}
