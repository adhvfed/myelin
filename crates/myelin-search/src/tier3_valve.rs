use myelin_identity::{
    Consistency, ListObjectsResult, ObjectType, Permission, Principal, Result as AuthzResult,
    SetExpr, Zookie,
};
use myelin_query::QueryAst;

use crate::engine::{AclFilter, IndexBackend};
use crate::pipeline::{
    self, ListObjectsPort, Page, QueryError, QueryStats, RankedResults, RelationalLeaf,
    ReverseIndexAnswer, RevisionWatermark, ScopedEngine,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OltpBudget {
    pub max_rows: usize,
}

impl OltpBudget {
    pub const DEFAULT_MAX_ROWS: usize = 10_000;

    pub fn new(max_rows: usize) -> OltpBudget {
        OltpBudget { max_rows }
    }

    pub fn is_over_budget(&self, candidate_rows: usize) -> bool {
        candidate_rows > self.max_rows
    }
}

impl Default for OltpBudget {
    fn default() -> OltpBudget {
        OltpBudget::new(OltpBudget::DEFAULT_MAX_ROWS)
    }
}

#[derive(Clone, Debug)]
pub struct BoardQuery {
    pub ast: QueryAst,
    pub set_expr: SetExpr,
    pub zookie: Zookie,
}

impl BoardQuery {
    pub fn new(ast: QueryAst, set_expr: SetExpr, zookie: Zookie) -> BoardQuery {
        BoardQuery {
            ast,
            set_expr,
            zookie,
        }
    }
}

pub struct BoardEscalationAuthz<'a> {
    set_expr: SetExpr,
    zookie: Zookie,
    expect_type: ObjectType,
    reverse_resolver: Option<&'a dyn ReverseResolver>,
}

pub trait ReverseResolver {
    fn resolve(
        &self,
        subject: &Principal,
        form: &RelationalLeaf,
        required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer>;
}

impl<'a> BoardEscalationAuthz<'a> {
    pub fn new(
        set_expr: SetExpr,
        zookie: Zookie,
        expect_type: ObjectType,
    ) -> BoardEscalationAuthz<'a> {
        BoardEscalationAuthz {
            set_expr,
            zookie,
            expect_type,
            reverse_resolver: None,
        }
    }

    pub fn with_reverse_resolver(
        mut self,
        resolver: &'a dyn ReverseResolver,
    ) -> BoardEscalationAuthz<'a> {
        self.reverse_resolver = Some(resolver);
        self
    }
}

impl ListObjectsPort for BoardEscalationAuthz<'_> {
    fn list_objects(
        &self,
        _subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        if permission != &Permission(pipeline::READ_PERMISSION.to_string()) {
            return Err(myelin_identity::AuthzError::Unavailable(format!(
                "the Tier-3 valve escalates only a `read` board scan; got permission `{}` \
                 (the valve carries the board's own ACL pre-filter - it never widens the permission)",
                permission.0
            )));
        }
        if ty != &self.expect_type {
            return Err(myelin_identity::AuthzError::Unavailable(format!(
                "the Tier-3 valve escalated a board scan for type `{}` but Search asked for type `{}` \
                 (the seam carries the board's own `set_expr` - a type mismatch is a mis-wired board)",
                self.expect_type.0, ty.0
            )));
        }
        Ok(ListObjectsResult::Filter {
            set_expr: self.set_expr.clone(),
            zookie: self.zookie.clone(),
        })
    }

    fn resolve_relation(
        &self,
        subject: &Principal,
        form: &RelationalLeaf,
        required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        match self.reverse_resolver {
            Some(r) => r.resolve(subject, form, required),
            None => Err(myelin_identity::AuthzError::Unavailable(
                "the Tier-3 valve was handed a relational `SetExpr` leaf but no reverse-index \
                 resolver is wired - a relational form cannot be resolved (deny-when-unsure, ADR-03; \
                 wire `with_reverse_resolver` for a board whose ACL carries InRelation/TupleSet)"
                    .into(),
            )),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn escalate_to_search<B: IndexBackend>(
    engine: &ScopedEngine<'_, B>,
    board: &BoardQuery,
    viewer: &Principal,
    ty: &ObjectType,
    at: &Consistency,
    page: Page,
    stats: &QueryStats,
    reverse_resolver: Option<&dyn ReverseResolver>,
) -> Result<RankedResults, QueryError> {
    let mut authz =
        BoardEscalationAuthz::new(board.set_expr.clone(), board.zookie.clone(), ty.clone());
    if let Some(r) = reverse_resolver {
        authz = authz.with_reverse_resolver(r);
    }
    pipeline::query(engine, &authz, &board.ast, viewer, ty, at, page, stats)
}

pub fn oltp_board_admits(
    set_expr: &SetExpr,
    candidate_rows: &[String],
    subject: &Principal,
    zookie: &Zookie,
    reverse_resolver: Option<&dyn ReverseResolver>,
) -> Result<Vec<String>, QueryError> {
    let acl = board_acl_filter(set_expr, subject, zookie, reverse_resolver)?;
    Ok(candidate_rows
        .iter()
        .filter(|row| acl.admits(row, row))
        .cloned()
        .collect())
}

pub fn board_acl_filter(
    set_expr: &SetExpr,
    subject: &Principal,
    zookie: &Zookie,
    reverse_resolver: Option<&dyn ReverseResolver>,
) -> Result<AclFilter, QueryError> {
    let required = pipeline::watermark_from_zookie(&zookie.0);
    let stats = QueryStats::new();
    let port = BoundedSetOnly { reverse_resolver };
    pipeline::lower_set_expr(set_expr, subject, &port, &required, &stats)
}

struct BoundedSetOnly<'a> {
    reverse_resolver: Option<&'a dyn ReverseResolver>,
}

impl ListObjectsPort for BoundedSetOnly<'_> {
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        Err(myelin_identity::AuthzError::Unavailable(
            "board_acl_filter lowers a known board `set_expr` directly - list_objects is not part of \
             the OLTP-board reference path"
                .into(),
        ))
    }

    fn resolve_relation(
        &self,
        subject: &Principal,
        form: &RelationalLeaf,
        required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        match self.reverse_resolver {
            Some(r) => r.resolve(subject, form, required),
            None => Err(myelin_identity::AuthzError::Unavailable(
                "the OLTP-board reference was handed a relational `SetExpr` leaf but no reverse-index \
                 resolver is wired (deny-when-unsure, ADR-03)"
                    .into(),
            )),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{ObjectId, PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn subject() -> Principal {
        Principal::stub(
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn zk(s: &str) -> Zookie {
        Zookie(s.into())
    }

    fn oid(s: &str) -> ObjectId {
        ObjectId(s.into())
    }

    #[test]
    fn over_budget_triggers_escalation() {
        let budget = OltpBudget::new(100);
        assert!(!budget.is_over_budget(100), "exactly at budget is NOT over");
        assert!(!budget.is_over_budget(50), "under budget stays on OLTP");
        assert!(
            budget.is_over_budget(101),
            "over budget escalates to Search"
        );
        assert_eq!(OltpBudget::default().max_rows, OltpBudget::DEFAULT_MAX_ROWS);
    }

    #[test]
    fn bounded_set_board_acl_admits_byte_identically() {
        let allow = SetExpr::Ids(vec![oid("A"), oid("C")]);
        let rows = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let visible = oltp_board_admits(&allow, &rows, &subject(), &zk("z@0"), None).unwrap();
        assert_eq!(visible, vec!["A".to_string(), "C".to_string()]);

        let acl = board_acl_filter(&allow, &subject(), &zk("z@0"), None).unwrap();
        assert_eq!(acl, AclFilter::Ids(vec!["A".into(), "C".into()]));
        assert!(acl.admits("A", "A") && !acl.admits("B", "B") && acl.admits("C", "C"));
    }

    #[test]
    fn difference_board_acl_is_left_and_not_right() {
        let set_expr = SetExpr::Difference(
            Box::new(SetExpr::Ids(vec![oid("A"), oid("B"), oid("C")])),
            Box::new(SetExpr::Ids(vec![oid("B")])),
        );
        let rows = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let visible = oltp_board_admits(&set_expr, &rows, &subject(), &zk("z@0"), None).unwrap();
        assert_eq!(
            visible,
            vec!["A".to_string(), "C".to_string()],
            "the confidential B is excluded by the set-difference (the `- confidential` shape)"
        );
        let acl = board_acl_filter(&set_expr, &subject(), &zk("z@0"), None).unwrap();
        assert!(acl.admits("A", "A") && !acl.admits("B", "B") && acl.admits("C", "C"));
    }

    #[test]
    fn relational_without_resolver_is_loud_not_widened() {
        let set_expr = SetExpr::InRelation {
            relation: myelin_identity::RelName("viewer".into()),
            via_column: myelin_identity::ColRef {
                table: "issue".into(),
                column: "id".into(),
            },
        };
        let err = board_acl_filter(&set_expr, &subject(), &zk("z@5"), None).unwrap_err();
        assert!(
            matches!(err, QueryError::Authz(_)),
            "a relational leaf with no reverse resolver is a loud Authz error, never a silent widen"
        );
    }

    #[test]
    fn relational_with_resolver_joins_and_honours_watermark() {
        struct Resolver {
            visible: Vec<String>,
            revision: u64,
        }
        impl ReverseResolver for Resolver {
            fn resolve(
                &self,
                _s: &Principal,
                _f: &RelationalLeaf,
                _required: &RevisionWatermark,
            ) -> AuthzResult<ReverseIndexAnswer> {
                Ok(ReverseIndexAnswer {
                    object_ids: self.visible.clone(),
                    revision: RevisionWatermark(self.revision),
                })
            }
        }
        let set_expr = SetExpr::TupleSet {
            index: myelin_identity::AuthzIndexRef("authz_visible".into()),
        };

        let fresh = Resolver {
            visible: vec!["A".into()],
            revision: 10,
        };
        let acl = board_acl_filter(&set_expr, &subject(), &zk("z@10"), Some(&fresh)).unwrap();
        assert_eq!(acl, AclFilter::Ids(vec!["A".into()]));

        let stale = Resolver {
            visible: vec!["A".into()],
            revision: 5,
        };
        let err = board_acl_filter(&set_expr, &subject(), &zk("z@10"), Some(&stale)).unwrap_err();
        assert!(
            matches!(
                err,
                QueryError::StaleReverseIndex {
                    required: 10,
                    served: 5,
                    ..
                }
            ),
            "a reverse-index revision below the watermark is refused (the new-enemy problem)"
        );
    }

}
