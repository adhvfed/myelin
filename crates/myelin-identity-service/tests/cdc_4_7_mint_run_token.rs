//! # The CDC pair for contract 4.7 (`mint_run_token`) — the MINT half (P-ID-18 / global P-076)
//!
//! **Contract-index row 4.7** (**`mint_run_token`** + `revoke(jti|principal_id)` — per-run
//! attenuated token, life == run life; callable mid-workflow on resume; self-hosted-runner token
//! scoped to one tenant's `SelfHosted` jobs). This is the dedicated provider+consumer pair the
//! P-ID-18 TESTS field names — the focused, in-CI evidence that the two sides of the **mint** seam
//! cannot drift apart:
//!
//! - the **PROVIDER** is Identity's per-run-token mint
//!   ([`StoreBackedCheck::mint_run_token_in`] over the [`RunTokenMinter`]): a mint applies the
//!   monotone delegation intersection (the token never exceeds the effective policy), stamps the run
//!   identity, enforces the self-hosted-runner one-tenant scope, and registers the
//!   `expires_at == run-life` TTL in the S7 store (the revoke-on-crash defence).
//! - the **CONSUMER** is a **CI-dispatch / workflow-activity caller** (contract 4.7 "consumed by
//!   Agent Fabric, CI dispatch, workflow"): it dispatches a run under a minted per-run token, honours
//!   the token ONLY while it is live (`is_live`), tears it down at the run boundary, and re-mints a
//!   fresh attenuated token when a multi-day HITL approval resumes the workflow days later.
//!
//! The provider's promise (a minted token carries exactly `agent ∩ delegation ∩ tenant`, lives ==
//! the run, auto-expires at run-life, and a self-hosted token cannot act cross-tenant) and the
//! consumer's promise (it honours the token iff `is_live`, refuses a torn-down / expired token, and
//! re-mints on resume) are pinned here so a change to either side fails this test in the same CI job.

use std::sync::Arc;

use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    DelegationCaveats, FailStaticBound, Principal, PrincipalId, PrincipalKind, RunId, RunToken,
    RuntimeRef,
};
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_identity_service::{
    Authority, CiJobAuthorizationError, CredentialPurpose, DelegationInput, MachineKind,
    PasetoCapabilityVerifier, RunTokenState, StoreBackedCheck, TupleStore,
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

/// **MR-012:** a minted per-run token is now a REAL signed PASETO v4.public token (not a plaintext
/// envelope), so its grants are read by VERIFYING the token through the provider's cell trust anchor
/// (the real crypto round-trip) — never by string-matching the now-opaque token bytes. (These tokens
/// are minted under `MachineKind::Agent` → the `agent` scheme.)
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

/// The PROVIDER: the store-backed mint surface (the S7 store + S3 tuples + the delegation algebra).
fn provider() -> StoreBackedCheck {
    StoreBackedCheck::new(TupleStore::new(OutboxStore::new()))
}

/// The CONSUMER: a CI-dispatch / workflow-activity caller. It dispatches a run under a minted token
/// and proceeds with an action ONLY while the token is live (the canonical 4.7 mint consumer shape).
fn dispatch_under_token_is_honoured(
    svc: &StoreBackedCheck,
    s: &TenantScope,
    token: &RunToken,
    now: &Timestamp,
) -> bool {
    svc.run_token_minter().is_live(s, token, now)
}

/// **The 4.7 mint happy path: a minted token carries exactly the effective policy and is honoured
/// within run-life.** The consumer dispatches a run under the token and proceeds (it is live).
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
    // The token carries exactly the effective grant (the mint applied the intersection) — read via
    // the real PASETO verify round-trip (MR-012), not a plaintext substring.
    assert!(minted_authority(&svc, &token).holds("repo:acme/web#read"));
    // The CONSUMER dispatches under the token and proceeds (it is live within run-life).
    assert!(
        dispatch_under_token_is_honoured(&svc, &s, &token, &ts("2026-06-19T00:02:00Z")),
        "the CI-dispatch consumer honours a live per-run token"
    );
}

/// A CI launch consumer verifies the real signed CI-job token again at the final boundary, binding
/// the exact subject/job/scope/capability and consulting the same S7 lifecycle the provider minted.
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

    svc.tear_down_run_token_in(&s, &token, &ts("2026-06-19T00:02:00Z"));
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

/// **The 4.7 mint never exceeds the effective policy: a grant the delegator never held is not minted
/// (the mint re-check).** The provider's mint drops the un-held grant; the consumer therefore can
/// never act with authority no one delegated.
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
            // The delegation NAMES #admin but the delegator never HELD it.
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

/// **The 4.7 self-hosted-runner scope: a runner token cannot act cross-tenant.** The provider
/// refuses a self-hosted mint whose authority names another tenant's `SelfHosted` scope (the
/// no-global-pool property — the consumer can never dispatch a cross-tenant runner job).
#[test]
fn cdc_4_7_self_hosted_runner_token_is_one_tenant_scoped() {
    let s = scope("acme");
    let svc = provider();
    // Own-tenant scope mints.
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
    // Another tenant's scope is refused.
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

/// **The 4.7 mid-resume re-mint: a days-later HITL approval re-mints a fresh, possibly-narrower
/// token.** The provider re-mints as-of-resume; the consumer (the workflow activity) gets a distinct
/// token. The dispatch leg's token and the resumed leg's token are different (life == run life, and
/// the resumed leg is its own life).
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

    // Days later the workflow resumes; the delegator lost g:write in the interim.
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
        "the re-mint is a fresh token (distinct jti — its own life)"
    );
    let resumed_authority = minted_authority(&svc, &resumed);
    assert!(resumed_authority.holds("g:read"));
    assert!(
        !resumed_authority.holds("g:write"),
        "the re-minted token is narrower (the delegator lost g:write — recomputed as-of-resume)"
    );
}

/// **The 4.7 teardown + auto-expire: a torn-down OR expired token is refused by the consumer.** The
/// provider tears down the token (the immediate deny); the consumer refuses. And even without
/// teardown the token auto-expires at run-life (the consumer refuses past run-life).
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

    // Live mid-run → honoured.
    assert!(dispatch_under_token_is_honoured(
        &svc,
        &s,
        &token,
        &ts("2026-06-19T00:01:00Z")
    ));

    // PROVIDER tears it down (the run ended) → CONSUMER refuses immediately.
    svc.tear_down_run_token_in(&s, &token, &ts("2026-06-19T00:01:30Z"));
    assert!(
        !dispatch_under_token_is_honoured(&svc, &s, &token, &ts("2026-06-19T00:01:31Z")),
        "the consumer refuses a torn-down token (the immediate deny)"
    );

    // And the auto-expire leg: a SECOND run whose teardown is SKIPPED still dies at run-life.
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
    // No teardown — but past run-life the consumer refuses (the auto-expire defence-in-depth).
    assert!(
        !dispatch_under_token_is_honoured(&svc, &s, &token2, &ts("2026-06-19T00:06:00Z")),
        "the consumer refuses an auto-expired token even if teardown was skipped (revoke-on-crash)"
    );
}
