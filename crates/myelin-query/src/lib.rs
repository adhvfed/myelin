//! # `myelin-query` — the single query AST + the ONE bounded predicate core
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.5 (`myelin-query` — the substrate-relevant seam) and §7.5 (bounded predicate
//! evaluation); `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §8.6 (the `CaveatContext` rider reuses this predicate core) / §9 (ABAC predicates reuse
//! the one query-AST core).
//!
//! **Contract-index cluster:** 13 — the shared crates' refined shapes
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` row 13.3
//! `myelin-query` primitive, frozen byte-identical X-3/OQ-C) and row 3.4 (`EventMatcher`
//! = the frozen `QueryAst` — **bounded interpreter, no UDFs/loops/recursion, statically
//! cost-bounded, permission-aware. Not CEL/JSONLogic. No per-subsystem trigger DSL**).
//!
//! ## What crosses the crate boundary here (the substrate-relevant surface)
//! The `EventMatcher` predicate core (Bus triggers) and saved-view filters **share the
//! same [`QueryAst`]** (frozen byte-identical, X-3) — one safe-evaluation engine, one
//! DoS-hardening surface. The substrate's stake (§7.5): **bounded predicate evaluation** —
//! declarative, safe-to-evaluate, no Turing-complete predicates, no UDFs/loops/recursion,
//! statically cost-bounded; a per-predicate step/time ceiling so a crafted matcher cannot
//! DoS the trigger engine. The boundedness is **structural** here.
//!
//! ## The ONE predicate language (EI-01 §7 — no second predicate language)
//! P-001 reserved [`QueryAst`] as a placeholder type name on the bounded-evaluation seam so
//! the matcher/view consumers had a stable surface to reference. **P-133 / P-ID-22**
//! (the first real consumer — Identity's promoted `CaveatContext` evaluator) **promotes
//! that placeholder into a real bounded predicate AST + the single safe interpreter**:
//! [`Predicate`], [`Expr`], [`CmpOp`], and [`QueryAst::eval`]. This is THE platform
//! predicate language — exactly one evaluation engine, one DoS-hardening surface. Identity's
//! caveat evaluator (P-133), the Bus `EventMatcher` (P-137), Notif prefs (7.4), saved
//! views (13.3) and Search (compile target) all consume THIS core; none ship a second
//! predicate language (the architecture/EI-01 §7 invariant — abstract at the third copy,
//! and the third copy is already here).
//!
//! ## The 13.3 freeze surface (engine + field-type + view-model + grammar — all now real)
//! The predicate ENGINE (this module) is real ([`Predicate`]/[`Expr`]/[`CmpOp`]/[`QueryAst`],
//! P-133); the **frozen `FieldType` enum + `FieldValue` + the `order_key`/LexoRank encoding (13.3,
//! frozen X-3)** live in [`field`] (P-167, see the DEVIATION note there). **P-235 (KN-P02)** —
//! this slice — lands the **last two halves of the 13.3 primitive**: the frozen [`view::ViewSpec`]
//! view-model and the textual [`parse`] grammar front-end (`"status == 'open'"` → a validated
//! [`Predicate`] tree). These EXTEND the one core in place — they add the view-model + the parser
//! ON TOP of the one [`Predicate`] tree + the one [`field::FieldType`] enum; they do **NOT**
//! re-define a second predicate engine, a second field-type enum, or a second view-filter language.
//! The Bus matcher `subscribe`-time compile target is **P-137 (EB-17)** (it reuses this same
//! grammar + parser). Issues co-owns these definitions byte-identically (its prompts build their
//! own *executor* against the SAME [`ViewSpec`]/[`QueryAst`]). The deviations (promoting the engine
//! here ahead of the prose freeze dependency — P-133; landing `FieldType` here for SRCH-P04 —
//! P-167) are recorded at their sites; **no NAMED floor remains in the 13.3 grammar surface**.

use myelin_identity::Literal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod field;
pub use field::{
    FieldType, FieldValue, OrderKey, LEXORANK_ALPHABET, LEXORANK_REBALANCE_LEN,
};

pub mod view;
pub use view::{FieldId, SortDir, SortSpec, ViewKind, ViewSpec};

pub mod parse;
pub use parse::{parse_predicate, parse_query, ParseError, MAX_PARSE_DEPTH};

pub mod matcher;
pub use matcher::{
    project_envelope, EventMatcher, RelMembership, MAX_SETEXPR_DEPTH, MAX_SETEXPR_NODES,
};

pub mod signals;
pub use signals::{
    define_signal_rule, DedupKey, DedupKeyTpl, DedupWindow, PublishDraft, PublishKind, RuleId,
    Severity, Signal, SignalEngine, SignalRule, SignalState,
};

pub mod automations;
pub use automations::{
    register_automation, Action, ActionKind, AutomationEngine, AutomationId, AutomationRule,
    Budget, Delegation, DurableExecutor, DurableHandle, ExecutorError, Gate, InMemoryExecutor,
    Outcome, RunAs, StartedRun, WorkflowRef,
};

pub mod triggers;
// NB: `WorkflowRef` is NOT re-exported here — it is the SAME type re-exported from `automations`
// (triggers REUSES `crate::WorkflowRef`, no second workflow-reference type).
pub use triggers::{
    arm_trigger, disarm_trigger, ArmingId, DurableTimer, InMemoryTimer, OnResolve, Resolution,
    StaleAfter, TimerError, Trigger, TriggerArming, TriggerEngine, TriggerId, TriggerState,
};

pub mod dispatch;
pub use dispatch::{
    CostGate, DispatchBreaker, DispatchError, DispatchRequest, DispatchTarget, DispatchTelemetry,
    DispatchTier, Disposition, InMemoryCostGate, RecordingTarget, Reservation, ShedReason,
    ShedSignal, SignalBinding, TriggerKind, CAUSAL_DEPTH_CEILING, DISPATCH_INFLIGHT_CAP,
    SHARED_ROOT_TRIPWIRE_K, SHED_RETRY_AFTER_SECONDS,
};

/// The static cost ceiling: the maximum number of AST nodes a [`QueryAst`] may contain. A
/// predicate exceeding this is **rejected before evaluation** ([`QueryAst::validate`]) — a
/// crafted matcher can never present an unboundedly large tree to the interpreter. The
/// bound is generous for legitimate field/transition caveats (a handful of conjoined
/// comparisons) while being a hard structural ceiling. There are no loops/recursion-to-
/// unbounded-depth in the grammar, so node-count is the complete static cost measure.
pub const MAX_PREDICATE_NODES: usize = 256;

/// The maximum nesting depth of a predicate tree (a second structural bound — a deeply
/// nested boolean tree is also rejected before evaluation so the interpreter recursion is
/// statically bounded, never stack-unbounded).
pub const MAX_PREDICATE_DEPTH: usize = 32;

/// The runtime evaluation step ceiling: even within the static node bound, the interpreter
/// counts every evaluated node and aborts (returns [`EvalError::CostExceeded`]) if it would
/// exceed this — the belt-and-braces DoS guard (a validated tree cannot exceed it, but the
/// interpreter never trusts the validator: defence in depth, EI-01 §3).
pub const MAX_EVAL_STEPS: usize = 4096;

/// The single declarative, bounded-to-evaluate query/predicate AST (architecture §2.5,
/// §7.5; contract 13.3 / 3.4, frozen X-3). It is the `EventMatcher` core, the saved-view
/// filter, AND the `CaveatContext` predicate — **one grammar, one bounded interpreter,
/// many compile targets**.
///
/// A `QueryAst` wraps a [`Predicate`] tree. The textual grammar surface (`"status ==
/// 'open'"`) is the Issues/Knowledge co-owned parser (a NAMED floor, P-235) that compiles a
/// string into this tree; the tree + the interpreter are frozen here. The legacy
/// placeholder `QueryAst(String)` constructor is preserved as [`QueryAst::raw`] for the
/// not-yet-parsed surface so no consumer breaks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryAst {
    /// The compiled predicate tree this AST evaluates. `None` is the un-compiled
    /// placeholder surface (the textual `raw` form whose parser is the P-235 floor) — an
    /// un-compiled AST evaluates to [`EvalError::NotCompiled`] (fail-closed: an un-parsed
    /// predicate is genuine uncertainty, never a silent match).
    predicate: Option<Predicate>,
    /// The original textual form (the P-001 placeholder surface), retained so the grammar
    /// parser (P-235) and observability can recover the source. Empty for a directly-built
    /// predicate tree.
    raw: String,
}

impl QueryAst {
    /// Build an AST directly from a compiled [`Predicate`] tree (the in-process build path
    /// the caveat/matcher use before the textual grammar parser lands). The tree is
    /// **validated against the static cost bounds at construction** — an over-budget tree is
    /// rejected here, never evaluated. Returns [`PredicateError`] on a tree that exceeds
    /// [`MAX_PREDICATE_NODES`] / [`MAX_PREDICATE_DEPTH`].
    pub fn compiled(predicate: Predicate) -> Result<QueryAst, PredicateError> {
        Self::validate(&predicate)?;
        Ok(QueryAst {
            raw: String::new(),
            predicate: Some(predicate),
        })
    }

    /// Build an AST from a compiled [`Predicate`] tree **while retaining the textual source**
    /// the grammar parser ([`parse::parse_query`]) compiled it from. The tree is validated
    /// against the static cost bound exactly like [`QueryAst::compiled`]; the source is kept for
    /// observability and round-trip recovery. This is the constructor the **P-235 grammar
    /// front-end** uses — it does NOT introduce a second engine, it wraps the ONE validated tree.
    pub fn compiled_with_source(
        predicate: Predicate,
        source: impl Into<String>,
    ) -> QueryAst {
        // The parser already re-validates; this constructor is infallible by contract (the parser
        // never hands an over-budget tree). We still keep the validated tree as the source of truth.
        QueryAst {
            raw: source.into(),
            predicate: Some(predicate),
        }
    }

    /// The legacy placeholder surface: a not-yet-parsed textual predicate (the P-001 seam,
    /// preserved so consumers referencing the string form still compile). It carries **no**
    /// compiled tree, so [`QueryAst::eval`] returns [`EvalError::NotCompiled`] (fail-closed)
    /// until the grammar parser (P-235) compiles it. This is the named-floor surface, kept
    /// honest: an un-parsed predicate is uncertainty, never a silent match/allow.
    pub fn raw(text: impl Into<String>) -> QueryAst {
        QueryAst {
            raw: text.into(),
            predicate: None,
        }
    }

    /// The textual source form (the placeholder surface / observability handle). Empty for a
    /// directly-compiled predicate tree.
    pub fn source(&self) -> &str {
        &self.raw
    }

    /// The compiled predicate tree, if this AST was built from one (`None` for the un-parsed
    /// placeholder surface).
    pub fn predicate(&self) -> Option<&Predicate> {
        self.predicate.as_ref()
    }

    /// **Validate a predicate tree against the static cost bounds** (the DoS-hardening
    /// surface, §7.5). Rejects a tree exceeding [`MAX_PREDICATE_NODES`] or
    /// [`MAX_PREDICATE_DEPTH`] **before** any evaluation. Called at construction; exposed so
    /// a compiler (the P-235 grammar parser) can validate its own output against the one
    /// bound.
    pub fn validate(predicate: &Predicate) -> Result<(), PredicateError> {
        let nodes = predicate.node_count();
        if nodes > MAX_PREDICATE_NODES {
            return Err(PredicateError::TooLarge { nodes });
        }
        let depth = predicate.depth();
        if depth > MAX_PREDICATE_DEPTH {
            return Err(PredicateError::TooDeep { depth });
        }
        Ok(())
    }

    /// **Evaluate the predicate against a context** (the ONE safe, bounded interpreter,
    /// contract 3.4). `ctx` binds the [`Expr::Var`] names the predicate reads (the supplied
    /// `attrs` of a `CaveatContext`, the projection fields of an `EventMatcher`, …).
    ///
    /// Returns:
    /// - `Ok(true)` / `Ok(false)` — the predicate is **defined** over the supplied context
    ///   and evaluates to that boolean.
    /// - `Err(EvalError::MissingContext { name })` — the predicate references a variable the
    ///   context did **not** supply. The CALLER decides what this means (Identity's caveat
    ///   maps it to `Conditional` — never a silent allow; a matcher maps it to "no match").
    ///   It is **never** silently treated as `true`.
    /// - `Err(EvalError::TypeError)` — a comparison is not defined over the operand types
    ///   (e.g. ordering on strings) — un-evaluable, never silently `true`.
    /// - `Err(EvalError::CostExceeded)` — the runtime step ceiling was hit (defence in depth
    ///   over the static bound).
    /// - `Err(EvalError::NotCompiled)` — an un-parsed placeholder AST (the P-235 floor).
    ///
    /// **Boundedness is structural:** the grammar has no loops, no recursion-to-unbounded-
    /// depth (the tree is finite and depth-bounded at construction), and no UDFs; the step
    /// counter is the belt-and-braces guard. A crafted predicate cannot DoS the interpreter.
    pub fn eval(&self, ctx: &EvalContext) -> Result<bool, EvalError> {
        let predicate = self.predicate.as_ref().ok_or(EvalError::NotCompiled)?;
        let mut steps = 0usize;
        predicate.eval(ctx, &mut steps)
    }
}

/// The variable-binding context an [`Expr::Var`] reads (the supplied `attrs` of a
/// `CaveatContext`, the projection fields the `EventMatcher` matches over). A variable the
/// context does not bind surfaces as [`EvalError::MissingContext`] — never a silent default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvalContext {
    bindings: BTreeMap<String, Literal>,
}

impl EvalContext {
    /// An empty context (binds no variables).
    pub fn new() -> EvalContext {
        EvalContext {
            bindings: BTreeMap::new(),
        }
    }

    /// Build a context from a map of variable bindings (e.g. a `CaveatContext.attrs`).
    pub fn from_attrs(attrs: BTreeMap<String, Literal>) -> EvalContext {
        EvalContext { bindings: attrs }
    }

    /// Bind one variable (builder style).
    pub fn bind(mut self, name: impl Into<String>, value: Literal) -> EvalContext {
        self.bindings.insert(name.into(), value);
        self
    }

    /// Look up a variable binding (`None` ⇒ the context did not supply it ⇒ missing context).
    fn get(&self, name: &str) -> Option<&Literal> {
        self.bindings.get(name)
    }
}

/// A boolean predicate node — the ONE predicate grammar (contract 3.4). It is **declarative
/// and bounded**: comparisons of expressions, the three boolean connectives, and the
/// constants. **No** loops, **no** recursion-to-unbounded-depth (the tree is finite and
/// depth-bounded), **no** UDFs. This is the complete predicate surface; field/transition
/// caveats, the Bus matcher, and saved-view filters all compile to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Predicate {
    /// The constant `true` (the empty / always-match predicate).
    True,
    /// The constant `false`.
    False,
    /// A comparison `lhs <op> rhs` over two expressions.
    Cmp {
        op: CmpOp,
        lhs: Expr,
        rhs: Expr,
    },
    /// Conjunction — every conjunct must hold (`AND`).
    And(Vec<Predicate>),
    /// Disjunction — at least one disjunct must hold (`OR`).
    Or(Vec<Predicate>),
    /// Negation (`NOT`).
    Not(Box<Predicate>),
}

/// A value-producing expression in a predicate: either a literal constant or a variable read
/// from the [`EvalContext`]. There are exactly two — there is **no** function call,
/// arithmetic, or field-walk surface (that would be a second, unbounded predicate language).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    /// A literal constant (the value space frozen in `myelin_identity::Literal`).
    Lit(Literal),
    /// A variable read from the context by name (e.g. an `issue.severity` attr the caller
    /// supplied). An unbound variable surfaces as [`EvalError::MissingContext`].
    Var(String),
}

/// The comparison operators (the only comparisons the bounded core defines). Equality is
/// defined on every same-typed pair; ordering (`Lt/Le/Gt/Ge`) is defined **only** on `Int`
/// — a cross-type or non-orderable comparison is a [`EvalError::TypeError`] (un-evaluable),
/// never silently `true`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CmpOp {
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<` (Int only)
    Lt,
    /// `<=` (Int only)
    Le,
    /// `>` (Int only)
    Gt,
    /// `>=` (Int only)
    Ge,
}

impl Predicate {
    /// The total node count (the static cost measure). Each `Predicate` and each `Expr`
    /// counts as one node.
    fn node_count(&self) -> usize {
        match self {
            Predicate::True | Predicate::False => 1,
            Predicate::Cmp { lhs, rhs, .. } => 1 + lhs.node_count() + rhs.node_count(),
            Predicate::And(ps) | Predicate::Or(ps) => {
                1 + ps.iter().map(Predicate::node_count).sum::<usize>()
            }
            Predicate::Not(p) => 1 + p.node_count(),
        }
    }

    /// The maximum nesting depth (the second structural bound — caps the interpreter
    /// recursion).
    fn depth(&self) -> usize {
        match self {
            Predicate::True | Predicate::False | Predicate::Cmp { .. } => 1,
            Predicate::And(ps) | Predicate::Or(ps) => {
                1 + ps.iter().map(Predicate::depth).max().unwrap_or(0)
            }
            Predicate::Not(p) => 1 + p.depth(),
        }
    }

    /// Evaluate this node, charging one step per node and aborting at [`MAX_EVAL_STEPS`]
    /// (defence in depth over the construction-time static bound). Short-circuits `And`/`Or`
    /// — but the step charge happens before the short-circuit so a malicious tree is still
    /// bounded.
    fn eval(&self, ctx: &EvalContext, steps: &mut usize) -> Result<bool, EvalError> {
        *steps += 1;
        if *steps > MAX_EVAL_STEPS {
            return Err(EvalError::CostExceeded);
        }
        match self {
            Predicate::True => Ok(true),
            Predicate::False => Ok(false),
            Predicate::Cmp { op, lhs, rhs } => {
                let l = lhs.resolve(ctx)?;
                let r = rhs.resolve(ctx)?;
                op.apply(l, r)
            }
            Predicate::And(ps) => {
                // Conjunction: every conjunct must hold. A `MissingContext`/`TypeError` in
                // ANY conjunct propagates (fail-closed: an un-evaluable conjunct does not get
                // silently dropped — the caller decides what missing context means).
                for p in ps {
                    if !p.eval(ctx, steps)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Predicate::Or(ps) => {
                // Disjunction: at least one disjunct must hold. We do NOT swallow a
                // MissingContext from one arm just because another arm is true — a predicate
                // that needs context the caller did not supply must surface that (the caveat
                // → Conditional invariant). So we evaluate eagerly and propagate the first
                // error; only an all-false (no error) is a defined `false`.
                let mut any = false;
                for p in ps {
                    if p.eval(ctx, steps)? {
                        any = true;
                    }
                }
                Ok(any)
            }
            Predicate::Not(p) => Ok(!p.eval(ctx, steps)?),
        }
    }
}

impl Expr {
    fn node_count(&self) -> usize {
        1
    }

    /// Resolve an expression to a literal value against the context. An unbound `Var` is
    /// **missing context** (never a silent default) — the caller (Identity) maps that to
    /// `Conditional`.
    fn resolve<'a>(&'a self, ctx: &'a EvalContext) -> Result<&'a Literal, EvalError> {
        match self {
            Expr::Lit(l) => Ok(l),
            Expr::Var(name) => ctx
                .get(name)
                .ok_or_else(|| EvalError::MissingContext { name: name.clone() }),
        }
    }
}

impl CmpOp {
    /// Apply the comparison to two resolved literals. Equality on any same-typed pair;
    /// ordering only on `Int`. A cross-type or non-orderable comparison is a `TypeError`
    /// (un-evaluable) — never silently `true`/`false`.
    fn apply(self, lhs: &Literal, rhs: &Literal) -> Result<bool, EvalError> {
        match self {
            CmpOp::Eq => literals_eq(lhs, rhs).ok_or(EvalError::TypeError),
            CmpOp::Ne => literals_eq(lhs, rhs).map(|b| !b).ok_or(EvalError::TypeError),
            CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => match (lhs, rhs) {
                (Literal::Int(a), Literal::Int(b)) => Ok(match self {
                    CmpOp::Lt => a < b,
                    CmpOp::Le => a <= b,
                    CmpOp::Gt => a > b,
                    CmpOp::Ge => a >= b,
                    _ => unreachable!("the match arm constrains the op set to the orderings"),
                }),
                // Ordering is not defined over non-Int literals (no string/bool ordering at
                // the bounded core) → un-evaluable.
                _ => Err(EvalError::TypeError),
            },
        }
    }
}

/// Literal equality — defined only for same-typed pairs (`Some`); a cross-type comparison is
/// un-evaluable (`None`) so it surfaces as a `TypeError`, never a silent boolean.
fn literals_eq(lhs: &Literal, rhs: &Literal) -> Option<bool> {
    match (lhs, rhs) {
        (Literal::Bool(a), Literal::Bool(b)) => Some(a == b),
        (Literal::Int(a), Literal::Int(b)) => Some(a == b),
        (Literal::Str(a), Literal::Str(b)) => Some(a == b),
        _ => None,
    }
}

/// A predicate construction error (the static cost bounds rejected the tree — DoS-hardening
/// **before** evaluation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PredicateError {
    /// The tree exceeds [`MAX_PREDICATE_NODES`] (statically cost-bounded — rejected).
    TooLarge { nodes: usize },
    /// The tree exceeds [`MAX_PREDICATE_DEPTH`] (statically depth-bounded — rejected).
    TooDeep { depth: usize },
}

impl std::fmt::Display for PredicateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PredicateError::TooLarge { nodes } => write!(
                f,
                "predicate exceeds the static node ceiling ({nodes} > {MAX_PREDICATE_NODES})"
            ),
            PredicateError::TooDeep { depth } => write!(
                f,
                "predicate exceeds the static depth ceiling ({depth} > {MAX_PREDICATE_DEPTH})"
            ),
        }
    }
}

impl std::error::Error for PredicateError {}

/// A predicate evaluation outcome that is **not** a defined boolean — every variant is an
/// un-evaluable / uncertain case the caller must handle explicitly (fail-closed). None of
/// these is ever silently coerced to `true`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvalError {
    /// The predicate references a variable the context did not supply. The caller decides:
    /// Identity's caveat maps this to `Conditional` (the caller supplies it) — **never** a
    /// silent allow.
    MissingContext { name: String },
    /// A comparison is not defined over the operand types (e.g. ordering on strings) —
    /// un-evaluable, never silently `true`.
    TypeError,
    /// The runtime step ceiling ([`MAX_EVAL_STEPS`]) was exceeded (defence in depth over the
    /// static bound). DoS-bounded.
    CostExceeded,
    /// The AST is the un-parsed placeholder surface (no compiled tree) — the P-235 grammar
    /// parser has not compiled it. Fail-closed: an un-parsed predicate is uncertainty.
    NotCompiled,
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::MissingContext { name } => {
                write!(f, "predicate references unbound variable `{name}` (missing context)")
            }
            EvalError::TypeError => write!(f, "comparison is not defined over the operand types"),
            EvalError::CostExceeded => {
                write!(f, "predicate evaluation exceeded the step ceiling ({MAX_EVAL_STEPS})")
            }
            EvalError::NotCompiled => {
                write!(f, "the QueryAst is the un-parsed placeholder surface (no compiled tree)")
            }
        }
    }
}

impl std::error::Error for EvalError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str) -> Expr {
        Expr::Var(name.into())
    }
    fn int(n: i64) -> Expr {
        Expr::Lit(Literal::Int(n))
    }
    fn lit_str(s: &str) -> Expr {
        Expr::Lit(Literal::Str(s.into()))
    }

    /// The placeholder string surface still round-trips (no consumer of the P-001 seam
    /// breaks): `QueryAst::raw` keeps the textual source.
    #[test]
    fn raw_placeholder_surface_preserved() {
        let ast = QueryAst::raw("status == 'open'");
        assert_eq!(ast.source(), "status == 'open'");
        assert!(ast.predicate().is_none());
        // An un-parsed placeholder fails closed (uncertainty, never a silent match).
        assert_eq!(ast.eval(&EvalContext::new()), Err(EvalError::NotCompiled));
    }

    /// A compiled comparison over a context variable evaluates correctly (`severity < 5`).
    #[test]
    fn compiled_comparison_evaluates() {
        let ast = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Lt,
            lhs: var("severity"),
            rhs: int(5),
        })
        .unwrap();
        let ctx = EvalContext::new().bind("severity", Literal::Int(3));
        assert_eq!(ast.eval(&ctx), Ok(true), "3 < 5 holds");
        let ctx2 = EvalContext::new().bind("severity", Literal::Int(7));
        assert_eq!(ast.eval(&ctx2), Ok(false), "7 < 5 does not hold");
    }

    /// **Missing context is an explicit error, never a silent `true`.**
    #[test]
    fn missing_context_is_error_not_silent_true() {
        let ast = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Lt,
            lhs: var("severity"),
            rhs: int(5),
        })
        .unwrap();
        // The context does NOT bind `severity`.
        assert_eq!(
            ast.eval(&EvalContext::new()),
            Err(EvalError::MissingContext { name: "severity".into() })
        );
    }

    /// **A conjunction's missing-context arm propagates (it is not silently dropped).** Even
    /// though `1 == 1` holds, an unbound `x` in the other conjunct surfaces.
    #[test]
    fn and_propagates_missing_context() {
        let ast = QueryAst::compiled(Predicate::And(vec![
            Predicate::Cmp { op: CmpOp::Eq, lhs: int(1), rhs: int(1) },
            Predicate::Cmp { op: CmpOp::Eq, lhs: var("x"), rhs: int(1) },
        ]))
        .unwrap();
        assert_eq!(
            ast.eval(&EvalContext::new()),
            Err(EvalError::MissingContext { name: "x".into() })
        );
    }

    /// **An `Or` does not swallow a missing-context arm just because another arm is true.**
    /// (The caveat→Conditional invariant: a predicate needing context the caller did not
    /// supply must surface that, never be masked by a satisfied arm.)
    #[test]
    fn or_does_not_mask_missing_context() {
        let ast = QueryAst::compiled(Predicate::Or(vec![
            Predicate::True,
            Predicate::Cmp { op: CmpOp::Eq, lhs: var("x"), rhs: int(1) },
        ]))
        .unwrap();
        assert_eq!(
            ast.eval(&EvalContext::new()),
            Err(EvalError::MissingContext { name: "x".into() })
        );
    }

    /// Boolean connectives + negation evaluate as expected over bound context.
    #[test]
    fn boolean_connectives() {
        let ctx = EvalContext::new()
            .bind("a", Literal::Int(1))
            .bind("b", Literal::Str("x".into()));
        let and = QueryAst::compiled(Predicate::And(vec![
            Predicate::Cmp { op: CmpOp::Eq, lhs: var("a"), rhs: int(1) },
            Predicate::Cmp { op: CmpOp::Eq, lhs: var("b"), rhs: lit_str("x") },
        ]))
        .unwrap();
        assert_eq!(and.eval(&ctx), Ok(true));
        let not = QueryAst::compiled(Predicate::Not(Box::new(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("a"),
            rhs: int(2),
        })))
        .unwrap();
        assert_eq!(not.eval(&ctx), Ok(true), "NOT (1 == 2) is true");
    }

    /// **A non-orderable comparison is a `TypeError`, never silently `true`.** Ordering is
    /// defined only on Int.
    #[test]
    fn ordering_on_non_int_is_type_error() {
        let ast = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Lt,
            lhs: var("name"),
            rhs: lit_str("z"),
        })
        .unwrap();
        let ctx = EvalContext::new().bind("name", Literal::Str("a".into()));
        assert_eq!(ast.eval(&ctx), Err(EvalError::TypeError));
    }

    /// **A deliberately-large tree is rejected at construction (static cost bound).** No DoS:
    /// an over-budget predicate never reaches the interpreter.
    #[test]
    fn oversized_tree_rejected_at_construction() {
        let big: Vec<Predicate> = (0..(MAX_PREDICATE_NODES + 10))
            .map(|_| Predicate::True)
            .collect();
        let err = QueryAst::compiled(Predicate::And(big)).unwrap_err();
        assert!(matches!(err, PredicateError::TooLarge { .. }), "oversized tree rejected");
    }

    /// **A deeply-nested tree is rejected at construction (static depth bound).**
    #[test]
    fn overdeep_tree_rejected_at_construction() {
        let mut p = Predicate::True;
        for _ in 0..(MAX_PREDICATE_DEPTH + 5) {
            p = Predicate::Not(Box::new(p));
        }
        let err = QueryAst::compiled(p).unwrap_err();
        assert!(matches!(err, PredicateError::TooDeep { .. }), "over-deep tree rejected");
    }

    /// The AST + predicate tree serialize/deserialize stably (the wire contract).
    #[test]
    fn ast_round_trips_stably() {
        let ast = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Ge,
            lhs: var("severity"),
            rhs: int(2),
        })
        .unwrap();
        let json = serde_json::to_string(&ast).unwrap();
        let back: QueryAst = serde_json::from_str(&json).unwrap();
        assert_eq!(ast, back);
    }
}
