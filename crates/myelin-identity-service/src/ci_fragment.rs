//! # `ci_fragment` — Id's compiled **CI** ReBAC namespace fragment (contract 4.9, P-ID-27 → P-320)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §5 (the **frozen CI fragment**: `ci_project`, `environment`, `secret`, `run`;
//! **`run.view = parent_repo->pull`**; **`run.trigger = parent_repo->push`**; **`secret.read` is NOT
//! inherited** — a DIRECT NARROW relation, so secrets never leak via project-read inheritance, CI-1;
//! **plus the `read & !is_untrusted_fork` ABAC edge** (C7) — CI stamps `trust_tier` from run
//! provenance using this edge, a fork run is `untrusted_fork`).
//!
//! **Reconciliation (FROZEN):**
//! - `00-reconciliation-decisions.md` §1 (**CI-1: secret-non-inheritance** — secrets are NEVER reachable
//!   through `ci_project` read inheritance; the only path to a secret is a direct `secret.read` grant).
//! - `00-reconciliation-decisions.md` §X-1 (the `trust_tier` / fork-endorsement seam: CI stamps
//!   `trust_tier ∈ {trusted, untrusted_fork}` from run provenance via the `read & !is_untrusted_fork`
//!   ABAC edge; Git reads `trust_tier` off the fact and **Identity never recomputes trust**). The §5.9
//!   `CheckStatus` seam reads the stamped `trust_tier`; this fragment is the Id-side ABAC edge it rides.
//!
//! **Contract-index row 4.9 (OWNED here — the Identity-side compiled CI fragment).** Identity owns the
//! **engine + admit-contract + core hierarchy** ([`crate::namespace`], P-068). This module is the
//! **THIRD of the five per-subsystem fragments** (P-ID-24/26/27/29/30) that promote the M1 engine-only
//! floor; the first (Git) is [`crate::git_fragment`], the second (Knowledge) is
//! [`crate::knowledge_fragment`]. Like them it is the canonical **rich** [`crate::namespace::FragmentDef`]
//! declaration of the CI authz vocabulary, with the permission **rewrites** wired over the four
//! Zanzibar userset operators so `check`/`list_objects` resolve the CI permissions through the SAME
//! engine the core hierarchy uses (one primitive — no bespoke CI check path, the §5 design rule). The
//! CI data model (the pipeline/run tables themselves) is the CI-platform prompts'; this module ships
//! only the Id-side authz content. The self-hosted-runner token SCOPE is exercised AGAINST this live
//! fragment by P-ID-28 (P-321) — see `tests/drill_ci_d10_self_hosted_scope.rs` (CI-D10 scope side: a
//! self-hosted runner token is bounded to one tenant's `SelfHosted` jobs → 0 cross-tenant job/secret
//! reads against this fragment) + `tests/cdc_4_7_self_hosted_ci_scope.rs` (the CI-dispatch consumer ↔
//! Identity mint+check provider pair). The scope MECHANISM is the [`crate::mint`] one-tenant ceiling
//! (P-ID-18); P-ID-28 proves it end-to-end against the CI namespace.
//!
//! ## Why the rich fragment lives HERE (not in `myelin-ci-*`)
//! Same DAG discipline as [`crate::git_fragment`] / [`crate::knowledge_fragment`]:
//! `myelin-identity-service` (the engine) does NOT depend on a subsystem leaf crate (§2.9 acyclic
//! DAG). The names-only ABI carrier [`myelin_identity::NamespaceFragment`] cannot carry the rewrite
//! STRUCTURE (the `secret.read = direct_reader` NON-inheritance, the `run.read = run.view −
//! is_untrusted_fork` exclusion); only the engine's rich `FragmentDef` can. So **Id owns the compiled
//! rewrites** (this module), declared from the architecture §5 frozen vocabulary directly, and the CDC
//! test (`tests/cdc_4_9_ci_fragment.rs`) pins that the two sides agree on the relation/permission NAMES.
//!
//! ## The compiled CI fragment (§5)
//!
//! | object type   | relations                                          | permissions (rewrite)                                                                |
//! |---------------|----------------------------------------------------|--------------------------------------------------------------------------------------|
//! | `ci_project`  | `parent_repo` `reader` `admin`                     | `view = reader ∪ admin ∪ parent_repo->pull`; `administer = admin`                     |
//! | `environment` | `parent_ci_project` `deployer`                     | `deploy = deployer ∪ parent_ci_project->administer`                                   |
//! | `secret`      | `parent_ci_project` `direct_reader`                | **`read = direct_reader`** (DIRECT NARROW — **NOT** `∪ parent_ci_project->view`, CI-1) |
//! | `run`         | `parent_repo` `is_untrusted_fork`                  | `view = parent_repo->pull`; `trigger = parent_repo->push`; **`read = view − is_untrusted_fork`** (the `read & !is_untrusted_fork` ABAC edge, C7) |
//!
//! - **`run.view = parent_repo->pull`** / **`run.trigger = parent_repo->push`** — a run is viewed by
//!   anyone who can pull the parent repo, triggered by anyone who can push it (§5; the inheritance
//!   tuple-to-userset rewrites into the Git `repo` fragment's compiled `pull`/`push`).
//! - **`secret.read` is the DIRECT NARROW relation (CI-1, §1)** — it is NOT unioned with any
//!   `parent_ci_project->…` inheritance, so a `ci_project` reader CANNOT reach a secret: the only path
//!   to a secret is an explicit `secret#direct_reader@subject` grant. A mutation that makes
//!   `secret.read` inheritable MUST be caught (the mutation-tested core path —
//!   [`tests::secret_read_is_not_inherited_from_project`]).
//! - **`run.read = run.view − is_untrusted_fork`** — the `read & !is_untrusted_fork` ABAC edge (C7).
//!   CI stamps a run's `trust_tier` from its provenance: a run that executed untrusted contributor code
//!   (a fork PR) is marked by writing `run#is_untrusted_fork@subject`, which the **Exclusion** operator
//!   subtracts from the run's read set — so an untrusted-fork run's output/artifacts are gated by
//!   construction (the same exclusion crux as Issues' `− confidential` and Knowledge's `− direct_block`,
//!   §5: "the `!is_untrusted_fork` edge … all instances of these four operators"). Git reads the
//!   stamped `trust_tier` off the §5.9 `CheckStatus` fact; **Identity never recomputes trust**.

use crate::namespace::{FragmentDef, PermissionRule, Userset};
use myelin_identity::{ObjectType, Permission, RelName};

/// The four frozen CI object-type names (§5; mirrors the CI subsystem's names-only carrier).
/// Public so the CDC test + a live-wiring caller reference the SAME canonical strings.
pub mod object_types {
    /// A CI project — the pipeline/config root, parented to a Git `repo`.
    pub const CI_PROJECT: &str = "ci_project";
    /// A deployment environment (the `deploy` gate target).
    pub const ENVIRONMENT: &str = "environment";
    /// A secret — the DIRECT-NARROW-read object (NOT reachable via project-read inheritance, CI-1).
    pub const SECRET: &str = "secret";
    /// A CI run — the `is_untrusted_fork`-stamped object (`run.read = view − is_untrusted_fork`).
    pub const RUN: &str = "run";
}

/// **The `secret.read` DIRECT NARROW relation name (CI-1, §1).** A secret is read ONLY by a subject
/// holding this relation DIRECTLY on the secret — `secret.read` does NOT inherit from
/// `parent_ci_project`, so a `ci_project` reader cannot reach a secret. Exposed so the CDC + a live
/// caller reference the canonical name, not a stringly-typed literal.
pub const SECRET_DIRECT_READER: &str = "direct_reader";

/// **The `run.is_untrusted_fork` ABAC-stamp relation name (C7, §X-1).** CI stamps a run as an untrusted
/// fork (a run that executed untrusted contributor code) by writing `run#is_untrusted_fork@subject`;
/// the `run.read = view − is_untrusted_fork` **Exclusion** then subtracts the stamped subjects, gating
/// an untrusted-fork run's read by construction. CI derives the `trust_tier` from this edge; Git reads
/// it off the §5.9 fact and Identity never recomputes trust.
pub const IS_UNTRUSTED_FORK: &str = "is_untrusted_fork";

/// **The `read` permission name** — the read gate on `secret` (DIRECT NARROW) and `run` (gated by the
/// `!is_untrusted_fork` ABAC edge). Shared canonical string so the CDC / a live caller do not stringly
/// type it.
pub const READ: &str = "read";

/// **The `view` permission name** on `run`/`ci_project` (`run.view = parent_repo->pull`).
pub const VIEW: &str = "view";

/// **The `trigger` permission name** on `run` (`run.trigger = parent_repo->push`).
pub const TRIGGER: &str = "trigger";

/// **The `deploy` permission name** on `environment` (`deploy = deployer ∪
/// parent_ci_project->administer`). The gate the consequential CI `deploy`/`approve_deploy` agent
/// tools (§6.3) carry as their `required_caps` — exposed so the Fabric registration sources the
/// canonical name, not a stringly-typed literal (a rename here breaks the consumer test).
pub const DEPLOY: &str = "deploy";

/// **The `administer` permission name** on `ci_project` (`administer = admin`). The privileged
/// project-management gate the CI `write_secret` agent tool (§6.3) carries as its `required_caps`
/// (managing a secret is a project-admin op — and a secret's READ is the separate DIRECT NARROW
/// relation, CI-1, never inherited). Exposed canonically (a rename here breaks the consumer test).
pub const ADMINISTER: &str = "administer";

fn rel(n: &str) -> Userset {
    Userset::Relation(RelName(n.into()))
}

fn ttu(tupleset: &str, computed: &str) -> Userset {
    Userset::TupleToUserset {
        tupleset: RelName(tupleset.into()),
        computed: RelName(computed.into()),
    }
}

fn perm(name: &str, rewrite: Userset) -> PermissionRule {
    PermissionRule {
        permission: Permission(name.into()),
        rewrite,
    }
}

fn frag(object_type: &str, relations: &[&str], permissions: Vec<PermissionRule>) -> FragmentDef {
    FragmentDef {
        object_type: ObjectType(object_type.into()),
        relations: relations.iter().map(|r| RelName(r.to_string())).collect(),
        permissions,
    }
}

/// **The `ci_project` fragment** (§5 `definition ci_project`) — the pipeline/config root.
///
/// `view = reader ∪ admin ∪ parent_repo->pull` (a CI-project reader/admin, or anyone who can pull the
/// parent Git repo, can view the project); `administer = admin`. **Crucially, the project's `view` does
/// NOT reach a `secret`** — the secret's read is a separate DIRECT NARROW relation (CI-1); a project
/// reader sees the project but not its secrets.
pub fn ci_project_fragment() -> FragmentDef {
    frag(
        object_types::CI_PROJECT,
        &["parent_repo", "reader", "admin"],
        vec![
            perm(
                VIEW,
                Userset::Union(vec![
                    rel("reader"),
                    rel("admin"),
                    ttu("parent_repo", "pull"),
                ]),
            ),
            perm(ADMINISTER, rel("admin")),
        ],
    )
}

/// **The `environment` fragment** (§5 `definition environment`) — the deployment-environment gate.
///
/// `deploy = deployer ∪ parent_ci_project->administer` (a direct deployer, or a CI-project admin, may
/// deploy to the environment). Environment access inherits PROJECT administration — but a `secret`
/// never does (CI-1): the environment is a deploy target, the secret a separately-granted read object.
pub fn environment_fragment() -> FragmentDef {
    frag(
        object_types::ENVIRONMENT,
        &["parent_ci_project", "deployer"],
        vec![perm(
            DEPLOY,
            Userset::Union(vec![rel("deployer"), ttu("parent_ci_project", ADMINISTER)]),
        )],
    )
}

/// **The `secret` fragment** (§5 `definition secret`, CI-1) — the DIRECT-NARROW-read object.
///
/// **`read = direct_reader`** — a BARE direct relation, deliberately **NOT** unioned with any
/// `parent_ci_project->…` inheritance. This is the secret-non-inheritance invariant (CI-1, §1): the
/// ONLY path to a secret is an explicit `secret#direct_reader@subject` grant. Even a CI-project admin
/// who can `administer` the project, or a repo admin who can `pull`, gets NO secret read — secrets are
/// granted one subject at a time. `parent_ci_project` is declared as a relation (so a secret records
/// which project it belongs to, for listing / lifecycle), but it is NOT in any read rewrite — declaring
/// it does not make `read` inherit. A mutation that adds a `parent_ci_project->…` arm to `read` makes
/// the secret leak via project-read inheritance and MUST be caught (the mutation-tested core).
pub fn secret_fragment() -> FragmentDef {
    frag(
        object_types::SECRET,
        &["parent_ci_project", SECRET_DIRECT_READER],
        vec![
            // read = direct_reader (DIRECT NARROW — NOT inherited from parent_ci_project, CI-1).
            perm(READ, rel(SECRET_DIRECT_READER)),
        ],
    )
}

/// **The `run` fragment** (§5 `definition run`) — the run-visibility + `!is_untrusted_fork` ABAC object.
///
/// `view = parent_repo->pull`; `trigger = parent_repo->push` (a run is viewed by anyone who can pull the
/// parent Git repo, triggered by anyone who can push it — the §5 inheritance into the Git `repo`
/// fragment's compiled `pull`/`push`). **`read = view − is_untrusted_fork`** is the `read &
/// !is_untrusted_fork` ABAC edge (C7): the run's output/artifacts are readable by anyone who can `view`
/// the run UNLESS the run is stamped `is_untrusted_fork` (a fork PR / a run that executed untrusted
/// contributor code). CI writes the `is_untrusted_fork` stamp from run provenance; the **Exclusion**
/// operator subtracts the stamped subjects, gating an untrusted-fork run's read by construction — never
/// a post-filter, never bespoke logic (§5: all of these reduce to the four operators).
pub fn run_fragment() -> FragmentDef {
    frag(
        object_types::RUN,
        &["parent_repo", IS_UNTRUSTED_FORK],
        vec![
            perm(VIEW, ttu("parent_repo", "pull")),
            perm(TRIGGER, ttu("parent_repo", "push")),
            // read = view − is_untrusted_fork (the `read & !is_untrusted_fork` ABAC edge, C7).
            perm(
                READ,
                Userset::Exclusion {
                    base: Box::new(ttu("parent_repo", "pull")),
                    subtracted: Box::new(rel(IS_UNTRUSTED_FORK)),
                },
            ),
        ],
    )
}

/// **The complete compiled CI ReBAC namespace fragment (contract 4.9)** — the four rich
/// [`FragmentDef`]s Identity admits into the one cell schema. Order does not matter for admit (CI
/// inheritance edges (`parent_repo->pull`, `parent_repo->push`) terminate on the **Git `repo`**
/// fragment's compiled permissions, resolved at check-time on the parent's schema — not on a CI type),
/// but they are listed root-first (`ci_project` → `environment` → `secret` → `run`) for readability.
/// This is the SINGLE entry point [`crate::StoreBackedCheck::admit_ci_fragment`] and the CDC test
/// consume.
pub fn ci_fragment() -> Vec<FragmentDef> {
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
    use crate::namespace::NamespaceEngine;
    use myelin_identity::FragmentAdmit;

    /// **The compiled CI fragment admits into the cell schema (the engine-only-floor progression).**
    /// Every CI object type admits on top of the core org/team/project hierarchy; the four types enter
    /// the compiled vocabulary; `secret.read` + `run.read` resolve as compiled permissions.
    #[test]
    fn ci_fragment_admits_into_the_cell_schema() {
        let mut eng = NamespaceEngine::with_core_hierarchy();
        for def in ci_fragment() {
            let admit = eng.admit(&def);
            assert!(
                matches!(admit, FragmentAdmit::Admitted { .. }),
                "the CI `{}` fragment admits into the cell schema: {admit:?}",
                def.object_type.0
            );
        }
        for ty in ["ci_project", "environment", "secret", "run"] {
            assert!(
                eng.object_types().contains(&ty.to_string()),
                "`{ty}` is admitted"
            );
        }
        assert!(
            eng.resolve_permission("secret", READ).is_some(),
            "secret.read is a compiled permission"
        );
        assert!(
            eng.resolve_permission("run", READ).is_some(),
            "run.read is a compiled permission"
        );
        assert!(
            eng.resolve_permission("run", VIEW).is_some(),
            "run.view is a compiled permission"
        );
    }

    /// **`secret.read` is NOT inherited (CI-1, the mutation-tested core).** The `secret.read` rewrite
    /// MUST be the bare direct relation `direct_reader` — NOT a Union/TupleToUserset that pulls in
    /// `parent_ci_project`. A mutation adding a `parent_ci_project->…` inheritance arm (making the
    /// secret leak via project-read) is caught HERE structurally — and behaviourally in the drill.
    #[test]
    fn secret_read_is_a_bare_direct_relation_not_inherited() {
        let secret = secret_fragment();
        let read = secret
            .permissions
            .iter()
            .find(|p| p.permission.0 == READ)
            .expect("secret declares read");
        assert_eq!(
            read.rewrite,
            rel(SECRET_DIRECT_READER),
            "secret.read = direct_reader (DIRECT NARROW, NOT inherited — CI-1, §1)"
        );
        // It is, specifically, NOT a tuple-to-userset (no project inheritance) and NOT a union that
        // reaches the parent project — the two ways a leak could be smuggled in.
        assert!(
            !matches!(read.rewrite, Userset::TupleToUserset { .. }),
            "secret.read does NOT inherit via tuple-to-userset"
        );
        assert!(
            !rewrite_mentions_tupleset(&read.rewrite, "parent_ci_project"),
            "secret.read never reaches parent_ci_project (no project-read inheritance, CI-1)"
        );
    }

    /// **`run.read = run.view − is_untrusted_fork` (the `read & !is_untrusted_fork` ABAC edge, C7).** The
    /// run-read rewrite MUST be an Exclusion subtracting `is_untrusted_fork` over the `parent_repo->pull`
    /// base — not a bare inheritance (a mutation dropping the exclusion lets an untrusted-fork run's
    /// output be read and is caught here, and behaviourally in the drill).
    #[test]
    fn run_read_is_view_minus_is_untrusted_fork() {
        let run = run_fragment();
        let read = run
            .permissions
            .iter()
            .find(|p| p.permission.0 == READ)
            .expect("run declares read");
        match &read.rewrite {
            Userset::Exclusion { base, subtracted } => {
                assert_eq!(
                    **subtracted,
                    rel(IS_UNTRUSTED_FORK),
                    "the ABAC edge subtracts is_untrusted_fork (the !is_untrusted_fork edge, C7)"
                );
                assert_eq!(
                    **base,
                    ttu("parent_repo", "pull"),
                    "the read base is run.view = parent_repo->pull (§5)"
                );
            }
            other => panic!("run.read must be an Exclusion (− is_untrusted_fork), got {other:?}"),
        }
    }

    /// **`run.view = parent_repo->pull` and `run.trigger = parent_repo->push` (§5).** The run-visibility
    /// rewrites inherit the parent Git repo's compiled `pull`/`push` (the cross-fragment inheritance
    /// into [`crate::git_fragment`]'s `repo` permissions).
    #[test]
    fn run_view_and_trigger_inherit_the_repo() {
        let run = run_fragment();
        let view = run
            .permissions
            .iter()
            .find(|p| p.permission.0 == VIEW)
            .expect("run declares view");
        assert_eq!(
            view.rewrite,
            ttu("parent_repo", "pull"),
            "run.view = parent_repo->pull (§5)"
        );
        let trigger = run
            .permissions
            .iter()
            .find(|p| p.permission.0 == TRIGGER)
            .expect("run declares trigger");
        assert_eq!(
            trigger.rewrite,
            ttu("parent_repo", "push"),
            "run.trigger = parent_repo->push (§5)"
        );
    }

    /// **No CI fragment name smuggles an object id (Id never invents object ids).** Every
    /// type/relation/permission name is a bare identifier — the engine's `mints_object_id` admit check
    /// would reject one that wasn't.
    #[test]
    fn no_ci_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in ci_fragment() {
            assert!(
                !mints(&f.object_type.0),
                "type `{}` is a bare identifier",
                f.object_type.0
            );
            for r in &f.relations {
                assert!(!mints(&r.0), "relation `{}` is a bare identifier", r.0);
            }
            for p in &f.permissions {
                assert!(
                    !mints(&p.permission.0),
                    "permission `{}` is a bare identifier",
                    p.permission.0
                );
            }
        }
    }

    /// Whether a rewrite tree anywhere reaches a tuple-to-userset over `tupleset` (i.e. inherits via
    /// that relation). Used to prove `secret.read` never reaches `parent_ci_project`.
    fn rewrite_mentions_tupleset(rw: &Userset, tupleset: &str) -> bool {
        match rw {
            Userset::Relation(_) => false,
            Userset::Union(arms) | Userset::Intersect(arms) => {
                arms.iter().any(|a| rewrite_mentions_tupleset(a, tupleset))
            }
            Userset::Exclusion { base, subtracted } => {
                rewrite_mentions_tupleset(base, tupleset)
                    || rewrite_mentions_tupleset(subtracted, tupleset)
            }
            Userset::TupleToUserset { tupleset: t, .. } => t.0 == tupleset,
        }
    }
}
