use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, ObjectType, Permission, Principal, PrincipalId,
    PrincipalKind, RelName, RelationTuple, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_identity_service::{
    namespace::{FragmentDef, PermissionRule, Userset},
    ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
};
use myelin_issues::governance::{PermissionInspector, PermissionResolver};
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
            Timestamp("2026-06-24T00:00:00Z".into()),
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
        object_type: ObjectType("issue".into()),
        relations: vec![RelName("approver".into()), RelName("lead".into())],
        permissions: vec![PermissionRule {
            permission: Permission("approve".into()),
            rewrite: Userset::Union(vec![
                Userset::Relation(RelName("approver".into())),
                Userset::Relation(RelName("lead".into())),
            ]),
        }],
    });
    svc
}

struct IdentityExpandResolver {
    svc: StoreBackedCheck,
    scope: TenantScope,
}

impl PermissionResolver for IdentityExpandResolver {
    fn list_subjects(
        &self,
        object: &ObjectId,
        permission: &Permission,
        at: &Consistency,
    ) -> SubjectTree {
        self.svc
            .list_subjects_in(&self.scope, object, permission, at)
            .expect("read relationships for the inspector test adapter")
    }

    fn explain(
        &self,
        subject: &PrincipalId,
        permission: &Permission,
        object: &ObjectId,
        at: &Consistency,
    ) -> RewriteTrace {
        self.svc
            .explain_in(&self.scope, subject, permission, object, at)
            .expect("read relationships for the inspector explanation adapter")
    }
}

#[test]
fn cdc_4_4_inspector_membership_equals_list_subjects() {
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
    let object = ObjectId("issue:PROJ-1".into());
    let perm = Permission("approve".into());
    let at = at_latest();

    let tree = svc
        .list_subjects_in(&s, &object, &perm, &at)
        .expect("read issue approval relationships");

    let inspector = PermissionInspector::new(IdentityExpandResolver {
        svc,
        scope: s.clone(),
    });
    let answer = inspector.who_can(&object, &perm, &at);

    assert_eq!(
        answer.members, tree.members,
        "the inspector's membership must EQUAL Identity's list_subjects (0 private recompute)"
    );
    assert_eq!(
        answer.members,
        vec![
            PrincipalId("p:alice".into()),
            PrincipalId("p:bob".into()),
            PrincipalId("p:carol".into()),
        ],
        "exactly the approve membership (PROJ-2's approver absent - leak-free)"
    );
    assert_eq!(answer.object, object);
    assert_eq!(answer.permission, perm);
}

#[test]
fn cdc_4_4_inspector_why_equals_explain() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            grant("issue:PROJ-1", "approver", "p:alice"),
            grant("issue:PROJ-1", "lead", "p:carol"),
        ],
    );
    let object = ObjectId("issue:PROJ-1".into());
    let perm = Permission("approve".into());
    let at = at_latest();

    let alice = PrincipalId("p:alice".into());
    let mallory = PrincipalId("p:mallory".into());

    let provider_allow = svc
        .explain_in(&s, &alice, &perm, &object, &at)
        .expect("read relationships for the allow explanation");
    let provider_deny = svc
        .explain_in(&s, &mallory, &perm, &object, &at)
        .expect("read relationships for the deny explanation");

    let inspector = PermissionInspector::new(IdentityExpandResolver {
        svc,
        scope: s.clone(),
    });

    let inspector_allow = inspector.why(&alice, &perm, &object, &at);
    let inspector_deny = inspector.why(&mallory, &perm, &object, &at);

    assert_eq!(
        inspector_allow.steps, provider_allow.steps,
        "the inspector's 'why' trace must EQUAL Identity's explain (0 private recompute)"
    );
    assert_eq!(
        inspector_deny.steps, provider_deny.steps,
        "the inspector's 'why' trace must EQUAL Identity's explain for a denied subject too"
    );

    assert!(
        !inspector_allow.steps.is_empty()
            && inspector_allow.steps.last().unwrap().starts_with("ALLOW"),
        "a granted subject's trace ends in ALLOW"
    );
    assert!(
        !inspector_deny.steps.is_empty()
            && inspector_deny.steps.last().unwrap().starts_with("DENY"),
        "a denied subject's trace ends in DENY (never empty, never a silent allow)"
    );
}
