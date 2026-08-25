use std::sync::Arc;

use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    DelegationCaveats, FailStaticBound, Principal, PrincipalId, PrincipalKind, RunId, RunToken,
    RuntimeRef,
};
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_identity_service::{
    Authority, CiJobAuthorizationError, CredentialPurpose, DelegationInput, MachineKind,
    PasetoCapabilityVerifier, ResolvedDelegationPolicy, RunTokenState, StoreBackedCheck,
    TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn agent(id: &str, tenant: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt-1".into()),
            on_behalf_of: Some(PrincipalId("p:human".into())),
        },
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn human(id: &str, tenant: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn auth(grants: &[&str]) -> Authority {
    Authority::of(grants.iter().copied())
}

fn minted_authority(svc: &StoreBackedCheck, token: &RunToken) -> Authority {
    svc.introspect_run_token_at("agent", token, &ts("2026-06-19T00:00:01Z"))
        .expect("a minted per-run token verifies through the real cell trust anchor (MR-012)")
        .authority
}

fn input(agent: &[&str], deleg: &[&str], tenant: &[&str], held: &[&str]) -> DelegationInput {
    DelegationInput {
        agent_policy: auth(agent),
        delegation: auth(deleg),
        tenant_policy: auth(tenant),
        trigger_actor_held: auth(held),
    }
}

fn ts(s: &str) -> Timestamp {
    Timestamp(s.into())
}

fn ttl(secs: u64) -> FailStaticBound {
    FailStaticBound {
        static_max_secs: secs,
    }
}

fn caveats(g: &[&str]) -> DelegationCaveats {
    DelegationCaveats(g.iter().map(|s| s.to_string()).collect())
}

fn provider() -> StoreBackedCheck {
    StoreBackedCheck::new(TupleStore::new(OutboxStore::new()))
}

fn dispatch_under_token_is_honoured(
    svc: &StoreBackedCheck,
    s: &TenantScope,
    token: &RunToken,
    now: &Timestamp,
) -> bool {
    svc.run_token_minter().is_live(s, token, now)
}

#[test]
fn cdc_4_7_minted_token_honoured_within_run_life() {
    let s = scope("acme");
    let svc = provider();
    let token = svc
        .mint_run_token_in(
            &s,
            &PrincipalId("p:agent".into()),
            &RunId("run-1".into()),
            &agent("p:agent", "acme"),
            &human("p:human", "acme"),
            &input(
                &["repo:acme/web#read"],
                &["repo:acme/web#read"],
                &["repo:acme/web#read"],
                &["repo:acme/web#read"],
            ),
            &caveats(&["repo:acme/web#read"]),
            MachineKind::Agent,
            &ttl(300),
            &ts("2026-06-19T00:00:00Z"),
        )
        .expect("the provider mints a per-run token");
    assert!(minted_authority(&svc, &token).holds("repo:acme/web#read"));
    assert!(
        dispatch_under_token_is_honoured(&svc, &s, &token, &ts("2026-06-19T00:02:00Z")),
        "the CI-dispatch consumer honours a live per-run token"
    );
}

#[test]
fn cdc_4_7_repository_caveat_survives_the_real_signed_token_boundary() {
    let s = scope("acme");
    let svc = provider();
    let run_id = RunId("run-repo-scope".into());
    let run_agent = agent("p:agent", "acme");
    let founder = human("p:human", "acme");
    let policy = ResolvedDelegationPolicy::synthetic_for_test(
        run_id.clone(),
        run_agent.principal_id.clone(),
        founder.principal_id.clone(),
        input(
            &["repo.pull"],
            &["repo.pull"],
            &["repo.pull"],
            &["repo.pull"],
        ),
        42,
    );
    let token = svc
        .mint_run_token_from_resolved_policy_in(
            &s,
            &run_agent.principal_id,
            &run_id,
            &run_agent,
            &founder,
            &policy,
            &caveats(&["repo.pull", "repo:core"]),
            MachineKind::Agent,
            &ttl(300),
            &ts("2026-06-19T00:00:00Z"),
        )
        .expect("the scoped agent token is signed by the cell authority");
    assert_ne!(
        token.token.split('|').nth(1),
        Some("W10"),
        "the trigger restriction is carried by the monotonic caveat chain"
    );
    let verifier =
        PasetoCapabilityVerifier::new(svc.token_trust_anchor()).with_clock(|| 1_781_827_260);
    let authorizer = RunTokenAuthorizer::new(Arc::new(verifier), svc.revocations().clone())
        .with_clock(|| ts("2026-06-19T00:01:00Z"));

    authorizer
        .authorize_repository(
            &s,
            &PrincipalId("p:agent".into()),
            &token,
            &["repo.pull".into()],
            "core",
        )
        .expect("the signed repository remains usable");
    assert!(authorizer
        .authorize_repository(
            &s,
            &PrincipalId("p:agent".into()),
            &token,
            &["repo.pull".into()],
            "payroll",
        )
        .is_err());
}

#[test]
fn cdc_4_7_ci_job_is_reauthorized_immediately_before_launch() {
    let s = scope("acme");
    let svc = provider();
    let token = svc
        .mint_run_token_in(
            &s,
            &PrincipalId("svc:ci".into()),
            &RunId("job:run-22:build".into()),
            &agent("svc:ci", "acme"),
            &human("p:human", "acme"),
            &input(
                &["job.launch", "artifact.write"],
                &["job.launch", "artifact.write"],
                &["job.launch", "artifact.write"],
                &["job.launch", "artifact.write"],
            ),
            &caveats(&["job.launch", "artifact.write"]),
            MachineKind::Ci,
            &ttl(300),
            &ts("2026-06-19T00:00:00Z"),
        )
        .expect("mint real signed CI-job token");
    let verifier =
        PasetoCapabilityVerifier::new(svc.token_trust_anchor()).with_clock(|| 1_781_827_260);
    let authorizer = RunTokenAuthorizer::new(Arc::new(verifier), svc.revocations().clone())
        .with_clock(|| ts("2026-06-19T00:01:00Z"));
    let verified = authorizer
        .authorize_ci_job(
            &s,
            &PrincipalId("svc:ci".into()),
            "job:run-22:build",
            &token,
            &["job.launch".into(), "artifact.write".into()],
        )
        .expect("live exact CI token authorizes the one launch");
    assert_eq!(verified.kind, MachineKind::Ci);
    assert_eq!(
        verified.purpose,
        CredentialPurpose::CiJob {
            run_id: "job:run-22:build".into()
        }
    );

    svc.tear_down_run_token_in(&s, &token, &ts("2026-06-19T00:02:00Z"))
        .expect("record run teardown");
    assert_eq!(
        authorizer.authorize_ci_job(
            &s,
            &PrincipalId("svc:ci".into()),
            "job:run-22:build",
            &token,
            &["job.launch".into()],
        ),
        Err(CiJobAuthorizationError::NotLive {
            state: RunTokenState::TornDown
        })
    );
}

#[test]
fn cdc_4_7_mint_never_exceeds_effective_policy() {
    let s = scope("acme");
    let svc = provider();
    let token = svc
        .mint_run_token_in(
            &s,
            &PrincipalId("p:agent".into()),
            &RunId("run-1".into()),
            &agent("p:agent", "acme"),
            &human("p:human", "acme"),
            &input(
                &["repo:acme/web#admin", "repo:acme/web#read"],
                &["repo:acme/web#admin", "repo:acme/web#read"],
                &["repo:acme/web#admin", "repo:acme/web#read"],
                &["repo:acme/web#read"],
            ),
            &caveats(&["repo:acme/web#admin", "repo:acme/web#read"]),
            MachineKind::Agent,
            &ttl(300),
            &ts("2026-06-19T00:00:00Z"),
        )
        .expect("mint");
    let minted = minted_authority(&svc, &token);
    assert!(
        !minted.holds("repo:acme/web#admin"),
        "the mint never mints a grant the delegator never held (cannot delegate what you lack)"
    );
    assert!(minted.holds("repo:acme/web#read"));
}

#[test]
fn cdc_4_7_self_hosted_runner_token_is_one_tenant_scoped() {
    let s = scope("acme");
    let svc = provider();
    let ok = svc.mint_run_token_in(
        &s,
        &PrincipalId("svc:runner".into()),
        &RunId("run-1".into()),
        &agent("svc:runner", "acme"),
        &human("p:human", "acme"),
        &input(
            &["selfhosted:acme"],
            &["selfhosted:acme"],
            &["selfhosted:acme"],
            &["selfhosted:acme"],
        ),
        &caveats(&["selfhosted:acme"]),
        MachineKind::PerJob,
        &ttl(300),
        &ts("2026-06-19T00:00:00Z"),
    );
    assert!(ok.is_ok(), "an own-tenant self-hosted run token mints");
    let cross = svc.mint_run_token_in(
        &s,
        &PrincipalId("svc:runner".into()),
        &RunId("run-2".into()),
        &agent("svc:runner", "acme"),
        &human("p:human", "acme"),
        &input(
            &["selfhosted:globex"],
            &["selfhosted:globex"],
            &["selfhosted:globex"],
            &["selfhosted:globex"],
        ),
        &caveats(&["selfhosted:globex"]),
        MachineKind::PerJob,
        &ttl(300),
        &ts("2026-06-19T00:00:00Z"),
    );
    assert!(
        cross.is_err(),
        "a self-hosted runner token naming another tenant's scope is refused (C6, no-global-pool)"
    );
}

#[test]
fn cdc_4_7_re_mint_on_resume_yields_a_fresh_token() {
    let s = scope("acme");
    let svc = provider();
    let agent_id = PrincipalId("p:agent".into());
    let run = RunId("run-1".into());

    let dispatch = svc
        .mint_run_token_in(
            &s,
            &agent_id,
            &run,
            &agent("p:agent", "acme"),
            &human("p:human", "acme"),
            &input(
                &["g:read", "g:write"],
                &["g:read", "g:write"],
                &["g:read", "g:write"],
                &["g:read", "g:write"],
            ),
            &caveats(&["g:read", "g:write"]),
            MachineKind::Agent,
            &ttl(300),
            &ts("2026-06-19T00:00:00Z"),
        )
        .expect("dispatch mint");

    let resumed = svc
        .re_mint_run_token_in(
            &s,
            &agent_id,
            &run,
            &agent("p:agent", "acme"),
            &human("p:human", "acme"),
            &input(
                &["g:read", "g:write"],
                &["g:read", "g:write"],
                &["g:read", "g:write"],
                &["g:read"],
            ),
            &caveats(&["g:read", "g:write"]),
            MachineKind::Agent,
            &ttl(300),
            &ts("2026-06-22T09:00:00Z"),
        )
        .expect("re-mint on resume");

    assert_ne!(
        resumed.jti, dispatch.jti,
        "the re-mint is a fresh token (distinct jti - its own life)"
    );
    let resumed_authority = minted_authority(&svc, &resumed);
    assert!(resumed_authority.holds("g:read"));
    assert!(
        !resumed_authority.holds("g:write"),
        "the re-minted token is narrower (the delegator lost g:write - recomputed as-of-resume)"
    );
}

#[test]
fn cdc_4_7_teardown_and_auto_expire_refuse_the_token() {
    let s = scope("acme");
    let svc = provider();
    let token = svc
        .mint_run_token_in(
            &s,
            &PrincipalId("p:agent".into()),
            &RunId("run-1".into()),
            &agent("p:agent", "acme"),
            &human("p:human", "acme"),
            &input(&["g"], &["g"], &["g"], &["g"]),
            &caveats(&["g"]),
            MachineKind::Agent,
            &ttl(300),
            &ts("2026-06-19T00:00:00Z"),
        )
        .expect("mint");

    assert!(dispatch_under_token_is_honoured(
        &svc,
        &s,
        &token,
        &ts("2026-06-19T00:01:00Z")
    ));

    svc.tear_down_run_token_in(&s, &token, &ts("2026-06-19T00:01:30Z"))
        .expect("record resumed-run teardown");
    assert!(
        !dispatch_under_token_is_honoured(&svc, &s, &token, &ts("2026-06-19T00:01:31Z")),
        "the consumer refuses a torn-down token (the immediate deny)"
    );

    let token2 = svc
        .mint_run_token_in(
            &s,
            &PrincipalId("p:agent".into()),
            &RunId("run-2".into()),
            &agent("p:agent", "acme"),
            &human("p:human", "acme"),
            &input(&["g"], &["g"], &["g"], &["g"]),
            &caveats(&["g"]),
            MachineKind::Agent,
            &ttl(300),
            &ts("2026-06-19T00:00:00Z"),
        )
        .expect("mint run-2");
    assert!(
        !dispatch_under_token_is_honoured(&svc, &s, &token2, &ts("2026-06-19T00:06:00Z")),
        "the consumer refuses an auto-expired token even if teardown was skipped (revoke-on-crash)"
    );
}
