use myelin_identity::{Literal, ObjectType};
use myelin_query::{CmpOp, EventMatcher, Expr, FieldType, Predicate, QueryAst};
use myelin_search::{
    compile, render, CompileError, FieldDecl, FieldSchema, FtClause, Sort, StructuredClause,
    FT_BODY_FIELD, SEMANTIC_FIELD, SORT_FIELD,
};

fn var(name: &str) -> Expr {
    Expr::Var(name.into())
}
fn s(v: &str) -> Expr {
    Expr::Lit(Literal::Str(v.into()))
}
fn i(n: i64) -> Expr {
    Expr::Lit(Literal::Int(n))
}

fn schema() -> FieldSchema {
    FieldSchema::new()
        .with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
        .with("status", FieldDecl::stored(FieldType::Select))
        .with("severity", FieldDecl::stored(FieldType::Int))
        .with(
            myelin_search::ORDER_KEY_FIELD,
            FieldDecl::stored(FieldType::OrderKey),
        )
        .with("progress", FieldDecl::read_time(FieldType::Int))
}

#[test]
fn search_compiles_the_same_frozen_queryast_as_the_eventmatcher() {
    let predicate = QueryAst::compiled(Predicate::And(vec![
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(FT_BODY_FIELD),
            rhs: s("deadlock"),
        },
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("status"),
            rhs: s("open"),
        },
    ]))
    .expect("the frozen AST is within the cost bounds");

    let matcher = EventMatcher::new(ObjectType("issue".into()), predicate.clone());

    assert_eq!(
        serde_json::to_value(matcher.predicate()).unwrap(),
        serde_json::to_value(&predicate).unwrap(),
        "ONE QueryAst serialisation - Search and the EventMatcher core do not drift (X-3/13.3)"
    );

    let plan = compile(&predicate, &schema()).expect("Search compiles the frozen AST");
    assert_eq!(
        plan.ft,
        vec![FtClause {
            field: "text".into(),
            query: "deadlock".into()
        }]
    );
    assert_eq!(
        plan.structured,
        vec![StructuredClause::Cmp {
            field: "status".into(),
            ty: FieldType::Select,
            op: CmpOp::Eq,
            value: Literal::Str("open".into()),
        }]
    );
}

#[test]
fn field_type_taxonomy_is_byte_identical_frozen() {
    let wire_ids: Vec<&str> = FieldType::all().iter().map(|t| t.wire_id()).collect();
    assert_eq!(
        wire_ids,
        [
            "text",
            "int",
            "bool",
            "date",
            "select",
            "relation",
            "principal",
            "order_key"
        ],
        "the frozen FieldType taxonomy (byte-identical across Search / EventMatcher / Issues / KN)"
    );
    for (n, t) in FieldType::all().into_iter().enumerate() {
        assert_eq!(t as u8, n as u8, "{} discriminant pinned", t.wire_id());
    }
}

#[test]
fn bounded_cost_guard_rejects_crafted_ast() {
    let big: Vec<Predicate> = (0..(myelin_query::MAX_PREDICATE_NODES + 10))
        .map(|_| Predicate::True)
        .collect();
    assert!(
        QueryAst::compiled(Predicate::And(big)).is_err(),
        "an oversized AST is rejected by the cost guard before it can reach Search's compiler"
    );
}

#[test]
fn read_time_rollup_is_post_fetch_not_indexed() {
    let plan = compile(
        &QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Ge,
            lhs: var("progress"),
            rhs: i(80),
        })
        .unwrap(),
        &schema(),
    )
    .expect("compile");
    assert!(
        plan.structured.is_empty() && plan.ft.is_empty(),
        "no stored clause over a derived value"
    );
    assert_eq!(
        plan.post_fetch.len(),
        1,
        "the rollup is a post-fetch predicate"
    );
    assert_eq!(plan.post_fetch[0].field, "progress");
}

#[test]
fn compiled_plan_is_inert_until_the_acl_conjoin_seam() {
    let plan = compile(
        &QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("status"),
            rhs: s("open"),
        })
        .unwrap(),
        &schema(),
    )
    .expect("compile");
    let conjoined = plan.with_acl("acl_clause(list_objects(viewer, read, issue))");
    assert_eq!(
        conjoined.acl,
        "acl_clause(list_objects(viewer, read, issue))"
    );
}

#[test]
fn render_is_the_one_canonical_form() {
    let p = Predicate::And(vec![
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("status"),
            rhs: s("open"),
        },
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(SORT_FIELD),
            rhs: s(myelin_search::ORDER_KEY_FIELD),
        },
    ]);
    let ui = compile(&QueryAst::compiled(p.clone()).unwrap(), &schema()).expect("ui");
    let agent = compile(&QueryAst::compiled(p).unwrap(), &schema()).expect("agent");
    assert_eq!(
        ui, agent,
        "the identical AST compiles to the identical plan"
    );
    assert_eq!(
        render(&ui),
        render(&agent),
        "the ONE canonical rendered form"
    );
    assert_eq!(ui.sort, Some(Sort::OrderKeyAsc));
}

#[test]
fn semantic_request_lowers_to_the_vector_branch() {
    let plan = compile(
        &QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(SEMANTIC_FIELD),
            rhs: s("reset my password"),
        })
        .unwrap(),
        &schema(),
    )
    .expect("compile");
    assert_eq!(plan.vector.unwrap().query_text, "reset my password");
}

#[test]
fn undeclared_or_mismatched_is_a_loud_compile_error() {
    let undeclared = compile(
        &QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("ghost"),
            rhs: s("x"),
        })
        .unwrap(),
        &schema(),
    )
    .expect_err("undeclared");
    assert!(matches!(undeclared, CompileError::UndeclaredField { .. }));

    let mismatch = compile(
        &QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("severity"),
            rhs: s("hi"),
        })
        .unwrap(),
        &schema(),
    )
    .expect_err("mismatch");
    assert!(matches!(mismatch, CompileError::TypeMismatch { .. }));
}
