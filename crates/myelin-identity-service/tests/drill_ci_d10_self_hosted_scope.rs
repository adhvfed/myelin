use myelin_events::{OutboxStore, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, DelegationCaveats, FailStaticBound, IdentityService,
    ObjectId, Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, RunId,
    RuntimeRef, TupleDelta, Zookie,
};
use myelin_identity_service::{
    Authority, DelegationInput, MachineKind, MintError, StoreBackedCheck, TupleStore, CI_READ,
    CI_VIEW, SECRET_DIRECT_READER, SELFHOSTED_GRANT_PREFIX,
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

fn auth(grants: &[&str]) -> Authority {
    Authority::of(grants.iter().copied())
}

fn input_all(grants: &[&str]) -> DelegationInput {
    DelegationInput {
        agent_policy: auth(grants),
        delegation: auth(grants),
        tenant_policy: auth(grants),
        trigger_actor_held: auth(grants),
    }
}

fn ttl(secs: u64) -> FailStaticBound {
    FailStaticBound {
        static_max_secs: secs,
    }
}

fn caveats(grants: &[&str]) -> DelegationCaveats {
    DelegationCaveats(grants.iter().map(|s| s.to_string()).collect())
}

fn admit_git_and_ci(svc: &StoreBackedCheck) {
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
}

#[test]
fn ci_d10_self_hosted_mint_cannot_mint_cross_tenant() {
    let mut signals = SignalSource::new();
    let acme = scope_of(&runner("acme", "svc:runner-acme"));
    let svc = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));

    let own = svc
        .mint_run_token_in(
            &acme,
            &PrincipalId("svc:runner-acme".into()),
            &RunId("run-acme-1".into()),
            &runner("acme", "svc:runner-acme"),
            &human("acme", "p:trigger"),
            &input_all(&["selfhosted:acme"]),
            &caveats(&["selfhosted:acme"]),
            MachineKind::PerJob,
            &ttl(300),
            &Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("an own-tenant self-hosted run token mints (the scope bounds, it does not blind)");
    let own_authority = svc
        .introspect_run_token_at("per_job", &own, &Timestamp("2026-06-22T00:00:01Z".into()))
        .expect(
            "an own-tenant self-hosted token verifies through the real cell trust anchor (MR-012)",
        )
        .authority;
    assert!(
        own_authority.holds(&format!("{SELFHOSTED_GRANT_PREFIX}acme")),
        "the minted token carries ONLY the own-tenant SelfHosted grant"
    );
    assert!(
        !own_authority.grants().any(|g| g.contains("globex")),
        "the minted token never names another tenant's scope"
    );

    let mut cross_tenant_tokens_minted: i64 = 0;
    let attacks: &[&[&str]] = &[
        &["selfhosted:globex"],
        &["selfhosted:initech"],
        &["selfhosted:acme", "selfhosted:globex"],
        &["repo:globex/secret#read"],
    ];
    for (i, grants) in attacks.iter().enumerate() {
        let r = svc.mint_run_token_in(
            &acme,
            &PrincipalId("svc:runner-acme".into()),
            &RunId(format!("run-attack-{i}")),
            &runner("acme", "svc:runner-acme"),
            &human("acme", "p:trigger"),
            &input_all(grants),
            &caveats(grants),
            MachineKind::PerJob,
            &ttl(300),
            &Timestamp("2026-06-22T00:00:00Z".into()),
        );
        match r {
            Err(MintError::SelfHostedScopeViolation(_)) => {}
            Ok(_) => cross_tenant_tokens_minted += 1,
            Err(other) => panic!(
                "a cross-tenant self-hosted mint must fail with SelfHostedScopeViolation, got \
                 {other:?} for grants {grants:?}"
            ),
        }
    }

    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_tokens_minted);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        cross_tenant_tokens_minted, 0,
        "0 cross-tenant self-hosted-runner tokens minted (the no-global-pool ceiling, recon §1, C6)"
    );

    println!(
        "[P-321 DRILL GREEN 2026-06-22] CI-D10 (scope side) mint ceiling: a PerJob (self-hosted) \
         runner token for tenant=acme mints ONLY `selfhosted:acme`; {} cross-tenant mint attempts \
         (selfhosted:globex / selfhosted:initech / own+cross widen / a non-selfhosted cross grant) \
         → ALL refused (SelfHostedScopeViolation) → cross-tenant-tokens-minted count=0 \
         (the no-global-pool property at the identity layer, recon §1 / id&access §4, C6)",
        attacks.len()
    );
}

#[test]
fn ci_d10_self_hosted_runner_zero_cross_tenant_ci_reads() {
    let mut signals = SignalSource::new();
    let store = TupleStore::new(OutboxStore::new());

    let globex = scope_of(&human("globex", "p-globex-admin"));
    let globex_tuples: Vec<TupleDelta> = vec![
        add("run:globex-deploy", "parent_repo", "repo:globex-infra#pull"),
        add("repo:globex-infra", "reader", "p:globex-eng"),
        add(
            "secret:globex-db-pw",
            "parent_ci_project",
            "ci_project:globex-web#view",
        ),
        add(
            "secret:globex-db-pw",
            SECRET_DIRECT_READER,
            "p:globex-deployer",
        ),
    ];
    store
        .write_tuples(
            &globex,
            &human("globex", "p-globex-admin"),
            &globex_tuples,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed globex CI grants");

    let acme = scope_of(&runner("acme", "svc:runner-acme"));
    let acme_tuples: Vec<TupleDelta> = vec![
        add("run:acme-deploy", "parent_repo", "repo:acme-app#pull"),
        add("repo:acme-app", "reader", "svc:runner-acme"),
        add(
            "secret:acme-db-pw",
            "parent_ci_project",
            "ci_project:acme-web#view",
        ),
        add("secret:acme-db-pw", SECRET_DIRECT_READER, "svc:runner-acme"),
    ];
    store
        .write_tuples(
            &acme,
            &human("acme", "p-acme-admin"),
            &acme_tuples,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed acme CI grants");

    let svc = StoreBackedCheck::new(store);
    admit_git_and_ci(&svc);

    assert!(
        allows(
            &svc,
            &human("globex", "p:globex-eng"),
            CI_VIEW,
            "run:globex-deploy"
        ),
        "a legitimate globex viewer views globex's run (the globex grants are live)"
    );
    let acme_runner = runner("acme", "svc:runner-acme");
    assert!(
        allows(&svc, &acme_runner, CI_VIEW, "run:acme-deploy"),
        "the acme self-hosted runner views its OWN tenant's run (the scope bounds, it does not blind)"
    );
    assert!(
        allows(&svc, &acme_runner, CI_READ, "secret:acme-db-pw"),
        "the acme runner reads its OWN tenant's secret via its direct grant (the legitimate path)"
    );

    let cross_tenant_attacks: &[(&str, &str)] = &[
        (CI_VIEW, "run:globex-deploy"),
        (CI_READ, "run:globex-deploy"),
        (CI_READ, "secret:globex-db-pw"),
    ];
    let mut cross_tenant_reads: i64 = 0;
    for (perm, object) in cross_tenant_attacks {
        if allows(&svc, &acme_runner, perm, object) {
            cross_tenant_reads += 1;
        }
    }
    assert_eq!(
        cross_tenant_reads, 0,
        "a tenant-acme self-hosted runner read a tenant-globex CI object - the per-tenant scope \
         FAILED (cross-tenant CI read)"
    );

    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_reads);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        cross_tenant_reads, 0,
        "0 cross-tenant job/secret reads by a self-hosted runner against the live CI fragment (C6)"
    );

    println!(
        "[P-321 DRILL GREEN 2026-06-22] CI-D10 (scope side) check isolation: a tenant=acme \
         self-hosted runner attempted {} cross-tenant CI reads against tenant=globex's live CI \
         fragment (run.view / run.read / secret.read) → ALL denied → cross-tenant-read count=0; \
         the SAME runner reads its OWN tenant's run + secret (the scope bounds to one tenant's \
         SelfHosted jobs, it does not blind it) - check is tenant-from-token (ID-3), S3 has no \
         cross-tenant query path (C6, the no-global-pool property at the identity layer)",
        cross_tenant_attacks.len()
    );
}

#[test]
fn ci_d10_cross_tenant_grant_in_other_partition_is_invisible() {
    let mut signals = SignalSource::new();
    let store = TupleStore::new(OutboxStore::new());

    let globex = scope_of(&human("globex", "p-globex-admin"));
    store
        .write_tuples(
            &globex,
            &human("globex", "p-globex-admin"),
            &[
                add(
                    "secret:globex-db-pw",
                    SECRET_DIRECT_READER,
                    "svc:runner-acme",
                ),
                add("run:globex-deploy", "parent_repo", "repo:globex-infra#pull"),
                add("repo:globex-infra", "reader", "svc:runner-acme"),
            ],
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed the (hypothetical) cross-tenant grant in globex's partition");

    let svc = StoreBackedCheck::new(store);
    admit_git_and_ci(&svc);

    let acme_runner = runner("acme", "svc:runner-acme");
    let mut cross_tenant_reads: i64 = 0;
    for (perm, object) in [
        (CI_READ, "secret:globex-db-pw"),
        (CI_VIEW, "run:globex-deploy"),
        (CI_READ, "run:globex-deploy"),
    ] {
        if allows(&svc, &acme_runner, perm, object) {
            cross_tenant_reads += 1;
        }
    }

    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_reads);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        cross_tenant_reads, 0,
        "a cross-tenant grant in another tenant's partition is invisible to a tenant-from-token \
         scoped check - 0 cross-tenant reads even WITH a (mis-written) cross-tenant grant (C6)"
    );

    println!(
        "[P-321 DRILL GREEN 2026-06-22] CI-D10 (scope side) defence-in-depth: a (hypothetical) \
         cross-tenant grant secret#direct_reader@svc:runner-acme written into GLOBEX's partition is \
         INVISIBLE to the acme-scoped runner's check (tenant-from-token reads acme's partition only) \
         → cross-tenant-read count=0 - the scope, not the grant, is authoritative (no-global-pool)"
    );
}
