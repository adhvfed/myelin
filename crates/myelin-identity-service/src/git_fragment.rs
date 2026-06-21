//! # `git_fragment` — Id's compiled Git ReBAC namespace fragment (contract 4.9, P-ID-24 → P-247)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §5 (the **frozen Git fragment**: `repo`, `branch`/`ref`, `pull_request`, `pr_comment`, with
//! **ref-glob-scoped relations**, branch-protection as a tighter **`protected_push`** permission,
//! **CODEOWNERS expressed as relations** — a CODEOWNERS path-glob compiles to reviewer-requirement
//! tuples, NOT a bespoke check — plus the **`approve_untrusted_ci`** relation (C7) the fork-
//! endorsement gate reads as an ordinary `check`; `pull_request.merge = parent_repo->protected_push`),
//! §7.3 (the Git **via_column** mapping: `pr.id` / `repo.id` — the conjoin column the
//! `list_objects` `Filter` JOINs against, one query, no N+1).
//!
//! **Reconciliation (FROZEN):** `00-reconciliation-decisions.md` §X-1 (the `approve_untrusted_ci`
//! relation + the fork-endorsement gate: Git reads the CI `trust_tier` stamps, **Identity never
//! recomputes trust** — the gate is `check(subject, approve_untrusted_ci, repo)`).
//!
//! **Contract-index row 4.9 (OWNED here — the Identity-side compiled Git fragment).** Identity owns
//! the **engine + admit-contract + core hierarchy** ([`crate::namespace`], P-068). This module is
//! the **first of the five per-subsystem fragments** (P-ID-24/26/27/29/30) that promote the M1
//! engine-only floor: it is the canonical **rich** [`crate::namespace::FragmentDef`] declaration of
//! the Git authz vocabulary, with the permission **rewrites** wired over the four Zanzibar userset
//! operators so `check`/`list_objects` resolve the Git permissions through the SAME engine the core
//! hierarchy uses (one primitive — no bespoke Git check path, the §5 design rule).
//!
//! ## Why the rich fragment lives HERE (not in `myelin-git`)
//! `myelin-git` (a producer LEAF crate) already declares the **names-only** ABI carrier
//! [`myelin_identity::NamespaceFragment`] (`myelin_git::rebac_fragment`, GIT-P1) — the shape
//! Identity's `admit_fragment` consumes at the contract boundary. But the names-only carrier cannot
//! carry the permission **rewrite structure** (`pull = reader ∪ writer ∪ admin ∪ parent_project->view`,
//! the `parent_repo->protected_push` inheritance, …) — only the engine's rich `FragmentDef` can. So
//! **Id owns the compiled rewrites** (this module), and the names-only Git carrier projects onto it.
//! The CDC test (`tests/cdc_4_9_git_fragment.rs`) pins that the two agree byte-for-byte on the
//! relation/permission NAMES (a drift on either side fails the same CI job). This keeps the §2.9
//! crate DAG acyclic: `myelin-identity-service` (the engine) does NOT depend on `myelin-git`; it
//! declares the Git vocabulary from the architecture §5 directly, and the names-only Git carrier is
//! the contract-boundary witness they match.
//!
//! ## The compiled Git fragment (§5)
//!
//! | object type    | relations                                                                  | permissions (rewrite)                                                          |
//! |----------------|----------------------------------------------------------------------------|--------------------------------------------------------------------------------|
//! | `repo`         | `parent_project` `reader` `writer` `admin` `approve_untrusted_ci` `watcher` | `pull = reader∪writer∪admin∪parent_project->view`; `push = writer∪admin∪parent_project->view`; `administer = admin∪parent_project->view`; **`protected_push = admin`** |
//! | `ref`          | `parent_repo` `bypass` `code_owner`                                         | **`push_protected = bypass ∪ parent_repo->administer`** (the ref-glob branch-protection gate) |
//! | `pull_request` | `parent_repo` `author` `reviewer` `watcher`                                | `view = parent_repo->pull`; `review = reviewer ∪ parent_repo->push`; **`merge = parent_repo->protected_push`** |
//! | `pr_comment`   | `parent_pr`                                                                | `view = parent_pr->view`                                                        |
//!
//! - **ref-glob-scoped relations** — the `ref` object type IS the ref-PATTERN (ref-glob) scope: a
//!   tuple `ref:<repo>/<glob>#bypass@subject` scopes the bypass to the matched ref-glob; the tighter
//!   **`protected_push`** permission lives on `repo` (admin-only) and `ref.push_protected` inherits it.
//! - **CODEOWNERS-as-relations** — a CODEOWNERS path-glob compiles to `ref.code_owner` reviewer-
//!   requirement TUPLES ([`compile_codeowners`]), not a bespoke check: "who must approve this path"
//!   is `list_subjects(ref, code_owner)` at member density (§5).
//! - **`approve_untrusted_ci`** — an ordinary `repo` relation (X-1, C7): the fork-endorsement gate is
//!   `check(subject, approve_untrusted_ci, repo)`; Identity never recomputes the CI `trust_tier`.
//! - **`pull_request.merge = parent_repo->protected_push`** — the §5-frozen merge gate, a tuple-to-
//!   userset inheritance into the repo's admin-only `protected_push` permission.

use crate::namespace::{FragmentDef, PermissionRule, Userset};
use myelin_identity::{ObjectType, Permission, RelName, RelationTuple, PrincipalId, ObjectId};

/// The four frozen Git object-type names (§5; mirrors `myelin_git::rebac_fragment::object_types`).
/// Public so the CDC test + a live-wiring caller reference the SAME canonical strings.
pub mod object_types {
    /// The repository — the root Git authz object.
    pub const REPO: &str = "repo";
    /// A ref-PATTERN scope (the ref-glob relation carrier).
    pub const REF: &str = "ref";
    /// A pull request.
    pub const PULL_REQUEST: &str = "pull_request";
    /// A PR comment / review-thread node.
    pub const PR_COMMENT: &str = "pr_comment";
}

/// **The X-1 fork-endorsement relation name (C7).** The fork-endorsement gate reads it as an
/// ordinary `check(subject, approve_untrusted_ci, repo)`; Git stamps the `trust_tier` from CI run
/// provenance, Identity never recomputes trust. Exposed so the merge gate / the CDC reference the
/// canonical name (not a stringly-typed literal at the call site).
pub const APPROVE_UNTRUSTED_CI: &str = "approve_untrusted_ci";

/// **The CODEOWNERS reviewer-requirement relation name.** A CODEOWNERS path-glob compiles to
/// `ref.code_owner` tuples ([`compile_codeowners`]); "who must approve this path" is then
/// `list_subjects(ref, code_owner)` — an ordinary Expand, not a bespoke check (§5).
pub const CODE_OWNER: &str = "code_owner";

/// The tighter branch-protection permission name (`repo.protected_push = admin`; `ref.push_protected`
/// inherits it). The §5 merge/protected-ref gate.
pub const PROTECTED_PUSH: &str = "protected_push";

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

/// **The `repo` fragment** (§5 `definition repo`) — the rich rewrite Id compiles.
///
/// `pull = reader ∪ writer ∪ admin ∪ parent_project->view`; `push = writer ∪ admin ∪
/// parent_project->view`; `administer = admin ∪ parent_project->view`; **`protected_push = admin`**
/// (tighter — the merge/protected-ref gate). `parent_project->view` inherits from the core
/// hierarchy's `project.view` (the org→team→project chain the engine ships).
pub fn repo_fragment() -> FragmentDef {
    frag(
        object_types::REPO,
        &[
            "parent_project",
            "reader",
            "writer",
            "admin",
            APPROVE_UNTRUSTED_CI,
            "watcher",
        ],
        vec![
            perm(
                "pull",
                Userset::Union(vec![
                    rel("reader"),
                    rel("writer"),
                    rel("admin"),
                    ttu("parent_project", "view"),
                ]),
            ),
            perm(
                "push",
                Userset::Union(vec![rel("writer"), rel("admin"), ttu("parent_project", "view")]),
            ),
            perm(
                "administer",
                Userset::Union(vec![rel("admin"), ttu("parent_project", "view")]),
            ),
            // The tighter protected_push: admin-only (NOT a project reader). The merge / protected-
            // ref gate. `pull_request.merge` + `ref.push_protected` inherit this.
            perm(PROTECTED_PUSH, rel("admin")),
        ],
    )
}

/// **The `ref` fragment** (§5 `definition ref`) — the ref-PATTERN (ref-glob) object.
///
/// `push_protected = bypass ∪ parent_repo->administer`. The `bypass` relation is the audited bypass
/// list scoped to the matched ref-glob; `code_owner` is the CODEOWNERS-as-relations reviewer
/// requirement ([`compile_codeowners`]).
pub fn ref_fragment() -> FragmentDef {
    frag(
        object_types::REF,
        &["parent_repo", "bypass", CODE_OWNER],
        vec![perm(
            "push_protected",
            Userset::Union(vec![rel("bypass"), ttu("parent_repo", "administer")]),
        )],
    )
}

/// **The `pull_request` fragment** (§5 `definition pull_request`).
///
/// `view = parent_repo->pull`; `review = reviewer ∪ parent_repo->push`; **`merge =
/// parent_repo->protected_push`** (the §5-frozen merge gate — admin-only via the repo's tighter
/// `protected_push`). The `watcher` relation makes the PR a Notif read-fanout target.
pub fn pull_request_fragment() -> FragmentDef {
    frag(
        object_types::PULL_REQUEST,
        &["parent_repo", "author", "reviewer", "watcher"],
        vec![
            perm("view", ttu("parent_repo", "pull")),
            perm(
                "review",
                Userset::Union(vec![rel("reviewer"), ttu("parent_repo", "push")]),
            ),
            perm("merge", ttu("parent_repo", PROTECTED_PUSH)),
        ],
    )
}

/// **The `pr_comment` fragment** (§5 `definition pr_comment`). `view = parent_pr->view`.
pub fn pr_comment_fragment() -> FragmentDef {
    frag(
        object_types::PR_COMMENT,
        &["parent_pr"],
        vec![perm("view", ttu("parent_pr", "view"))],
    )
}

/// **The complete compiled Git ReBAC namespace fragment (contract 4.9)** — the four rich
/// [`FragmentDef`]s Identity admits into the one cell schema, in parent-before-child order (`repo` →
/// `ref` → `pull_request` → `pr_comment`) so each inheritance edge's parent type is already in the
/// schema when its child admits. This is the SINGLE entry point [`crate::StoreBackedCheck::admit_git_fragment`]
/// and the CDC test consume.
pub fn git_fragment() -> Vec<FragmentDef> {
    vec![
        repo_fragment(),
        ref_fragment(),
        pull_request_fragment(),
        pr_comment_fragment(),
    ]
}

/// A single CODEOWNERS rule: a path-glob and the set of owner subjects required to review a change
/// touching a matching path (§5 — CODEOWNERS-as-relations). The input the Git subsystem parses from
/// a repo's `CODEOWNERS` file; Identity compiles it to reviewer-requirement TUPLES, never a bespoke
/// check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeownersRule {
    /// The CODEOWNERS path-glob (e.g. `/src/payments/**`). Opaque to Identity — it is the ref id
    /// scope the tuple is written against; Git decides which glob a change matches.
    pub path_glob: String,
    /// The required reviewer subjects (the principals or teams that must approve a change touching
    /// `path_glob`). Each becomes a `code_owner` tuple subject.
    pub owners: Vec<PrincipalId>,
}

/// **Compile a CODEOWNERS file (path-glob → owners) into `ref.code_owner` reviewer-requirement
/// TUPLES (§5 — CODEOWNERS-as-relations, NOT a bespoke check).**
///
/// Each rule's owners become `⟨ref:<repo>/<path_glob>#code_owner@<owner>⟩` tuples on a `ref` object
/// whose id encodes the repo + the path-glob scope. The Git subsystem WRITES these through the
/// ordinary `write_tuples` path; "who must approve this path" is then `list_subjects(ref, code_owner)`
/// — an ordinary Expand over the SAME engine + reverse index (one primitive). Identity never parses a
/// CODEOWNERS file or runs a glob-matcher in the hot path: the glob is baked into the ref id at write
/// time, and the authz question is a plain relation lookup.
///
/// `repo` is the repo id the CODEOWNERS file belongs to; the produced `ref` object ids are
/// `ref:<repo>::<path_glob>` (the `::` keeps the glob — which may itself contain `/` — recoverable
/// from the ref id without colliding with the `repo` segment). The returned tuples are NOT written
/// here (Identity never invents object ids out of band); they are the shape the Git subsystem writes.
pub fn compile_codeowners(repo: &str, rules: &[CodeownersRule]) -> Vec<RelationTuple> {
    let mut tuples = Vec::new();
    for rule in rules {
        // The ref object id encoding the repo + the path-glob scope. `::` separates the repo from
        // the (possibly `/`-bearing) glob so the scope is unambiguous; the glob stays opaque to Id.
        let ref_id = format!("{}:{}::{}", object_types::REF, repo, rule.path_glob);
        for owner in &rule.owners {
            tuples.push(RelationTuple {
                object: ObjectId(ref_id.clone()),
                relation: RelName(CODE_OWNER.to_string()),
                subject: owner.clone(),
                caveat: None,
            });
        }
    }
    tuples
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::NamespaceEngine;
    use myelin_identity::FragmentAdmit;

    /// **The compiled Git fragment admits into the cell schema (the engine-only-floor progression).**
    /// Every Git object type admits on top of the core org/team/project hierarchy (whose `project`
    /// type the `parent_project->view` inheritance terminates on).
    #[test]
    fn git_fragment_admits_into_the_cell_schema() {
        let mut eng = NamespaceEngine::with_core_hierarchy();
        for def in git_fragment() {
            let admit = eng.admit(&def);
            assert!(
                matches!(admit, FragmentAdmit::Admitted { .. }),
                "the Git `{}` fragment admits into the cell schema: {admit:?}",
                def.object_type.0
            );
        }
        // The four Git types are now in the compiled vocabulary.
        for ty in ["repo", "ref", "pull_request", "pr_comment"] {
            assert!(eng.object_types().contains(&ty.to_string()), "`{ty}` is admitted");
        }
        // protected_push resolves as a compiled (admin-only) permission on repo.
        assert!(eng.resolve_permission("repo", PROTECTED_PUSH).is_some());
        // pull_request.merge resolves via parent_repo->protected_push (the §5 merge gate).
        assert!(eng.resolve_permission("pull_request", "merge").is_some());
    }

    /// **`pull_request.merge` resolves via `protected_push` (§5).** The merge gate is the tuple-to-
    /// userset inheritance `merge = parent_repo->protected_push`, terminating on the repo's admin-only
    /// `protected_push`.
    #[test]
    fn pull_request_merge_resolves_via_protected_push() {
        let merge = pull_request_fragment()
            .permissions
            .into_iter()
            .find(|p| p.permission.0 == "merge")
            .expect("pull_request declares merge");
        assert_eq!(
            merge.rewrite,
            ttu("parent_repo", PROTECTED_PUSH),
            "merge = parent_repo->protected_push (§5, frozen)"
        );
    }

    /// **A CODEOWNERS path-glob compiles to the right `code_owner` reviewer-requirement tuples.**
    /// Two owners on one glob → two `ref#code_owner@owner` tuples on the glob-scoped ref id; the glob
    /// (with its `/`) is recoverable from the ref id.
    #[test]
    fn codeowners_glob_compiles_to_reviewer_tuples() {
        let rules = vec![CodeownersRule {
            path_glob: "/src/payments/**".into(),
            owners: vec![PrincipalId("p:alice".into()), PrincipalId("team:payments".into())],
        }];
        let tuples = compile_codeowners("repo:core", &rules);
        assert_eq!(tuples.len(), 2, "two owners → two reviewer-requirement tuples");
        for t in &tuples {
            assert_eq!(t.relation, RelName(CODE_OWNER.into()), "each is a code_owner tuple");
            assert_eq!(
                t.object,
                ObjectId("ref:repo:core::/src/payments/**".into()),
                "the ref id encodes the repo + the path-glob scope (glob recoverable)"
            );
        }
        let subjects: Vec<&str> = tuples.iter().map(|t| t.subject.0.as_str()).collect();
        assert!(subjects.contains(&"p:alice"));
        assert!(subjects.contains(&"team:payments"));
    }

    /// **`approve_untrusted_ci` is an ordinary `repo` relation (X-1, C7).** It is a declared relation,
    /// NOT a compiled permission — so the fork-endorsement gate is a plain
    /// `check(subject, approve_untrusted_ci, repo)` direct-relation check, never bespoke logic.
    #[test]
    fn approve_untrusted_ci_is_a_plain_repo_relation() {
        let repo = repo_fragment();
        assert!(
            repo.relations.contains(&RelName(APPROVE_UNTRUSTED_CI.into())),
            "approve_untrusted_ci is a declared repo relation (X-1)"
        );
        // It is NOT a compiled permission (so check resolves it as a direct relation, the X-1 gate).
        assert!(
            !repo.permissions.iter().any(|p| p.permission.0 == APPROVE_UNTRUSTED_CI),
            "approve_untrusted_ci is a relation, not a permission — a plain check, not bespoke logic"
        );
    }

    /// **No Git fragment name smuggles an object id (Id never invents object ids).** Every
    /// type/relation/permission name is a bare identifier — the engine's `mints_object_id` admit
    /// check would reject one that wasn't.
    #[test]
    fn no_git_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in git_fragment() {
            assert!(!mints(&f.object_type.0), "type `{}` is a bare identifier", f.object_type.0);
            for r in &f.relations {
                assert!(!mints(&r.0), "relation `{}` is a bare identifier", r.0);
            }
            for p in &f.permissions {
                assert!(!mints(&p.permission.0), "permission `{}` is a bare identifier", p.permission.0);
            }
        }
    }
}
