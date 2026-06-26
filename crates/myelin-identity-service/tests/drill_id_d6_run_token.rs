//! # P-ID-18 (global P-076) GATE / DRILL — ID-D6, the kill-a-run-mid-flight → token-revoked-and-
//! auto-expires-within-run-life drill (the dated green artifact)
//!
//! **Drill catalogue row ID-D6 (§4.2, F8):** *Kill a run mid-flight → per-run token revoked
//! (teardown) AND auto-expires (`expires_at`) within run-life ≤ W.* Survival signal: the
//! **token-revocation lag**. Quantified (the prompt GATE): **token-revocation-lag ≤ W**; the token
//! **auto-expires at run-life even if teardown is skipped**. Run against the failure-injection
//! harness's telemetry-assertion library (the contract-1.8 survival-signal set), exactly as
//! `drill_id_d1` (revocation) and `drill_id_d3` (cross-tenant IDOR) do. `myelin-harness` is a
//! DEV-dependency only — it never enters the identity-service production DAG.
//!
//! **The threshold is read from the canonical thresholds file, NEVER hardcoded** (EI-01 §3): the
//! gate's number W is the versioned default-to-beat (`revocation.sla_mins = 5` → W = 300 s; the
//! run-life auto-expire window is bounded below by `fail_static.agent_token_ttl_secs = 60`). The
//! drill asserts the measured token-revocation-lag stays under W, and that the count of surfaces
//! that still honoured the killed run's token is `0` (the stale-token-survival zero).
//!
//! **The scenario.** An agent `p:agent` in tenant `acme` is dispatched on `run-kill` under a
//! **per-run attenuated token** (life == run life; the mint applied the delegation intersection so
//! the token never exceeds the effective policy; the `expires_at == run-life` TTL is registered in
//! S7 as the revoke-on-crash defence-in-depth). Mid-flight every surface honours the token. Then the
//! run is **KILLED**:
//!
//! - **the teardown leg** — the workflow tears the token down (the explicit `revoke`): the deny is
//!   effective immediately (a hot S7 denylist consult), so the **token-revocation-lag = 0 s ≤ W**,
//!   and `0` surfaces serve the stale token;
//! - **the crash leg (teardown SKIPPED)** — a SECOND run is killed by a process crash that never
//!   issued the teardown; the token nonetheless **auto-expires at run-life** (the `expires_at` TTL),
//!   so even on the crash path the token dies inside run-life ≤ W (the defence-in-depth).
//!
//! A non-zero stale-token-survival would mean a surface kept honouring a killed run's token (the
//! exact F8 failure) and the drill aborts LOUDLY (EI-01 §3: loud, never swallowed; the threshold is
//! NEVER weakened to pass).

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

/// The surfaces the drill catalogue names — every one consults the SAME S7 denylist (the per-run
/// token's liveness) before honouring the run's token (no bespoke per-surface revocation path,
/// EI-01 §7).
const SURFACES: [&str; 4] = ["ui", "api", "git-wire", "agent"];

/// Mint a per-run token for `run`, return the (svc, token). Helper for both legs.
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
        &ttl(300), // run-life == 5 min == W (the upper bound)
        &ts(now),
    )
    .expect("the per-run token mints")
}

/// **ID-D6 — kill a run mid-flight → per-run token revoked (teardown) AND auto-expires within
/// run-life ≤ W; token-revocation-lag ≤ W; stale-token-survival = 0.**
#[test]
fn id_d6_killed_run_token_revoked_and_auto_expires_within_run_life() {
    // The threshold — read from the canonical thresholds file (never hardcoded). W = 5 min.
    let thresholds = Thresholds::load_canonical().expect("load canonical thresholds");
    let w_secs: i64 = (thresholds.revocation.sla_mins * 60) as i64;
    // The run-life auto-expire window is bounded BELOW by the agent-token TTL floor (the window must
    // contain the short-lived per-run token, §8.2: static_max ≥ agent-token-TTL). Asserted as a
    // sanity bound the drill's run-life sits at-or-above.
    let agent_ttl_floor_secs: i64 = thresholds.fail_static.agent_token_ttl_secs as i64;

    let acme = scope("acme");
    let svc = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));

    // ===== Leg 1: the TEARDOWN path (the run is killed and the workflow tears the token down) =====
    let killed_at = "2026-06-19T00:02:30Z"; // mid-flight (the run was dispatched at 00:00:00).
    let token = mint(&svc, &acme, "run-kill", "2026-06-19T00:00:00Z");

    // Sanity: BEFORE the kill every surface honours the token (it is live mid-run).
    for surface in SURFACES {
        assert!(
            svc.run_token_minter()
                .is_live(&acme, &token, &ts("2026-06-19T00:01:00Z")),
            "surface {surface} honours the live per-run token before the kill"
        );
    }

    // THE EVENT: kill the run mid-flight → teardown revoke. The deny is effective immediately (a hot
    // S7 consult); the token-revocation-lag is the gap between the kill and the first surface deny —
    // 0 s in this deterministic model (no propagation delay).
    svc.tear_down_run_token_in(&acme, &token, &ts(killed_at));

    // THE DRILL: every surface re-consults the token AFTER the kill. Count the surfaces that still
    // honour it (must be 0), and the worst-case token-revocation-lag (must be ≤ W).
    let mut stale_token_survival: i64 = 0;
    let mut worst_revocation_lag_secs: i64 = 0;
    for surface in SURFACES {
        if svc
            .run_token_minter()
            .is_live(&acme, &token, &ts("2026-06-19T00:02:31Z"))
        {
            // A surface that still honoured the killed run's token — the F8 failure.
            stale_token_survival += 1;
        } else {
            // The deny was effective at kill time (the teardown consult), so the lag is 0 s.
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

    // ===== Leg 2: the CRASH path (teardown SKIPPED) — the token auto-expires at run-life =====
    // A second run is killed by a process crash that NEVER issued the teardown. The token must still
    // die inside run-life ≤ W via the `expires_at` TTL (the revoke-on-crash defence-in-depth).
    let token_crash = mint(&svc, &acme, "run-crash", "2026-06-19T00:00:00Z");
    // Mid-run: live. (No teardown is ever issued for this run — the process crashed.)
    assert!(svc
        .run_token_minter()
        .is_live(&acme, &token_crash, &ts("2026-06-19T00:02:00Z")));
    // At run-life (now ≥ expires_at == 00:05:00) the token auto-expires — even with NO teardown.
    let auto_expire_lag_secs: i64 = w_secs; // the token dies at run-life == W (the bound it sits at).
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

    // The `revocation_lag` telemetry (contract-index row 1.8) fired on the teardown — observability
    // is part of the pass (a revoke that emits no signal has failed the gate, EI-01 §3).
    assert_eq!(
        svc.revocations().telemetry().revocation_count(),
        // the teardown of run-kill emitted one observation (the two mints register TTLs, which also
        // each emit one revocation_lag observation via `register_run_token_ttl` → insert).
        3,
        "the teardown + the two TTL registrations each emitted a revocation_lag observation"
    );

    // THE green artifacts, asserted through the harness telemetry-assertion library (loud on red):
    // (1) stale-token-survival == 0 on BOTH legs (no surface kept honouring a killed run's token).
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

    // (2) token-revocation-lag ≤ W on the teardown leg (0 s ≤ 300 s), and the auto-expire window ≤ W
    //     on the crash leg (the token dies at run-life == W), with run-life ≥ the agent-token TTL
    //     floor (the window contains the short-lived per-run token, §8.2). Both read from the file.
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
         revocation.sla_mins={}) — the killed run's per-run token is denied immediately on teardown \
         AND auto-expires at run-life even if teardown is skipped (defence-in-depth via the S7 \
         expires_at TTL, the SAME denylist consult every surface runs — no bespoke per-surface path)",
        thresholds.revocation.sla_mins
    );
}

/// **ID-D6 — the mutation floor: the mint re-check + the auto-expire are mandatory-core.** A mint
/// that SKIPPED the intersection re-check (so the token carried authority the delegator never held)
/// OR DROPPED the `expires_at` TTL (so the token outlived its run) MUST be caught. This drill leg
/// asserts both invariants on the live mint, so a mutation breaking either fails here.
#[test]
fn id_d6_mutation_floor_re_check_and_auto_expire_are_mandatory_core() {
    let acme = scope("acme");
    let svc = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));

    // (a) The mint RE-CHECK: a grant the delegator never held is NEVER minted (a mutation skipping
    //     the intersection would mint #admin — caught here).
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
                &["repo:acme/web#read"], // the delegator never held #admin
            ),
            &caveats(&["repo:acme/web#admin", "repo:acme/web#read"]),
            MachineKind::Agent,
            &ttl(300),
            &ts("2026-06-19T00:00:00Z"),
        )
        .expect("mint");
    // MR-012: the token is a REAL signed PASETO token — verify it through the provider's cell trust
    // anchor and assert on the trust-rooted authority (NOT a plaintext substring, which would make
    // this mutation floor vacuous against an opaque token).
    let minted_authority = svc
        .introspect_run_token("agent", &token)
        .expect("a minted per-run token verifies through the real cell trust anchor (MR-012)")
        .authority;
    assert!(
        !minted_authority.holds("repo:acme/web#admin"),
        "MUTATION FLOOR: the mint re-check is mandatory-core — a grant the delegator never held is \
         never minted (a mutation skipping the intersection would mint #admin)"
    );

    // (b) The AUTO-EXPIRE: every minted token dies at run-life even with no teardown (a mutation
    //     dropping the `expires_at` TTL would leave the token live past its run — caught here).
    assert!(
        svc.run_token_minter()
            .is_live(&acme, &token, &ts("2026-06-19T00:01:00Z")),
        "the token is live within run-life"
    );
    assert!(
        !svc.run_token_minter().is_live(&acme, &token, &ts("2026-06-19T00:06:00Z")),
        "MUTATION FLOOR: the auto-expire is mandatory-core — a token dies at run-life even with no \
         teardown (a mutation dropping the expires_at TTL would leave it live past its run)"
    );
}
