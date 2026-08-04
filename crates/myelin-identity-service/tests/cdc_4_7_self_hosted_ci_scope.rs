use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, DelegationCaveats, FailStaticBound, IdentityService,
    ObjectId, Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, RunId,
    RunToken, RuntimeRef, TupleDelta, Zookie,
};
use myelin_identity_service::{
    Authority, DelegationInput, MachineKind, MintError, StoreBackedCheck, TupleStore, CI_READ,
    CI_VIEW, SECRET_DIRECT_READER,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn region() -> Region {
    Region("eu-west".into())
}

fn runner(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("self-hosted-rt".into()),
            on_behalf_of: None,
        },
        TenantId(tenant.into()),
    );
    p.region = region();
    p
}

fn human(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = region();
    p
}

fn scope_of(p: &Principal) -> TenantScope {
    TenantScope::from_verified_token(p, p.region.clone())
}

fn auth(g: &[&str]) -> Authority {
    Authority::of(g.iter().copied())
}

fn input_all(g: &[&str]) -> DelegationInput {
    DelegationInput {
        agent_policy: auth(g),
        delegation: auth(g),
        tenant_policy: auth(g),
        trigger_actor_held: auth(g),
    }
}

fn caveats(g: &[&str]) -> DelegationCaveats {
    DelegationCaveats(g.iter().map(|s| s.to_string()).collect())
}

fn ttl(secs: u64) -> FailStaticBound {
    FailStaticBound {
        static_max_secs: secs,
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

fn ci_dispatch_mints_self_hosted(
    svc: &StoreBackedCheck,
    scope: &TenantScope,
    runner_id: &str,
    run_id: &str,
    tenant: &str,
) -> Result<RunToken, MintError> {
    let grant = format!("selfhosted:{tenant}");
    svc.mint_run_token_in(
        scope,
        &PrincipalId(runner_id.into()),
        &RunId(run_id.into()),
        &runner(tenant, runner_id),
        &human(tenant, "p:dispatch-trigger"),
        &input_all(&[&grant]),
        &caveats(&[&grant]),
        MachineKind::PerJob,
        &ttl(300),
        &Timestamp("2026-06-22T00:00:00Z".into()),
    )
}

fn admit_git_and_ci(svc: &StoreBackedCheck) {
    for a in svc.admit_git_fragment() {
        assert!(matches!(a, myelin_identity::FragmentAdmit::Admitted { .. }));
    }
    for a in svc.admit_ci_fragment() {
        assert!(matches!(a, myelin_identity::FragmentAdmit::Admitted { .. }));
    }
}

#[test]
fn ci_dispatch_self_hosted_token_is_bounded_to_its_tenants_ci() {
    let store = TupleStore::new(OutboxStore::new());

    let globex = scope_of(&human("globex", "p-globex-admin"));
    store
        .write_tuples(
            &globex,
            &human("globex", "p-globex-admin"),
            &[
                add("run:globex-deploy", "parent_repo", "repo:globex-infra#pull"),
                add("repo:globex-infra", "reader", "p:globex-eng"),
                add(
                    "secret:globex-db-pw",
                    SECRET_DIRECT_READER,
                    "p:globex-deployer",
                ),
            ],
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed globex CI");

    let svc = StoreBackedCheck::new(store);
    admit_git_and_ci(&svc);

    let acme = scope_of(&runner("acme", "svc:runner-acme"));
    let token = ci_dispatch_mints_self_hosted(&svc, &acme, "svc:runner-acme", "run-1", "acme")
        .expect("the CI-dispatch consumer mints an own-tenant self-hosted token");
    let minted = svc
        .introspect_run_token_at("per_job", &token, &Timestamp("2026-06-22T00:00:01Z".into()))
        .expect("a minted self-hosted token verifies through the real cell trust anchor (MR-012)")
        .authority;
    assert!(
        minted.holds("selfhosted:acme") && !minted.grants().any(|g| g.contains("globex")),
        "the minted token carries ONLY the own-tenant SelfHosted grant"
    );

    let acme_runner = runner("acme", "svc:runner-acme");
    assert!(!allows(&svc, &acme_runner, CI_VIEW, "run:globex-deploy"));
    assert!(!allows(&svc, &acme_runner, CI_READ, "run:globex-deploy"));
    assert!(!allows(&svc, &acme_runner, CI_READ, "secret:globex-db-pw"));
}

#[test]
fn provider_refuses_a_cross_tenant_self_hosted_dispatch() {
    let svc = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));
    let acme = scope_of(&runner("acme", "svc:runner-acme"));

    let r = svc.mint_run_token_in(
        &acme,
        &PrincipalId("svc:runner-acme".into()),
        &RunId("run-x".into()),
        &runner("acme", "svc:runner-acme"),
        &human("acme", "p:dispatch-trigger"),
        &input_all(&["selfhosted:globex"]),
        &caveats(&["selfhosted:globex"]),
        MachineKind::PerJob,
        &ttl(300),
        &Timestamp("2026-06-22T00:00:00Z".into()),
    );
    assert!(
        matches!(r, Err(MintError::SelfHostedScopeViolation(_))),
        "the provider's self-hosted ceiling refuses a grant outside the own-tenant SelfHosted scope"
    );
}

#[test]
fn own_tenant_runner_reads_its_own_ci() {
    let store = TupleStore::new(OutboxStore::new());
    let acme = scope_of(&runner("acme", "svc:runner-acme"));
    store
        .write_tuples(
            &acme,
            &human("acme", "p-acme-admin"),
            &[
                add("run:acme-deploy", "parent_repo", "repo:acme-app#pull"),
                add("repo:acme-app", "reader", "svc:runner-acme"),
                add("secret:acme-db-pw", SECRET_DIRECT_READER, "svc:runner-acme"),
            ],
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed acme CI");

    let svc = StoreBackedCheck::new(store);
    admit_git_and_ci(&svc);

    let _token = ci_dispatch_mints_self_hosted(&svc, &acme, "svc:runner-acme", "run-1", "acme")
        .expect("own-tenant mint");
    let acme_runner = runner("acme", "svc:runner-acme");
    assert!(
        allows(&svc, &acme_runner, CI_VIEW, "run:acme-deploy"),
        "the runner views its OWN tenant's run (the scope bounds, it does not blind)"
    );
    assert!(
        allows(&svc, &acme_runner, CI_READ, "secret:acme-db-pw"),
        "the runner reads its OWN tenant's directly-granted secret (the legitimate path)"
    );
}
