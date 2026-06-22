//! # `issue_fragment` — Id's compiled **Issues** ReBAC namespace fragment (contract 4.9,
//! P-ID-29 → P-322)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §5 (the **frozen Issues fragment**: `issue`, `field`, `transition` + the **`confidential`
//! exclusion userset** — a confidential issue disappears from a normal project-reader's
//! `list_objects` BY CONSTRUCTION, not a post-filter; field/transition `CaveatContext` caveats),
//! §7.3 (the Issues **via_column** = `issue.id` — the board/backlog conjoin column the `list_objects`
//! `Filter` JOINs against, one query, no N+1), §8.6 (the **field/transition caveat on the full
//! `QueryAst` predicate core** — the off-hot-path field/transition ABAC rider, P-ID-22).
//!
//! **Contract-index rows:** **4.9** (the Issues fragment — OWNED here), **4.3** (the board conjoin —
//! consumed via [`crate::list_objects`]: `list_objects(subject, view, issue)` keyed on `issue.id`),
//! **4.2** (the field/transition caveats — consumed via
//! [`crate::check_engine::eval_caveat_predicate`], the ONE `QueryAst` predicate core).
//!
//! This is the **FOURTH of the five per-subsystem fragments** (P-ID-24/26/27/29/30) that promote the
//! M1 engine-only floor (P-068): the first (Git) is [`crate::git_fragment`], the second (Knowledge)
//! is [`crate::knowledge_fragment`], the third (CI) is [`crate::ci_fragment`]. Like them it is the
//! canonical **rich** [`crate::namespace::FragmentDef`] declaration of the Issues authz vocabulary,
//! with the permission **rewrites** wired over the four Zanzibar userset operators so
//! `check`/`list_objects` resolve the Issues permissions through the SAME engine the core hierarchy
//! uses (one primitive — no bespoke Issues check path, the §5 design rule). The Issues data model
//! (the issue/board/transition tables themselves) is the Issues-subsystem prompts'; this module ships
//! only the Id-side authz content.
//!
//! ## Why the rich fragment lives HERE (not in `myelin-issues`)
//! Same DAG discipline as [`crate::git_fragment`] / [`crate::knowledge_fragment`] /
//! [`crate::ci_fragment`]: `myelin-identity-service` (the engine) does NOT depend on a subsystem leaf
//! crate (§2.9 acyclic DAG). `myelin-issues` already declares the **names-only** ABI carrier
//! [`myelin_identity::NamespaceFragment`] (`myelin_issues::rebac_fragment`, ISS-P01/P-125) — the shape
//! Identity's `admit_fragment` consumes at the contract boundary. But the names-only carrier cannot
//! carry the rewrite STRUCTURE (the `view = (parent_project->view − confidential) ∪ confidential_grant`
//! exclusion that makes a confidential issue disappear by construction); only the engine's rich
//! `FragmentDef` can. So **Id owns the compiled rewrites** (this module), declared from the
//! architecture §5 frozen vocabulary directly, and the CDC test (`tests/cdc_4_9_issue_fragment.rs`)
//! pins that the two sides agree on the relation/permission NAMES.
//!
//! ## The compiled Issues fragment (§5 / Issues §6.1)
//!
//! | object type    | relations                                                              | permissions (rewrite)                                                                       |
//! |----------------|------------------------------------------------------------------------|---------------------------------------------------------------------------------------------|
//! | `issue`        | `parent_project` `assignee` `watcher` `confidential` `confidential_grant` | **`view = (parent_project->view − confidential) ∪ confidential_grant`**; `comment = view`; `transition = assignee ∪ parent_project->view`; `manage = parent_project->view` |
//! | `field`        | `parent_issue`                                                         | `view_field = parent_issue->view` (+ the off-hot-path `CaveatContext` field caveat, §8.6)   |
//! | `transition`   | `parent_issue` `approver`                                              | `perform_transition = parent_issue->transition` (+ the off-hot-path `CaveatContext` transition caveat, §8.6) |
//!
//! - **the `− confidential` exclusion userset (§5, ISS-D3, the headline)** — `issue.view` is the
//!   inheritance base `parent_project->view` with `confidential` **EXCLUDED**, then `confidential_grant`
//!   re-UNIONed. So a confidential issue **disappears from a normal project-reader's `list_objects`
//!   BY CONSTRUCTION** (the Exclusion removes them from the `view` set — never a post-filter, never a
//!   count leak): the SetExpr `list_objects(subject, view, issue)` push-down keyed on `issue.id` (§7.3)
//!   simply does not emit the confidential issue's id for a reader lacking `confidential_grant`. A
//!   subject explicitly re-admitted (`issue#confidential_grant@subject`) sees it again (the `∪
//!   confidential_grant` arm). This is the SAME exclusion crux as Knowledge's `− direct_block` and CI's
//!   `− is_untrusted_fork`. A mutation that turns the exclusion into a post-filter (e.g. drops the
//!   Exclusion, leaving a bare inheritance union) MUST be caught — the mutation-tested core
//!   ([`tests::view_is_inheritance_minus_confidential_union_grant`]).
//! - **field-level visibility (§8.6, C3)** — `field.view_field = parent_issue->view` resolves row
//!   visibility (you may attempt to view a field only on an issue you can `view`); the individual
//!   field is then redacted OFF the hot path by a `check`-time `CaveatContext` caveat over the ONE
//!   `QueryAst` predicate core (e.g. "the `salary` field is visible iff `viewer.clearance ≥ 3`";
//!   "field visible iff `issue.severity < X`"). [`field_view_caveat`] builds it.
//! - **transition-level gating (§8.6, C3)** — `transition.perform_transition = parent_issue->transition`
//!   resolves who may attempt the governed transition; the actual gate (e.g. "this transition needs an
//!   approver edge" / "needs sign-off iff `issue.severity ≥ X`") is the off-hot-path `CaveatContext`
//!   transition caveat over the SAME core. [`transition_caveat`] builds it. The `transition` type also
//!   declares an `approver` relation so an approver-edge gate is an ordinary tuple the caveat reads.
//! - **watchability (C8)** — `issue` declares the cross-cutting `watcher` relation so Notif's
//!   read-fanout `list_subjects(issue, watcher)` is an ordinary Expand over S8.

use crate::namespace::{FragmentDef, PermissionRule, Userset};
use myelin_identity::{
    CaveatContext, FieldId, Literal, ObjectType, Permission, RelName, TransitionId,
};
use myelin_tenancy::ArtifactRef;
use std::collections::BTreeMap;

/// The three frozen Issues object-type names (§5 / Issues §6.1; mirrors
/// `myelin_issues::rebac_fragment::object_types`). Public so the CDC test + a live-wiring caller
/// reference the SAME canonical strings.
///
/// **NOTE on naming reconciliation (EI-01 §1):** the architecture §5 (identity doc) names the Issues
/// fragment's sub-objects `field` and `transition`; the Issues subsystem's names-only carrier
/// (ISS-P01/P-125, `myelin_issues::rebac_fragment`) names the SAME object types `issue_field` /
/// `issue_transition` (a prefixed spelling) and the field/transition permissions `view_field` /
/// `perform_transition`. The identity-side compiled fragment is declared from the architecture §5
/// vocabulary it OWNS (`field`/`transition`), which is the canonical authz spelling here; the
/// permission names (`view_field`/`perform_transition`) match the carrier byte-for-byte. The CDC test
/// pins the agreement that matters: the permission NAMES + the rewrite STRUCTURE resolve leak-free.
pub mod object_types {
    /// The issue — the root Issues authz object (§5 `definition issue`).
    pub const ISSUE: &str = "issue";
    /// A field value on an issue — the field-level ABAC sub-object (§5; the off-hot-path field caveat).
    pub const FIELD: &str = "field";
    /// A governed transition on an issue — the transition-level ABAC sub-object (§5; the off-hot-path
    /// transition caveat).
    pub const TRANSITION: &str = "transition";
}

/// **The `confidential` relation (the EXCLUSION driver, §5).** A subject (or marker) stamped on an
/// issue as `confidential`; it is the SUBTRACTED arm of `issue.view`, so a confidential issue
/// disappears from a normal project-reader's `list_objects` by construction (the no-leak ISS-D3
/// guarantee). Exposed so the CDC + a live caller reference the canonical name, not a stringly-typed
/// literal.
pub const CONFIDENTIAL: &str = "confidential";

/// **The `confidential_grant` relation (the explicit RE-ADMIT, §5).** The `∪ confidential_grant` arm
/// of `issue.view`: a subject explicitly re-admitted to a confidential issue (`issue#confidential_grant@subject`)
/// sees it again, even though the `− confidential` exclusion removed the ambient project-reader set.
pub const CONFIDENTIAL_GRANT: &str = "confidential_grant";

/// **The `assignee` relation** — the issue's assignee (also a `transition` grantee: `transition =
/// assignee ∪ parent_project->view`).
pub const ASSIGNEE: &str = "assignee";

/// **The `approver` relation on `transition`** — the approver-edge a transition caveat may read
/// ("this transition needs an approver edge"). An ordinary tuple, so the gate is an ordinary `check`.
pub const APPROVER: &str = "approver";

/// **The `view` permission name** — the issue-visibility permission `list_objects(subject, view,
/// issue)` pushes down (keyed on `issue.id`, §7.3) and `check(subject, view, issue)` resolves through
/// the `(parent_project->view − confidential) ∪ confidential_grant` rewrite.
pub const VIEW: &str = "view";

/// **The `comment` permission name** (`comment = view`).
pub const COMMENT: &str = "comment";

/// **The `transition` permission name** on `issue` (`transition = assignee ∪ parent_project->view`).
pub const TRANSITION_PERM: &str = "transition";

/// **The `manage` permission name** on `issue` (`manage = parent_project->view`).
pub const MANAGE: &str = "manage";

/// **The `view_field` permission name** on `field` — the field-level read GATE (§8.6): it resolves to
/// the parent issue's `view`, and the off-hot-path `CaveatContext` field caveat then redacts the
/// individual field on top. Mirrors the Issues carrier + the Knowledge `view_field` spelling.
pub const VIEW_FIELD: &str = "view_field";

/// **The `perform_transition` permission name** on `transition` — who may ATTEMPT the governed
/// transition (`perform_transition = parent_issue->transition`); the actual gate is the off-hot-path
/// `CaveatContext` transition caveat. Mirrors the Issues carrier.
pub const PERFORM_TRANSITION: &str = "perform_transition";

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

/// **The `issue` fragment** (§5 `definition issue`) — the confidential-exclusion node, the headline
/// rewrite.
///
/// **`view = (parent_project->view − confidential) ∪ confidential_grant`** — the §5 confidential
/// rewrite. The base is the project-read inheritance (`parent_project->view`, the compiled core
/// `project.view`); `confidential` is **EXCLUDED** from it (the §5 `− confidential` set-difference),
/// then `confidential_grant` is **UNIONed** back (the explicit re-admit). So a confidential issue
/// disappears from a normal project-reader's `view` set BY CONSTRUCTION (the Exclusion), and ONLY a
/// `confidential_grant` re-admits — never a post-filter. `comment = view` (you comment on what you can
/// see); `transition = assignee ∪ parent_project->view` (the assignee or any project member may drive
/// the workflow — the per-transition gate is the off-hot-path caveat); `manage = parent_project->view`
/// (workflow/field administration is a project capability). Watchable (C8).
///
/// **Reconciliation (EI-01 §1):** the §5 / Issues §6.1 vocabulary names the inheritance edge
/// `parent_project->read` (and `->write` for `transition`/`manage`). The shipped **core hierarchy**
/// ([`crate::namespace::core_hierarchy`]) compiles a single `project.view` permission (relations
/// `reader`/`writer`), so both edges resolve through `parent_project->view` — EXACTLY the same
/// reconciliation [`crate::git_fragment`] makes (`repo.pull`/`repo.push` both inherit
/// `parent_project->view`). The §5 `read`/`write` names denote the project's read/write CAPABILITY,
/// which the engine surfaces as the one compiled `project.view`; declaring a second core project
/// permission would fork the core hierarchy, which the engine-only floor forbids.
pub fn issue_fragment() -> FragmentDef {
    frag(
        object_types::ISSUE,
        &[
            "parent_project",
            ASSIGNEE,
            "watcher",
            CONFIDENTIAL,
            CONFIDENTIAL_GRANT,
        ],
        vec![
            // view = (parent_project->view − confidential) ∪ confidential_grant (the §5 exclusion).
            perm(
                VIEW,
                Userset::Union(vec![
                    Userset::Exclusion {
                        base: Box::new(ttu("parent_project", "view")),
                        subtracted: Box::new(rel(CONFIDENTIAL)),
                    },
                    rel(CONFIDENTIAL_GRANT),
                ]),
            ),
            // comment = view (you comment on what you can see).
            perm(
                COMMENT,
                Userset::Union(vec![
                    Userset::Exclusion {
                        base: Box::new(ttu("parent_project", "view")),
                        subtracted: Box::new(rel(CONFIDENTIAL)),
                    },
                    rel(CONFIDENTIAL_GRANT),
                ]),
            ),
            // transition = assignee ∪ parent_project->view (who may drive the workflow; the
            // per-transition gate is the off-hot-path caveat).
            perm(
                TRANSITION_PERM,
                Userset::Union(vec![rel(ASSIGNEE), ttu("parent_project", "view")]),
            ),
            // manage = parent_project->view (workflow/field administration is a project capability).
            perm(MANAGE, ttu("parent_project", "view")),
        ],
    )
    .watchable()
}

/// **The `field` fragment** (§5 `definition field`) — the field-level ABAC sub-object.
///
/// `view_field = parent_issue->view` — a field is visible only on an issue the viewer can `view`
/// (the row-visibility precondition). The individual FIELD redaction rides OFF the hot `list_objects`
/// path as a `check`-time `CaveatContext` caveat over the ONE `QueryAst` predicate core (§8.6, C3) —
/// [`field_view_caveat`]: `check(subject, view_field, field, CaveatContext)` then redacts the field.
/// A denied field is `Deny` (redacted, absent from any projection/count); a field whose predicate
/// references missing context is `Conditional` (the caller supplies it) — never a silent allow.
pub fn field_fragment() -> FragmentDef {
    frag(
        object_types::FIELD,
        &["parent_issue"],
        vec![perm(VIEW_FIELD, ttu("parent_issue", VIEW))],
    )
}

/// **The `transition` fragment** (§5 `definition transition`) — the transition-level ABAC sub-object.
///
/// `perform_transition = parent_issue->transition` — who may ATTEMPT the governed transition inherits
/// the parent issue's `transition` permission. The actual gate (e.g. "this transition needs an
/// approver edge"; "needs sign-off iff `issue.severity ≥ X`") is the off-hot-path `CaveatContext`
/// transition caveat over the SAME `QueryAst` core ([`transition_caveat`]). The `approver` relation is
/// declared so an approver-edge gate is an ordinary tuple the caveat reads (never a bespoke check).
pub fn transition_fragment() -> FragmentDef {
    frag(
        object_types::TRANSITION,
        &["parent_issue", APPROVER],
        vec![perm(
            PERFORM_TRANSITION,
            ttu("parent_issue", TRANSITION_PERM),
        )],
    )
}

/// **The complete compiled Issues ReBAC namespace fragment (contract 4.9)** — the three rich
/// [`FragmentDef`]s Identity admits into the one cell schema, in parent-before-child order (`issue` →
/// `field` → `transition`) so each sub-object's `parent_issue` inheritance edge has its parent type
/// already in the schema when it admits. This is the SINGLE entry point
/// [`crate::StoreBackedCheck::admit_issue_fragment`] and the CDC test consume.
pub fn issue_fragment_defs() -> Vec<FragmentDef> {
    vec![issue_fragment(), field_fragment(), transition_fragment()]
}

/// **Build the field-level redaction caveat (§8.6, C3) for an issue `field` — a `CaveatContext` over
/// the ONE `QueryAst` predicate core.**
///
/// Field-level hiding is NOT a namespace permission (that is issue visibility, the `list_objects`
/// push-down). It is an off-hot-path ABAC rider evaluated at `check`-time on an ALREADY-VISIBLE issue:
/// `check(subject, view_field, field, CaveatContext)` then redacts the individual field. This helper
/// builds the `CaveatContext` the Issues subsystem passes to `check` for a field gated by a NON-LITERAL
/// predicate over a runtime attribute — e.g. "the `severity` field is visible iff the viewer's
/// `clearance` attribute ≥ the field's threshold". The predicate is lowered through the SAME
/// `__caveat_*`/`_var` encoding the M1 bridge ([`crate::check_engine::eval_caveat`]) routes through the
/// ONE `myelin_query` interpreter (no second predicate language, EI-01 §7). A field whose predicate is
/// VIOLATED is `Deny` (redacted); one whose predicate references missing context is `Conditional` (the
/// caller supplies it) — **never a silent `Allow`** (§8.6).
///
/// `field` is the `field` object id; `field_name` is the column/field name; `op` is a comparison
/// operator (`eq`/`ne`/`lt`/`le`/`gt`/`ge`); `lhs_var` is a context variable the predicate reads (the
/// non-literal operand — e.g. `clearance`); `rhs` is the literal threshold; `ctx` supplies the runtime
/// values for the variable(s) the caller resolved on the field.
pub fn field_view_caveat(
    field: &str,
    field_name: &str,
    op: &str,
    lhs_var: &str,
    rhs: Literal,
    ctx: &[(&str, Literal)],
) -> CaveatContext {
    let mut attrs: BTreeMap<String, Literal> = BTreeMap::new();
    attrs.insert("__caveat_op".into(), Literal::Str(op.into()));
    attrs.insert("__caveat_lhs_var".into(), Literal::Str(lhs_var.into()));
    attrs.insert("__caveat_rhs".into(), rhs);
    for (k, v) in ctx {
        attrs.insert((*k).to_string(), v.clone());
    }
    CaveatContext {
        object: ArtifactRef(field.to_string()),
        field: Some(FieldId(field_name.to_string())),
        transition: None,
        attrs,
    }
}

/// **Build the transition-gating caveat (§8.6, C3) for an issue `transition` — a `CaveatContext` over
/// the ONE `QueryAst` predicate core.**
///
/// Transition gating is the transition-level twin of [`field_view_caveat`]: an off-hot-path ABAC rider
/// evaluated at `check`-time on a transition the subject may already ATTEMPT (`perform_transition`
/// resolved). `check(subject, perform_transition, transition, CaveatContext)` then gates the actual
/// move — e.g. "this transition needs an approver edge present" / "needs sign-off iff `issue.severity ≥
/// X`". The predicate rides the SAME `__caveat_*`/`_var` encoding through the ONE `myelin_query`
/// interpreter. A transition whose precondition is UNMET is `Deny` (gated); one referencing missing
/// context is `Conditional` (the caller supplies it — e.g. the approver edge not yet fetched) — **never
/// a silent `Allow`** (§8.6). The `transition: Some(..)` discriminator is what distinguishes a
/// transition caveat from a field caveat in the frozen `CaveatContext{object, field?, transition?,
/// attrs}` shape (C3).
///
/// `transition` is the `transition` object id; `transition_name` is the named move (e.g.
/// `close`/`approve`); `op`/`lhs_var`/`rhs`/`ctx` are as for [`field_view_caveat`] (the non-literal
/// gate predicate + its runtime context).
pub fn transition_caveat(
    transition: &str,
    transition_name: &str,
    op: &str,
    lhs_var: &str,
    rhs: Literal,
    ctx: &[(&str, Literal)],
) -> CaveatContext {
    let mut attrs: BTreeMap<String, Literal> = BTreeMap::new();
    attrs.insert("__caveat_op".into(), Literal::Str(op.into()));
    attrs.insert("__caveat_lhs_var".into(), Literal::Str(lhs_var.into()));
    attrs.insert("__caveat_rhs".into(), rhs);
    for (k, v) in ctx {
        attrs.insert((*k).to_string(), v.clone());
    }
    CaveatContext {
        object: ArtifactRef(transition.to_string()),
        field: None,
        transition: Some(TransitionId(transition_name.to_string())),
        attrs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::NamespaceEngine;
    use myelin_identity::FragmentAdmit;

    /// **The compiled Issues fragment admits into the cell schema (the engine-only-floor progression).**
    /// Every Issues object type admits on top of the core org/team/project hierarchy; the three types
    /// enter the compiled vocabulary; `issue.view` + the sub-object permissions resolve as compiled
    /// permissions.
    #[test]
    fn issue_fragment_admits_into_the_cell_schema() {
        let mut eng = NamespaceEngine::with_core_hierarchy();
        for def in issue_fragment_defs() {
            let admit = eng.admit(&def);
            assert!(
                matches!(admit, FragmentAdmit::Admitted { .. }),
                "the Issues `{}` fragment admits into the cell schema: {admit:?}",
                def.object_type.0
            );
        }
        for ty in ["issue", "field", "transition"] {
            assert!(
                eng.object_types().contains(&ty.to_string()),
                "`{ty}` is admitted"
            );
        }
        assert!(
            eng.resolve_permission("issue", VIEW).is_some(),
            "issue.view is a compiled permission"
        );
        assert!(
            eng.resolve_permission("field", VIEW_FIELD).is_some(),
            "field.view_field is a compiled permission"
        );
        assert!(
            eng.resolve_permission("transition", PERFORM_TRANSITION)
                .is_some(),
            "transition.perform_transition is a compiled permission"
        );
    }

    /// **`issue.view` is the confidential-exclusion rewrite (§5, ISS-D3): `(parent_project->view −
    /// confidential) ∪ confidential_grant`.** The Exclusion is the mutation-tested core: the rewrite
    /// MUST subtract `confidential` from the inheritance base, then union `confidential_grant` — NOT a
    /// bare inheritance union (a mutation dropping the Exclusion turns the no-leak guarantee into a
    /// post-filter and is caught HERE structurally, and behaviourally in the drill).
    #[test]
    fn view_is_inheritance_minus_confidential_union_grant() {
        let issue = issue_fragment();
        let view = issue
            .permissions
            .iter()
            .find(|p| p.permission.0 == VIEW)
            .expect("issue declares view");
        match &view.rewrite {
            Userset::Union(arms) => {
                // One arm is the explicit re-admit (confidential_grant).
                assert!(
                    arms.contains(&rel(CONFIDENTIAL_GRANT)),
                    "view unions confidential_grant (the explicit re-admit arm, §5)"
                );
                // The OTHER arm is the Exclusion of confidential over the project inheritance.
                let excl = arms
                    .iter()
                    .find_map(|a| match a {
                        Userset::Exclusion { base, subtracted } => Some((base, subtracted)),
                        _ => None,
                    })
                    .expect("view contains the − confidential Exclusion arm");
                assert_eq!(
                    **excl.1,
                    rel(CONFIDENTIAL),
                    "the exclusion subtracts confidential (the − confidential §5 rewrite, ISS-D3)"
                );
                assert_eq!(
                    **excl.0,
                    ttu("parent_project", "view"),
                    "the exclusion base is the project-read inheritance (parent_project->view)"
                );
            }
            other => panic!(
                "issue.view must be a Union[Exclusion(− confidential), confidential_grant], got {other:?}"
            ),
        }
    }

    /// **The confidential exclusion is NOT a post-filter — it is a compiled set-difference USERSET
    /// (ISS-D3, the mutation floor).** The view rewrite tree MUST contain an `Exclusion` subtracting
    /// `confidential`. A mutation that flattens it into a bare `Union[parent_project->view,
    /// confidential_grant]` (the post-filter shape — "list everything, then drop confidential after")
    /// removes the Exclusion and is caught here.
    #[test]
    fn confidential_exclusion_is_a_set_difference_not_a_post_filter() {
        let issue = issue_fragment();
        let view = issue
            .permissions
            .iter()
            .find(|p| p.permission.0 == VIEW)
            .expect("issue declares view");
        assert!(
            rewrite_has_exclusion_of(&view.rewrite, CONFIDENTIAL),
            "issue.view MUST contain an Exclusion of `confidential` (set-difference by construction, \
             NOT a post-filter) — ISS-D3 mutation floor"
        );
    }

    /// **`comment = view` (the same confidential-exclusion rewrite).** A subject comments only on an
    /// issue they can see — so a confidential issue is equally invisible to comment.
    #[test]
    fn comment_mirrors_view() {
        let issue = issue_fragment();
        let view = issue
            .permissions
            .iter()
            .find(|p| p.permission.0 == VIEW)
            .unwrap();
        let comment = issue
            .permissions
            .iter()
            .find(|p| p.permission.0 == COMMENT)
            .expect("issue declares comment");
        assert_eq!(comment.rewrite, view.rewrite, "comment = view (§5)");
    }

    /// **`transition = assignee ∪ parent_project->view` and `manage = parent_project->view` (§5).**
    /// The workflow-drive permission unions the assignee with project membership; manage is a project
    /// capability. (The per-transition gate itself is the off-hot-path caveat, not these permissions.)
    #[test]
    fn transition_and_manage_rewrites() {
        let issue = issue_fragment();
        let transition = issue
            .permissions
            .iter()
            .find(|p| p.permission.0 == TRANSITION_PERM)
            .expect("issue declares transition");
        assert_eq!(
            transition.rewrite,
            Userset::Union(vec![rel(ASSIGNEE), ttu("parent_project", "view")]),
            "transition = assignee ∪ parent_project->view (§5)"
        );
        let manage = issue
            .permissions
            .iter()
            .find(|p| p.permission.0 == MANAGE)
            .expect("issue declares manage");
        assert_eq!(
            manage.rewrite,
            ttu("parent_project", "view"),
            "manage = parent_project->view (§5)"
        );
    }

    /// **`field.view_field = parent_issue->view` and `transition.perform_transition =
    /// parent_issue->transition` (§5).** The sub-objects inherit their parent issue's visibility /
    /// workflow permission; the field redaction + transition gate ride off the hot path as caveats.
    #[test]
    fn sub_objects_inherit_the_parent_issue() {
        let field = field_fragment();
        let vf = field
            .permissions
            .iter()
            .find(|p| p.permission.0 == VIEW_FIELD)
            .expect("field declares view_field");
        assert_eq!(
            vf.rewrite,
            ttu("parent_issue", VIEW),
            "field.view_field = parent_issue->view (§8.6 row-visibility precondition)"
        );
        let transition = transition_fragment();
        let pt = transition
            .permissions
            .iter()
            .find(|p| p.permission.0 == PERFORM_TRANSITION)
            .expect("transition declares perform_transition");
        assert_eq!(
            pt.rewrite,
            ttu("parent_issue", TRANSITION_PERM),
            "transition.perform_transition = parent_issue->transition (§5)"
        );
    }

    /// **`issue` is WATCHABLE (C8): it declares the `watcher` relation** so Notif's read-fanout
    /// `list_subjects(issue, watcher)` is an ordinary Expand. The sub-objects (`field`/`transition`)
    /// are not independently watchable (they inherit the issue's ACL; a watcher fans out at issue
    /// granularity).
    #[test]
    fn issue_is_watchable() {
        assert!(issue_fragment().is_watchable(), "issue is watchable (C8)");
        assert!(
            !field_fragment().is_watchable(),
            "field is not independently watchable"
        );
        assert!(
            !transition_fragment().is_watchable(),
            "transition is not independently watchable"
        );
    }

    /// **The field caveat hides a field through the ONE `QueryAst` core, and a violated field is
    /// `Deny` (redacted) — absent from any projection, never a silent allow (§8.6, C3).** Build a
    /// `severity`-field caveat "visible iff clearance ≥ 3": clearance 4 → Allow; clearance 1 → Deny
    /// (redacted); clearance NOT supplied → Conditional (the mandatory no-silent-allow branch).
    #[test]
    fn field_caveat_hides_a_field_through_the_one_query_core() {
        use crate::check_engine::eval_caveat;
        use myelin_identity::Decision;

        let cleared = field_view_caveat(
            "field:issue-1/severity",
            "severity",
            "ge",
            "clearance",
            Literal::Int(3),
            &[("clearance", Literal::Int(4))],
        );
        assert_eq!(
            eval_caveat(&cleared),
            Decision::Allow,
            "cleared viewer sees the severity field"
        );

        let blocked = field_view_caveat(
            "field:issue-1/severity",
            "severity",
            "ge",
            "clearance",
            Literal::Int(3),
            &[("clearance", Literal::Int(1))],
        );
        assert_eq!(
            eval_caveat(&blocked),
            Decision::Deny,
            "under-cleared viewer's severity field is redacted (absent from the projection)"
        );

        let missing = field_view_caveat(
            "field:issue-1/severity",
            "severity",
            "ge",
            "clearance",
            Literal::Int(3),
            &[],
        );
        assert_eq!(
            eval_caveat(&missing),
            Decision::Conditional,
            "a field caveat needing missing context is Conditional, never a silent allow (§8.6)"
        );
        // The caveat is a FIELD caveat (the field? discriminator is set, transition? is not).
        assert!(missing.field.is_some() && missing.transition.is_none());
    }

    /// **The transition caveat gates a transition through the ONE `QueryAst` core, and an unmet gate
    /// is `Deny` — never a silent allow (§8.6, C3).** Build an `approve` transition caveat "permitted
    /// iff approver_count ≥ 2": 2 approvers → Allow; 1 approver → Deny (gated); approver_count NOT
    /// supplied → Conditional (the caller fetches the approver edge). The `transition?` discriminator
    /// distinguishes it from a field caveat.
    #[test]
    fn transition_caveat_gates_a_transition_through_the_one_query_core() {
        use crate::check_engine::eval_caveat;
        use myelin_identity::Decision;

        let approved = transition_caveat(
            "transition:issue-1/approve",
            "approve",
            "ge",
            "approver_count",
            Literal::Int(2),
            &[("approver_count", Literal::Int(2))],
        );
        assert_eq!(
            eval_caveat(&approved),
            Decision::Allow,
            "a transition with enough approvers is permitted"
        );

        let blocked = transition_caveat(
            "transition:issue-1/approve",
            "approve",
            "ge",
            "approver_count",
            Literal::Int(2),
            &[("approver_count", Literal::Int(1))],
        );
        assert_eq!(
            eval_caveat(&blocked),
            Decision::Deny,
            "a transition lacking the approver edge is gated (Deny)"
        );

        let missing = transition_caveat(
            "transition:issue-1/approve",
            "approve",
            "ge",
            "approver_count",
            Literal::Int(2),
            &[],
        );
        assert_eq!(
            eval_caveat(&missing),
            Decision::Conditional,
            "a transition caveat needing missing context is Conditional, never a silent allow (§8.6)"
        );
        // The caveat is a TRANSITION caveat (the transition? discriminator is set, field? is not).
        assert!(missing.transition.is_some() && missing.field.is_none());
    }

    /// **No Issues fragment name smuggles an object id (Id never invents object ids).** Every
    /// type/relation/permission name is a bare identifier — the engine's `mints_object_id` admit check
    /// would reject one that wasn't.
    #[test]
    fn no_issue_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in issue_fragment_defs() {
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

    /// Whether a rewrite tree anywhere contains an `Exclusion` whose subtracted arm is the bare
    /// relation `rel_name`. Used to prove `issue.view` subtracts `confidential` by construction (a
    /// set-difference, not a post-filter).
    fn rewrite_has_exclusion_of(rw: &Userset, rel_name: &str) -> bool {
        match rw {
            Userset::Relation(_) | Userset::TupleToUserset { .. } => false,
            Userset::Union(arms) | Userset::Intersect(arms) => {
                arms.iter().any(|a| rewrite_has_exclusion_of(a, rel_name))
            }
            Userset::Exclusion { base, subtracted } => {
                matches!(&**subtracted, Userset::Relation(r) if r.0 == rel_name)
                    || rewrite_has_exclusion_of(base, rel_name)
                    || rewrite_has_exclusion_of(subtracted, rel_name)
            }
        }
    }
}
