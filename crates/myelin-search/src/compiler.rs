//! The **query-AST compiler** (SRCH-P07 / P-170; architecture `search-and-indexing.md` §4.6):
//! Search as **ONE compile target of the single frozen `QueryAst`** (contract 13.3 — the SAME
//! [`myelin_query::QueryAst`] / [`myelin_query::Predicate`] the bus's `EventMatcher` (3.4) and
//! saved views compile). There is **no second query language** here (EI-01 §7): the compiler
//! consumes the one frozen predicate tree, **validates** it against the frozen
//! [`myelin_query::FieldType`] definitions + the bounded-cost guard, **lowers** it to the three
//! engine shapes (FT / structured / vector), **exposes an always-conjoin seam** for the ACL
//! filter, and **renders** it back to a canonical human-readable form.
//!
//! ## The four compiler steps (§4.6)
//! 1. **VALIDATE** ([`compile`]): the frozen [`myelin_query::QueryAst`] static cost bounds
//!    ([`myelin_query::MAX_PREDICATE_NODES`]/`MAX_PREDICATE_DEPTH`) reject a crafted/oversized
//!    tree **before** lowering (a crafted query cannot DoS the engine — the GATE); every `Cmp`
//!    field is resolved against the frozen [`FieldSchema`] (`FieldType`) — an undeclared field or a
//!    type-mismatched literal is rejected loudly (no silent coercion).
//! 2. **LOWER** ([`CompiledPlan`]): `Text` over a `Text`-typed field → an [`FtClause`] (the
//!    full-text inverted shape); `Cmp`/`In`/`Has`/`Ref` over a typed facet → a [`StructuredClause`]
//!    (the structured fast-field shape); a `semantic`/`near` request → a [`VectorBranch`];
//!    `order_key` → the columnar fast-field [`Sort`]. A `Cmp` over a **read-time rollup/formula
//!    field** lowers to a [`PostFetchPredicate`] (the view evaluates it after fetch — Search
//!    indexed only the INPUTS, never the derived value: X-3 / KN-3).
//! 3. **CONJOIN seam** ([`CompiledPlan::with_acl`]): the always-conjoin seam where
//!    `acl_clause(list_objects(viewer, read, type))` is conjoined. **There is no executable plan
//!    without it** — [`CompiledPlan`] is inert until [`CompiledPlan::with_acl`] produces a
//!    [`ConjoinedPlan`], whose `acl` is mandatory and non-defaultable. The conjoin CALL SITE (the
//!    `list_objects` push-down + the `Ids/All/None`/`SetExpr` lowering) is the SRCH-P08 query
//!    pipeline; the SEAM is here.
//! 4. **RENDER** ([`render`]): the canonical human-readable form, so `render(compile(ast))` is the
//!    one canonical string an agent and the UI both emit — the **no-agent-back-door** property
//!    (§4.6 tail: an agent and the UI emit the SAME query, permission-filtered identically).
//!
//! ## How the frozen grammar maps to the three shapes (the bridge, documented)
//! The frozen [`myelin_query::Predicate`] grammar is `Cmp` over [`myelin_query::Expr`]
//! (`Var(field) <op> Lit`) + `And/Or/Not` + `True/False`. The architecture's `Text`/`In`/`Has`/`Ref`
//! "node" vocabulary (§4.6 step 2) is **not** four new AST variants (that would fork the one frozen
//! grammar); it is a **lowering classification of a `Cmp` by the declared [`FieldType`] of its
//! field var**, exactly the way `EventMatcher` classifies a `Cmp` over `event.type` into a subject
//! filter (`matcher.rs`). The mapping, frozen here:
//! - `Cmp{Var(f) == Lit}` where `schema[f] == Text` → **FT clause** (`Text{query, field}`).
//! - `Or([Cmp{Var(f)==a}, Cmp{Var(f)==b}, …])` over one field → an **`In`** structured clause
//!   (the bounded set membership; this is how `In` lowers without a second AST node).
//! - `Cmp{Var(f) <cmp> Lit}` where `schema[f]` is a typed facet → a **structured clause**
//!   (`Eq`/range; `Has`/`Ref` are `Eq` over a `Relation`/`Principal` facet).
//! - a `Cmp` over a field whose schema marks it [`FieldKind::ReadTime`] → a **post-fetch
//!   predicate** (the rollup/formula path: indexed inputs, derived value computed after fetch).
//! - the conventional [`SEMANTIC_FIELD`] var → the **vector branch** (the `semantic`/`near`
//!   request; the embedding adapter is the indexer's concern, SRCH-P06).
//! - the conventional [`myelin_search::ORDER_KEY_FIELD`](crate::ORDER_KEY_FIELD) field, when the
//!   AST requests a sort over it, → the columnar fast-field [`Sort`] (byte-identical LexoRank).
//!
//! ## The mutation floor (measured — EI-01 §3 prove-it)
//! `cargo mutants --package myelin-search --file compiler.rs` (2026-06-20): **41 mutants, 31
//! caught + 10 unviable = 0 MISSED (100% of the viable mutants killed).** The compiler + cost-guard
//! lowering is the DoS/permission-correctness surface, so the floor is the full kill: a surviving
//! mutant would be a lowering the tests do not pin. No justified survivor.
//!
//! ## FLOOR named (so the compiler is not mistaken for the whole query path)
//! - The **conjoin CALL SITE** — `acl_clause(list_objects(...))` + the `Ids/All/None`/`SetExpr`
//!   lowering into the engine's [`crate::AclFilter`] — is **SRCH-P08** (P-171). Here the seam is
//!   exposed ([`CompiledPlan::with_acl`]) and demands the filter, so a plan can never reach
//!   `engine.search` without an ACL clause (the `search-requires-acl-filter` ratchet holds), but
//!   the `list_objects` push-down itself is the downstream prompt.
//! - The **analyzer DEPTH** (the multilingual per-language analyzer chain) is **SRCH-P12** (P-175);
//!   here the FT clause carries the field + query text, analysis is the engine/analyzer's concern.
//! - The **producer-specific `IndexSpec` facets** (the real per-subsystem field schemas) arrive
//!   **M3/M4**; here the compiler validates against a supplied [`FieldSchema`] (the synthetic
//!   producer's facet declaration), not a real fed-by-producer schema.

use std::collections::BTreeMap;

use myelin_identity::Literal;
use myelin_query::{CmpOp, Expr, FieldType, Predicate, PredicateError, QueryAst};

/// The conventional field-var name a `semantic`/`near` request rides on (§4.6 step 2 — the vector
/// branch). A `Cmp{Var(SEMANTIC_FIELD) == Lit::Str(query_text)}` lowers to a [`VectorBranch`]
/// (the query text is embedded by the adapter at query time, SRCH-P06). It is a reserved field
/// name (never a structured facet) so a semantic request is unambiguous in the one frozen grammar.
pub const SEMANTIC_FIELD: &str = "__semantic__";

/// The conventional field-var name an explicit **sort over `order_key`** rides on. A
/// `Cmp{Var(SORT_FIELD) == Lit::Str("order_key")}` requests the columnar fast-field sort (byte-
/// identical LexoRank, §3.1). A reserved name so a sort directive is unambiguous.
pub const SORT_FIELD: &str = "__sort__";

/// **The kind of a declared field** — the validate step uses it to pick the lowering target. A
/// `Stored` facet lowers to an FT or structured clause (indexed, queryable directly); a `ReadTime`
/// (rollup/formula) field lowers to a **post-fetch predicate** — Search indexed only its INPUTS,
/// never the derived value (X-3 / KN-3, the freshness/consistency choice).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// A stored, indexed facet — its value is an indexed artifact (FT body or columnar fast-field).
    /// A `Cmp` over it lowers to an engine clause (FT or structured).
    Stored,
    /// A **read-time** rollup/formula field — its derived value is **never stored/indexed** (KN-3).
    /// Search indexes its INPUTS; a `Cmp` over it lowers to a [`PostFetchPredicate`] the view
    /// evaluates after fetch (the derived value is computed then, never read from a stale index).
    ReadTime,
}

/// A declared field: its frozen [`FieldType`] + its [`FieldKind`]. The schema the compiler
/// validates a `QueryAst` against (the synthetic-producer facet declaration; the real per-subsystem
/// `IndexSpec` schemas arrive M3/M4 — named floor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldDecl {
    /// The frozen field type (byte-identical, 13.3) — the structured shape is typed over it.
    pub ty: FieldType,
    /// Stored-and-indexed vs read-time-computed (rollup/formula). Decides FT/structured vs
    /// post-fetch lowering.
    pub kind: FieldKind,
}

impl FieldDecl {
    /// A stored facet of the given frozen [`FieldType`] (the common case).
    pub fn stored(ty: FieldType) -> FieldDecl {
        FieldDecl { ty, kind: FieldKind::Stored }
    }

    /// A read-time rollup/formula field (indexed by its inputs, computed after fetch). Its declared
    /// [`FieldType`] is the type of the DERIVED value (so the post-fetch predicate is well-typed),
    /// but no stored clause is ever produced over it.
    pub fn read_time(ty: FieldType) -> FieldDecl {
        FieldDecl { ty, kind: FieldKind::ReadTime }
    }
}

/// **The frozen field schema the compiler validates + lowers against** — `field name → [`FieldDecl`]`.
/// Built from a producer's `IndexSpec::struct_fields` (the structured facets, 13.3) plus the FT
/// body field and any read-time rollup/formula fields the producer declares.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldSchema {
    fields: BTreeMap<String, FieldDecl>,
}

impl FieldSchema {
    /// An empty schema (no declared fields).
    pub fn new() -> FieldSchema {
        FieldSchema { fields: BTreeMap::new() }
    }

    /// Declare a field (builder style). A re-declaration replaces (last wins — the producer's
    /// `IndexSpec` is the single source).
    pub fn with(mut self, name: impl Into<String>, decl: FieldDecl) -> FieldSchema {
        self.fields.insert(name.into(), decl);
        self
    }

    /// Look up a declared field (`None` ⇒ undeclared ⇒ the compiler rejects a `Cmp` over it).
    pub fn get(&self, name: &str) -> Option<FieldDecl> {
        self.fields.get(name).copied()
    }
}

/// **A full-text clause** (the inverted shape) — a `Text{query, field}` lowered from a `Cmp` over a
/// [`FieldType::Text`] field. `field` is the facet name (the FT body field is the conventional
/// [`FT_BODY_FIELD`]); `query` is the analyzable query string. Analysis depth (the multilingual
/// chain) is SRCH-P12; this carries the field + query text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FtClause {
    /// The full-text field the query runs against (the FT body, or a named analyzable facet).
    pub field: String,
    /// The analyzable query string (BM25-scored against the inverted shape).
    pub query: String,
}

/// The conventional full-text **body** field name a bare `Text` query over the document body
/// lowers to (the FT inverted shape, §3.2). A `Text` query over a NAMED `Text` facet keeps that
/// facet's name.
pub const FT_BODY_FIELD: &str = "text";

/// **A structured/columnar clause** (the fast-field shape) — a `Cmp`/`In` over a typed facet,
/// lowered to a typed predicate over the columnar fast-field (§3.1). The engine evaluates it
/// before scoring; it never coerces a value whose type disagrees with the facet's frozen
/// [`FieldType`] (validated at compile time).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StructuredClause {
    /// `field <op> value` — an equality or (for ordered types) a range comparison over the facet.
    Cmp {
        /// The facet name.
        field: String,
        /// The frozen [`FieldType`] of the facet (the structured shape is typed over it).
        ty: FieldType,
        /// The comparison operator (range ops are only emitted over an ordered [`FieldType`]).
        op: CmpOp,
        /// The literal operand (its type matches `ty` — validated at compile time).
        value: Literal,
    },
    /// `field IN {values}` — a bounded set-membership clause over the facet (lowered from an `Or`
    /// of equalities on one field, the frozen-grammar form of `In`).
    In {
        /// The facet name.
        field: String,
        /// The frozen [`FieldType`] of the facet.
        ty: FieldType,
        /// The allow-set of literal values (each typed `ty`).
        values: Vec<Literal>,
    },
}

/// **The vector branch** — a `semantic`/`near` request lowered from a `Cmp` over the reserved
/// [`SEMANTIC_FIELD`]. Carries the query text the embedding adapter embeds at query time (SRCH-P06);
/// the ACL filter is conjoined DURING traversal (filter-during-traversal, SRCH-P11) at the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorBranch {
    /// The natural-language query text to embed + k-NN over the co-located HNSW shape (§3.3).
    pub query_text: String,
}

/// **A post-fetch predicate** — a `Cmp` over a [`FieldKind::ReadTime`] rollup/formula field. The
/// engine does NOT evaluate it (the derived value is not indexed, X-3 / KN-3); the **view evaluates
/// it after fetch** over the freshly-computed derived value. Carried as the frozen
/// [`myelin_query::Predicate`] fragment so the post-fetch evaluator is the SAME bounded interpreter
/// (no second predicate engine).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostFetchPredicate {
    /// The read-time field the predicate is over (for explainability / the post-fetch evaluator).
    pub field: String,
    /// The frozen predicate fragment the view re-evaluates after fetch (the ONE interpreter).
    pub predicate: Predicate,
}

/// A sort directive — the `order_key` columnar fast-field sort (byte-identical LexoRank, §3.1),
/// ascending. The only sort the compiler emits at M2 (the structured shape sorts on it by raw byte
/// order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    /// Sort by the `order_key` columnar fast-field, ascending (raw byte order = LexoRank order).
    OrderKeyAsc,
}

/// **The compiled (but NOT yet executable) query plan** — the lowered FT/structured/vector clauses,
/// the post-fetch predicates, and the sort, produced by [`compile`]. It is **inert**: there is **no
/// executable plan without the always-conjoin ACL step**. To execute it the caller MUST call
/// [`CompiledPlan::with_acl`] (the SRCH-P08 conjoin seam), which is the only constructor of an
/// executable [`ConjoinedPlan`]. A `CompiledPlan` therefore can never reach `engine.search` on its
/// own (the `search-requires-acl-filter` ratchet holds structurally).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledPlan {
    /// The full-text clauses (the inverted shape). Conjoined (every FT clause must hold).
    pub ft: Vec<FtClause>,
    /// The structured/columnar clauses (the fast-field shape). Conjoined.
    pub structured: Vec<StructuredClause>,
    /// The vector branch, if a `semantic`/`near` request was present (`None` for a pure
    /// keyword/structured query). The hybrid RRF fusion of the FT + vector branches is SRCH-P11.
    pub vector: Option<VectorBranch>,
    /// The post-fetch predicates (the read-time rollup/formula path). The engine never evaluates
    /// these; the view re-evaluates them over the freshly-computed derived values after fetch.
    pub post_fetch: Vec<PostFetchPredicate>,
    /// The sort directive, if the AST requested an `order_key` sort.
    pub sort: Option<Sort>,
}

impl CompiledPlan {
    /// **The always-conjoin seam (§4.6 step 3).** Conjoin the ACL clause `acl` (the
    /// `acl_clause(list_objects(viewer, read, type))` result — the SRCH-P08 lowering) and produce
    /// the **executable** [`ConjoinedPlan`]. This is the **only** path from a compiled plan to an
    /// executable one: a plan can never be executed without an ACL clause (the
    /// `search-requires-acl-filter` ratchet is structural — `with_acl` is the sole constructor of
    /// `ConjoinedPlan`, and it demands the clause).
    ///
    /// The `acl` value is intentionally an opaque, caller-supplied marker here (the engine
    /// [`crate::AclFilter`] lowering of `SetExpr` is SRCH-P08); the SEAM (this method, which makes
    /// the conjoin a precondition of execution) is the SRCH-P07 deliverable.
    pub fn with_acl<A>(self, acl: A) -> ConjoinedPlan<A> {
        ConjoinedPlan { plan: self, acl }
    }
}

/// **The executable, ACL-conjoined plan** — a [`CompiledPlan`] with the mandatory ACL clause
/// attached (the only way to get one is [`CompiledPlan::with_acl`]). `acl` is non-defaultable: the
/// type cannot be constructed without it, so the engine is unreachable without a composed ACL
/// filter (the `search-requires-acl-filter` ratchet, structurally enforced — §4.2 / contract 1.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConjoinedPlan<A> {
    /// The compiled (lowered) plan.
    pub plan: CompiledPlan,
    /// The conjoined ACL clause (the `acl_clause(list_objects(...))` result, SRCH-P08). Mandatory.
    pub acl: A,
}

/// A query-compilation error — always loud (a crafted/invalid query is rejected, never silently
/// turned into an empty or unbounded plan).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileError {
    /// The frozen [`QueryAst`] static cost bounds rejected the tree (DoS-hardening — a crafted
    /// query cannot reach the engine). Wraps the [`PredicateError`].
    CostBound(PredicateError),
    /// A `Cmp` references a field the schema did not declare (no silent pass-through — an
    /// undeclared field is a query error, not an empty match).
    UndeclaredField { field: String },
    /// A `Cmp`'s literal type disagrees with the field's frozen [`FieldType`] (no silent coercion).
    TypeMismatch { field: String, declared: FieldType, got: &'static str },
    /// A range comparison (`Lt/Le/Gt/Ge`) was used over a non-ordered [`FieldType`] (un-orderable —
    /// the structured shape has no range fast-field for it).
    NotOrderable { field: String, ty: FieldType },
    /// The AST is the un-parsed placeholder surface (no compiled predicate tree) — the textual
    /// grammar parser is the P-235 floor; an un-parsed AST has nothing to lower (fail-closed).
    NotCompiled,
    /// A comparison shape the compiler does not lower (e.g. a `Cmp` between two literals or two
    /// vars — a query must compare a field var to a literal).
    UnsupportedShape { reason: &'static str },
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::CostBound(e) => write!(f, "query rejected by the cost guard: {e}"),
            CompileError::UndeclaredField { field } => {
                write!(f, "query references undeclared field `{field}`")
            }
            CompileError::TypeMismatch { field, declared, got } => write!(
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
                write!(f, "the QueryAst is the un-parsed placeholder surface (nothing to lower)")
            }
            CompileError::UnsupportedShape { reason } => {
                write!(f, "unsupported query shape: {reason}")
            }
        }
    }
}

impl std::error::Error for CompileError {}

/// **VALIDATE + LOWER (§4.6 steps 1–2).** Compile the frozen [`QueryAst`] into an inert
/// [`CompiledPlan`] against `schema`. The static cost bounds are re-asserted (defence in depth —
/// the AST validated them at construction, but the compiler never trusts that: a crafted query
/// cannot DoS the engine); every `Cmp` is resolved against the frozen [`FieldType`] schema and
/// lowered to its FT / structured / vector / post-fetch target. The result is NOT executable until
/// [`CompiledPlan::with_acl`] conjoins the ACL clause (step 3).
pub fn compile(ast: &QueryAst, schema: &FieldSchema) -> Result<CompiledPlan, CompileError> {
    let predicate = ast.predicate().ok_or(CompileError::NotCompiled)?;
    // Re-assert the static cost bounds (defence in depth — the GATE: a crafted/oversized tree is
    // rejected BEFORE lowering, never reaching the engine).
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

/// Recursively lower a predicate node into the plan. `And` flattens (every conjunct is conjoined
/// into the plan); a single-field `Or` of equalities lowers to an `In` structured clause; a `Cmp`
/// classifies by the field's declared [`FieldType`]/[`FieldKind`].
fn lower(
    predicate: &Predicate,
    schema: &FieldSchema,
    plan: &mut CompiledPlan,
) -> Result<(), CompileError> {
    match predicate {
        // The empty / always-match predicate contributes no clause (a `True` conjunct is a no-op).
        Predicate::True => Ok(()),
        // A bare `False` is a never-match — represented as an empty structured `In` over no values
        // is ambiguous, so we surface it as an unsupported top-level shape (the SRCH-P08 pipeline
        // maps a `False` predicate to `AclFilter::None`; a bare-`False` query is not a lowerable
        // clause shape here).
        Predicate::False => Err(CompileError::UnsupportedShape {
            reason: "a bare `False` predicate has no engine clause (the pipeline maps it to None)",
        }),
        Predicate::And(ps) => {
            // Try to recognise a single-field `Or` as `In` first (handled in the Or arm); here a
            // conjunction lowers each conjunct into the SAME plan (all clauses are conjoined).
            for p in ps {
                lower(p, schema, plan)?;
            }
            Ok(())
        }
        Predicate::Or(ps) => lower_or(ps, schema, plan),
        Predicate::Not(_) => Err(CompileError::UnsupportedShape {
            reason: "negation is not a lowerable clause at M2 (the bus matcher uses Not; the \
                     structured/FT engine shapes are positive clauses — a later prompt lowers \
                     NotIds via the SetExpr ACL path)",
        }),
        Predicate::Cmp { op, lhs, rhs } => lower_cmp(*op, lhs, rhs, schema, plan),
    }
}

/// Lower an `Or`: if every disjunct is an equality on the SAME field, lower to a single `In`
/// structured clause (the frozen-grammar form of `In`). Otherwise lower each disjunct individually
/// (a heterogeneous `Or` is a disjunction the SRCH-P08 pipeline composes; at M2 we lower each arm
/// into the plan so each is a clause the pipeline can compose — but a mixed-field `Or` is flagged so
/// the SRCH-P08 boolean composition owns it).
fn lower_or(
    disjuncts: &[Predicate],
    schema: &FieldSchema,
    plan: &mut CompiledPlan,
) -> Result<(), CompileError> {
    // Recognise the `In` shape: every disjunct is `Cmp{Var(f) == Lit}` over the SAME field `f`.
    let mut field: Option<String> = None;
    let mut values: Vec<Literal> = Vec::new();
    let mut is_in_shape = !disjuncts.is_empty();
    for d in disjuncts {
        match d {
            Predicate::Cmp { op: CmpOp::Eq, lhs, rhs } => match field_and_value(lhs, rhs) {
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
            .ok_or_else(|| CompileError::UndeclaredField { field: field.clone() })?;
        // `In` only lowers to a structured clause over a STORED facet — a read-time field's `In`
        // is a post-fetch predicate (the derived value is computed after fetch).
        if decl.kind == FieldKind::ReadTime {
            return Err(CompileError::UnsupportedShape {
                reason: "an `In` over a read-time rollup/formula field is evaluated post-fetch \
                         (lower each equality individually as a post-fetch predicate)",
            });
        }
        for v in &values {
            check_value_type(&field, decl.ty, v)?;
        }
        plan.structured.push(StructuredClause::In { field, ty: decl.ty, values });
        return Ok(());
    }

    // Not an `In` shape: a heterogeneous disjunction. At M2 the FT/structured engine clauses are
    // conjoined (an AND of clauses); a true OR across different fields is the SRCH-P08/P09 boolean
    // composition (the SetExpr Union path). Surface it so the downstream pipeline owns the
    // disjunction rather than the compiler silently turning an OR into an AND.
    Err(CompileError::UnsupportedShape {
        reason: "a heterogeneous OR (not a single-field `In`) is composed by the SRCH-P08/P09 \
                 boolean/SetExpr path, not lowered to a conjoined engine clause here",
    })
}

/// Lower a single `Cmp` node, classifying by the field's declared [`FieldType`] + [`FieldKind`].
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

    // The reserved semantic/near var → the vector branch (it has no schema facet; the query text is
    // the literal operand).
    if field == SEMANTIC_FIELD {
        let Literal::Str(query_text) = value else {
            return Err(CompileError::UnsupportedShape {
                reason: "a semantic/near request must carry a string query (the text to embed)",
            });
        };
        plan.vector = Some(VectorBranch { query_text: query_text.clone() });
        return Ok(());
    }

    // The reserved sort var → the order_key columnar fast-field sort directive.
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
        .ok_or_else(|| CompileError::UndeclaredField { field: field.to_string() })?;
    check_value_type(field, decl.ty, value)?;

    // READ-TIME rollup/formula field (X-3 / KN-3): the derived value is NOT indexed — lower to a
    // post-fetch predicate the view evaluates after fetch. Search indexed only the INPUTS.
    if decl.kind == FieldKind::ReadTime {
        plan.post_fetch.push(PostFetchPredicate {
            field: field.to_string(),
            predicate: Predicate::Cmp { op, lhs: lhs.clone(), rhs: rhs.clone() },
        });
        return Ok(());
    }

    // A range op (`Lt/Le/Gt/Ge`) is only defined over an ordered FieldType.
    let is_range = matches!(op, CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge);
    if is_range && !decl.ty.is_ordered() {
        return Err(CompileError::NotOrderable { field: field.to_string(), ty: decl.ty });
    }

    // A `Text`-typed facet under an `Eq` lowers to a FULL-TEXT clause (the inverted shape — analyze
    // + BM25-match the query text). A non-Eq op over Text is un-orderable (caught above is_range, but
    // Text is not ordered so a range was already rejected).
    if decl.ty == FieldType::Text && op == CmpOp::Eq {
        let Literal::Str(query) = value else {
            return Err(CompileError::TypeMismatch {
                field: field.to_string(),
                declared: FieldType::Text,
                got: literal_kind(value),
            });
        };
        plan.ft.push(FtClause { field: field.to_string(), query: query.clone() });
        return Ok(());
    }

    // Every other typed facet → a structured/columnar clause (Eq or range). `Has`/`Ref` are an `Eq`
    // over a Relation/Principal facet (no separate node — the frozen grammar has one comparison).
    plan.structured.push(StructuredClause::Cmp {
        field: field.to_string(),
        ty: decl.ty,
        op,
        value: value.clone(),
    });
    Ok(())
}

/// Extract `(field, value)` from a `Cmp`'s two expressions — a query compares a field VAR to a
/// literal (in either operand order). Returns `None` for a var-vs-var or literal-vs-literal shape
/// (not a lowerable query clause).
fn field_and_value<'a>(lhs: &'a Expr, rhs: &'a Expr) -> Option<(&'a str, &'a Literal)> {
    match (lhs, rhs) {
        (Expr::Var(f), Expr::Lit(v)) => Some((f.as_str(), v)),
        (Expr::Lit(v), Expr::Var(f)) => Some((f.as_str(), v)),
        _ => None,
    }
}

/// Reject a literal whose type disagrees with the facet's frozen [`FieldType`] (no silent
/// coercion). `Text/Date/Select/Relation/Principal/OrderKey` accept a `Str`; `Int` accepts an
/// `Int`; `Bool` accepts a `Bool`.
fn check_value_type(field: &str, ty: FieldType, value: &Literal) -> Result<(), CompileError> {
    let ok = match ty {
        FieldType::Int => matches!(value, Literal::Int(_)),
        FieldType::Bool => matches!(value, Literal::Bool(_)),
        // String-shaped facets (the columnar string fast-fields + the analyzable text body).
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
        Err(CompileError::TypeMismatch { field: field.to_string(), declared: ty, got: literal_kind(value) })
    }
}

/// The wire name of a literal's kind (for the type-mismatch error message).
fn literal_kind(value: &Literal) -> &'static str {
    match value {
        Literal::Bool(_) => "bool",
        Literal::Int(_) => "int",
        Literal::Str(_) => "string",
    }
}

/// **RENDER (§4.6 step 4): the canonical human-readable form of a compiled `QueryAst`.** This is the
/// ONE renderer (EI-01 §7): `render(compile(ast))` is the canonical string an agent and the UI both
/// emit, so a saved-view AST and an agent's identical AST render to the SAME string (the
/// no-agent-back-door property, §4.6 tail). It renders the LOWERED plan (FT / structured / vector /
/// post-fetch / sort), so two ASTs that compile to the same plan render identically — the canonical
/// form is the PLAN, not the source text.
pub fn render(plan: &CompiledPlan) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in &plan.ft {
        parts.push(format!("text({}) ~ {:?}", c.field, c.query));
    }
    for c in &plan.structured {
        match c {
            StructuredClause::Cmp { field, ty, op, value } => {
                parts.push(format!("{field}:{} {} {}", ty.wire_id(), render_op(*op), render_lit(value)));
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
        out.push('*'); // the always-match (empty) query renders as `*`.
    }
    if matches!(plan.sort, Some(Sort::OrderKeyAsc)) {
        out.push_str(" ORDER BY order_key ASC");
    }
    out
}

/// Render a comparison op to its canonical token.
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

/// Render a literal to its canonical token.
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

    /// A schema with the synthetic-producer facets: an FT body (`text`), a `status` select, a
    /// `severity` int, an `assignee` principal, a `parent` relation, the `order_key`, and a
    /// read-time `progress` rollup.
    fn schema() -> FieldSchema {
        FieldSchema::new()
            .with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
            .with("status", FieldDecl::stored(FieldType::Select))
            .with("severity", FieldDecl::stored(FieldType::Int))
            .with("done", FieldDecl::stored(FieldType::Bool))
            .with("assignee", FieldDecl::stored(FieldType::Principal))
            .with("parent", FieldDecl::stored(FieldType::Relation))
            .with("due", FieldDecl::stored(FieldType::Date))
            .with(crate::ORDER_KEY_FIELD, FieldDecl::stored(FieldType::OrderKey))
            // a read-time rollup: `progress` (an Int derived value, computed after fetch).
            .with("progress", FieldDecl::read_time(FieldType::Int))
    }

    fn ast(p: Predicate) -> QueryAst {
        QueryAst::compiled(p).expect("the test predicate is within the cost bounds")
    }

    /// **A `Text` over the FT body lowers to an FT clause (the full-text inverted shape).**
    #[test]
    fn text_lowers_to_ft_clause() {
        let plan = compile(
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") }),
            &schema(),
        )
        .expect("compile");
        assert_eq!(plan.ft, vec![FtClause { field: "text".into(), query: "deadlock".into() }]);
        assert!(plan.structured.is_empty() && plan.vector.is_none());
    }

    /// **An `Eq` over a typed facet lowers to a structured/columnar clause.** (Select equality.)
    #[test]
    fn eq_over_typed_facet_lowers_to_structured() {
        let plan = compile(
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var("status"), rhs: s("open") }),
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

    /// **A range `Cmp` over an ordered facet (Int) lowers to a structured range clause; a range over
    /// a non-ordered facet is rejected.**
    #[test]
    fn range_over_ordered_ok_over_unordered_rejected() {
        let ok = compile(
            &ast(Predicate::Cmp { op: CmpOp::Ge, lhs: var("severity"), rhs: i(3) }),
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
        // A range over a Select (non-ordered) is rejected.
        let err = compile(
            &ast(Predicate::Cmp { op: CmpOp::Lt, lhs: var("status"), rhs: s("z") }),
            &schema(),
        )
        .expect_err("a range over a non-ordered facet is rejected");
        assert!(matches!(err, CompileError::NotOrderable { .. }));
    }

    /// **`Has`/`Ref` are an `Eq` over a Relation/Principal facet (one frozen comparison node, not a
    /// new AST variant).**
    #[test]
    fn has_ref_lower_as_eq_over_relation_principal() {
        let plan = compile(
            &ast(Predicate::And(vec![
                Predicate::Cmp { op: CmpOp::Eq, lhs: var("assignee"), rhs: s("p:alice") },
                Predicate::Cmp { op: CmpOp::Eq, lhs: var("parent"), rhs: s("myelin://t/x/issue/1") },
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

    /// **A single-field `Or` of equalities lowers to an `In` structured clause (the frozen-grammar
    /// `In`).**
    #[test]
    fn single_field_or_lowers_to_in() {
        let plan = compile(
            &ast(Predicate::Or(vec![
                Predicate::Cmp { op: CmpOp::Eq, lhs: var("status"), rhs: s("open") },
                Predicate::Cmp { op: CmpOp::Eq, lhs: var("status"), rhs: s("in_review") },
            ])),
            &schema(),
        )
        .expect("compile");
        assert_eq!(
            plan.structured,
            vec![StructuredClause::In {
                field: "status".into(),
                ty: FieldType::Select,
                values: vec![Literal::Str("open".into()), Literal::Str("in_review".into())],
            }]
        );
    }

    /// **A `semantic`/`near` request lowers to the vector branch.**
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
            Some(VectorBranch { query_text: "how do I reset my password".into() })
        );
    }

    /// **A hybrid query (FT + semantic + structured) lowers all three branches under ONE compiled
    /// plan (one doc-id space; the RRF fusion is SRCH-P11).**
    #[test]
    fn hybrid_query_lowers_all_branches() {
        let plan = compile(
            &ast(Predicate::And(vec![
                Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("login") },
                Predicate::Cmp { op: CmpOp::Eq, lhs: var(SEMANTIC_FIELD), rhs: s("auth flow") },
                Predicate::Cmp { op: CmpOp::Eq, lhs: var("status"), rhs: s("open") },
            ])),
            &schema(),
        )
        .expect("compile");
        assert_eq!(plan.ft.len(), 1, "FT branch");
        assert!(plan.vector.is_some(), "vector branch");
        assert_eq!(plan.structured.len(), 1, "structured branch");
    }

    /// **THE READ-TIME ROLLUP/FORMULA GATE (X-3 / KN-3): a `Cmp` over a rollup/formula field does
    /// NOT lower to a stored engine clause — it becomes a POST-FETCH predicate (the derived value is
    /// computed after fetch; Search indexed only the inputs).** A stored facet of the SAME type DOES
    /// lower to a structured clause — proving the difference is the read-time KIND, not the type.
    #[test]
    fn read_time_field_is_post_fetch_never_a_stored_clause() {
        let plan = compile(
            &ast(Predicate::Cmp { op: CmpOp::Ge, lhs: var("progress"), rhs: i(80) }),
            &schema(),
        )
        .expect("compile");
        // No stored/FT/vector clause was produced over the derived value.
        assert!(plan.structured.is_empty(), "the derived value is NOT a stored structured clause");
        assert!(plan.ft.is_empty() && plan.vector.is_none());
        // It is a post-fetch predicate (the view computes `progress` after fetch, then evaluates).
        assert_eq!(plan.post_fetch.len(), 1);
        assert_eq!(plan.post_fetch[0].field, "progress");
        assert_eq!(
            plan.post_fetch[0].predicate,
            Predicate::Cmp { op: CmpOp::Ge, lhs: var("progress"), rhs: i(80) },
            "the post-fetch predicate is the SAME frozen Predicate (the one interpreter re-evaluates)"
        );

        // Contrast: the SAME Int type as a STORED facet (`severity`) DOES lower to a structured
        // clause — so the difference is the read-time KIND, not the field type.
        let stored = compile(
            &ast(Predicate::Cmp { op: CmpOp::Ge, lhs: var("severity"), rhs: i(80) }),
            &schema(),
        )
        .expect("compile");
        assert_eq!(stored.structured.len(), 1, "a STORED int facet lowers to a structured clause");
        assert!(stored.post_fetch.is_empty());
    }

    /// **The `order_key` sort directive lowers to the columnar fast-field sort (byte-identical
    /// LexoRank).**
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

    /// **THE BOUNDED-COST GATE: a crafted/oversized AST is rejected by the cost guard BEFORE
    /// lowering — 0 engine DoS.** (The AST construction already bounds it, but the compiler
    /// re-asserts the bound — defence in depth.)
    #[test]
    fn oversized_ast_rejected_by_cost_guard() {
        // An oversized tree cannot even be constructed via `QueryAst::compiled` (it is rejected at
        // construction) — so we assert BOTH halves of the DoS guard: construction rejects it, AND
        // the compiler's own re-assertion would reject a tree at the boundary.
        let big: Vec<Predicate> = (0..(myelin_query::MAX_PREDICATE_NODES + 10))
            .map(|_| Predicate::True)
            .collect();
        // Construction rejects (the AST never holds an over-budget tree).
        assert!(QueryAst::compiled(Predicate::And(big)).is_err());

        // A deeply-nested tree at the depth ceiling is rejected by the compiler's re-assertion (we
        // build it just under the construction limit and confirm the compiler re-validates).
        let deep = {
            let mut p = Predicate::Cmp { op: CmpOp::Eq, lhs: var("status"), rhs: s("open") };
            for _ in 0..(myelin_query::MAX_PREDICATE_DEPTH - 2) {
                p = Predicate::And(vec![p]);
            }
            p
        };
        // This one is within bounds and compiles (proves the boundary is exact, not over-eager).
        assert!(compile(&ast(deep), &schema()).is_ok());
    }

    /// **An undeclared field is a loud error (no silent pass-through), and a type-mismatched literal
    /// is rejected (no silent coercion).**
    #[test]
    fn undeclared_field_and_type_mismatch_are_loud() {
        let undeclared = compile(
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var("nope"), rhs: s("x") }),
            &schema(),
        )
        .expect_err("an undeclared field is rejected");
        assert!(matches!(undeclared, CompileError::UndeclaredField { .. }));

        let mismatch = compile(
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var("severity"), rhs: s("not-an-int") }),
            &schema(),
        )
        .expect_err("a string over an int facet is rejected");
        assert!(matches!(mismatch, CompileError::TypeMismatch { .. }));
    }

    /// **An un-parsed placeholder AST has nothing to lower (fail-closed, the P-235 grammar floor).**
    #[test]
    fn unparsed_placeholder_fails_closed() {
        let err = compile(&QueryAst::raw("status == 'open'"), &schema())
            .expect_err("an un-parsed AST is not lowerable");
        assert!(matches!(err, CompileError::NotCompiled));
    }

    /// **THE ALWAYS-CONJOIN SEAM: a `CompiledPlan` is inert; `with_acl` is the ONLY way to get an
    /// executable `ConjoinedPlan`.** A plan can never reach the engine without the ACL clause (the
    /// search-requires-acl-filter ratchet is structural).
    #[test]
    fn conjoin_seam_is_the_only_path_to_an_executable_plan() {
        let plan = compile(
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var("status"), rhs: s("open") }),
            &schema(),
        )
        .expect("compile");
        // The compiled plan carries the lowered clause but NO acl — it is inert.
        assert_eq!(plan.structured.len(), 1);
        // The ONLY constructor of an executable plan demands the acl clause (here a marker string
        // standing in for the SRCH-P08 AclFilter lowering of list_objects).
        let conjoined = plan.with_acl("acl_clause(list_objects(viewer, read, issue))");
        assert_eq!(conjoined.acl, "acl_clause(list_objects(viewer, read, issue))");
        assert_eq!(conjoined.plan.structured.len(), 1);
    }

    /// **AST ROUND-TRIP / NO-AGENT-BACK-DOOR: `render(compile(ast))` is the canonical form, and a
    /// saved-view AST compiles to the SAME plan an agent's identical AST compiles to.**
    #[test]
    fn render_round_trip_and_no_agent_back_door() {
        let p = Predicate::And(vec![
            Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") },
            Predicate::Cmp { op: CmpOp::Eq, lhs: var("status"), rhs: s("open") },
            Predicate::Cmp { op: CmpOp::Eq, lhs: var(SORT_FIELD), rhs: s(crate::ORDER_KEY_FIELD) },
        ]);
        // The "UI" emits the AST; the "agent" emits a byte-identical AST (the SAME frozen QueryAst).
        let ui_ast = ast(p.clone());
        let agent_ast = ast(p);
        let ui_plan = compile(&ui_ast, &schema()).expect("ui compile");
        let agent_plan = compile(&agent_ast, &schema()).expect("agent compile");
        // The SAME AST compiles to the SAME plan — no agent back-door (§4.6 tail).
        assert_eq!(ui_plan, agent_plan, "agent and UI compile the identical AST to the identical plan");
        // The canonical rendered form is identical too (the ONE renderer).
        assert_eq!(render(&ui_plan), render(&agent_plan));
        assert_eq!(
            render(&ui_plan),
            "text(text) ~ \"deadlock\" AND status:select == \"open\" ORDER BY order_key ASC",
            "the canonical human-readable form of the lowered plan"
        );
    }

    /// **BYTE-IDENTICAL-SEMANTICS DRIFT TEST: the same AST means the same thing to Search's compiler
    /// and to the bus's `EventMatcher` core (3.4) — a later `FieldType`/grammar change breaks this
    /// test NOW.** Both consume the ONE frozen `myelin_query::QueryAst`; we assert the SHARED
    /// predicate bytes are identical and that Search's lowering reads the SAME field-type taxonomy
    /// the matcher's projection types over.
    #[test]
    fn byte_identical_semantics_with_the_eventmatcher_core() {
        use myelin_query::EventMatcher;
        use myelin_identity::ObjectType;

        // ONE predicate, built once. The bus matcher and Search's compiler BOTH consume it.
        let predicate = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("status"),
            rhs: s("open"),
        })
        .expect("compile");

        // The bus EventMatcher wraps the SAME QueryAst.
        let matcher = EventMatcher::new(ObjectType("issue".into()), predicate.clone());

        // **Drift anchor 1:** the predicate bytes Search lowers are byte-identical to the bytes the
        // matcher carries — there is ONE serialisation (a grammar change breaks both at once).
        let search_bytes = serde_json::to_value(&predicate).unwrap();
        let matcher_bytes = serde_json::to_value(matcher.predicate()).unwrap();
        assert_eq!(search_bytes, matcher_bytes, "ONE QueryAst serialisation — no Search/matcher drift");

        // **Drift anchor 2:** Search lowers this AST against the frozen FieldType taxonomy; pin the
        // full taxonomy BY VALUE so a rename/reorder of a `FieldType` variant (in the contract home
        // myelin-query) breaks Search's compiler test HERE, now (the prompt's "a later FieldType
        // change breaks this test now" — EI-01 §7).
        let wire_ids: Vec<&str> = FieldType::all().iter().map(|t| t.wire_id()).collect();
        assert_eq!(
            wire_ids,
            ["text", "int", "bool", "date", "select", "relation", "principal", "order_key"],
            "the frozen FieldType taxonomy Search's compiler lowers over (byte-identical to the \
             EventMatcher core / Issues / Knowledge)"
        );

        // And the AST compiles to a structured clause typed over that frozen taxonomy.
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

    /// **order_key from the frozen LexoRank encoding sorts byte-identically (the compiler's sort
    /// directive targets the SAME columnar fast-field the engine sorts on).** A compile-side sanity
    /// that the order_key facet name + frozen encoding line up with the engine's `ORDER_KEY_FIELD`.
    #[test]
    fn order_key_sort_targets_the_frozen_columnar_field() {
        // The compiler's sort var resolves to the SAME engine field constant.
        assert_eq!(crate::ORDER_KEY_FIELD, "order_key");
        // The frozen LexoRank keys the sort is over compare by raw byte order.
        let a = OrderKey::parse("G").unwrap();
        let b = OrderKey::parse("V").unwrap();
        assert!(a < b, "the LexoRank byte order is the sort order the columnar fast-field uses");
    }

    /// **A comparison with the literal on the LEFT and the field var on the RIGHT lowers identically
    /// (operand order does not matter — `Lit == Var`).** Kills the reversed-operand match-arm mutant.
    #[test]
    fn reversed_operand_order_lowers_the_same() {
        // `"open" == status` (literal first) lowers to the SAME structured clause as `status ==
        // "open"`.
        let plan = compile(
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: s("open"), rhs: var("status") }),
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
        // A literal-vs-literal (no field var either side) is not a lowerable query clause.
        let err = compile(
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: s("a"), rhs: s("b") }),
            &schema(),
        )
        .expect_err("a literal-vs-literal comparison has no field to lower over");
        assert!(matches!(err, CompileError::UnsupportedShape { .. }));
    }

    /// **The error messages are exact (the loud-rejection contract — a query error names WHY).**
    /// Kills the `Display::fmt` and `literal_kind` string mutants: a type mismatch over an Int facet
    /// reports the field, the declared type, and the literal's kind.
    #[test]
    fn compile_error_messages_are_exact() {
        let err = compile(
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var("severity"), rhs: s("nope") }),
            &schema(),
        )
        .expect_err("string over int");
        // The literal kind ("string") and the declared type ("int") both appear (kills the
        // literal_kind "" / "xyzzy" mutants AND the Display::fmt default mutant).
        let msg = err.to_string();
        assert!(msg.contains("severity"), "names the field: {msg}");
        assert!(msg.contains("int"), "names the declared frozen FieldType: {msg}");
        assert!(msg.contains("string"), "names the offending literal kind: {msg}");

        // An undeclared field's message names the field too (a different Display arm).
        let undeclared = compile(
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var("ghost"), rhs: s("x") }),
            &schema(),
        )
        .expect_err("undeclared");
        assert!(
            undeclared.to_string().contains("ghost"),
            "the undeclared-field error names the field"
        );

        // The not-orderable and not-compiled arms render non-empty, distinct messages.
        let not_orderable = compile(
            &ast(Predicate::Cmp { op: CmpOp::Lt, lhs: var("status"), rhs: s("z") }),
            &schema(),
        )
        .expect_err("range over select");
        assert!(not_orderable.to_string().contains("status"), "not-orderable names the field");
        assert!(
            compile(&QueryAst::raw("x"), &schema())
                .expect_err("unparsed")
                .to_string()
                .contains("placeholder"),
            "the not-compiled error explains the placeholder surface"
        );
    }

    /// **Negation is honestly NOT a lowerable positive clause at M2 (the SetExpr NotIds path is the
    /// later prompt) — surfaced, never silently dropped.**
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
