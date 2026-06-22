//! # P-ID-27 (global P-320) GATE / DRILL — CI-D10 (fragment side): secret-non-inheritance +
//! the `!is_untrusted_fork` ABAC edge (dated green artifact)
//!
//! **Drill catalogue row CI-D10 (F2):** *A compromised self-hosted runner → scoped job token bounds it
//! to its own tenant's `SelfHosted` jobs; **0 cross-tenant job/secret reads**; attestation failure →
//! cannot claim.* The TOKEN-SCOPE half (the runner's attenuated token) is P-ID-28 (P-321); THIS prompt
//! ships the **fragment side** — the two structural authz invariants the runner's scope acts against:
//!
//! 1. **`secret.read` is NON-inherited (CI-1, §1):** secrets are NEVER reachable through `ci_project`
//!    read inheritance — the only path to a secret is a DIRECT `secret#direct_reader@subject` grant. A
//!    `ci_project` reader/admin who can `view`/`administer` the project gets **0** secret reads via
//!    inheritance. This is the structural reason a compromised runner (or any over-broad principal)
//!    cannot harvest secrets by reading the project.
//! 2. **`run.read = run.view − is_untrusted_fork` (C7, §X-1):** an untrusted-fork run (a run that
//!    executed untrusted contributor code) is stamped `is_untrusted_fork`, and the **Exclusion**
//!    operator gates its read by construction — a fork run cannot leak its output to a subject the edge
//!    excludes, even one who can `view` the run.
//!
//! Survival signal: **secret-inheritance leaks = 0** AND **fork-read leaks = 0**, projected onto the
//! load-bearing [`CrossTenantCount`]-style zero (the same zero-leak survival signal `git_d8` asserts).
//! A non-zero on EITHER counter means a secret was reachable via project inheritance, or an
//! untrusted-fork run's output leaked — and the drill aborts LOUDLY (EI-01 §3: loud, never swallowed;
//! the threshold is NEVER weakened to pass).
//!
//! Run against the failure-injection harness's telemetry-assertion library (the contract-1.8
//! survival-signal set), exactly as `git_d8` does. `myelin-harness` is a DEV-dependency only — it never
//! enters the identity-service production DAG.

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

/// **CI-D10 (fragment side) — secret-non-inheritance: 0 secret reads via project inheritance.**
///
/// Seed a CI project with a fleet of `ci_project` readers/admins, and a secret that records its
/// `parent_ci_project` (lifecycle/listing) but is granted to NOBODY directly. A batch of project
/// readers/admins each attempt `secret.read` — every one must DENY (the read is the DIRECT NARROW
/// relation, NOT inherited, CI-1). Only the one principal with a direct `secret#direct_reader` grant
/// reads it. We assert the secret-inheritance-leak count is `0`.
#[test]
fn ci_d10_secret_read_is_not_reachable_via_project_inheritance() {
    let mut signals = SignalSource::new();
    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    // A fleet of project members + the secret's project relation + ONE direct grant.
    let mut tuples: Vec<TupleDelta> = vec![
        // The secret belongs to ci_project:web (a lifecycle relation), but read does NOT inherit it.
        add(
            "secret:db-password",
            "parent_ci_project",
            "ci_project:web#view",
        ),
        // The ONLY legitimate path: a direct secret grant to the deploy principal.
        add("secret:db-password", SECRET_DIRECT_READER, "p:deployer"),
    ];
    // 64 project readers + an admin — none of whom should reach the secret.
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

    // Sanity: a project admin really CAN administer the project (the inheritance edge is live)…
    assert!(
        allows(
            &svc,
            &principal("acme", "p:proj-admin"),
            "administer",
            "ci_project:web"
        ),
        "a CI-project admin administers the project (the project edge resolves)"
    );
    // …and the direct grantee reads the secret (the only path).
    assert!(
        allows(
            &svc,
            &principal("acme", "p:deployer"),
            CI_READ,
            "secret:db-password"
        ),
        "the direct secret#direct_reader grantee reads the secret (CI-1: the only path)"
    );

    // THE ATTACK: every project reader + the project admin attempts secret.read via inheritance.
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
         project inheritance (secret.read = direct_reader, DIRECT NARROW — NOT ∪ parent_ci_project->…) \
         → inheritance-leak count=0; only the direct secret#direct_reader grantee reads it (CI-1, §1)"
    );
}

/// **CI-D10 (fragment side) — the `!is_untrusted_fork` ABAC edge gates correctly: 0 fork-read leaks.**
///
/// A run from a trusted source is readable by anyone who can `view` it; an untrusted-fork run is
/// stamped `is_untrusted_fork` and its read is gated by the Exclusion (`read = view − is_untrusted_fork`).
/// A fleet of would-be readers attempt `run.read` on a fork run they are stamped out of — every one
/// must DENY. We assert the fork-read-leak count is `0`, and that the SAME readers CAN view + read the
/// trusted run (the edge gates the fork, not the legitimate path).
#[test]
fn ci_d10_is_untrusted_fork_edge_gates_run_read() {
    let mut signals = SignalSource::new();
    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    const READERS: usize = 64;
    let mut tuples: Vec<TupleDelta> = vec![
        // Both runs belong to repo:core; run.view = parent_repo->pull.
        add("run:trusted", "parent_repo", "repo:core#pull"),
        add("run:fork", "parent_repo", "repo:core#pull"),
    ];
    // A fleet of repo readers (each can pull → can view both runs) — but each is stamped
    // is_untrusted_fork on the FORK run (CI stamps the run from its provenance).
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

    // Sanity: a reader CAN view the fork run (view is unconditional) AND read the trusted run.
    let reader0 = principal("acme", "p:reader-0");
    assert!(
        allows(&svc, &reader0, "view", "run:fork"),
        "a repo reader views the fork run (run.view is unconditional)"
    );
    assert!(
        allows(&svc, &reader0, CI_READ, "run:trusted"),
        "a repo reader reads the TRUSTED run's output (view − ∅)"
    );

    // THE ATTACK: every stamped reader attempts run.read on the untrusted-fork run.
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
         fork, not the legitimate path) — CI stamps trust_tier, Identity never recomputes trust (C7)"
    );
}
