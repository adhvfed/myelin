//! # CDC — the Search compile-target side of contract 13.3 (SRCH-P07 → P-170)
//!
//! **Architecture:** `search-and-indexing.md` §4.6 (Search is **one compile target of the single
//! frozen `QueryAst`**, contract 13.3 — the SAME AST the bus's `EventMatcher` (3.4) and saved views
//! compile). Reconciliation `00-reconciliation-decisions.md` X-3/OQ-C (the `QueryAst`/`FieldType`/
//! `ViewSpec` frozen byte-identical) + KN-3 (rollup/formula read-time-computed, never stored).
//!
//! - **PROVIDER** = the frozen `myelin-query` primitive (contract 13.3, byte-identical X-3): the
//!   [`myelin_query::QueryAst`] / [`myelin_query::Predicate`] grammar + the
//!   [`myelin_query::FieldType`] enum. Search does **not** define a second query language; it
//!   CONSUMES this one frozen AST.
//! - **CONSUMER** = Search's query-AST [`myelin_search::compile`] (this prompt). It validates the
//!   frozen AST against the frozen `FieldType` schema + the bounded-cost guard, lowers it to the
//!   FT/structured/vector shapes + the read-time post-fetch path, and exposes the always-conjoin
//!   seam.
//!
//! The dated green artifact (2026-06-20): Search compiles the SAME frozen AST the EventMatcher core
//! consumes; the predicate bytes are byte-identical (no Search/matcher drift); a `FieldType`
//! rename/reorder in the contract home (`myelin-query`) breaks THIS test now (the byte-identical
//! drift anchor); the bounded-cost guard rejects a crafted AST (0 engine DoS); a read-time
//! rollup/formula field is post-fetch, never a stored clause (KN-3); and the compiled plan is inert
//! until the ACL conjoin seam (`with_acl`) produces an executable plan — the
//! `search-requires-acl-filter` ratchet is structural. If the 13.3 AST/`FieldType` shape drifts,
//! this stops compiling/passing — that is the contract.

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

/// The synthetic-producer facet schema (the structured facets a subsystem's `IndexSpec` declares,
/// 13.3) + the FT body + a read-time rollup. The real per-subsystem schemas arrive M3/M4.
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

/// **PROVIDER↔CONSUMER: Search compiles the SAME frozen `QueryAst` the EventMatcher core consumes,
/// byte-identically.** The predicate the bus matcher carries and the predicate Search lowers are the
/// SAME `myelin_query::QueryAst` with ONE serialisation — a grammar change breaks both at once.
#[test]
fn search_compiles_the_same_frozen_queryast_as_the_eventmatcher() {
    // ONE frozen predicate, consumed by BOTH the matcher (provider 3.4) and Search's compiler.
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

    // Byte-identical: the matcher's predicate bytes and the bytes Search lowers are equal (no drift).
    assert_eq!(
        serde_json::to_value(matcher.predicate()).unwrap(),
        serde_json::to_value(&predicate).unwrap(),
        "ONE QueryAst serialisation — Search and the EventMatcher core do not drift (X-3/13.3)"
    );

    // Search's compiler lowers it to the FT + structured shapes.
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

/// **The byte-identical `FieldType` drift anchor (X-3/OQ-C).** Search's compiler lowers over the
/// frozen `FieldType` taxonomy; pin the full taxonomy by value so a rename/reorder of a variant in
/// the contract home breaks THIS CDC now, not in prod.
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
    // The discriminants are pinned (the byte-identical wire encoding).
    for (n, t) in FieldType::all().into_iter().enumerate() {
        assert_eq!(t as u8, n as u8, "{} discriminant pinned", t.wire_id());
    }
}

/// **CONTRACT: the bounded-cost guard rejects a crafted AST (0 engine DoS).** An oversized AST never
/// even constructs (the frozen `QueryAst` rejects it); a within-bounds AST compiles.
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

/// **CONTRACT: a read-time rollup/formula field is post-fetch, never a stored engine clause (KN-3 /
/// X-3).** Search indexed only the inputs; the derived value is computed after fetch.
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

/// **CONTRACT: the always-conjoin seam — a compiled plan is inert until `with_acl` (the SRCH-P08
/// conjoin) produces an executable plan.** The `search-requires-acl-filter` ratchet is structural.
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
    // The only path to an executable plan demands the ACL clause (here a marker for the SRCH-P08
    // AclFilter lowering of list_objects).
    let conjoined = plan.with_acl("acl_clause(list_objects(viewer, read, issue))");
    assert_eq!(
        conjoined.acl,
        "acl_clause(list_objects(viewer, read, issue))"
    );
}

/// **CONTRACT: render(compile(ast)) is the canonical form (one renderer) — an agent and the UI emit
/// the SAME query, permission-filtered identically (no agent back-door).**
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

/// **CONTRACT: a semantic/near request lowers to the vector branch (the compiler's vector target).**
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

/// **CONTRACT: an undeclared field / type mismatch is a loud compile error (no silent coercion).**
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
