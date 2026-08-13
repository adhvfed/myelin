use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    ColRef, Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService,
    ListObjectsResult, Literal, ObjectId, ObjectType, Permission, Principal, PrincipalId,
    PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    eval_caveat, issue_field_view_caveat, transition_caveat, ListObjects, NamespaceEngine,
    ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore, CONFIDENTIAL,
    CONFIDENTIAL_GRANT, ISSUE_PERFORM_TRANSITION, ISSUE_VIEW, ISSUE_VIEW_FIELD,
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
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed tuples");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    let svc = StoreBackedCheck::with_index(store, index);
    for admit in svc.admit_issue_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "Id's compiled Issues fragment admits: {admit:?}"
        );
    }
    svc
}

#[test]
fn cdc_4_9_id_compiled_issue_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);
    let ns = svc.namespace();
    for ty in ["issue", "field", "transition"] {
        assert!(
            ns.object_types().contains(&ty.to_string()),
            "`{ty}` admitted into the cell schema"
        );
    }
    assert!(ns.resolve_permission("issue", ISSUE_VIEW).is_some());
    assert!(ns.resolve_permission("field", ISSUE_VIEW_FIELD).is_some());
    assert!(ns
        .resolve_permission("transition", ISSUE_PERFORM_TRANSITION)
        .is_some());
}

#[test]
fn cdc_4_9_confidential_issue_disappears_for_a_normal_reader() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("project:proj", "reader", "p:alice"),
            add("project:proj", "reader", "p:bob"),
            add("issue:normal", "parent_project", "project:proj#view"),
            add("issue:secret", "parent_project", "project:proj#view"),
            add("issue:secret", CONFIDENTIAL, "p:alice"),
            add("issue:secret", CONFIDENTIAL_GRANT, "p:bob"),
        ],
    );
    let can_view = |actor: &Principal, issue: &str| {
        matches!(
            svc.check(
                actor,
                &Permission(ISSUE_VIEW.into()),
                &ArtifactRef(issue.into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can_view(&subject("p:alice"), "issue:normal"),
        "a project reader views a normal issue (parent_project->view)"
    );
    assert!(
        !can_view(&subject("p:alice"), "issue:secret"),
        "a confidential issue disappears for a normal reader (the − confidential exclusion, ISS-D3)"
    );
    assert!(
        can_view(&subject("p:bob"), "issue:secret"),
        "an explicit confidential_grant re-admits the issue (the ∪ confidential_grant arm)"
    );
    assert!(
        !can_view(&subject("p:carol"), "issue:normal"),
        "an outsider views nothing (fail-closed)"
    );
}

fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> ListObjects {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    for def in myelin_identity_service::issue_fragment::issue_fragment_defs() {
        let admit = namespace.admit(&def);
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "the Issues `{}` fragment admits",
            def.object_type.0
        );
    }
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            grants,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
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
fn cdc_4_9_board_conjoins_in_one_query() {
    let s = scope("acme");
    let mut grants: Vec<TupleDelta> = Vec::new();
    for i in 0..8 {
        grants.push(add(&format!("issue:b-{i}"), CONFIDENTIAL_GRANT, "p:alice"));
    }
    let lo = wired(2, &s, &grants);
    let result = lo
        .list_objects(
            &s,
            &subject("p:alice"),
            &Permission(ISSUE_VIEW.into()),
            &ObjectType("issue".into()),
            &at_latest(),
        )
        .expect("read relationships for the pushed-down issue board");
    match result {
        ListObjectsResult::Filter { set_expr, .. } => match set_expr {
            SetExpr::InRelation { via_column, .. } => {
                assert_eq!(
                    via_column,
                    ColRef {
                        table: "issue".into(),
                        column: "id".into()
                    },
                    "the board Filter names the consumer's own id column (issue.id, §7.3) - \
                     one query, no N+1"
                );
            }
            other => panic!("the board Filter is the InRelation push-down shape, got {other:?}"),
        },
        ListObjectsResult::Ids { .. } => {
            panic!("above the cap the board must push down to the issue.id Filter (the one-query conjoin)")
        }
    }
}

#[test]
fn cdc_4_9_board_materialise_is_leak_free_confidential_absent() {
    let s = scope("acme");
    let grants = vec![
        add("issue:visible-1", CONFIDENTIAL_GRANT, "p:alice"),
        add("issue:visible-2", CONFIDENTIAL_GRANT, "p:alice"),
        add("issue:secret", CONFIDENTIAL_GRANT, "p:other"),
        add("issue:secret", CONFIDENTIAL, "p:alice"),
    ];
    let lo = wired(100, &s, &grants);
    let result = lo
        .list_objects(
            &s,
            &subject("p:alice"),
            &Permission(ISSUE_VIEW.into()),
            &ObjectType("issue".into()),
            &at_latest(),
        )
        .expect("read relationships for the materialized issue board");
    let ids = match result {
        ListObjectsResult::Ids { ids, .. } => ids.into_iter().map(|o| o.0).collect::<Vec<String>>(),
        ListObjectsResult::Filter { .. } => panic!("below the cap the board materialises Ids"),
    };
    assert!(
        ids.iter().any(|i| i == "issue:visible-1") && ids.iter().any(|i| i == "issue:visible-2"),
        "alice's board lists her two re-admitted issues: {ids:?}"
    );
    assert!(
        !ids.iter().any(|i| i == "issue:secret"),
        "the confidential issue alice has no grant on is ABSENT from her board (leak-free, no count \
         leak - ISS-D3): {ids:?}"
    );
}

#[test]
fn cdc_4_9_field_caveat_hides_a_field_off_the_hot_path() {
    let cleared = issue_field_view_caveat(
        "field:issue-1/salary",
        "salary",
        "ge",
        "clearance",
        Literal::Int(3),
        &[("clearance", Literal::Int(5))],
    );
    assert_eq!(
        eval_caveat(&cleared),
        Decision::Allow,
        "cleared viewer sees the field"
    );

    let blocked = issue_field_view_caveat(
        "field:issue-1/salary",
        "salary",
        "ge",
        "clearance",
        Literal::Int(3),
        &[("clearance", Literal::Int(1))],
    );
    assert_eq!(
        eval_caveat(&blocked),
        Decision::Deny,
        "under-cleared viewer's field is redacted"
    );

    let missing = issue_field_view_caveat(
        "field:issue-1/salary",
        "salary",
        "ge",
        "clearance",
        Literal::Int(3),
        &[],
    );
    assert_eq!(
        eval_caveat(&missing),
        Decision::Conditional,
        "a field caveat needing missing context is Conditional, never a silent allow (§8.6)"
    );
    assert!(
        missing.field.is_some() && missing.transition.is_none(),
        "it is a FIELD caveat (field? set, transition? unset)"
    );
}

#[test]
fn cdc_4_9_transition_caveat_gates_a_transition_off_the_hot_path() {
    let approved = transition_caveat(
        "transition:issue-1/approve",
        "approve",
        "ge",
        "approver_count",
        Literal::Int(2),
        &[("approver_count", Literal::Int(2))],
    );
    assert_eq!(
        eval_caveat(&approved),
        Decision::Allow,
        "a transition with enough approvers is permitted"
    );

    let gated = transition_caveat(
        "transition:issue-1/approve",
        "approve",
        "ge",
        "approver_count",
        Literal::Int(2),
        &[("approver_count", Literal::Int(1))],
    );
    assert_eq!(
        eval_caveat(&gated),
        Decision::Deny,
        "a transition lacking the approver edge is gated (Deny)"
    );

    let missing = transition_caveat(
        "transition:issue-1/approve",
        "approve",
        "ge",
        "approver_count",
        Literal::Int(2),
        &[],
    );
    assert_eq!(
        eval_caveat(&missing),
        Decision::Conditional,
        "a transition caveat needing missing context is Conditional, never a silent allow (§8.6)"
    );
    assert!(
        missing.transition.is_some() && missing.field.is_none(),
        "it is a TRANSITION caveat (transition? set, field? unset)"
    );
}
