//! # The CDC pair for contract 4.9 (CI half) — the **CI ReBAC namespace fragment** (CI-P8 → P-351, M4)
//!
//! **Contract-index row 4.9** (per-subsystem ReBAC namespace fragment — each subsystem DECLARES its
//! relations + permissions; Identity owns the engine + the admit-contract + the core hierarchy and
//! never invents object ids). The ENGINE half (Id's compiled rich rewrites — the `secret.read =
//! direct_reader` DIRECT NARROW non-inheritance, the `run.read = run.view − is_untrusted_fork`
//! Exclusion) is pinned BEHAVIOURALLY by the engine crate's `cdc_4_9_ci_fragment.rs` (P-ID-27 / P-320).
//! THIS file is the **CI subsystem side**: the CONSUMER declares its names-only fragment vocabulary +
//! proves the frozen §5.2 relation/permission NAMES (incl. the FROZEN `read & !is_untrusted_fork` edge
//! relations + the protected-env `approve` `list_subjects` target + the `watcher` read-fanout
//! relation), so Identity's cell schema compiles against it (the build-time gate of this prompt).
//!
//! ## The name-agreement anchor (why this side does not import the engine crate)
//! `myelin-ci-controlplane` is a CI subsystem LEAF consumer; the §2.9 acyclic DAG forbids it depending
//! on the ReBAC ENGINE (`myelin-identity-service`). The PROVIDER (the engine's compiled rewrites)
//! cannot be imported here, so the name-agreement is asserted against the architecture §5.2 frozen
//! vocabulary literals. The engine-side CDC (`cdc_4_9_ci_fragment.rs`) asserts the SAME literals
//! against its rich `FragmentDef` AND admits them through the real engine (proving the fragment COMPILES
//! into the cell schema + the two security-critical rewrites resolve); a rename on either side is a CDC
//! break, never a silent drift (EI-01 §7). The two CDCs together are the row-4.9 CI slice: this one
//! freezes the CONSUMER's declared shape, that one proves the PROVIDER admits + resolves it.

use myelin_ci_controlplane::rebac_fragment::{
    self, ci_fragment, ci_project_fragment, environment_fragment, object_types, run_fragment,
    secret_fragment, ADMIN, ADMINISTER, APPROVE, APPROVER, DEPLOY, DEPLOYER, IS_UNTRUSTED_FORK,
    PARENT_CI_PROJECT, PARENT_REPO, READ, READER, ROLLBACK, SECRET_DIRECT_READER, TRIGGER, VIEW,
    WATCHER,
};
use myelin_identity::{NamespaceFragment, Permission, RelName};

fn rels(f: &NamespaceFragment) -> Vec<String> {
    f.relations.iter().map(|r| r.0.clone()).collect()
}
fn perms(f: &NamespaceFragment) -> Vec<String> {
    f.permissions.iter().map(|p| p.0.clone()).collect()
}

/// **CONSUMER → the CI fragment declares the four §5.2 object types in root-first order.** The
/// vocabulary Identity admits into the cell schema (`ci_project` → `environment` → `secret` → `run`).
/// These are byte-identical to the canonical §5.2 frozen `namespace` spelling
/// (`ci_project`/`environment`/`secret`/`run`, contract-index row 4.9).
#[test]
fn cdc_4_9_ci_declares_the_four_object_types() {
    let frags = ci_fragment();
    let types: Vec<String> = frags.iter().map(|f| f.object_type.0.clone()).collect();
    assert_eq!(
        types,
        ["ci_project", "environment", "secret", "run"],
        "the four frozen CI object types, root-first (§5.2 / contract-index 4.9)"
    );
    // the canonical constants agree with the declared types (one source of truth).
    assert_eq!(object_types::CI_PROJECT, "ci_project");
    assert_eq!(object_types::ENVIRONMENT, "environment");
    assert_eq!(object_types::SECRET, "secret");
    assert_eq!(object_types::RUN, "run");
}

/// **CONSUMER ↔ the canonical §5.2 vocabulary: every relation/permission NAME is frozen.** A rename
/// here is a CDC break (the engine-side CDC asserts the SAME literals against its rich `FragmentDef`).
#[test]
fn cdc_4_9_relation_and_permission_names_match_the_frozen_vocabulary() {
    // ci_project (§5.2 `namespace ci_project`).
    assert_eq!(rels(&ci_project_fragment()), [PARENT_REPO, READER, ADMIN]);
    assert_eq!(perms(&ci_project_fragment()), [VIEW, ADMINISTER]);
    // environment (§5.2 `namespace ci_environment`).
    assert_eq!(
        rels(&environment_fragment()),
        [PARENT_CI_PROJECT, DEPLOYER, APPROVER]
    );
    assert_eq!(perms(&environment_fragment()), [DEPLOY, APPROVE, ROLLBACK]);
    // secret (§5.2 `namespace ci_secret`, CI-1).
    assert_eq!(
        rels(&secret_fragment()),
        [PARENT_CI_PROJECT, SECRET_DIRECT_READER]
    );
    assert_eq!(perms(&secret_fragment()), [READ]);
    // run (§5.2 `namespace ci_run`).
    assert_eq!(
        rels(&run_fragment()),
        [PARENT_REPO, IS_UNTRUSTED_FORK, WATCHER]
    );
    assert_eq!(perms(&run_fragment()), [VIEW, TRIGGER, READ]);
}

/// **THE FROZEN `read & !is_untrusted_fork` ABAC edge — its CI-side names anchor (the headline
/// security-critical invariant).** A fork subject is classified as a NON-reader BY CONSTRUCTION: the
/// `is_untrusted_fork` SUBTRACTED relation and the `read` permission it gates are BOTH declared on
/// `run`, so the engine's `run.read = run.view − is_untrusted_fork` Exclusion compiles (it would be
/// REJECTED at admit — UndeclaredRelation — if either name were missing/renamed). This is the
/// fork-tier-never-reads rule the prompt names: the names the engine subtracts a fork's read by.
#[test]
fn cdc_4_9_the_read_and_not_is_untrusted_fork_edge_classifies_a_fork_as_non_reader() {
    let run = run_fragment();
    // the SUBTRACTED arm (the fork stamp) is declared on `run`.
    assert!(
        run.relations.contains(&RelName(IS_UNTRUSTED_FORK.into())),
        "the `is_untrusted_fork` SUBTRACTED relation is declared on `run` (the fork-tier stamp, §5.2)"
    );
    // the gated permission is declared on `run`.
    assert!(
        run.permissions.contains(&Permission(READ.into())),
        "`run.read` (gated by − is_untrusted_fork) is declared on `run`"
    );
    // The engine compiles `run.read = run.view − is_untrusted_fork`. The CONSUMER-side guarantee this
    // CDC pins: a subject stamped `is_untrusted_fork` (a fork run) is, by construction, NOT in the
    // `read` set even when it is in `view` — the Exclusion removes it. Here we assert the names that
    // make that exclusion compilable; the behavioural resolution is the engine-side CDC
    // (`cdc_4_9_ci_fragment.rs::cdc_4_9_run_read_is_gated_by_the_is_untrusted_fork_edge`).
    assert_eq!(
        IS_UNTRUSTED_FORK, "is_untrusted_fork",
        "the FROZEN edge name (the `!is_untrusted_fork` exclusion driver) — drift here is a compile break"
    );
}

/// **`secret.read` is the DIRECT NARROW relation (CI-1) — no project-read inheritance on the CI side.**
/// `direct_reader` is the only read path; `read` is the only permission; `parent_ci_project` is a
/// lifecycle relation, NOT a second `view`-style inherited permission. A fork (or any project reader)
/// reaches a secret ONLY through an explicit direct grant.
#[test]
fn cdc_4_9_secret_read_is_direct_narrow_not_inherited() {
    let secret = secret_fragment();
    assert!(
        secret
            .relations
            .contains(&RelName(SECRET_DIRECT_READER.into())),
        "`direct_reader` (the only secret-read path, CI-1) is declared"
    );
    assert_eq!(
        perms(&secret),
        [READ],
        "secret declares ONLY `read` — no project-inherited view permission (CI-1 non-inheritance)"
    );
    // `parent_ci_project` is declared (lifecycle/listing) but is NOT a permission, so declaring it
    // cannot make `read` inherit — the names-only guard against smuggling inheritance in here.
    assert!(
        secret
            .relations
            .contains(&RelName(PARENT_CI_PROJECT.into())),
        "`parent_ci_project` is a lifecycle relation (NOT a read path)"
    );
}

/// **The protected-env HITL `approve` `list_subjects` target is declared (§5.2 / contract 4.4).** The
/// `approver` relation + the `approve` permission on `environment` are the resolver target the
/// secret-broker / protected-env HITL gate (CI-P24) consumes via `list_subjects(environment, approve)`.
#[test]
fn cdc_4_9_environment_declares_the_approve_list_subjects_target() {
    let env = environment_fragment();
    assert!(
        env.relations.contains(&RelName(APPROVER.into())),
        "`approver` (the HITL list_subjects target) is declared (§5.2)"
    );
    assert!(
        env.permissions.contains(&Permission(APPROVE.into())),
        "`approve` (the HITL approval permission) is declared (§5.2 / contract 4.4)"
    );
}

/// **The `watcher` read-fanout relation is on the watchable `run` type (§5.2 / contract 4.4).** Notif
/// resolves `list_subjects(run, watcher)` for the unbounded ambient watcher set.
#[test]
fn cdc_4_9_watcher_is_on_the_watchable_run_type() {
    assert!(
        run_fragment().relations.contains(&RelName(WATCHER.into())),
        "the `run` watchable type declares `watcher` (Notif read-fanout, §5.2)"
    );
}

/// **No CI fragment name smuggles an object id (Id never invents object ids).** Every type/relation/
/// permission NAME is a bare identifier (no `:`/`/`/`#`) — mirrors the engine's `mints_object_id` admit
/// check; a fragment that tripped it would be REJECTED at admit.
#[test]
fn cdc_4_9_no_ci_name_smuggles_an_object_id() {
    let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
    for f in ci_fragment() {
        assert!(!mints(&f.object_type.0), "type name is a bare identifier");
        for r in &f.relations {
            assert!(!mints(&r.0), "relation `{}` is a bare identifier", r.0);
        }
        for p in &f.permissions {
            assert!(!mints(&p.0), "permission `{}` is a bare identifier", p.0);
        }
    }
    // the module re-exports the single canonical entry point.
    assert_eq!(rebac_fragment::ci_fragment().len(), 4);
}
