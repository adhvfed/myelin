//! # The CDC pair for contract 4.7 — the SELF-HOSTED SCOPE *against the live CI fragment*
//! (CI-dispatch consumer ↔ Identity mint+check provider), P-ID-28 / global P-321.
//!
//! **Contract-index row 4.7** (`mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token`)
//! — Identity, §4: *"the self-hosted-runner token is scoped to **one tenant's `SelfHosted` jobs**
//! (cannot mint cross-tenant)"* — exercised AGAINST the contract-4.9 **CI fragment** (row 4.9). The
//! mint-half CDC (`cdc_4_7_mint_run_token.rs`, P-ID-18) pins the mint ceiling in isolation; THIS pair
//! pins the END-TO-END agreement the P-ID-28 deliverable names: a **CI-dispatch consumer** mints a
//! self-hosted run token (own-tenant scope only) and dispatches a run, and the **Identity provider**
//! both (a) refuses any cross-tenant self-hosted mint AND (b) denies that runner's `check` against any
//! OTHER tenant's CI `run`/`secret` object — so a compromised runner is bounded to one tenant's
//! `SelfHosted` jobs, **0 cross-tenant job/secret reads**.
//!
//! ## What this pair pins (the CI-dispatch ↔ Identity agreement of 4.7's self-hosted scope on CI)
//!
//! **The CI-dispatch CONSUMER** mints a per-run token through the contract-4.7 mint surface with
//! [`MachineKind::PerJob`] carrying ONLY its own tenant's `selfhosted:<tenant>` grant, then runs the
//! dispatched job AS the runner machine principal (the run token's subject). It never names another
//! tenant's scope.
//!
//! **The Identity PROVIDER** ([`StoreBackedCheck`]) (a) applies the self-hosted one-tenant CEILING at
//! the mint ([`MintError::SelfHostedScopeViolation`] on a cross-tenant grant), and (b) resolves the
//! runner principal's `check` against the live (admitted) CI fragment under the runner's OWN verified
//! `(tenant, region)` scope (tenant-from-token, ID-3) — so a cross-tenant CI `run`/`secret` read finds
//! no grant and DENIES.
//!
//! The agreement: the SAME `selfhosted:<tenant>` ceiling flows CI-dispatch consumer → caveat →
//! Identity mint, the provider refuses a cross-tenant grant, AND the provider's CI-fragment `check`
//! denies the runner cross-tenant — so a token for tenant A can never act on tenant B's CI. A change
//! to either side fails this test in the same CI job.

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

/// **THE CI-DISPATCH CONSUMER (contract 4.7 — "consumed by … CI dispatch").** Mints a per-run token
/// for an attested self-hosted runner of `tenant` through the provider's mint surface, carrying ONLY
/// the own-tenant `selfhosted:<tenant>` grant. Returns the minted token (the dispatched run runs AS
/// the runner machine principal, with this token as its subject). The consumer NEVER names another
/// tenant's scope.
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

/// Admit the Git + CI fragments (the CI `run` inheritance edges terminate on the Git `repo` fragment).
fn admit_git_and_ci(svc: &StoreBackedCheck) {
    for a in svc.admit_git_fragment() {
        assert!(matches!(a, myelin_identity::FragmentAdmit::Admitted { .. }));
    }
    for a in svc.admit_ci_fragment() {
        assert!(matches!(a, myelin_identity::FragmentAdmit::Admitted { .. }));
    }
}

/// **The pair PINS: the CI-dispatch consumer mints an own-tenant self-hosted token (the provider
/// accepts it), and the SAME token's runner is denied any cross-tenant CI object by the provider's
/// CI-fragment check.**
#[test]
fn ci_dispatch_self_hosted_token_is_bounded_to_its_tenants_ci() {
    let store = TupleStore::new(OutboxStore::new());

    // Seed globex's CI objects (legitimate globex grants) in globex's partition.
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

    // CONSUMER: the CI-dispatch mints an own-tenant (acme) self-hosted token — the PROVIDER accepts it.
    let acme = scope_of(&runner("acme", "svc:runner-acme"));
    let token = ci_dispatch_mints_self_hosted(&svc, &acme, "svc:runner-acme", "run-1", "acme")
        .expect("the CI-dispatch consumer mints an own-tenant self-hosted token");
    // MR-012: the minted token is a REAL signed PASETO token; read its grants via the verify
    // round-trip through the provider's cell trust anchor, not a plaintext substring.
    let minted = svc
        .introspect_run_token_at("per_job", &token, &Timestamp("2026-06-22T00:00:01Z".into()))
        .expect("a minted self-hosted token verifies through the real cell trust anchor (MR-012)")
        .authority;
    assert!(
        minted.holds("selfhosted:acme") && !minted.grants().any(|g| g.contains("globex")),
        "the minted token carries ONLY the own-tenant SelfHosted grant"
    );

    // PROVIDER (check against the CI fragment): the acme runner cannot view/read globex's run or
    // read globex's secret — every cross-tenant CI read denies (tenant-from-token scope).
    let acme_runner = runner("acme", "svc:runner-acme");
    assert!(!allows(&svc, &acme_runner, CI_VIEW, "run:globex-deploy"));
    assert!(!allows(&svc, &acme_runner, CI_READ, "run:globex-deploy"));
    assert!(!allows(&svc, &acme_runner, CI_READ, "secret:globex-db-pw"));
}

/// **The pair PINS: a CROSS-TENANT self-hosted mint is REFUSED at the provider's ceiling.** Were the
/// CI-dispatch consumer to name another tenant's scope (a fork-attempt / misconfiguration), the
/// provider's one-tenant ceiling refuses it — the two layers agree, the consumer can never dispatch a
/// cross-tenant runner job.
#[test]
fn provider_refuses_a_cross_tenant_self_hosted_dispatch() {
    let svc = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));
    let acme = scope_of(&runner("acme", "svc:runner-acme"));

    // The mint scope is acme's; naming globex's SelfHosted scope is refused by the ceiling.
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

/// **The pair PINS: the scope BOUNDS, it does not BLIND — the runner reads its OWN tenant's CI.** The
/// own-tenant self-hosted token's runner views its own run and reads its own (directly-granted)
/// secret. (The cross-tenant denial above is the scope; this is the legitimate path it preserves.)
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

    // The consumer mints the own-tenant token (the provider accepts), and the runner reads its OWN CI.
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
