use myelin_chat::{AclConjoinedSearchFeeder, FT_BODY_FIELD};
use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectId, ObjectType, Permission,
    Principal, PrincipalId, PrincipalKind, Result as AuthzResult, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, Predicate, QueryAst};
use myelin_search::{
    FieldDecl, FieldSchema, IndexBackend, IndexDocument, ListObjectsPort, Page, QueryStats,
    ScopedEngine, TantivyBackend,
};
use myelin_tenancy::TenantId;
use std::cell::RefCell;
use std::collections::BTreeMap;

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p-opaque-bob".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn consistency() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn schema() -> FieldSchema {
    FieldSchema::new().with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
}

fn corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&BTreeMap::new()).expect("open");
    for (id, body) in [
        (
            "myelin://acme/chat/message/m-private-1",
            "incident bridge in #ops",
        ),
        (
            "myelin://acme/chat/message/m-private-2",
            "incident postmortem in #ops",
        ),
    ] {
        be.upsert(&IndexDocument::new(id, body)).unwrap();
    }
    be
}

struct LiveAuthz {
    allow: RefCell<ListObjectsResult>,
}
impl LiveAuthz {
    fn non_member() -> LiveAuthz {
        LiveAuthz {
            allow: RefCell::new(ListObjectsResult::Filter {
                set_expr: SetExpr::None,
                zookie: Zookie("z".into()),
            }),
        }
    }
    fn grant(&self, ids: &[&str]) {
        *self.allow.borrow_mut() = ListObjectsResult::Ids {
            ids: ids.iter().map(|i| ObjectId((*i).into())).collect(),
            zookie: Zookie("z".into()),
        };
    }
    fn revoke(&self) {
        *self.allow.borrow_mut() = ListObjectsResult::Filter {
            set_expr: SetExpr::None,
            zookie: Zookie("z".into()),
        };
    }
}
impl ListObjectsPort for LiveAuthz {
    fn list_objects(
        &self,
        _s: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        _a: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        assert_eq!(
            permission.0, "read",
            "the message search lists under the frozen Search `read` permission"
        );
        assert_eq!(
            ty.0, "message",
            "the conjoin keys on the `message` object (message.id)"
        );
        Ok(self.allow.borrow().clone())
    }
}

fn ast(term: &str) -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(FT_BODY_FIELD.into()),
        rhs: Expr::Lit(Literal::Str(term.into())),
    })
    .expect("within cost bounds")
}

#[test]
fn search_as_non_member_returns_zero_then_grant_then_revoke() {
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
    let authz = LiveAuthz::non_member();
    let feeder = AclConjoinedSearchFeeder::new(&eng, &authz);

    let r0 = feeder
        .search_messages(
            &ast("incident"),
            &viewer(),
            &consistency(),
            Page::FIRST,
            &QueryStats::new(),
        )
        .expect("reachable");
    assert_eq!(
        r0.hits.len(),
        0,
        "CHAT-D11: a non-member sees 0 message results from channels they're not in (incl. count)"
    );

    authz.grant(&[
        "myelin://acme/chat/message/m-private-1",
        "myelin://acme/chat/message/m-private-2",
    ]);
    let r1 = feeder
        .search_messages(
            &ast("incident"),
            &viewer(),
            &consistency(),
            Page::FIRST,
            &QueryStats::new(),
        )
        .expect("reachable");
    assert_eq!(
        r1.hits.len(),
        2,
        "after grant the member sees the channel's messages"
    );

    authz.revoke();
    let r2 = feeder
        .search_messages(
            &ast("incident"),
            &viewer(),
            &consistency(),
            Page::FIRST,
            &QueryStats::new(),
        )
        .expect("reachable");
    assert_eq!(
        r2.hits.len(),
        0,
        "CHAT-D11 chained: post-revoke the non-member sees 0 again (live membership, not a cache)"
    );
}

#[test]
fn partial_membership_sees_only_granted_messages() {
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
    let authz = LiveAuthz::non_member();
    authz.grant(&["myelin://acme/chat/message/m-private-1"]);
    let feeder = AclConjoinedSearchFeeder::new(&eng, &authz);
    let res = feeder
        .search_messages(
            &ast("incident"),
            &viewer(),
            &consistency(),
            Page::FIRST,
            &QueryStats::new(),
        )
        .expect("reachable");
    let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert_eq!(ids, ["myelin://acme/chat/message/m-private-1"]);
    assert_eq!(
        res.hits.len(),
        1,
        "the count reveals only the visible message"
    );
}
