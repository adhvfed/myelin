use myelin_events::{OutboxStore, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    StoreBackedCheck, TupleStore, CI_READ, IS_UNTRUSTED_FORK, SECRET_DIRECT_READER,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

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

fn allows(svc: &StoreBackedCheck, actor: &Principal, perm: &str, object: &str) -> bool {
    matches!(
        svc.check(
            actor,
            &Permission(perm.into()),
            &ArtifactRef(object.into()),
            &at_latest(),
            None
        ),
        Ok(Decision::Allow)
    )
}

#[test]
fn ci_d10_secret_read_is_not_reachable_via_project_inheritance() {
    let mut signals = SignalSource::new();
    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    let mut tuples: Vec<TupleDelta> = vec![
        add(
            "secret:db-password",
            "parent_ci_project",
            "ci_project:web#view",
        ),
        add("secret:db-password", SECRET_DIRECT_READER, "p:deployer"),
    ];
    const FLEET: usize = 64;
    for i in 0..FLEET {
        tuples.push(add("ci_project:web", "reader", &format!("p:reader-{i}")));
    }
    tuples.push(add("ci_project:web", "admin", "p:proj-admin"));

    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &tuples,
            None,
            None,
            Timestamp("2026-06-21T00:00:00Z".into()),
        )
        .expect("seed acme CI grants");

    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_git_fragment() {
        assert!(matches!(
            admit,
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
    }
    for admit in svc.admit_ci_fragment() {
        assert!(matches!(
            admit,
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
    }

    assert!(
        allows(
            &svc,
            &principal("acme", "p:proj-admin"),
            "administer",
            "ci_project:web"
        ),
        "a CI-project admin administers the project (the project edge resolves)"
    );
    assert!(
        allows(
            &svc,
            &principal("acme", "p:deployer"),
            CI_READ,
            "secret:db-password"
        ),
        "the direct secret#direct_reader grantee reads the secret (CI-1: the only path)"
    );

    let mut inheritance_leaks: i64 = 0;
    for i in 0..FLEET {
        if allows(
            &svc,
            &principal("acme", &format!("p:reader-{i}")),
            CI_READ,
            "secret:db-password",
        ) {
            inheritance_leaks += 1;
        }
    }
    if allows(
        &svc,
        &principal("acme", "p:proj-admin"),
        CI_READ,
        "secret:db-password",
    ) {
        inheritance_leaks += 1;
    }

    signals.set_scalar(SignalName::CrossTenantCount, inheritance_leaks);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        inheritance_leaks, 0,
        "0 secret reads via ci_project read inheritance (CI-1 secret-non-inheritance, §1)"
    );

    println!(
        "[P-320 DRILL GREEN 2026-06-22] CI-D10 (fragment side) secret-non-inheritance: \
         fleet={FLEET} ci_project readers + 1 admin attempted secret.read on secret:db-password via \
         project inheritance (secret.read = direct_reader, DIRECT NARROW - NOT ∪ parent_ci_project->…) \
         → inheritance-leak count=0; only the direct secret#direct_reader grantee reads it (CI-1, §1)"
    );
}

#[test]
fn ci_d10_is_untrusted_fork_edge_gates_run_read() {
    let mut signals = SignalSource::new();
    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    const READERS: usize = 64;
    let mut tuples: Vec<TupleDelta> = vec![
        add("run:trusted", "parent_repo", "repo:core#pull"),
        add("run:fork", "parent_repo", "repo:core#pull"),
    ];
    for i in 0..READERS {
        let r = format!("p:reader-{i}");
        tuples.push(add("repo:core", "reader", &r));
        tuples.push(add("run:fork", IS_UNTRUSTED_FORK, &r));
    }

    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &tuples,
            None,
            None,
            Timestamp("2026-06-21T00:00:00Z".into()),
        )
        .expect("seed acme run grants");

    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_git_fragment() {
        assert!(matches!(
            admit,
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
    }
    for admit in svc.admit_ci_fragment() {
        assert!(matches!(
            admit,
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
    }

    let reader0 = principal("acme", "p:reader-0");
    assert!(
        allows(&svc, &reader0, "view", "run:fork"),
        "a repo reader views the fork run (run.view is unconditional)"
    );
    assert!(
        allows(&svc, &reader0, CI_READ, "run:trusted"),
        "a repo reader reads the TRUSTED run's output (view − ∅)"
    );

    let mut fork_read_leaks: i64 = 0;
    for i in 0..READERS {
        if allows(
            &svc,
            &principal("acme", &format!("p:reader-{i}")),
            CI_READ,
            "run:fork",
        ) {
            fork_read_leaks += 1;
        }
    }

    signals.set_scalar(SignalName::CrossTenantCount, fork_read_leaks);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        fork_read_leaks, 0,
        "0 fork-run-output reads through the !is_untrusted_fork ABAC edge (C7, §X-1)"
    );

    println!(
        "[P-320 DRILL GREEN 2026-06-22] CI-D10 (fragment side) !is_untrusted_fork edge: \
         readers={READERS} repo pullers attempted run.read on run:fork (stamped is_untrusted_fork) \
         → fork-read-leak count=0 (run.read = run.view − is_untrusted_fork, the Exclusion gates by \
         construction); the SAME readers view the fork run + read the TRUSTED run (the edge gates the \
         fork, not the legitimate path) - CI stamps trust_tier, Identity never recomputes trust (C7)"
    );
}
