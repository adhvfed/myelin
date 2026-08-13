use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, ObjectId, Permission,
    Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    git_fragment, ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
    APPROVE_UNTRUSTED_CI, PROTECTED_PUSH,
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
    if !tuples.is_empty() {
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
    }
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    let svc = StoreBackedCheck::with_index(store, index);
    for admit in svc.admit_git_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "Id's compiled Git fragment admits: {admit:?}"
        );
    }
    svc
}

#[test]
fn cdc_4_9_id_compiled_git_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);
    let ns = svc.namespace();
    for ty in ["repo", "ref", "pull_request", "pr_comment"] {
        assert!(
            ns.object_types().contains(&ty.to_string()),
            "`{ty}` admitted into the cell schema"
        );
    }
    assert!(ns.resolve_permission("repo", PROTECTED_PUSH).is_some());
    assert!(ns.resolve_permission("pull_request", "merge").is_some());
}

#[test]
fn cdc_4_9_git_rewrites_resolve_through_the_engine() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("repo:core", "admin", "p:alice"),
            add("repo:core", "parent_project", "project:web#view"),
            add("project:web", "reader", "p:carol"),
        ],
    );
    let repo = ArtifactRef("repo:core".into());
    let can = |actor: &Principal, p: &str| {
        matches!(
            svc.check(actor, &Permission(p.into()), &repo, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can(&subject("p:alice"), "pull"),
        "a direct repo admin pulls"
    );
    assert!(
        can(&subject("p:carol"), "pull"),
        "a project member inherits repo pull (parent_project->view)"
    );
    assert!(
        !can(&subject("p:bob"), "pull"),
        "an outsider cannot pull (fail-closed)"
    );
    assert!(
        can(&subject("p:alice"), PROTECTED_PUSH),
        "admin → protected_push"
    );
    assert!(
        !can(&subject("p:carol"), PROTECTED_PUSH),
        "a project reader does NOT get protected_push (admin-only, §5)"
    );
}

#[test]
fn cdc_4_9_pull_request_merge_via_protected_push() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("repo:core", "admin", "p:alice"),
            add("pull_request:42", "parent_repo", "repo:core#protected_push"),
            add("repo:core", "writer", "p:bob"),
        ],
    );
    let pr = ArtifactRef("pull_request:42".into());
    let can_merge = |actor: &Principal| {
        matches!(
            svc.check(actor, &Permission("merge".into()), &pr, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can_merge(&subject("p:alice")),
        "a repo admin can merge (parent_repo->protected_push)"
    );
    assert!(
        !can_merge(&subject("p:bob")),
        "a writer cannot merge (protected_push is admin-only, §5)"
    );
}

#[test]
fn cdc_4_9_approve_untrusted_ci_is_a_plain_relation_check() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[add("repo:core", APPROVE_UNTRUSTED_CI, "p:maintainer")],
    );
    let repo = ArtifactRef("repo:core".into());
    let endorse = |actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission(APPROVE_UNTRUSTED_CI.into()),
                &repo,
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    assert!(
        endorse(&subject("p:maintainer")),
        "a maintainer endorses an untrusted fork run"
    );
    assert!(
        !endorse(&subject("p:bob")),
        "an outsider cannot endorse (X-1, fail-closed)"
    );
}

#[test]
fn cdc_4_9_codeowners_glob_compiles_to_resolvable_reviewer_tuples() {
    let s = scope("acme");
    let rules = vec![git_fragment::CodeownersRule {
        path_glob: "/src/payments/**".into(),
        owners: vec![
            PrincipalId("p:alice".into()),
            PrincipalId("team:payments".into()),
        ],
    }];
    let tuples = git_fragment::compile_codeowners("repo:core", &rules);
    assert_eq!(tuples.len(), 2, "two owners → two code_owner tuples");

    let deltas: Vec<TupleDelta> = tuples.iter().cloned().map(TupleDelta::Add).collect();
    let svc = provider(&s, &deltas);

    let ref_obj = ObjectId("ref:repo:core::/src/payments/**".into());
    let owners = svc
        .list_subjects_in(&s, &ref_obj, &Permission("code_owner".into()), &at_latest())
        .expect("read code-owner relationships");
    let members: Vec<&str> = owners.members.iter().map(|m| m.0.as_str()).collect();
    assert!(
        members.contains(&"p:alice"),
        "alice is a required reviewer for the path"
    );
    assert!(
        members.contains(&"team:payments"),
        "team:payments is a required reviewer for the path"
    );
}
