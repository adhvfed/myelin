use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, ObjectId, ObjectType, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    ListObjects, NamespaceEngine, ReverseIndex, ReverseIndexConsumer, TupleStore,
};
use myelin_knowledge::{
    compose_db_count_query, compose_db_view_query, db_row_id_colref, lower_over_db_row_id,
    AuthzVisibleIndex, FilterMode,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

const ROW_PRODUCER_PERMISSION: &str = "read";

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
    for def in myelin_identity_service::knowledge_fragment::knowledge_fragment() {
        let admit = namespace.admit(&def);
        assert!(
            matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }),
            "the Knowledge `{}` fragment admits",
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
fn cdc_4_3_identity_filter_lowers_to_one_knowledge_view_and_count_query() {
    let s = scope_of(&principal("acme", "p-admin"));
    let grants = [
        add("database_row:row-1", "direct_reader", "p:viewer"),
        add("database_row:row-2", "direct_reader", "p:viewer"),
    ];
    let lo = wired(1, &s, &grants);
    let viewer = principal("acme", "p:viewer");

    let r = lo.list_objects(
        &s,
        &viewer,
        &Permission(ROW_PRODUCER_PERMISSION.into()),
        &ObjectType("database_row".into()),
        &at_latest(),
    );
    let set_expr = match r {
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
        ListObjectsResult::Ids { .. } => panic!("above the cap the producer pushes down to Filter"),
    };
    assert!(
        matches!(set_expr, SetExpr::InRelation { .. }),
        "the producer emits the InRelation push-down shape"
    );
    if let SetExpr::InRelation { ref via_column, .. } = set_expr {
        assert_eq!(
            via_column,
            &db_row_id_colref(),
            "the producer + consumer agree on the §7.3 db_row.id via_column"
        );
    }

    let view = compose_db_view_query(&set_expr, &viewer, s.tenant(), "db:projects");
    assert_eq!(
        view.statement_count(),
        1,
        "the consumer composes EXACTLY ONE view query (no N+1)"
    );
    assert!(
        view.sql
            .contains("JOIN authz_visible av0 ON av0.object_id = db_row.id"),
        "the consumer JOINs the producer's reverse index over its own db_row.id (§4.1/§7.3): {}",
        view.sql
    );
    assert_eq!(view.filter_mode, FilterMode::PushedDown);

    let count = compose_db_count_query(&set_expr, &viewer, s.tenant(), "db:projects");
    assert_eq!(
        count.statement_count(),
        1,
        "the consumer composes EXACTLY ONE COUNT query"
    );
    assert!(count.is_count && count.sql.starts_with("SELECT COUNT(*) FROM db_row"));
    assert!(
        count.sql.contains("AND (av0.object_id IS NOT NULL)"),
        "the ACL is conjoined INSIDE the COUNT (the count-leak closed by construction): {}",
        count.sql
    );
}

#[test]
fn kn_d5_chained_grant_list_zero_leak_zero_count_leak_one_query_then_revoke_reflected() {
    let s = scope_of(&principal("acme", "p-admin"));
    let viewer = principal("acme", "p:viewer");
    let region = Region("fr-par".into());

    const VISIBLE: usize = 3;
    let mut grants: Vec<TupleDelta> = Vec::new();
    for i in 0..VISIBLE {
        grants.push(add(
            &format!("database_row:row-{i}"),
            "direct_reader",
            "p:viewer",
        ));
    }
    grants.push(add("database_row:row-secret", "direct_reader", "p:other"));
    let lo = wired(VISIBLE - 1, &s, &grants);

    let r = lo.list_objects(
        &s,
        &viewer,
        &Permission(ROW_PRODUCER_PERMISSION.into()),
        &ObjectType("database_row".into()),
        &at_latest(),
    );
    let set_expr = match r {
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
        ListObjectsResult::Ids { .. } => {
            panic!("the partial-visibility list pushes down to Filter")
        }
    };
    let view = compose_db_view_query(&set_expr, &viewer, s.tenant(), "db:projects");
    let count_q = compose_db_count_query(&set_expr, &viewer, s.tenant(), "db:projects");
    assert_eq!(
        view.statement_count(),
        1,
        "ONE view query (KN-D5 signal: 1 query)"
    );
    assert_eq!(count_q.statement_count(), 1, "ONE COUNT query");
    let lowered = lower_over_db_row_id(&set_expr, &viewer);
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
            ROW_PRODUCER_PERMISSION,
            &format!("database_row:row-{i}"),
            "zk-0000000001",
        );
    }
    av.grant(
        s.tenant(),
        &region,
        "p:other",
        ROW_PRODUCER_PERMISSION,
        "database_row:row-secret",
        "zk-0000000001",
    );

    let mut candidates: Vec<String> = (0..VISIBLE)
        .map(|i| format!("database_row:row-{i}"))
        .collect();
    candidates.push("database_row:row-secret".into());
    let candidate_refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();

    let visible = av.evaluate(s.tenant(), &region, &viewer, &lowered, &candidate_refs);
    assert_eq!(
        visible.len(),
        VISIBLE,
        "exactly the {VISIBLE} visible rows survive"
    );
    assert!(
        !visible.iter().any(|o| o == "database_row:row-secret"),
        "0 leak: a row the viewer cannot see never appears (KN-D5)"
    );
    let n = av.count_visible(s.tenant(), &region, &viewer, &lowered, &candidate_refs);
    assert_eq!(n, VISIBLE, "0 count-leak: COUNT = {VISIBLE} (the visible rows), NOT {} (the universe incl. the secret)", candidate_refs.len());
    assert_eq!(
        n,
        visible.len(),
        "the COUNT equals the listed cardinality - no second path can diverge"
    );

    av.revoke(
        s.tenant(),
        &region,
        "p:viewer",
        ROW_PRODUCER_PERMISSION,
        "database_row:row-0",
        "zk-0000000099",
    );
    let after = av.evaluate(s.tenant(), &region, &viewer, &lowered, &candidate_refs);
    assert!(
        !after.iter().any(|o| o == "database_row:row-0"),
        "the just-revoked row is reflected - it drops out of the list (zookie, KN-D5)"
    );
    assert_eq!(
        after.len(),
        VISIBLE - 1,
        "exactly one row (the revoked one) dropped out"
    );
    let n_after = av.count_visible(s.tenant(), &region, &viewer, &lowered, &candidate_refs);
    assert_eq!(n_after, VISIBLE - 1, "0 count-leak after revoke: the COUNT decremented - a revoked grant cannot be counted stale");

    assert!(
        av.serves(s.tenant(), &region, &Zookie("zk-0000000099".into())),
        "the watermark caught up to the revoke's revision → the JOIN serves (no stale grant)"
    );

    println!(
        "[P-306 CDC GREEN] KN-D5 db-row-list + COUNT SetExpr push-down: the REAL Identity producer \
         emits Filter{{InRelation read, db_row.id}}; Knowledge lowers it to ONE view + ONE COUNT over \
         db_row.id - {VISIBLE} visible of {} rows (0 leak: row-secret absent), COUNT={n} (0 count-leak); \
         a revoke drops row-0 from BOTH the list AND the COUNT (read-your-writes).",
        candidate_refs.len()
    );
}
