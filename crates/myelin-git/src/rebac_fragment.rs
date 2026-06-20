//! # `rebac_fragment` — the Git ReBAC namespace fragment (contract 4.9, FROZEN, GIT-P1 / P-123)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md`
//! §5.2 (the ReBAC namespace fragment — *declared by Git, engine owned by Id, frozen*: ref-glob-
//! scoped relations + CODEOWNERS-as-relations + the `approve_untrusted_ci` fork-endorsement
//! relation + the per-watchable-type `watcher` relation) and `00-overview.md` §1.2 (owns-vs-
//! delegates: Git *owns* its fragment definition; Identity *owns* the engine that compiles it).
//!
//! **Reconciliation (FROZEN):**
//! `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §1 — *"The
//! frozen per-subsystem fragments: Git (ref-glob-scoped relations + CODEOWNERS-as-relations +
//! `approve_untrusted_ci`) … Each declares a `watcher` relation per watchable type (Notif
//! read-fanout)."*
//!
//! **Contract-index row 4.9 (OWNED here — the Git fragment slice):** the per-subsystem ReBAC
//! namespace fragment. Identity owns the *engine + admit-contract + core hierarchy*
//! (`myelin-identity-service::namespace`, P-068); Git owns *this fragment's definition*. The
//! contract boundary Identity compiles against is the frozen names-only ABI carrier
//! [`myelin_identity::NamespaceFragment`] — this module emits exactly that, one carrier per Git
//! object type, so **Identity's cell schema compiles against the Git fragment** (the gate of this
//! prompt — a build-time property, not a runtime drill).
//!
//! ## What this prompt (GIT-P1 / P-123) ships — and what it deliberately does NOT (VISION §3)
//! **Ships:** the FROZEN Git fragment as data — the four Git object types and their relations +
//! permission NAMES, in the frozen [`myelin_identity::NamespaceFragment`] shape Identity admits:
//!
//! | object type    | relations                                                              | permission names                                    |
//! |----------------|------------------------------------------------------------------------|------------------------------------------------------|
//! | `repo`         | `parent_project` `reader` `writer` `admin` `approve_untrusted_ci` `watcher` | `pull` `push` `administer` `protected_push`     |
//! | `ref`          | `parent_repo` `bypass` `code_owner`                                     | `push_protected`                                     |
//! | `pull_request` | `parent_repo` `author` `reviewer` `watcher`                            | `view` `review` `merge`                              |
//! | `pr_comment`   | `parent_pr`                                                             | `view`                                               |
//!
//! Every relation the architecture §5.2 names is present:
//! - **ref-glob-scoped relations** — the `ref` object type IS the ref-PATTERN scope (its tuples are
//!   written per ref-glob); `bypass` + the `push_protected` permission ride it.
//! - **CODEOWNERS-as-relations** — `ref.code_owner` (the CODEOWNERS path-glob → reviewer-requirement
//!   relation; "who must approve this path" is a `list_subjects(pr, review)` query at member
//!   density, §5.2).
//! - **`approve_untrusted_ci`** — `repo.approve_untrusted_ci` (the X-1 fork-endorsement relation the
//!   merge gate rides as a plain `check(subject, approve_untrusted_ci, repo)`, never bespoke logic).
//! - **`watcher`** — declared on each watchable type (`repo`, `pull_request`) for the Notif
//!   read-fanout (identity §5 C8).
//!
//! **Does NOT ship (FLOOR named — VISION §3):** *no Git feature.* No tuples are written, no
//! `check`/`list_objects` is served, no CODEOWNERS resolver runs, no merge gate evaluates. This is a
//! **contract-fragment freeze** — the relation/permission SHAPES Identity compiles against, nothing
//! more. The fragment is wired **LIVE** (admitted into the running cell schema at boot + the
//! permission *rewrites* — `pull = reader + writer + admin + parent_project->read`, the
//! `parent_repo->administer` / `parent_team->view` tuple-to-userset inheritance — carried through
//! the rich engine `FragmentDef`) at **GIT-P13 (M3-G2)**, riding Identity's Git-fragment admit
//! prompt **P-ID-24 (P-247)**. Until then the *names* freeze here is the compile anchor; the
//! *rewrite structure* is documented (the doc-comments below name each permission's frozen rewrite)
//! and proven admissible by the CDC test (`tests/cdc_4_9_git_fragment.rs`), which compiles the rich
//! rewrites through the real engine.
//!
//! ## Why names-only here (the DAG, EI-01 §7 — extend, never re-define)
//! `myelin-git` is a producer LEAF crate; it depends on the frozen contract surface
//! `myelin-identity` (which carries the names-only [`myelin_identity::NamespaceFragment`]), NOT on
//! `myelin-identity-service` (the rich `FragmentDef`/`Userset` engine — a service crate). So the
//! *runtime* fragment Git ships is the names-only carrier Identity's `admit_fragment` consumes; the
//! rich rewrite structure is exercised only by the CDC TEST (a dev-dependency on the engine), never
//! re-defined here. This keeps the §2.9 crate DAG acyclic (no producer→service edge) while still
//! freezing the full fragment shape Identity must compile.

use myelin_identity::{NamespaceFragment, ObjectType, Permission, RelName};

/// The four frozen Git object-type names (the §5.2 `definition` blocks). Public so the M3 live-
/// wiring prompt (GIT-P13) and the CDC test reference the SAME canonical strings (one source of
/// truth — a typo here is a typo everywhere, caught by the admit).
pub mod object_types {
    /// The repository — the root Git authz object (§5.2 `definition repo`).
    pub const REPO: &str = "repo";
    /// A ref-PATTERN scope (the ref-glob relation carrier — §5.2 `definition ref`).
    pub const REF: &str = "ref";
    /// A pull request (§5.2 `definition pull_request`).
    pub const PULL_REQUEST: &str = "pull_request";
    /// A PR comment / review-thread node (§5.2 `definition pr_comment`).
    pub const PR_COMMENT: &str = "pr_comment";
}

/// Build a [`NamespaceFragment`] (the frozen names-only ABI carrier) from `&str` slices, deduping
/// nothing (the engine's `admit` dedupes relations) — a small constructor that keeps the four
/// fragment definitions below declarative.
fn fragment(object_type: &str, relations: &[&str], permissions: &[&str]) -> NamespaceFragment {
    NamespaceFragment {
        object_type: ObjectType(object_type.to_string()),
        relations: relations.iter().map(|r| RelName(r.to_string())).collect(),
        permissions: permissions.iter().map(|p| Permission(p.to_string())).collect(),
    }
}

/// **The `repo` fragment** (§5.2 `definition repo`).
///
/// Relations: `parent_project` (inheritance edge into the org hierarchy), `reader`, `writer`,
/// `admin`, **`approve_untrusted_ci`** (the X-1 fork-endorsement relation), **`watcher`** (Notif
/// read-fanout). Permissions (names frozen here; rewrites — the FLOOR wired LIVE in GIT-P13 —
/// documented):
/// - `pull           = reader + writer + admin + parent_project->read`
/// - `push           = writer + admin + parent_project->write`
/// - `administer      = admin + parent_project->admin`
/// - `protected_push  = admin` (tighter; the merge/protected-ref gate)
pub fn repo_fragment() -> NamespaceFragment {
    fragment(
        object_types::REPO,
        &[
            "parent_project",
            "reader",
            "writer",
            "admin",
            "approve_untrusted_ci",
            "watcher",
        ],
        &["pull", "push", "administer", "protected_push"],
    )
}

/// **The `ref` fragment** (§5.2 `definition ref`) — the ref-PATTERN-scoped (ref-glob) object.
///
/// Relations: `parent_repo`, `bypass` (the audited bypass list), **`code_owner`** (the
/// CODEOWNERS path-glob → reviewer-requirement relation). Permission (name frozen; rewrite the
/// GIT-P13 floor):
/// - `push_protected = bypass + parent_repo->administer`
pub fn ref_fragment() -> NamespaceFragment {
    fragment(
        object_types::REF,
        &["parent_repo", "bypass", "code_owner"],
        &["push_protected"],
    )
}

/// **The `pull_request` fragment** (§5.2 `definition pull_request`).
///
/// Relations: `parent_repo`, `author`, `reviewer`, **`watcher`** (Notif read-fanout). Permissions
/// (names frozen; rewrites the GIT-P13 floor):
/// - `view   = parent_repo->pull`
/// - `review = reviewer + parent_repo->push`
/// - `merge  = parent_repo->protected_push` (agent_needs_human enforced in the merge gate)
pub fn pull_request_fragment() -> NamespaceFragment {
    fragment(
        object_types::PULL_REQUEST,
        &["parent_repo", "author", "reviewer", "watcher"],
        &["view", "review", "merge"],
    )
}

/// **The `pr_comment` fragment** (§5.2 `definition pr_comment`).
///
/// Relation: `parent_pr`. Permission (name frozen; rewrite the GIT-P13 floor):
/// - `view = parent_pr->view`
pub fn pr_comment_fragment() -> NamespaceFragment {
    fragment(object_types::PR_COMMENT, &["parent_pr"], &["view"])
}

/// **The complete frozen Git ReBAC namespace fragment** — the four [`NamespaceFragment`] carriers
/// Identity admits into the one cell schema (contract 4.9). The order is repo → ref → pull_request
/// → pr_comment (parent-before-child, the order Identity admits them so each inheritance edge's
/// parent type is already in the schema). This is the SINGLE entry point GIT-P13 (the live wiring)
/// + the CDC test consume.
pub fn git_fragment() -> Vec<NamespaceFragment> {
    vec![
        repo_fragment(),
        ref_fragment(),
        pull_request_fragment(),
        pr_comment_fragment(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frozen relation set per object type (§5.2) — the well-formedness witness. A relation
    /// dropped or renamed here is caught by this test BEFORE it reaches Identity's admit.
    #[test]
    fn each_definition_declares_its_frozen_relations() {
        let repo = repo_fragment();
        let repo_rels: Vec<&str> = repo.relations.iter().map(|r| r.0.as_str()).collect();
        // §5.2 repo: parent_project, reader, writer, admin, approve_untrusted_ci, watcher.
        for expected in [
            "parent_project",
            "reader",
            "writer",
            "admin",
            "approve_untrusted_ci",
            "watcher",
        ] {
            assert!(
                repo_rels.contains(&expected),
                "repo must declare the `{expected}` relation (§5.2)"
            );
        }

        // CODEOWNERS-as-relations lives on `ref` as `code_owner` (§5.2).
        let r = ref_fragment();
        assert!(
            r.relations.contains(&RelName("code_owner".into())),
            "the CODEOWNERS-as-relations `code_owner` relation is on `ref`"
        );
        // ref-glob: the `ref` object type IS the ref-pattern scope; `bypass` rides it.
        assert!(r.relations.contains(&RelName("bypass".into())));

        // The watcher relation is declared on each watchable type (repo + pull_request).
        assert!(repo.relations.contains(&RelName("watcher".into())));
        assert!(
            pull_request_fragment()
                .relations
                .contains(&RelName("watcher".into())),
            "the `pull_request` watchable type declares `watcher` (Notif read-fanout)"
        );
    }

    /// The X-1 fork-endorsement relation `approve_untrusted_ci` is present on `repo` (so the merge
    /// gate is a plain `check(subject, approve_untrusted_ci, repo)`, never bespoke logic — §5.2).
    #[test]
    fn approve_untrusted_ci_is_a_plain_repo_relation() {
        assert!(
            repo_fragment()
                .relations
                .contains(&RelName("approve_untrusted_ci".into())),
            "approve_untrusted_ci is an ordinary relation on repo (X-1)"
        );
    }

    /// The four object types are frozen + non-empty + carry their permission names. This is the
    /// shape Identity compiles; a name dropped here breaks the cell-schema compile.
    #[test]
    fn the_four_git_object_types_are_frozen() {
        let frag = git_fragment();
        let types: Vec<&str> = frag.iter().map(|f| f.object_type.0.as_str()).collect();
        assert_eq!(types, vec!["repo", "ref", "pull_request", "pr_comment"]);
        // protected_push (the merge/protected-ref gate permission) is a repo permission.
        assert!(repo_fragment()
            .permissions
            .contains(&Permission("protected_push".into())));
        // merge (the X-1 merge gate) is a pull_request permission.
        assert!(pull_request_fragment()
            .permissions
            .contains(&Permission("merge".into())));
    }

    /// No fragment smuggles an object id (Id never invents object ids): every type/relation/
    /// permission NAME is a bare identifier (no `:`/`/`/`#`). This mirrors the engine's
    /// `mints_object_id` admit check — a fragment that tripped it would be REJECTED at admit.
    #[test]
    fn no_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in git_fragment() {
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
