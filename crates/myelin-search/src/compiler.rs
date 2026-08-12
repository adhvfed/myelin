use std::collections::BTreeMap;

use myelin_identity::Literal;
use myelin_query::{CmpOp, Expr, FieldType, Predicate, PredicateError, QueryAst};

pub const SEMANTIC_FIELD: &str = "__semantic__";

pub const SORT_FIELD: &str = "__sort__";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Stored,
    ReadTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldDecl {
    pub ty: FieldType,
    pub kind: FieldKind,
}

impl FieldDecl {
    pub fn stored(ty: FieldType) -> FieldDecl {
        FieldDecl {
            ty,
            kind: FieldKind::Stored,
        }
    }

    pub fn read_time(ty: FieldType) -> FieldDecl {
        FieldDecl {
            ty,
            kind: FieldKind::ReadTime,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldSchema {
    fields: BTreeMap<String, FieldDecl>,
}

impl FieldSchema {
    pub fn new() -> FieldSchema {
        FieldSchema {
            fields: BTreeMap::new(),
        }
    }

    pub fn with(mut self, name: impl Into<String>, decl: FieldDecl) -> FieldSchema {
        self.fields.insert(name.into(), decl);
        self
    }

    pub fn get(&self, name: &str) -> Option<FieldDecl> {
        self.fields.get(name).copied()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FtClause {
    pub field: String,
    pub query: String,
}

pub const FT_BODY_FIELD: &str = "text";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StructuredClause {
    Cmp {
        field: String,
        ty: FieldType,
        op: CmpOp,
        value: Literal,
    },
    In {
        field: String,
        ty: FieldType,
        values: Vec<Literal>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorBranch {
    pub query_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostFetchPredicate {
    pub field: String,
    pub predicate: Predicate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    OrderKeyAsc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledPlan {
    pub ft: Vec<FtClause>,
    pub structured: Vec<StructuredClause>,
    pub vector: Option<VectorBranch>,
    pub post_fetch: Vec<PostFetchPredicate>,
    pub sort: Option<Sort>,
}

impl CompiledPlan {
    pub fn with_acl<A>(self, acl: A) -> ConjoinedPlan<A> {
        ConjoinedPlan { plan: self, acl }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConjoinedPlan<A> {
    pub plan: CompiledPlan,
    pub acl: A,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileError {
    CostBound(PredicateError),
    UndeclaredField {
        field: String,
    },
    TypeMismatch {
        field: String,
        declared: FieldType,
        got: &'static str,
    },
    NotOrderable {
        field: String,
        ty: FieldType,
    },
    NotCompiled,
    UnsupportedShape {
        reason: &'static str,
    },
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::CostBound(e) => write!(f, "query rejected by the cost guard: {e}"),
            CompileError::UndeclaredField { field } => {
                write!(f, "query references undeclared field `{field}`")
            }
            CompileError::TypeMismatch {
                field,
                declared,
                got,
            } => write!(
                f,
                "field `{field}` is {} but the query value is {got}",
                declared.wire_id()
            ),
            CompileError::NotOrderable { field, ty } => write!(
                f,
                "range comparison over non-ordered field `{field}` ({})",
                ty.wire_id()
            ),
            CompileError::NotCompiled => {
                write!(
                    f,
                    "the QueryAst is the un-parsed placeholder surface (nothing to lower)"
                )
            }
            CompileError::UnsupportedShape { reason } => {
                write!(f, "unsupported query shape: {reason}")
            }
        }
    }
}

impl std::error::Error for CompileError {}

pub fn compile(ast: &QueryAst, schema: &FieldSchema) -> Result<CompiledPlan, CompileError> {
    let predicate = ast.predicate().ok_or(CompileError::NotCompiled)?;
    QueryAst::validate(predicate).map_err(CompileError::CostBound)?;

    let mut plan = CompiledPlan {
        ft: Vec::new(),
        structured: Vec::new(),
        vector: None,
        post_fetch: Vec::new(),
        sort: None,
    };
    lower(predicate, schema, &mut plan)?;
    Ok(plan)
}

fn lower(
    predicate: &Predicate,
    schema: &FieldSchema,
    plan: &mut CompiledPlan,
) -> Result<(), CompileError> {
    match predicate {
        Predicate::True => Ok(()),
        Predicate::False => Err(CompileError::UnsupportedShape {
            reason: "a bare `False` predicate has no engine clause (the pipeline maps it to None)",
        }),
        Predicate::And(ps) => {
            for p in ps {
                lower(p, schema, plan)?;
            }
            Ok(())
        }
        Predicate::Or(ps) => lower_or(ps, schema, plan),
        Predicate::Not(_) => Err(CompileError::UnsupportedShape {
            reason: "negation is not a lowerable clause at M2 (the bus matcher uses Not; the \
                     structured/FT engine shapes are positive clauses - a later prompt lowers \
                     NotIds via the SetExpr ACL path)",
        }),
        Predicate::Cmp { op, lhs, rhs } => lower_cmp(*op, lhs, rhs, schema, plan),
    }
}

fn lower_or(
    disjuncts: &[Predicate],
    schema: &FieldSchema,
    plan: &mut CompiledPlan,
) -> Result<(), CompileError> {
    let mut field: Option<String> = None;
    let mut values: Vec<Literal> = Vec::new();
    let mut is_in_shape = !disjuncts.is_empty();
    for d in disjuncts {
        match d {
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs,
                rhs,
            } => match field_and_value(lhs, rhs) {
                Some((f, v)) => {
                    if let Some(prev) = &field {
                        if prev != f {
                            is_in_shape = false;
                            break;
                        }
                    } else {
                        field = Some(f.to_string());
                    }
                    values.push(v.clone());
                }
                None => {
                    is_in_shape = false;
                    break;
                }
            },
            _ => {
                is_in_shape = false;
                break;
            }
        }
    }

    if is_in_shape {
        let field = field.expect("an In-shaped Or has at least one equality (non-empty checked)");
        let decl = schema
            .get(&field)
            .ok_or_else(|| CompileError::UndeclaredField {
                field: field.clone(),
            })?;
        if decl.kind == FieldKind::ReadTime {
            return Err(CompileError::UnsupportedShape {
                reason: "an `In` over a read-time rollup/formula field is evaluated post-fetch \
                         (lower each equality individually as a post-fetch predicate)",
            });
        }
        for v in &values {
            check_value_type(&field, decl.ty, v)?;
        }
        plan.structured.push(StructuredClause::In {
            field,
            ty: decl.ty,
            values,
        });
        return Ok(());
    }

    Err(CompileError::UnsupportedShape {
        reason: "a heterogeneous OR (not a single-field `In`) is composed by the SRCH-P08/P09 \
                 boolean/SetExpr path, not lowered to a conjoined engine clause here",
    })
}

fn lower_cmp(
    op: CmpOp,
    lhs: &Expr,
    rhs: &Expr,
    schema: &FieldSchema,
    plan: &mut CompiledPlan,
) -> Result<(), CompileError> {
    let (field, value) = field_and_value(lhs, rhs).ok_or(CompileError::UnsupportedShape {
        reason: "a query comparison must be `field <op> literal` (a var vs a literal)",
    })?;

    if field == SEMANTIC_FIELD {
        let Literal::Str(query_text) = value else {
            return Err(CompileError::UnsupportedShape {
                reason: "a semantic/near request must carry a string query (the text to embed)",
            });
        };
        plan.vector = Some(VectorBranch {
            query_text: query_text.clone(),
        });
        return Ok(());
    }

    if field == SORT_FIELD {
        if value == &Literal::Str(crate::ORDER_KEY_FIELD.to_string()) {
            plan.sort = Some(Sort::OrderKeyAsc);
            return Ok(());
        }
        return Err(CompileError::UnsupportedShape {
            reason: "the only sort at M2 is the order_key columnar fast-field sort",
        });
    }

    let decl = schema
        .get(field)
        .ok_or_else(|| CompileError::UndeclaredField {
            field: field.to_string(),
        })?;
    check_value_type(field, decl.ty, value)?;

    if decl.kind == FieldKind::ReadTime {
        plan.post_fetch.push(PostFetchPredicate {
            field: field.to_string(),
            predicate: Predicate::Cmp {
                op,
                lhs: lhs.clone(),
                rhs: rhs.clone(),
            },
        });
        return Ok(());
    }

    let is_range = matches!(op, CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge);
    if is_range && !decl.ty.is_ordered() {
        return Err(CompileError::NotOrderable {
            field: field.to_string(),
            ty: decl.ty,
        });
    }

    if decl.ty == FieldType::Text && op == CmpOp::Eq {
        let Literal::Str(query) = value else {
            return Err(CompileError::TypeMismatch {
                field: field.to_string(),
                declared: FieldType::Text,
                got: literal_kind(value),
            });
        };
        plan.ft.push(FtClause {
            field: field.to_string(),
            query: query.clone(),
        });
        return Ok(());
    }

    plan.structured.push(StructuredClause::Cmp {
        field: field.to_string(),
        ty: decl.ty,
        op,
        value: value.clone(),
    });
    Ok(())
}

fn field_and_value<'a>(lhs: &'a Expr, rhs: &'a Expr) -> Option<(&'a str, &'a Literal)> {
    match (lhs, rhs) {
        (Expr::Var(f), Expr::Lit(v)) => Some((f.as_str(), v)),
        (Expr::Lit(v), Expr::Var(f)) => Some((f.as_str(), v)),
        _ => None,
    }
}

fn check_value_type(field: &str, ty: FieldType, value: &Literal) -> Result<(), CompileError> {
    let ok = match ty {
        FieldType::Int => matches!(value, Literal::Int(_)),
        FieldType::Bool => matches!(value, Literal::Bool(_)),
        FieldType::Text
        | FieldType::Date
        | FieldType::Select
        | FieldType::Relation
        | FieldType::Principal
        | FieldType::OrderKey => matches!(value, Literal::Str(_)),
    };
    if ok {
        Ok(())
    } else {
        Err(CompileError::TypeMismatch {
            field: field.to_string(),
            declared: ty,
            got: literal_kind(value),
        })
    }
}

fn literal_kind(value: &Literal) -> &'static str {
    match value {
        Literal::Bool(_) => "bool",
        Literal::Int(_) => "int",
        Literal::Str(_) => "string",
    }
}

pub fn render(plan: &CompiledPlan) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in &plan.ft {
        parts.push(format!("text({}) ~ {:?}", c.field, c.query));
    }
    for c in &plan.structured {
        match c {
            StructuredClause::Cmp {
                field,
                ty,
                op,
                value,
            } => {
                parts.push(format!(
                    "{field}:{} {} {}",
                    ty.wire_id(),
                    render_op(*op),
                    render_lit(value)
                ));
            }
            StructuredClause::In { field, ty, values } => {
                let vs: Vec<String> = values.iter().map(render_lit).collect();
                parts.push(format!("{field}:{} in [{}]", ty.wire_id(), vs.join(", ")));
            }
        }
    }
    if let Some(v) = &plan.vector {
        parts.push(format!("semantic ~ {:?}", v.query_text));
    }
    for p in &plan.post_fetch {
        parts.push(format!("post_fetch({})", p.field));
    }
    let mut out = parts.join(" AND ");
    if out.is_empty() {
        out.push('*');
    }
    if matches!(plan.sort, Some(Sort::OrderKeyAsc)) {
        out.push_str(" ORDER BY order_key ASC");
    }
    out
}

fn render_op(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "==",
        CmpOp::Ne => "!=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    }
}

fn render_lit(value: &Literal) -> String {
    match value {
        Literal::Bool(b) => b.to_string(),
        Literal::Int(n) => n.to_string(),
        Literal::Str(s) => format!("{s:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_query::OrderKey;

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
            .with("done", FieldDecl::stored(FieldType::Bool))
            .with("assignee", FieldDecl::stored(FieldType::Principal))
            .with("parent", FieldDecl::stored(FieldType::Relation))
            .with("due", FieldDecl::stored(FieldType::Date))
            .with(
                crate::ORDER_KEY_FIELD,
                FieldDecl::stored(FieldType::OrderKey),
            )
            .with("progress", FieldDecl::read_time(FieldType::Int))
    }

    fn ast(p: Predicate) -> QueryAst {
        QueryAst::compiled(p).expect("the test predicate is within the cost bounds")
    }

    #[test]
    fn text_lowers_to_ft_clause() {
        let plan = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &schema(),
        )
        .expect("compile");
        assert_eq!(
            plan.ft,
            vec![FtClause {
                field: "text".into(),
                query: "deadlock".into()
            }]
        );
        assert!(plan.structured.is_empty() && plan.vector.is_none());
    }

    #[test]
    fn eq_over_typed_facet_lowers_to_structured() {
        let plan = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("status"),
                rhs: s("open"),
            }),
            &schema(),
        )
        .expect("compile");
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
    fn range_over_ordered_ok_over_unordered_rejected() {
        let ok = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Ge,
                lhs: var("severity"),
                rhs: i(3),
            }),
            &schema(),
        )
        .expect("compile");
        assert_eq!(
            ok.structured,
            vec![StructuredClause::Cmp {
                field: "severity".into(),
                ty: FieldType::Int,
                op: CmpOp::Ge,
                value: Literal::Int(3),
            }]
        );
        let err = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Lt,
                lhs: var("status"),
                rhs: s("z"),
            }),
            &schema(),
        )
        .expect_err("a range over a non-ordered facet is rejected");
        assert!(matches!(err, CompileError::NotOrderable { .. }));
    }

    #[test]
    fn has_ref_lower_as_eq_over_relation_principal() {
        let plan = compile(
            &ast(Predicate::And(vec![
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var("assignee"),
                    rhs: s("p:alice"),
                },
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var("parent"),
                    rhs: s("myelin://t/x/issue/1"),
                },
            ])),
            &schema(),
        )
        .expect("compile");
        let kinds: Vec<FieldType> = plan
            .structured
            .iter()
            .map(|c| match c {
                StructuredClause::Cmp { ty, .. } => *ty,
                StructuredClause::In { ty, .. } => *ty,
            })
            .collect();
        assert!(kinds.contains(&FieldType::Principal) && kinds.contains(&FieldType::Relation));
    }

    #[test]
    fn single_field_or_lowers_to_in() {
        let plan = compile(
            &ast(Predicate::Or(vec![
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var("status"),
                    rhs: s("open"),
                },
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var("status"),
                    rhs: s("in_review"),
                },
            ])),
            &schema(),
        )
        .expect("compile");
        assert_eq!(
            plan.structured,
            vec![StructuredClause::In {
                field: "status".into(),
                ty: FieldType::Select,
                values: vec![
                    Literal::Str("open".into()),
                    Literal::Str("in_review".into())
                ],
            }]
        );
    }

    #[test]
    fn semantic_request_lowers_to_vector_branch() {
        let plan = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(SEMANTIC_FIELD),
                rhs: s("how do I reset my password"),
            }),
            &schema(),
        )
        .expect("compile");
        assert_eq!(
            plan.vector,
            Some(VectorBranch {
                query_text: "how do I reset my password".into()
            })
        );
    }

    #[test]
    fn hybrid_query_lowers_all_branches() {
        let plan = compile(
            &ast(Predicate::And(vec![
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var(FT_BODY_FIELD),
                    rhs: s("login"),
                },
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var(SEMANTIC_FIELD),
                    rhs: s("auth flow"),
                },
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var("status"),
                    rhs: s("open"),
                },
            ])),
            &schema(),
        )
        .expect("compile");
        assert_eq!(plan.ft.len(), 1, "FT branch");
        assert!(plan.vector.is_some(), "vector branch");
        assert_eq!(plan.structured.len(), 1, "structured branch");
    }

    #[test]
    fn read_time_field_is_post_fetch_never_a_stored_clause() {
        let plan = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Ge,
                lhs: var("progress"),
                rhs: i(80),
            }),
            &schema(),
        )
        .expect("compile");
        assert!(
            plan.structured.is_empty(),
            "the derived value is NOT a stored structured clause"
        );
        assert!(plan.ft.is_empty() && plan.vector.is_none());
        assert_eq!(plan.post_fetch.len(), 1);
        assert_eq!(plan.post_fetch[0].field, "progress");
        assert_eq!(
            plan.post_fetch[0].predicate,
            Predicate::Cmp { op: CmpOp::Ge, lhs: var("progress"), rhs: i(80) },
            "the post-fetch predicate is the SAME frozen Predicate (the one interpreter re-evaluates)"
        );

        let stored = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Ge,
                lhs: var("severity"),
                rhs: i(80),
            }),
            &schema(),
        )
        .expect("compile");
        assert_eq!(
            stored.structured.len(),
            1,
            "a STORED int facet lowers to a structured clause"
        );
        assert!(stored.post_fetch.is_empty());
    }

    #[test]
    fn order_key_lowers_to_columnar_sort() {
        let plan = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(SORT_FIELD),
                rhs: s(crate::ORDER_KEY_FIELD),
            }),
            &schema(),
        )
        .expect("compile");
        assert_eq!(plan.sort, Some(Sort::OrderKeyAsc));
    }

    #[test]
    fn oversized_ast_rejected_by_cost_guard() {
        let big: Vec<Predicate> = (0..(myelin_query::MAX_PREDICATE_NODES + 10))
            .map(|_| Predicate::True)
            .collect();
        assert!(QueryAst::compiled(Predicate::And(big)).is_err());

        let deep = {
            let mut p = Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("status"),
                rhs: s("open"),
            };
            for _ in 0..(myelin_query::MAX_PREDICATE_DEPTH - 2) {
                p = Predicate::And(vec![p]);
            }
            p
        };
        assert!(compile(&ast(deep), &schema()).is_ok());
    }

    #[test]
    fn undeclared_field_and_type_mismatch_are_loud() {
        let undeclared = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("nope"),
                rhs: s("x"),
            }),
            &schema(),
        )
        .expect_err("an undeclared field is rejected");
        assert!(matches!(undeclared, CompileError::UndeclaredField { .. }));

        let mismatch = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("severity"),
                rhs: s("not-an-int"),
            }),
            &schema(),
        )
        .expect_err("a string over an int facet is rejected");
        assert!(matches!(mismatch, CompileError::TypeMismatch { .. }));
    }

    #[test]
    fn unparsed_placeholder_fails_closed() {
        let err = compile(&QueryAst::raw("status == 'open'"), &schema())
            .expect_err("an un-parsed AST is not lowerable");
        assert!(matches!(err, CompileError::NotCompiled));
    }

    #[test]
    fn conjoin_seam_is_the_only_path_to_an_executable_plan() {
        let plan = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("status"),
                rhs: s("open"),
            }),
            &schema(),
        )
        .expect("compile");
        assert_eq!(plan.structured.len(), 1);
        let conjoined = plan.with_acl("acl_clause(list_objects(viewer, read, issue))");
        assert_eq!(
            conjoined.acl,
            "acl_clause(list_objects(viewer, read, issue))"
        );
        assert_eq!(conjoined.plan.structured.len(), 1);
    }

    #[test]
    fn render_round_trip_and_no_agent_back_door() {
        let p = Predicate::And(vec![
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
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(SORT_FIELD),
                rhs: s(crate::ORDER_KEY_FIELD),
            },
        ]);
        let ui_ast = ast(p.clone());
        let agent_ast = ast(p);
        let ui_plan = compile(&ui_ast, &schema()).expect("ui compile");
        let agent_plan = compile(&agent_ast, &schema()).expect("agent compile");
        assert_eq!(
            ui_plan, agent_plan,
            "agent and UI compile the identical AST to the identical plan"
        );
        assert_eq!(render(&ui_plan), render(&agent_plan));
        assert_eq!(
            render(&ui_plan),
            "text(text) ~ \"deadlock\" AND status:select == \"open\" ORDER BY order_key ASC",
            "the canonical human-readable form of the lowered plan"
        );
    }

    #[test]
    fn byte_identical_semantics_with_the_eventmatcher_core() {
        use myelin_identity::ObjectType;
        use myelin_query::EventMatcher;

        let predicate = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("status"),
            rhs: s("open"),
        })
        .expect("compile");

        let matcher = EventMatcher::new(ObjectType("issue".into()), predicate.clone());

        let search_bytes = serde_json::to_value(&predicate).unwrap();
        let matcher_bytes = serde_json::to_value(matcher.predicate()).unwrap();
        assert_eq!(
            search_bytes, matcher_bytes,
            "ONE QueryAst serialisation - no Search/matcher drift"
        );

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
            "the frozen FieldType taxonomy Search's compiler lowers over (byte-identical to the \
             EventMatcher core / Issues / Knowledge)"
        );

        let plan = compile(&predicate, &schema()).expect("compile");
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
    fn order_key_sort_targets_the_frozen_columnar_field() {
        assert_eq!(crate::ORDER_KEY_FIELD, "order_key");
        let a = OrderKey::parse("G").unwrap();
        let b = OrderKey::parse("V").unwrap();
        assert!(
            a < b,
            "the LexoRank byte order is the sort order the columnar fast-field uses"
        );
    }

    #[test]
    fn reversed_operand_order_lowers_the_same() {
        let plan = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: s("open"),
                rhs: var("status"),
            }),
            &schema(),
        )
        .expect("compile");
        assert_eq!(
            plan.structured,
            vec![StructuredClause::Cmp {
                field: "status".into(),
                ty: FieldType::Select,
                op: CmpOp::Eq,
                value: Literal::Str("open".into()),
            }],
            "the field var resolves regardless of operand side"
        );
        let err = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: s("a"),
                rhs: s("b"),
            }),
            &schema(),
        )
        .expect_err("a literal-vs-literal comparison has no field to lower over");
        assert!(matches!(err, CompileError::UnsupportedShape { .. }));
    }

    #[test]
    fn compile_error_messages_are_exact() {
        let err = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("severity"),
                rhs: s("nope"),
            }),
            &schema(),
        )
        .expect_err("string over int");
        let msg = err.to_string();
        assert!(msg.contains("severity"), "names the field: {msg}");
        assert!(
            msg.contains("int"),
            "names the declared frozen FieldType: {msg}"
        );
        assert!(
            msg.contains("string"),
            "names the offending literal kind: {msg}"
        );

        let undeclared = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("ghost"),
                rhs: s("x"),
            }),
            &schema(),
        )
        .expect_err("undeclared");
        assert!(
            undeclared.to_string().contains("ghost"),
            "the undeclared-field error names the field"
        );

        let not_orderable = compile(
            &ast(Predicate::Cmp {
                op: CmpOp::Lt,
                lhs: var("status"),
                rhs: s("z"),
            }),
            &schema(),
        )
        .expect_err("range over select");
        assert!(
            not_orderable.to_string().contains("status"),
            "not-orderable names the field"
        );
        assert!(
            compile(&QueryAst::raw("x"), &schema())
                .expect_err("unparsed")
                .to_string()
                .contains("placeholder"),
            "the not-compiled error explains the placeholder surface"
        );
    }

    #[test]
    fn negation_is_surfaced_not_silently_dropped() {
        let err = compile(
            &ast(Predicate::Not(Box::new(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("status"),
                rhs: s("closed"),
            }))),
            &schema(),
        )
        .expect_err("negation is not a lowerable engine clause at M2");
        assert!(matches!(err, CompileError::UnsupportedShape { .. }));
    }
}
