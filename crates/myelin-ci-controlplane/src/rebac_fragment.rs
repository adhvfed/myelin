//! # `rebac_fragment` — the CI ReBAC namespace fragment (contract 4.9, FROZEN, CI-P8 / P-351)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md`
//! §5.2 (the CI ReBAC namespace fragment — *declared by CI, engine owned by Id, FROZEN*: the
//! `ci_project` / `environment` / `secret` / `run` definitions + the **`read & !is_untrusted_fork`**
//! ABAC edge — the fork-tier-never-reads rule + the `approve`r `list_subjects` target + the `watcher`
//! relation per watchable type).
//!
//! **Reconciliation (FROZEN):**
//! `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §1 — the frozen
//! CI ReBAC fragment (`ci_project` / `environment` / `secret` / `run` + `read & !is_untrusted_fork`):
//! - **CI-1 (secret-non-inheritance):** a secret is NEVER reachable through `ci_project` read
//!   inheritance; the only path to a secret is a DIRECT `secret#direct_reader@subject` grant.
//! - **X-1 (the `trust_tier` / fork-endorsement seam):** CI stamps `trust_tier ∈ {trusted,
//!   untrusted_fork}` from run provenance via the `read & !is_untrusted_fork` ABAC edge; Git reads
//!   `trust_tier` off the §5.9 `CheckStatus` fact and **Identity never recomputes trust**.
//!
//! **Contract-index row 4.9 (OWNED here — the CI fragment slice):** the per-subsystem ReBAC namespace
//! fragment. Identity owns the *engine + admit-contract + core hierarchy*
//! (`myelin-identity-service::namespace`, P-068); CI owns *this fragment's definition*. The contract
//! boundary Identity compiles against is the frozen names-only ABI carrier
//! [`myelin_identity::NamespaceFragment`] — this module emits exactly that, one carrier per CI object
//! type, so **Identity's cell schema compiles against the CI fragment** (the gate of this prompt — a
//! build-time property, not a runtime drill).
//!
//! ## Reconciliation with the EXISTING Id-side compiled fragment (EI-01 §7 — extend, never re-define)
//! The Id-side **rich** CI fragment ALREADY exists — `myelin_identity_service::ci_fragment` (P-ID-27 /
//! P-320), which carries the permission **rewrites** (the `secret.read = direct_reader` DIRECT NARROW
//! non-inheritance, the `run.read = run.view − is_untrusted_fork` Exclusion) the engine resolves
//! `check`/`list_objects` through, plus its CDC (`cdc_4_9_ci_fragment.rs`). That side is the PROVIDER.
//! This module is the **CI subsystem CONSUMER side** — the read-side names-only carrier the same way
//! Git ([`myelin_git`]) / Issues ([`myelin_issues`]) / Knowledge ([`myelin_knowledge`]) / Chat
//! ([`myelin_chat`]) each ship their own `rebac_fragment`. The two sides MUST agree byte-for-byte on
//! the relation/permission NAMES; the CDC (`tests/cdc_4_9_ci_rebac_fragment.rs`) pins that agreement
//! against the architecture §5.2 frozen vocabulary literals (it does NOT import the engine crate — the
//! §2.9 acyclic DAG forbids a leaf consumer depending on `myelin-identity-service`).
//!
//! ## What this prompt (CI-P8 / P-351) ships — and what it deliberately does NOT (VISION §3)
//! **Ships:** the FROZEN CI fragment as data — the four CI object types and their relations +
//! permission NAMES, in the frozen [`myelin_identity::NamespaceFragment`] shape Identity admits:
//!
//! | object type   | relations                                                  | permission names                        |
//! |---------------|------------------------------------------------------------|-----------------------------------------|
//! | `ci_project`  | `parent_repo` `reader` `admin`                             | `view` `administer`                     |
//! | `environment` | `parent_ci_project` `deployer` `approver`                  | `deploy` `approve` `rollback`           |
//! | `secret`      | `parent_ci_project` `direct_reader`                        | `read`                                  |
//! | `run`         | `parent_repo` `is_untrusted_fork` `watcher`                | `view` `trigger` `read`                 |
//!
//! Every relation the architecture §5.2 names is present:
//! - **`approver`** (on `environment`) — the protected-env HITL approval relation; the
//!   `list_subjects(environment, approve)` target that resolves the approver set for a protected
//!   deploy (the HITL gate the secret-broker prompt CI-P24 consumes; contract 4.4).
//! - **`is_untrusted_fork`** (on `run`) — the SUBTRACTED arm of the `read` exclusion: a run CI stamps
//!   `is_untrusted_fork` (a fork PR / a run that executed untrusted contributor code) is removed from
//!   `read` BY CONSTRUCTION (the frozen `read & !is_untrusted_fork` ABAC edge), so a fork run can never
//!   read a secret / turn its own gate green (EI-02 §1 blast-radius; the poisoned-pipeline attack).
//! - **`direct_reader`** (on `secret`) — the DIRECT NARROW secret-read relation (CI-1): `secret.read`
//!   is NOT unioned with any `parent_ci_project->…` inheritance, so a `ci_project` reader/admin CANNOT
//!   reach a secret; the only path is an explicit `secret#direct_reader@subject` grant.
//! - **`watcher`** (on `run`) — the Notif read-fanout relation (Notif resolves
//!   `list_subjects(run, watcher)` for the unbounded ambient set, contract 4.4 / §5.2): a run's
//!   watchers = its trigger-actor + project members who opted in.
//!
//! The frozen permission **rewrites** (names freeze here; the rewrite STRUCTURE is documented below
//! and is the engine-side `myelin_identity_service::ci_fragment`'s compiled `Userset` rewrite — proven
//! admissible by THAT side's CDC against the real engine):
//! - `ci_project.view       = reader ∪ admin ∪ parent_repo->pull`  (a repo viewer can view its CI)
//! - `ci_project.administer = admin`
//! - `environment.deploy    = deployer ∪ parent_ci_project->administer`
//! - `environment.approve   = approver`  ← the HITL approver `list_subjects` target
//! - `environment.rollback  = deployer ∪ approver`
//! - `secret.read           = direct_reader`  ← the DIRECT NARROW non-inheritance (CI-1)
//! - `run.view              = parent_repo->pull`
//! - `run.trigger           = parent_repo->push`
//! - `run.read              = run.view − is_untrusted_fork`  ← the FROZEN `read & !is_untrusted_fork`
//!   ABAC edge (the fork-tier-never-reads rule)
//!
//! **Does NOT ship (FLOORS named — VISION §3):** *no CI feature.* No tuples are written, no
//! `check`/`list_objects` is served, no run/secret/deploy gate evaluates here. This is a
//! **contract-fragment freeze** — the relation/permission SHAPES Identity compiles against, nothing
//! more. The fragment CONSUMERS land in their own prompts:
//! - the **trust-tier classifier** that stamps `run#is_untrusted_fork@subject` from run provenance —
//!   CI-P12 (the trust classification + the single trust stamp);
//! - the **secret-broker fork-no-secrets edge** (a fork job gets no secrets, CI-D7) — CI-P24;
//! - the **`list_objects` push-down** that lowers `Id.list_objects(viewer, read, ci_run)`'s `SetExpr`
//!   into CI's `run_id` column (OQ-E) — CI-P25.
//!
//! ## Why names-only here (the DAG, EI-01 §7 — extend, never re-define)
//! `myelin-ci-controlplane` is a CI subsystem leaf consumer; it depends on the frozen contract surface
//! `myelin-identity` (which carries the names-only [`myelin_identity::NamespaceFragment`]), NOT on
//! `myelin-identity-service` (the rich `FragmentDef`/`Userset` engine — a service crate). So the
//! *runtime* fragment CI ships is the names-only carrier Identity's `admit_fragment` consumes; the
//! rich rewrite structure (incl. the `− is_untrusted_fork` Exclusion + the DIRECT NARROW
//! `secret.read`) lives on the engine side (`myelin_identity_service::ci_fragment`, already shipped),
//! never re-defined here. This keeps the §2.9 crate DAG acyclic (no consumer→service edge) while still
//! freezing the full fragment shape Identity must compile.

use myelin_identity::{NamespaceFragment, ObjectType, Permission, RelName};

/// The four frozen CI object-type names (the §5.2 `namespace` blocks). Public so the live-wiring
/// prompts (CI-P12/P24/P25 / P-ID-*) and the CDC test reference the SAME canonical strings (one
/// source of truth — a typo here is a typo everywhere, caught by the admit). These are byte-identical
/// to the engine-side `myelin_identity_service::ci_fragment::object_types`.
pub mod object_types {
    /// A CI project — the pipeline/config root, parented to a Git `repo` (§5.2 `namespace ci_project`).
    pub const CI_PROJECT: &str = "ci_project";
    /// A deployment environment — the `deploy`/`approve`/`rollback` gate target, and the protected-env
    /// HITL `approve` `list_subjects` target (§5.2 `namespace ci_environment`/`environment`).
    pub const ENVIRONMENT: &str = "environment";
    /// A secret — the DIRECT-NARROW-read object (NOT reachable via project-read inheritance, CI-1;
    /// §5.2 `namespace ci_secret`/`secret`).
    pub const SECRET: &str = "secret";
    /// A CI run — the `is_untrusted_fork`-stamped object (`run.read = view − is_untrusted_fork`) and a
    /// watchable type (Notif read-fanout; §5.2 `namespace ci_run`/`run`).
    pub const RUN: &str = "run";
}

// ---- the frozen relation names (§5.2) ---------------------------------------------------------

/// `ci_project.parent_repo` — the inheritance edge into the Git `repo` fragment (a repo viewer can
/// view its CI; `ci_project.view` unions `parent_repo->pull`).
pub const PARENT_REPO: &str = "parent_repo";
/// `ci_project.reader` — a direct CI-project reader.
pub const READER: &str = "reader";
/// `ci_project.admin` — a CI-project admin (the `administer` grantee; the `view` admin arm).
pub const ADMIN: &str = "admin";
/// `environment.parent_ci_project` / `secret.parent_ci_project` — the project an environment/secret
/// belongs to (a lifecycle/listing relation; `environment.deploy` inherits the project's
/// `administer`, but `secret.read` deliberately does NOT inherit — CI-1).
pub const PARENT_CI_PROJECT: &str = "parent_ci_project";
/// `environment.deployer` — a direct deployer (the `deploy`/`rollback` grantee).
pub const DEPLOYER: &str = "deployer";
/// **`environment.approver`** — the protected-env HITL approval relation; the
/// `list_subjects(environment, approve)` target that resolves the approver set for a protected deploy
/// (contract 4.4; consumed by the secret-broker / protected-env HITL gate CI-P24).
pub const APPROVER: &str = "approver";
/// **`secret.direct_reader`** — the DIRECT NARROW secret-read relation (CI-1): the ONLY path to a
/// secret. `secret.read` is NOT inherited from `parent_ci_project`.
pub const SECRET_DIRECT_READER: &str = "direct_reader";
/// **`run.is_untrusted_fork`** — the ABAC-stamp relation (the SUBTRACTED arm of `run.read`): CI stamps
/// a run that executed untrusted contributor code (a fork PR), and the `read = view − is_untrusted_fork`
/// Exclusion removes the stamped subjects BY CONSTRUCTION (the frozen `read & !is_untrusted_fork` edge).
pub const IS_UNTRUSTED_FORK: &str = "is_untrusted_fork";
/// **`run.watcher`** — the Notif read-fanout relation per watchable type (§5.2): a run's watchers are
/// resolved by `list_subjects(run, watcher)` for the unbounded ambient set (contract 4.4).
pub const WATCHER: &str = "watcher";

// ---- the frozen permission names (§5.2) -------------------------------------------------------

/// `ci_project.view` / `run.view` — the view gate (`ci_project.view = reader ∪ admin ∪
/// parent_repo->pull`; `run.view = parent_repo->pull`).
pub const VIEW: &str = "view";
/// `ci_project.administer` — the privileged project-management gate (`administer = admin`).
pub const ADMINISTER: &str = "administer";
/// `environment.deploy` — the deploy gate (`deploy = deployer ∪ parent_ci_project->administer`).
pub const DEPLOY: &str = "deploy";
/// **`environment.approve`** — the protected-env HITL approval permission (`approve = approver`); the
/// `list_subjects` target that resolves the HITL approver set (contract 4.4).
pub const APPROVE: &str = "approve";
/// `environment.rollback` — the rollback gate (`rollback = deployer ∪ approver`).
pub const ROLLBACK: &str = "rollback";
/// `run.trigger` — the manual-trigger gate (`trigger = parent_repo->push`).
pub const TRIGGER: &str = "trigger";
/// **`secret.read` / `run.read`** — the read gate on `secret` (DIRECT NARROW, CI-1) and `run` (gated
/// by the FROZEN `read & !is_untrusted_fork` ABAC edge: `run.read = run.view − is_untrusted_fork`).
pub const READ: &str = "read";

/// Build a [`NamespaceFragment`] (the frozen names-only ABI carrier) from `&str` slices — a small
/// constructor that keeps the four fragment definitions below declarative.
fn fragment(object_type: &str, relations: &[&str], permissions: &[&str]) -> NamespaceFragment {
    NamespaceFragment {
        object_type: ObjectType(object_type.to_string()),
        relations: relations.iter().map(|r| RelName(r.to_string())).collect(),
        permissions: permissions
            .iter()
            .map(|p| Permission(p.to_string()))
            .collect(),
    }
}

/// **The `ci_project` fragment** (§5.2 `namespace ci_project`) — the pipeline/config root.
///
/// Relations: `parent_repo` (the inheritance edge into the Git `repo` fragment — a repo viewer can
/// view its CI), `reader`, `admin`. Permissions (names frozen here; rewrites on the engine side):
/// - `view       = reader ∪ admin ∪ parent_repo->pull`
/// - `administer = admin`
///
/// **Crucially, the project's `view` does NOT reach a `secret`** — the secret's read is a separate
/// DIRECT NARROW relation (CI-1); a project reader sees the project but not its secrets.
pub fn ci_project_fragment() -> NamespaceFragment {
    fragment(
        object_types::CI_PROJECT,
        &[PARENT_REPO, READER, ADMIN],
        &[VIEW, ADMINISTER],
    )
}

/// **The `environment` fragment** (§5.2 `namespace ci_environment`) — the deployment-environment gate
/// + the protected-env HITL approver target.
///
/// Relations: `parent_ci_project` (deploy inherits the project's `administer`), `deployer`,
/// **`approver`** (the HITL `list_subjects(environment, approve)` target). Permissions:
/// - `deploy   = deployer ∪ parent_ci_project->administer`
/// - `approve  = approver`  ← the HITL approver set (consumed by CI-P24)
/// - `rollback = deployer ∪ approver`
pub fn environment_fragment() -> NamespaceFragment {
    fragment(
        object_types::ENVIRONMENT,
        &[PARENT_CI_PROJECT, DEPLOYER, APPROVER],
        &[DEPLOY, APPROVE, ROLLBACK],
    )
}

/// **The `secret` fragment** (§5.2 `namespace ci_secret`, CI-1) — the DIRECT-NARROW-read object.
///
/// Relations: `parent_ci_project` (a lifecycle/listing relation — NOT in any read rewrite, so
/// declaring it does NOT make `read` inherit), **`direct_reader`** (the only read path). Permission:
/// - `read = direct_reader`  ← the DIRECT NARROW non-inheritance (CI-1): a CI-project admin who can
///   `administer`, or a repo admin who can `pull`, gets NO secret read; secrets are granted one
///   subject at a time. A change that makes `read` inherit from `parent_ci_project` makes the secret
///   leak via project-read and MUST be caught (the engine-side mutation-tested core).
pub fn secret_fragment() -> NamespaceFragment {
    fragment(
        object_types::SECRET,
        &[PARENT_CI_PROJECT, SECRET_DIRECT_READER],
        &[READ],
    )
}

/// **The `run` fragment** (§5.2 `namespace ci_run`) — the run-visibility + `!is_untrusted_fork` ABAC
/// object + a watchable type (Notif read-fanout).
///
/// Relations: `parent_repo` (run.view/trigger inherit the repo's pull/push), **`is_untrusted_fork`**
/// (the SUBTRACTED arm of `read`), **`watcher`** (Notif read-fanout). Permissions:
/// - `view    = parent_repo->pull`
/// - `trigger = parent_repo->push`
/// - `read    = view − is_untrusted_fork`  ← the FROZEN `read & !is_untrusted_fork` ABAC edge: a run
///   stamped `is_untrusted_fork` (a fork PR / untrusted contributor code) is removed from `read` BY
///   CONSTRUCTION, so a fork run's output/artifacts are gated and a fork can never turn its own gate
///   green (EI-02 §1; the poisoned-pipeline-execution attack). CI stamps the tier; Git reads it off
///   the §5.9 `CheckStatus` fact; Identity never recomputes trust.
pub fn run_fragment() -> NamespaceFragment {
    fragment(
        object_types::RUN,
        &[PARENT_REPO, IS_UNTRUSTED_FORK, WATCHER],
        &[VIEW, TRIGGER, READ],
    )
}

/// **The complete frozen CI ReBAC namespace fragment** — the four [`NamespaceFragment`] carriers
/// Identity admits into the one cell schema (contract 4.9). Listed root-first (`ci_project` →
/// `environment` → `secret` → `run`); the CI inheritance edges (`parent_repo->pull/push`) terminate on
/// the **Git `repo`** fragment's compiled permissions, resolved at check-time on the parent's schema,
/// not on a CI type — so admit order among the CI types does not matter, but root-first reads cleanest.
/// This is the SINGLE entry point the live-wiring prompts + the CDC test consume.
pub fn ci_fragment() -> Vec<NamespaceFragment> {
    vec![
        ci_project_fragment(),
        environment_fragment(),
        secret_fragment(),
        run_fragment(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four frozen object types are present + non-empty, in root-first order (§5.2). A name
    /// dropped here breaks the cell-schema compile.
    #[test]
    fn the_four_ci_object_types_are_frozen() {
        let frag = ci_fragment();
        let types: Vec<&str> = frag.iter().map(|f| f.object_type.0.as_str()).collect();
        assert_eq!(types, vec!["ci_project", "environment", "secret", "run"]);
    }

    /// The frozen relation set per object type (§5.2) — the well-formedness witness. A relation
    /// dropped or renamed here is caught BEFORE it reaches Identity's admit.
    #[test]
    fn each_definition_declares_its_frozen_relations() {
        let rels = |f: &NamespaceFragment| -> Vec<String> {
            f.relations.iter().map(|r| r.0.clone()).collect()
        };
        assert_eq!(
            rels(&ci_project_fragment()),
            vec![PARENT_REPO, READER, ADMIN]
        );
        assert_eq!(
            rels(&environment_fragment()),
            vec![PARENT_CI_PROJECT, DEPLOYER, APPROVER]
        );
        assert_eq!(
            rels(&secret_fragment()),
            vec![PARENT_CI_PROJECT, SECRET_DIRECT_READER]
        );
        assert_eq!(
            rels(&run_fragment()),
            vec![PARENT_REPO, IS_UNTRUSTED_FORK, WATCHER]
        );
    }

    /// **The `read & !is_untrusted_fork` edge's structural anchor (the FROZEN ABAC edge).** Both the
    /// `is_untrusted_fork` SUBTRACTED relation and the `read` permission it gates are declared on `run`
    /// — the engine's `run.read = run.view − is_untrusted_fork` Exclusion references both, so a
    /// fragment missing either would be REJECTED at admit (UndeclaredRelation). This is the
    /// fork-tier-never-reads rule's names anchor on the CI side.
    #[test]
    fn the_is_untrusted_fork_edge_relations_are_declared_on_run() {
        let run = run_fragment();
        assert!(
            run.relations.contains(&RelName(IS_UNTRUSTED_FORK.into())),
            "`is_untrusted_fork` (the SUBTRACTED arm of `run.read`) must be declared (§5.2)"
        );
        assert!(
            run.permissions.contains(&Permission(READ.into())),
            "`run.read` (gated by the !is_untrusted_fork edge) must be declared (§5.2)"
        );
    }

    /// **`secret.read` is the DIRECT NARROW relation (CI-1) — `direct_reader` is declared on `secret`
    /// and `read` is its only permission.** `parent_ci_project` is declared (for lifecycle/listing)
    /// but is NOT a permission arm here (the names-only carrier cannot express the rewrite — the
    /// engine side proves non-inheritance). The secret never grows a `view`-style project-inheritance
    /// permission on the CI side.
    #[test]
    fn secret_declares_direct_reader_and_only_read() {
        let secret = secret_fragment();
        assert!(
            secret
                .relations
                .contains(&RelName(SECRET_DIRECT_READER.into())),
            "`direct_reader` (the only secret-read path, CI-1) must be declared"
        );
        assert_eq!(
            secret.permissions,
            vec![Permission(READ.into())],
            "secret declares ONLY `read` (the DIRECT NARROW gate) — no project-inherited view perm"
        );
    }

    /// **The protected-env HITL `approve` `list_subjects` target is declared (§5.2 / contract 4.4).**
    /// The `approver` relation + the `approve` permission on `environment` are the resolver target for
    /// `list_subjects(environment, approve)` (the HITL approver set CI-P24 consumes).
    #[test]
    fn environment_declares_the_approve_list_subjects_target() {
        let env = environment_fragment();
        assert!(
            env.relations.contains(&RelName(APPROVER.into())),
            "`approver` (the HITL list_subjects target) must be declared (§5.2)"
        );
        assert!(
            env.permissions.contains(&Permission(APPROVE.into())),
            "`approve` (the HITL approval permission) must be declared (§5.2 / 4.4)"
        );
    }

    /// **The `watcher` read-fanout relation is on the watchable `run` type (§5.2 / contract 4.4).**
    /// Notif resolves `list_subjects(run, watcher)` for the unbounded ambient set.
    #[test]
    fn watcher_is_declared_on_the_watchable_run_type() {
        assert!(
            run_fragment().relations.contains(&RelName(WATCHER.into())),
            "the `run` watchable type declares `watcher` (Notif read-fanout, §5.2)"
        );
    }

    /// No fragment smuggles an object id (Id never invents object ids): every type/relation/permission
    /// NAME is a bare identifier (no `:`/`/`/`#`). This mirrors the engine's `mints_object_id` admit
    /// check — a fragment that tripped it would be REJECTED at admit.
    #[test]
    fn no_ci_name_smuggles_an_object_id() {
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
    }
}
