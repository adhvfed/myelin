use crate::namespace::{FragmentDef, PermissionRule, Userset};
use myelin_identity::{
    CaveatContext, FieldId, Literal, ObjectType, Permission, RelName, TransitionId,
};
use myelin_tenancy::ArtifactRef;
use std::collections::BTreeMap;

pub mod object_types {
    pub const ISSUE: &str = "issue";
    pub const FIELD: &str = "field";
    pub const TRANSITION: &str = "transition";
}

pub const CONFIDENTIAL: &str = "confidential";

pub const CONFIDENTIAL_GRANT: &str = "confidential_grant";

pub const ASSIGNEE: &str = "assignee";

pub const APPROVER: &str = "approver";

pub const VIEW: &str = "view";

pub const COMMENT: &str = "comment";

pub const TRANSITION_PERM: &str = "transition";

pub const MANAGE: &str = "manage";

pub const VIEW_FIELD: &str = "view_field";

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
            perm(
                TRANSITION_PERM,
                Userset::Union(vec![rel(ASSIGNEE), ttu("parent_project", "view")]),
            ),
            perm(MANAGE, ttu("parent_project", "view")),
        ],
    )
    .watchable()
}

pub fn field_fragment() -> FragmentDef {
    frag(
        object_types::FIELD,
        &["parent_issue"],
        vec![perm(VIEW_FIELD, ttu("parent_issue", VIEW))],
    )
}

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

pub fn issue_fragment_defs() -> Vec<FragmentDef> {
    vec![issue_fragment(), field_fragment(), transition_fragment()]
}

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
                assert!(
                    arms.contains(&rel(CONFIDENTIAL_GRANT)),
                    "view unions confidential_grant (the explicit re-admit arm, §5)"
                );
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
             NOT a post-filter) - ISS-D3 mutation floor"
        );
    }

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
        assert!(missing.field.is_some() && missing.transition.is_none());
    }

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
        assert!(missing.transition.is_some() && missing.field.is_none());
    }

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
