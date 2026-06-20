//! # `rebac_fragment` — the Issues ReBAC namespace fragment (contract 4.9, FROZEN, ISS-P01 / P-125)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//! §6.1 (the Issues namespace fragment — *declared by Issues, engine owned by Id, frozen*: the
//! `issue` definition + the `- confidential` set-difference userset + the `watcher` read-fanout
//! relation + the `issue_field` / `issue_transition` ABAC sub-objects) and `00-overview.md` §1 (the
//! most-coupled posture) + §2.2 (thin-shell-over-identical-plumbing).
//!
//! **Reconciliation (FROZEN):**
//! `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §1 — *"The
//! frozen per-subsystem fragments: … Issues (`issue` namespace + field/transition ABAC caveats). Each
//! declares a `watcher` relation per watchable type (Notif read-fanout)."*
//!
//! **Contract-index row 4.9 (OWNED here — the Issues fragment slice):** the per-subsystem ReBAC
//! namespace fragment. Identity owns the *engine + admit-contract + core hierarchy*
//! (`myelin-identity-service::namespace`, P-068); Issues owns *this fragment's definition*. The
//! contract boundary Identity compiles against is the frozen names-only ABI carrier
//! [`myelin_identity::NamespaceFragment`] — this module emits exactly that, one carrier per Issues
//! object type, so **Identity's cell schema compiles against the Issues fragment** (the gate of this
//! prompt — a build-time property, not a runtime drill).
//!
//! ## What this prompt (ISS-P01 / P-125) ships — and what it deliberately does NOT (VISION §3)
//! **Ships:** the FROZEN Issues fragment as data — the three Issues object types and their relations
//! + permission NAMES, in the frozen [`myelin_identity::NamespaceFragment`] shape Identity admits:
//!
//! | object type        | relations                                                                  | permission names                       |
//! |--------------------|----------------------------------------------------------------------------|----------------------------------------|
//! | `issue`            | `parent_project` `assignee` `watcher` `confidential` `confidential_grant`   | `view` `comment` `transition` `manage` |
//! | `issue_field`      | `parent_issue`                                                             | `view_field`                           |
//! | `issue_transition` | `parent_issue`                                                             | `perform_transition`                   |
//!
//! Every relation the architecture §6.1 names is present:
//! - **`assignee`** — the issue's assignee (also a `transition` grantee).
//! - **`watcher`** — the Notif read-fanout relation (Notif resolves `list_subjects(issue, watcher)`
//!   for the unbounded ambient set, contract 4.4 / §6.1).
//! - **`confidential`** — marks an issue confidential; it is the SUBTRACTED arm of the `view`
//!   set-difference, so a confidential issue **disappears from a normal project-reader's
//!   `list_objects` by construction** (the no-leak guarantee, D3 — §6.1).
//! - **`confidential_grant`** — the explicit grant that re-admits a chosen subject to a confidential
//!   issue (the `+ confidential_grant` arm).
//!
//! The frozen permission **rewrites** (names freeze here; the rewrite STRUCTURE is documented below
//! and proven admissible by the CDC against the real engine — the LIVE wiring is ISS-P11/P-ID work):
//! - `view       = (parent_project->read − confidential) + confidential_grant`  ← the set-difference
//!   crux (Zanzibar Exclusion; the confidential-disappears-by-construction guarantee).
//! - `comment    = view`
//! - `transition = assignee + parent_project->write`
//! - `manage     = parent_project->write`
//! - `issue_field.view_field          = parent_issue->view`  (+ the frozen `CaveatContext` at
//!   check-time, §6.2 — field-level ABAC, OFF the hot `list_objects` path)
//! - `issue_transition.perform_transition = parent_issue->transition`  (+ the frozen `CaveatContext`,
//!   approver-role)
//!
//! **Does NOT ship (FLOOR named — VISION §3):** *no Issues feature.* No tuples are written, no
//! `check`/`list_objects` is served, no board scan runs, no transition gate evaluates. This is a
//! **contract-fragment freeze** — the relation/permission SHAPES Identity compiles against, nothing
//! more. The fragment is wired **LIVE** (admitted into the running cell schema + the permission
//! *rewrites* + the `CaveatContext` field/transition redaction carried through the rich engine
//! `FragmentDef`) at the Issues M2/M4 spine (ISS-P11 = the SetExpr lowering + the ABAC at-check, on
//! Identity's M2 `CaveatContext`/`list_objects` bodies). Until then the *names* freeze here is the
//! compile anchor; the *rewrite structure* is documented (the doc-comments above name each
//! permission's frozen rewrite) and proven admissible by the CDC test
//! (`tests/cdc_4_9_issues_fragment.rs`), which compiles the rich rewrites — including the
//! `- confidential` Exclusion — through the real engine and proves it resolves LEAK-FREE.
//!
//! ## Why names-only here (the DAG, EI-01 §7 — extend, never re-define)
//! `myelin-issues` is a consumer SUBSYSTEM crate; it depends on the frozen contract surface
//! `myelin-identity` (which carries the names-only [`myelin_identity::NamespaceFragment`]), NOT on
//! `myelin-identity-service` (the rich `FragmentDef`/`Userset` engine — a service crate). So the
//! *runtime* fragment Issues ships is the names-only carrier Identity's `admit_fragment` consumes;
//! the rich rewrite structure (incl. the set-difference) is exercised only by the CDC TEST (a
//! dev-dependency on the engine), never re-defined here. This keeps the §2.9 crate DAG acyclic (no
//! consumer→service edge) while still freezing the full fragment shape Identity must compile.

use myelin_identity::{NamespaceFragment, ObjectType, Permission, RelName};

/// The three frozen Issues object-type names (the §6.1 `definition` blocks). Public so the live-
/// wiring prompts (ISS-P11 / P-ID-*) and the CDC test reference the SAME canonical strings (one
/// source of truth — a typo here is a typo everywhere, caught by the admit).
pub mod object_types {
    /// The issue — the root Issues authz object (§6.1 `definition issue`).
    pub const ISSUE: &str = "issue";
    /// A field value on an issue — the field-level ABAC sub-object (§6.1 `definition issue_field`).
    pub const ISSUE_FIELD: &str = "issue_field";
    /// A governed transition on an issue — the transition-level ABAC sub-object (§6.1
    /// `definition issue_transition`).
    pub const ISSUE_TRANSITION: &str = "issue_transition";
}

/// Build a [`NamespaceFragment`] (the frozen names-only ABI carrier) from `&str` slices — a small
/// constructor that keeps the three fragment definitions below declarative.
fn fragment(object_type: &str, relations: &[&str], permissions: &[&str]) -> NamespaceFragment {
    NamespaceFragment {
        object_type: ObjectType(object_type.to_string()),
        relations: relations.iter().map(|r| RelName(r.to_string())).collect(),
        permissions: permissions.iter().map(|p| Permission(p.to_string())).collect(),
    }
}

/// **The `issue` fragment** (§6.1 `definition issue`).
///
/// Relations: `parent_project` (the inheritance edge into the org hierarchy — the `view`/`manage`
/// inherit from it), `assignee`, **`watcher`** (Notif read-fanout), **`confidential`** (the
/// set-difference subtraction driver), **`confidential_grant`** (the explicit re-admit). Permissions
/// (names frozen here; rewrites — wired LIVE in the Issues M2/M4 spine — documented):
/// - `view       = (parent_project->read − confidential) + confidential_grant`
/// - `comment    = view`
/// - `transition = assignee + parent_project->write`
/// - `manage     = parent_project->write`
pub fn issue_fragment() -> NamespaceFragment {
    fragment(
        object_types::ISSUE,
        &[
            "parent_project",
            "assignee",
            "watcher",
            "confidential",
            "confidential_grant",
        ],
        &["view", "comment", "transition", "manage"],
    )
}

/// **The `issue_field` fragment** (§6.1 `definition issue_field`) — field-level visibility, a
/// sub-object whose ABAC caveat is evaluated at `check`-time (§6.2), OFF the hot `list_objects` path.
///
/// Relation: `parent_issue`. Permission (name frozen; rewrite the live-wiring floor):
/// - `view_field = parent_issue->view` (+ the frozen `CaveatContext` at check-time — "hide the
///   salary column"; "field visible iff `issue.severity < X`").
pub fn issue_field_fragment() -> NamespaceFragment {
    fragment(
        object_types::ISSUE_FIELD,
        &["parent_issue"],
        &["view_field"],
    )
}

/// **The `issue_transition` fragment** (§6.1 `definition issue_transition`) — transition-level
/// visibility (governed transitions), a sub-object whose ABAC caveat (approver-role) is evaluated
/// at `check`-time (§6.2).
///
/// Relation: `parent_issue`. Permission (name frozen; rewrite the live-wiring floor):
/// - `perform_transition = parent_issue->transition` (+ the frozen `CaveatContext`, approver-role).
pub fn issue_transition_fragment() -> NamespaceFragment {
    fragment(
        object_types::ISSUE_TRANSITION,
        &["parent_issue"],
        &["perform_transition"],
    )
}

/// **The complete frozen Issues ReBAC namespace fragment** — the three [`NamespaceFragment`] carriers
/// Identity admits into the one cell schema (contract 4.9). The order is issue → issue_field →
/// issue_transition (parent-before-child, the order Identity admits them so each sub-object's
/// `parent_issue` inheritance edge has its parent type already in the schema). This is the SINGLE
/// entry point the live-wiring prompts + the CDC test consume.
pub fn issues_fragment() -> Vec<NamespaceFragment> {
    vec![
        issue_fragment(),
        issue_field_fragment(),
        issue_transition_fragment(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frozen relation set per object type (§6.1) — the well-formedness witness. A relation
    /// dropped or renamed here is caught by this test BEFORE it reaches Identity's admit.
    #[test]
    fn each_definition_declares_its_frozen_relations() {
        let issue = issue_fragment();
        let issue_rels: Vec<&str> = issue.relations.iter().map(|r| r.0.as_str()).collect();
        // §6.1 issue: parent_project, assignee, watcher, confidential, confidential_grant.
        for expected in [
            "parent_project",
            "assignee",
            "watcher",
            "confidential",
            "confidential_grant",
        ] {
            assert!(
                issue_rels.contains(&expected),
                "issue must declare the `{expected}` relation (§6.1)"
            );
        }

        // The two ABAC sub-objects each inherit through `parent_issue`.
        assert!(issue_field_fragment()
            .relations
            .contains(&RelName("parent_issue".into())));
        assert!(issue_transition_fragment()
            .relations
            .contains(&RelName("parent_issue".into())));
    }

    /// The set-difference driver (`confidential`) and its explicit re-admit (`confidential_grant`)
    /// are BOTH declared on `issue` — the `view = (parent_project->read − confidential) +
    /// confidential_grant` rewrite the engine compiles references both, so a fragment missing either
    /// would be REJECTED at admit (UndeclaredRelation). This is the no-leak guarantee's structural
    /// anchor (§6.1, D3).
    #[test]
    fn the_confidential_set_difference_relations_are_declared() {
        let issue = issue_fragment();
        assert!(
            issue.relations.contains(&RelName("confidential".into())),
            "`confidential` (the SUBTRACTED arm of `view`) must be declared (§6.1)"
        );
        assert!(
            issue.relations.contains(&RelName("confidential_grant".into())),
            "`confidential_grant` (the explicit re-admit arm) must be declared (§6.1)"
        );
    }

    /// The `watcher` read-fanout relation is on `issue` (Notif resolves `list_subjects(issue,
    /// watcher)` for the unbounded ambient set — §6.1 / contract 4.4).
    #[test]
    fn watcher_is_declared_on_the_watchable_issue_type() {
        assert!(
            issue_fragment().relations.contains(&RelName("watcher".into())),
            "the `issue` watchable type declares `watcher` (Notif read-fanout)"
        );
    }

    /// The three object types are frozen + non-empty + carry their permission names. This is the
    /// shape Identity compiles; a name dropped here breaks the cell-schema compile.
    #[test]
    fn the_three_issues_object_types_are_frozen() {
        let frag = issues_fragment();
        let types: Vec<&str> = frag.iter().map(|f| f.object_type.0.as_str()).collect();
        assert_eq!(types, vec!["issue", "issue_field", "issue_transition"]);
        // the four issue permissions are present.
        for p in ["view", "comment", "transition", "manage"] {
            assert!(
                issue_fragment().permissions.contains(&Permission(p.into())),
                "issue declares the `{p}` permission (§6.1)"
            );
        }
        // the sub-object permissions.
        assert!(issue_field_fragment()
            .permissions
            .contains(&Permission("view_field".into())));
        assert!(issue_transition_fragment()
            .permissions
            .contains(&Permission("perform_transition".into())));
    }

    /// No fragment smuggles an object id (Id never invents object ids): every type/relation/
    /// permission NAME is a bare identifier (no `:`/`/`/`#`). This mirrors the engine's
    /// `mints_object_id` admit check — a fragment that tripped it would be REJECTED at admit.
    #[test]
    fn no_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in issues_fragment() {
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
