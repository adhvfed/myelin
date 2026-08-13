use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, ObjectType, Permission, Principal, PrincipalId,
    PrincipalKind, RelName, RelationTuple, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_identity_service::{
    namespace::{FragmentDef, PermissionRule, Userset},
    ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore, WATCHER_RELATION,
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
    let _ = svc.admit_fragment_def(
        &FragmentDef {
            object_type: ObjectType("issue".into()),
            relations: vec![RelName("approver".into()), RelName("lead".into())],
            permissions: vec![PermissionRule {
                permission: Permission("approve".into()),
                rewrite: Userset::Union(vec![
                    Userset::Relation(RelName("approver".into())),
                    Userset::Relation(RelName("lead".into())),
                ]),
            }],
        }
        .watchable(),
    );
    svc
}

fn admin_inspector_renders(tree: &SubjectTree) -> Vec<String> {
    let mut out: Vec<String> = tree.members.iter().map(|m| m.0.clone()).collect();
    out.sort();
    out
}

fn hitl_approver_set_admits(tree: &SubjectTree, candidate: &str) -> bool {
    tree.members.iter().any(|m| m.0 == candidate)
}

fn inspector_renders_trace(trace: &RewriteTrace) -> Vec<String> {
    trace.steps.clone()
}

#[test]
fn cdc_4_4_list_subjects_flattens_membership_inspector_renders_it() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            grant("issue:PROJ-1", "approver", "p:alice"),
            grant("issue:PROJ-1", "approver", "p:bob"),
            grant("issue:PROJ-1", "lead", "p:carol"),
            grant("issue:PROJ-2", "approver", "p:dave"),
        ],
    );
    let tree = svc
        .list_subjects_in(
            &s,
            &ObjectId("issue:PROJ-1".into()),
            &Permission("approve".into()),
            &at_latest(),
        )
        .expect("read issue approval relationships");
    let rendered = admin_inspector_renders(&tree);
    assert_eq!(
        rendered,
        vec!["p:alice".to_string(), "p:bob".into(), "p:carol".into()],
        "the inspector renders exactly the approve membership (leak-free - PROJ-2's approver absent)"
    );
}

#[test]
fn cdc_4_4_hitl_approver_set_admits_only_members() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            grant("issue:PROJ-1", "approver", "p:alice"),
            grant("issue:PROJ-1", "lead", "p:carol"),
        ],
    );
    let tree = svc
        .list_subjects_in(
            &s,
            &ObjectId("issue:PROJ-1".into()),
            &Permission("approve".into()),
            &at_latest(),
        )
        .expect("read HITL approver relationships");
    assert!(
        hitl_approver_set_admits(&tree, "p:alice"),
        "an approver may approve (in the approver set)"
    );
    assert!(
        hitl_approver_set_admits(&tree, "p:carol"),
        "a lead may approve (the ∪ lead arm)"
    );
    assert!(
        !hitl_approver_set_admits(&tree, "p:mallory"),
        "a non-approver may NOT approve (leak-free - never in the set)"
    );
}

#[test]
fn cdc_4_4_explain_trace_is_non_empty_and_correct() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("issue:PROJ-1", "approver", "p:alice")]);

    let allow_trace = svc
        .explain_in(
            &s,
            &PrincipalId("p:alice".into()),
            &Permission("approve".into()),
            &ObjectId("issue:PROJ-1".into()),
            &at_latest(),
        )
        .expect("read relationships for the allow explanation");
    let rendered = inspector_renders_trace(&allow_trace);
    assert!(
        !rendered.is_empty(),
        "the inspector renders a non-empty trace"
    );
    assert!(
        rendered.last().unwrap().starts_with("ALLOW"),
        "an approver's trace ends in ALLOW: {rendered:?}"
    );

    let deny_trace = svc
        .explain_in(
            &s,
            &PrincipalId("p:mallory".into()),
            &Permission("approve".into()),
            &ObjectId("issue:PROJ-1".into()),
            &at_latest(),
        )
        .expect("read relationships for the deny explanation");
    assert!(
        deny_trace.steps.last().unwrap().starts_with("DENY"),
        "a non-approver's trace ends in DENY (never a silent allow): {:?}",
        deny_trace.steps
    );
}

#[test]
fn cdc_4_4_notif_read_fanout_flattens_the_watcher_set() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            grant("issue:PROJ-1", WATCHER_RELATION, "p:alice"),
            grant("issue:PROJ-1", WATCHER_RELATION, "p:bob"),
            grant("issue:PROJ-1", "approver", "p:carol"),
            grant("issue:PROJ-2", WATCHER_RELATION, "p:dave"),
        ],
    );
    let tree = svc
        .list_watchers_in(&s, &ObjectId("issue:PROJ-1".into()), &at_latest())
        .expect("read issue watcher relationships");
    assert_eq!(
        tree.relation,
        RelName(WATCHER_RELATION.into()),
        "the fanout expands the watcher relation"
    );
    let watchers = admin_inspector_renders(&tree);
    assert_eq!(
        watchers,
        vec!["p:alice".to_string(), "p:bob".into()],
        "the Notif fanout delivers to exactly the watchers (carol does not watch; PROJ-2's dave absent)"
    );
}

#[test]
fn cdc_4_4_no_cross_tenant_membership() {
    let acme = scope("acme");
    let svc = provider(&acme, &[grant("issue:PROJ-1", "approver", "p:alice")]);
    let globex = scope("globex");
    let tree = svc
        .list_subjects_in(
            &globex,
            &ObjectId("issue:PROJ-1".into()),
            &Permission("approve".into()),
            &at_latest(),
        )
        .expect("read the other tenant's relationship partition");
    assert!(
        admin_inspector_renders(&tree).is_empty(),
        "0 cross-tenant approvers - a globex inspector sees none of acme's membership"
    );
}
