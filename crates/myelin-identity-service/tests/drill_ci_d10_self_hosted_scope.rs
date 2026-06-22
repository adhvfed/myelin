//! # P-ID-28 (global P-321) GATE / DRILL — CI-D10 (SCOPE side): the self-hosted-runner token
//! scope exercised against the LIVE CI fragment (dated green artifact)
//!
//! **Drill catalogue row CI-D10 (F2):** *A compromised self-hosted runner → scoped job token bounds
//! it to its own tenant's `SelfHosted` jobs; **0 cross-tenant job/secret reads**; attestation
//! failure → cannot claim.* P-ID-27 (P-320) shipped the **fragment side** (`secret.read`
//! non-inheritance + the `!is_untrusted_fork` ABAC edge — the structural authz invariants). THIS
//! prompt ships the **SCOPE side**: the self-hosted-runner token scope (contract 4.7, §4) exercised
//! AGAINST that live CI fragment — a self-hosted runner token is scoped to ONE tenant's `SelfHosted`
//! jobs and cannot mint or act cross-tenant (the no-global-pool property at the identity layer,
//! recon §1 / identity-and-access §4). The scope MECHANISM shipped in P-ID-18 (the
//! [`MintError::SelfHostedScopeViolation`] ceiling on a [`MachineKind::PerJob`] mint); this drill
//! PROVES it against the CI namespace fragment, end to end.
//!
//! ## The two halves of "0 cross-tenant" this drill greens
//!
//! 1. **The MINT ceiling (no cross-tenant token can be minted).** A self-hosted-runner
//!    ([`MachineKind::PerJob`]) token for tenant `acme` may name ONLY `selfhosted:acme`. An attempt
//!    to mint a token naming ANOTHER tenant's scope (`selfhosted:globex`) — even when every
//!    delegation conjunct names it, so the intersection is non-empty — is REFUSED by the one-tenant
//!    ceiling ([`MintError::SelfHostedScopeViolation`]). A compromised runner cannot widen its mint
//!    to another tenant: **0 cross-tenant tokens minted.**
//!
//! 2. **The CHECK isolation against the live CI fragment (no cross-tenant read).** A tenant-`acme`
//!    self-hosted-runner PRINCIPAL, run against the live (admitted) CI fragment, attempts to read a
//!    DIFFERENT tenant's (`globex`'s) CI `run` outputs and `secret`s. Because `check` derives its
//!    `(tenant, region)` scope from the SUBJECT's own verified token (tenant-from-token, ID-3 — never
//!    a path), and the S3 tuple store is RLS-partitioned per `(tenant, region)` with NO cross-tenant
//!    query path, the acme runner finds NONE of globex's grants: every cross-tenant `run.read` /
//!    `run.view` / `secret.read` DENIES. **0 cross-tenant job/secret reads.** The runner can still
//!    read its OWN tenant's run/secret (the scope bounds it to its tenant, it does not blind it).
//!
//! Survival signal: **cross-tenant tokens minted = 0** AND **cross-tenant job/secret reads = 0**,
//! projected onto the load-bearing [`SignalName::CrossTenantCount`] zero (the same zero-leak
//! survival signal `drill_ci_d10_secret_non_inheritance` / `git_d8` assert). A non-zero on EITHER
//! counter means a self-hosted runner reached across the tenant boundary — and the drill aborts
//! LOUDLY (EI-01 §3: loud, never swallowed; the threshold is NEVER weakened to pass).
//!
//! Run against the failure-injection harness's telemetry-assertion library (the contract-1.8
//! survival-signal set), exactly as the fragment-side drill does. `myelin-harness` is a DEV-only
//! dependency — it never enters the identity-service production DAG.

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

// ───────────────────────────────────────── test fixtures ─────────────────────────────────────────

fn region() -> Region {
    Region("eu-west".into())
}

/// A self-hosted-runner SERVICE principal for `tenant` (the machine principal the runner acts as —
/// the per-job token's subject). A self-hosted runner is a `kind = service` machine identity.
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

/// A [`DelegationInput`] whose four conjuncts all name `grants` (a non-empty intersection over those
/// grants — so it is the SCOPE ceiling, not the empty intersection, that gates the cross-tenant mint).
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

/// Admit the Git + CI fragments into a fresh, tuple-store-backed check engine (CI inheritance edges
/// `run.view = parent_repo->pull` / `run.trigger = parent_repo->push` terminate on the Git `repo`
/// fragment, so BOTH must be admitted for the run edges to resolve at check time).
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

// ═══════════════════════════════ HALF 1 — the MINT ceiling (no cross-tenant token) ═══════════════════════════════

/// **CI-D10 (scope side) — the self-hosted-runner mint ceiling: 0 cross-tenant tokens minted.**
///
/// A self-hosted-runner ([`MachineKind::PerJob`]) token for tenant `acme` mints when (and only when)
/// its authority names ONLY `selfhosted:acme`. A mint whose effective authority names ANOTHER
/// tenant's `SelfHosted` scope — `selfhosted:globex`, or a non-`selfhosted:` grant — is REFUSED by
/// the one-tenant ceiling ([`MintError::SelfHostedScopeViolation`]), even though every delegation
/// conjunct names it (the intersection is non-empty — it is the SCOPE ceiling, not the intersection,
/// that gates). A compromised runner cannot widen its mint cross-tenant. We assert the
/// cross-tenant-token-minted count is `0`.
#[test]
fn ci_d10_self_hosted_mint_cannot_mint_cross_tenant() {
    let mut signals = SignalSource::new();
    let acme = scope_of(&runner("acme", "svc:runner-acme"));
    let svc = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));

    // (a) The OWN-tenant SelfHosted mint succeeds (the scope bounds the runner to its tenant; it does
    //     not refuse the legitimate own-tenant mint).
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
    assert!(
        own.token
            .contains(&format!("{SELFHOSTED_GRANT_PREFIX}acme")),
        "the minted token carries ONLY the own-tenant SelfHosted grant"
    );
    assert!(
        !own.token.contains("globex"),
        "the minted token never names another tenant's scope"
    );

    // (b) THE ATTACK — a compromised runner attempts to mint tokens naming OTHER tenants' scopes (or a
    //     non-selfhosted grant). Every such mint MUST be refused by the one-tenant ceiling.
    let mut cross_tenant_tokens_minted: i64 = 0;
    let attacks: &[&[&str]] = &[
        &["selfhosted:globex"],  // a different tenant's SelfHosted scope
        &["selfhosted:initech"], // another different tenant
        &["selfhosted:acme", "selfhosted:globex"], // own + cross (the widening attempt)
        &["repo:globex/secret#read"], // a non-selfhosted cross-tenant grant
    ];
    for (i, grants) in attacks.iter().enumerate() {
        let r = svc.mint_run_token_in(
            &acme, // the mint scope is acme's (tenant-from-token) — the ceiling is `selfhosted:acme`
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
            Err(MintError::SelfHostedScopeViolation(_)) => { /* refused — as required */ }
            Ok(_) => cross_tenant_tokens_minted += 1, // a cross-tenant token was minted (the leak).
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

// ═══════════════════════════ HALF 2 — the CHECK isolation against the live CI fragment ═══════════════════════════

/// **CI-D10 (scope side) — a self-hosted runner is bounded to its tenant's CI objects: 0
/// cross-tenant job/secret reads against the LIVE CI fragment.**
///
/// Seed tenant `globex`'s CI objects (a `run` with output + a `secret`) in globex's partition, with
/// legitimate globex grants. Then a tenant-`acme` self-hosted-runner PRINCIPAL attempts to read
/// globex's `run` (view + read) and `secret` (read) through the live (admitted) CI fragment. Because
/// `check` scopes to the SUBJECT's own verified tenant (tenant-from-token, ID-3) and S3 has no
/// cross-tenant query path, the acme runner finds NONE of globex's grants — every cross-tenant
/// CI read DENIES. The acme runner CAN read its OWN tenant's run/secret (the scope bounds it to its
/// tenant, it does not blind it). We assert the cross-tenant-read count is `0`.
#[test]
fn ci_d10_self_hosted_runner_zero_cross_tenant_ci_reads() {
    let mut signals = SignalSource::new();
    let store = TupleStore::new(OutboxStore::new());

    // ── Seed GLOBEX's CI objects in globex's partition (legitimate globex grants). ──
    let globex = scope_of(&human("globex", "p-globex-admin"));
    // NB object ids are slash-free (a `/` reads as a URN path separator in type inference, §7.3) —
    // the same `repo:<name>` convention the fragment-side drill uses.
    let globex_tuples: Vec<TupleDelta> = vec![
        // globex's run belongs to globex's repo; run.view = parent_repo->pull, run.read = view − fork.
        add("run:globex-deploy", "parent_repo", "repo:globex-infra#pull"),
        add("repo:globex-infra", "reader", "p:globex-eng"), // a legitimate globex viewer
        // globex's secret — read is the DIRECT NARROW relation, granted to a globex deployer only.
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

    // ── Seed ACME's OWN CI objects in acme's partition (so we prove the scope BOUNDS, not blinds). ──
    let acme = scope_of(&runner("acme", "svc:runner-acme"));
    let acme_tuples: Vec<TupleDelta> = vec![
        add("run:acme-deploy", "parent_repo", "repo:acme-app#pull"),
        // The acme self-hosted runner can pull acme's repo (its own-tenant viewer grant).
        add("repo:acme-app", "reader", "svc:runner-acme"),
        // The acme runner holds a DIRECT secret grant on acme's own secret (the only path to a secret).
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

    // Sanity (the scope BOUNDS, it does not BLIND): a legitimate globex viewer reads globex's run,
    // and the acme runner reads its OWN tenant's run + secret.
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

    // ── THE ATTACK: the acme self-hosted runner reaches across the tenant boundary for globex's CI. ──
    // Every cross-tenant CI read attempt — run.view, run.read, secret.read — MUST deny (the runner's
    // token scopes it to acme; check is tenant-from-token; S3 has no cross-tenant query path).
    let cross_tenant_attacks: &[(&str, &str)] = &[
        (CI_VIEW, "run:globex-deploy"),   // view another tenant's run
        (CI_READ, "run:globex-deploy"),   // read another tenant's run output
        (CI_READ, "secret:globex-db-pw"), // read another tenant's secret
    ];
    let mut cross_tenant_reads: i64 = 0;
    for (perm, object) in cross_tenant_attacks {
        if allows(&svc, &acme_runner, perm, object) {
            cross_tenant_reads += 1;
        }
    }
    assert_eq!(
        cross_tenant_reads, 0,
        "a tenant-acme self-hosted runner read a tenant-globex CI object — the per-tenant scope \
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
         SelfHosted jobs, it does not blind it) — check is tenant-from-token (ID-3), S3 has no \
         cross-tenant query path (C6, the no-global-pool property at the identity layer)",
        cross_tenant_attacks.len()
    );
}

/// **CI-D10 (scope side) — defence-in-depth: even a runner GRANTED a cross-tenant CI relation cannot
/// read across the boundary (the tenant-from-token scope, not the grant, is authoritative).**
///
/// This is the strongest form of the no-global-pool property: suppose a misconfiguration wrote a
/// `secret#direct_reader@svc:runner-acme` tuple INTO globex's partition (a cross-tenant grant should
/// never be written, but defence-in-depth must hold if one were). The acme runner STILL cannot read
/// globex's secret, because `check` reads acme's partition (tenant-from-token) and never globex's —
/// the grant living in globex's partition is invisible to an acme-scoped check. 0 cross-tenant reads.
#[test]
fn ci_d10_cross_tenant_grant_in_other_partition_is_invisible() {
    let mut signals = SignalSource::new();
    let store = TupleStore::new(OutboxStore::new());

    // A (hypothetical) cross-tenant grant written into GLOBEX's partition naming the ACME runner.
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

    // The acme runner — scoped to acme (tenant-from-token) — checks globex's objects. The grant lives
    // in GLOBEX's partition; an acme-scoped check never reads it → DENY on every attempt.
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
         scoped check — 0 cross-tenant reads even WITH a (mis-written) cross-tenant grant (C6)"
    );

    println!(
        "[P-321 DRILL GREEN 2026-06-22] CI-D10 (scope side) defence-in-depth: a (hypothetical) \
         cross-tenant grant secret#direct_reader@svc:runner-acme written into GLOBEX's partition is \
         INVISIBLE to the acme-scoped runner's check (tenant-from-token reads acme's partition only) \
         → cross-tenant-read count=0 — the scope, not the grant, is authoritative (no-global-pool)"
    );
}
