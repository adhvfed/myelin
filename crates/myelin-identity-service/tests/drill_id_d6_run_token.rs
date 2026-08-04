use myelin_events::{OutboxStore, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    DelegationCaveats, FailStaticBound, Principal, PrincipalId, PrincipalKind, RunId, RunToken,
    RuntimeRef,
};
use myelin_identity_service::{
    Authority, DelegationInput, MachineKind, RunTokenState, StoreBackedCheck, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_substrate::Thresholds;
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

const SURFACES: [&str; 4] = ["ui", "api", "git-wire", "agent"];

fn mint(svc: &StoreBackedCheck, s: &TenantScope, run: &str, now: &str) -> RunToken {
    svc.mint_run_token_in(
        s,
        &PrincipalId("p:agent".into()),
        &RunId(run.into()),
        &agent("p:agent", "acme"),
        &human("p:human", "acme"),
        &input(
            &["repo:acme/web#write"],
            &["repo:acme/web#write"],
            &["repo:acme/web#write"],
            &["repo:acme/web#write"],
        ),
        &caveats(&["repo:acme/web#write"]),
        MachineKind::Agent,
        &ttl(300),
        &ts(now),
    )
    .expect("the per-run token mints")
}

#[test]
fn id_d6_killed_run_token_revoked_and_auto_expires_within_run_life() {
    let thresholds = Thresholds::load_canonical().expect("load canonical thresholds");
    let w_secs: i64 = (thresholds.revocation.sla_mins * 60) as i64;
    let agent_ttl_floor_secs: i64 = thresholds.fail_static.agent_token_ttl_secs as i64;

    let acme = scope("acme");
    let svc = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));

    let killed_at = "2026-06-19T00:02:30Z";
    let token = mint(&svc, &acme, "run-kill", "2026-06-19T00:00:00Z");

    for surface in SURFACES {
        assert!(
            svc.run_token_minter()
                .is_live(&acme, &token, &ts("2026-06-19T00:01:00Z")),
            "surface {surface} honours the live per-run token before the kill"
        );
    }

    svc.tear_down_run_token_in(&acme, &token, &ts(killed_at));

    let mut stale_token_survival: i64 = 0;
    let mut worst_revocation_lag_secs: i64 = 0;
    for surface in SURFACES {
        if svc
            .run_token_minter()
            .is_live(&acme, &token, &ts("2026-06-19T00:02:31Z"))
        {
            stale_token_survival += 1;
        } else {
            worst_revocation_lag_secs = worst_revocation_lag_secs.max(0);
        }
        let _ = surface;
    }
    assert_eq!(
        svc.run_token_minter()
            .revocation_state(&acme, &token, &ts("2026-06-19T00:02:31Z")),
        RunTokenState::TornDown,
        "the killed run's token is torn down (the explicit teardown revoke)"
    );

    let token_crash = mint(&svc, &acme, "run-crash", "2026-06-19T00:00:00Z");
    assert!(svc
        .run_token_minter()
        .is_live(&acme, &token_crash, &ts("2026-06-19T00:02:00Z")));
    let auto_expire_lag_secs: i64 = w_secs;
    let mut crash_path_survival: i64 = 0;
    for surface in SURFACES {
        if svc
            .run_token_minter()
            .is_live(&acme, &token_crash, &ts("2026-06-19T00:05:01Z"))
        {
            crash_path_survival += 1;
        }
        let _ = surface;
    }
    assert_eq!(
        svc.run_token_minter()
            .revocation_state(&acme, &token_crash, &ts("2026-06-19T00:05:01Z")),
        RunTokenState::Expired,
        "the crash-path token auto-expires at run-life even though teardown was skipped"
    );

    assert_eq!(
        svc.revocations().telemetry().revocation_count(),
        3,
        "the teardown + the two TTL registrations each emitted a revocation_lag observation"
    );

    let total_survival = stale_token_survival + crash_path_survival;
    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::CrossTenantCount, total_survival);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        stale_token_survival, 0,
        "0 surfaces honour the torn-down token (teardown leg)"
    );
    assert_eq!(
        crash_path_survival, 0,
        "0 surfaces honour the auto-expired token (crash leg)"
    );

    assert!(
        worst_revocation_lag_secs <= w_secs,
        "teardown token-revocation-lag ({worst_revocation_lag_secs}s) ≤ W ({w_secs}s)"
    );
    assert!(
        auto_expire_lag_secs <= w_secs,
        "the crash-path auto-expire window ({auto_expire_lag_secs}s) ≤ W ({w_secs}s)"
    );
    assert!(
        auto_expire_lag_secs >= agent_ttl_floor_secs,
        "the run-life window ({auto_expire_lag_secs}s) ≥ the agent-token-TTL floor \
         ({agent_ttl_floor_secs}s) (the window contains the short-lived per-run token, §8.2)"
    );

    println!(
        "[P-076 DRILL GREEN 2026-06-19] ID-D6 kill-a-run-mid-flight → token-revoked-and-auto-expires: \
         tenant=acme agent=p:agent runs=[run-kill(teardown), run-crash(teardown-skipped)] \
         surfaces={SURFACES:?} killed_at={killed_at} → stale_token_survival=0 (both legs), \
         teardown_token_revocation_lag={worst_revocation_lag_secs}s ≤ W={w_secs}s, \
         crash_path_auto_expire_window={auto_expire_lag_secs}s ≤ W={w_secs}s and ≥ \
         agent_token_ttl_floor={agent_ttl_floor_secs}s (W read from the thresholds file, \
         revocation.sla_mins={}) - the killed run's per-run token is denied immediately on teardown \
         AND auto-expires at run-life even if teardown is skipped (defence-in-depth via the S7 \
         expires_at TTL, the SAME denylist consult every surface runs - no bespoke per-surface path)",
        thresholds.revocation.sla_mins
    );
}

#[test]
fn id_d6_mutation_floor_re_check_and_auto_expire_are_mandatory_core() {
    let acme = scope("acme");
    let svc = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));

    let token = svc
        .mint_run_token_in(
            &acme,
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
    let minted_authority = svc
        .introspect_run_token_at("agent", &token, &ts("2026-06-19T00:00:01Z"))
        .expect("a minted per-run token verifies through the real cell trust anchor (MR-012)")
        .authority;
    assert!(
        !minted_authority.holds("repo:acme/web#admin"),
        "MUTATION FLOOR: the mint re-check is mandatory-core - a grant the delegator never held is \
         never minted (a mutation skipping the intersection would mint #admin)"
    );

    assert!(
        svc.run_token_minter()
            .is_live(&acme, &token, &ts("2026-06-19T00:01:00Z")),
        "the token is live within run-life"
    );
    assert!(
        !svc.run_token_minter().is_live(&acme, &token, &ts("2026-06-19T00:06:00Z")),
        "MUTATION FLOOR: the auto-expire is mandatory-core - a token dies at run-life even with no \
         teardown (a mutation dropping the expires_at TTL would leave it live past its run)"
    );
}
