use myelin_identity::{DelegationCaveats, FailStaticBound, Principal, RunId};
use myelin_identity_service::delegation::{authority_of, DelegationInput};
use myelin_identity_service::machine_auth::MachineKind;
use myelin_identity_service::mint::{
    attenuate_for_caveats, repository_scope_grant, RunTokenMinter,
};
use myelin_identity_service::ResolvedDelegationPolicy;
use myelin_mcp::{GovernedRun, IssuedGovernedRun};
use myelin_storage::TenantScope;

pub struct TestRunIdentity {
    pub scope: TenantScope,
    pub agent: Principal,
    pub trigger_actor: Principal,
    pub run_id: RunId,
}

impl TestRunIdentity {
    pub fn new(agent: Principal, trigger_actor: Principal, run_id: impl Into<String>) -> Self {
        let scope = TenantScope::from_verified_token(&agent, agent.region.clone());
        Self {
            scope,
            agent,
            trigger_actor,
            run_id: RunId(run_id.into()),
        }
    }
}

pub fn issue_test_run(
    minter: &RunTokenMinter,
    identity: TestRunIdentity,
    input: DelegationInput,
    caveats: DelegationCaveats,
    ttl_secs: u64,
    now: &myelin_events::Timestamp,
) -> IssuedGovernedRun {
    let TestRunIdentity {
        scope,
        agent,
        trigger_actor,
        run_id,
    } = identity;
    let resolved = ResolvedDelegationPolicy::synthetic_for_test(
        run_id.clone(),
        agent.principal_id.clone(),
        trigger_actor.principal_id.clone(),
        input,
        1,
    );
    let authority = attenuate_for_caveats(
        authority_of(resolved.effective_policy()),
        &caveats,
        &scope,
        &run_id,
        &trigger_actor,
    )
    .expect("the test run caveats are valid");
    let effective_grants = authority
        .capabilities()
        .grants()
        .map(str::to_string)
        .chain(authority.repositories().map(repository_scope_grant));
    let token = minter
        .mint_from_resolved_policy(
            &scope,
            &agent.principal_id,
            &run_id,
            &agent,
            &trigger_actor,
            &resolved,
            &caveats,
            MachineKind::Agent,
            &FailStaticBound {
                static_max_secs: ttl_secs,
            },
            now,
        )
        .expect("the test run credential is issued");
    IssuedGovernedRun::new(
        GovernedRun {
            scope,
            agent_id: agent.principal_id.clone(),
            agent,
            run_id,
        },
        token,
        effective_grants,
    )
    .expect("the issued test run is internally coherent")
}
