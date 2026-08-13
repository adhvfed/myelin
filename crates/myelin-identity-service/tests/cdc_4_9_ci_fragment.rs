use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, ObjectId, Permission,
    Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore, CI_READ, CI_TRIGGER, CI_VIEW,
    IS_UNTRUSTED_FORK, SECRET_DIRECT_READER,
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
        assert!(matches!(admit, FragmentAdmit::Admitted { .. }));
    }
    for admit in svc.admit_ci_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "Id's compiled CI fragment admits: {admit:?}"
        );
    }
    svc
}

#[test]
fn cdc_4_9_id_compiled_ci_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);
    let ns = svc.namespace();
    for ty in ["ci_project", "environment", "secret", "run"] {
        assert!(
            ns.object_types().contains(&ty.to_string()),
            "`{ty}` admitted into the cell schema"
        );
    }
    assert!(ns.resolve_permission("secret", CI_READ).is_some());
    assert!(ns.resolve_permission("run", CI_READ).is_some());
    assert!(ns.resolve_permission("run", CI_VIEW).is_some());
}

#[test]
fn cdc_4_9_run_view_and_trigger_inherit_the_repo() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("repo:core", "reader", "p:alice"),
            add("repo:core", "writer", "p:bob"),
            add("run:99", "parent_repo", "repo:core#pull"),
            add("run:99", "parent_repo", "repo:core#push"),
        ],
    );
    let run = ArtifactRef("run:99".into());
    let can = |actor: &Principal, p: &str| {
        matches!(
            svc.check(actor, &Permission(p.into()), &run, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can(&subject("p:alice"), CI_VIEW),
        "a repo puller views the run (run.view = parent_repo->pull)"
    );
    assert!(
        can(&subject("p:bob"), CI_TRIGGER),
        "a repo pusher triggers the run (run.trigger = parent_repo->push)"
    );
    assert!(
        !can(&subject("p:carol"), CI_VIEW),
        "an outsider cannot view the run (fail-closed)"
    );
    assert!(
        !can(&subject("p:alice"), CI_TRIGGER),
        "a mere puller cannot trigger (push-only, §5)"
    );
}

#[test]
fn cdc_4_9_secret_read_is_not_inherited_from_the_project() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("ci_project:web", "admin", "p:alice"),
            add(
                "secret:db-password",
                "parent_ci_project",
                "ci_project:web#view",
            ),
            add("secret:db-password", SECRET_DIRECT_READER, "p:bob"),
        ],
    );
    let secret = ArtifactRef("secret:db-password".into());
    let can_read = |actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission(CI_READ.into()),
                &secret,
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    assert!(
        matches!(
            svc.check(
                &subject("p:alice"),
                &Permission("administer".into()),
                &ArtifactRef("ci_project:web".into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        ),
        "alice administers the CI project (the project edge resolves)"
    );
    assert!(
        !can_read(&subject("p:alice")),
        "a CI-project ADMIN cannot read a secret via project inheritance (CI-1 secret-non-inheritance)"
    );
    assert!(
        can_read(&subject("p:bob")),
        "a DIRECT secret#direct_reader grant reads the secret (the only path, CI-1)"
    );
    assert!(
        !can_read(&subject("p:carol")),
        "an outsider cannot read the secret (fail-closed)"
    );
}

#[test]
fn cdc_4_9_run_read_is_gated_by_the_is_untrusted_fork_edge() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("repo:core", "reader", "p:alice"),
            add("run:trusted", "parent_repo", "repo:core#pull"),
            add("run:fork", "parent_repo", "repo:core#pull"),
            add("run:fork", IS_UNTRUSTED_FORK, "p:alice"),
        ],
    );
    let trusted = ArtifactRef("run:trusted".into());
    let fork = ArtifactRef("run:fork".into());
    let chk = |actor: &Principal, obj: &ArtifactRef, p: &str| {
        matches!(
            svc.check(actor, &Permission(p.into()), obj, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(
        chk(&subject("p:alice"), &trusted, CI_VIEW),
        "views trusted run"
    );
    assert!(
        chk(&subject("p:alice"), &trusted, CI_READ),
        "reads a trusted run's output (view − ∅)"
    );
    assert!(
        chk(&subject("p:alice"), &fork, CI_VIEW),
        "views the fork run (run.view is unconditional)"
    );
    assert!(
        !chk(&subject("p:alice"), &fork, CI_READ),
        "an untrusted-fork run's output is gated by construction (read = view − is_untrusted_fork, C7)"
    );
}
