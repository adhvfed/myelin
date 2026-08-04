use myelin_identity::Literal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod field;
pub use field::{
    FieldType, FieldValue, Jitter, OrderKey, LEXORANK_ALPHABET, LEXORANK_FIRST,
    LEXORANK_JITTER_LEN, LEXORANK_REBALANCE_LEN,
};

pub mod order_key;
pub use order_key::{tiebreak, ConformanceStep, RankOp, CONFORMANCE_VECTOR};

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

pub const MAX_PREDICATE_NODES: usize = 256;

pub const MAX_PREDICATE_DEPTH: usize = 32;

pub const MAX_EVAL_STEPS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryAst {
    predicate: Option<Predicate>,
    raw: String,
}

impl QueryAst {
    pub fn compiled(predicate: Predicate) -> Result<QueryAst, PredicateError> {
        Self::validate(&predicate)?;
        Ok(QueryAst {
            raw: String::new(),
            predicate: Some(predicate),
        })
    }

    pub fn compiled_with_source(predicate: Predicate, source: impl Into<String>) -> QueryAst {
        QueryAst {
            raw: source.into(),
            predicate: Some(predicate),
        }
    }

    pub fn raw(text: impl Into<String>) -> QueryAst {
        QueryAst {
            raw: text.into(),
            predicate: None,
        }
    }

    pub fn source(&self) -> &str {
        &self.raw
    }

    pub fn predicate(&self) -> Option<&Predicate> {
        self.predicate.as_ref()
    }

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

    pub fn eval(&self, ctx: &EvalContext) -> Result<bool, EvalError> {
        let predicate = self.predicate.as_ref().ok_or(EvalError::NotCompiled)?;
        let mut steps = 0usize;
        predicate.eval(ctx, &mut steps)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvalContext {
    bindings: BTreeMap<String, Literal>,
}

impl EvalContext {
    pub fn new() -> EvalContext {
        EvalContext {
            bindings: BTreeMap::new(),
        }
    }

    pub fn from_attrs(attrs: BTreeMap<String, Literal>) -> EvalContext {
        EvalContext { bindings: attrs }
    }

    pub fn bind(mut self, name: impl Into<String>, value: Literal) -> EvalContext {
        self.bindings.insert(name.into(), value);
        self
    }

    fn get(&self, name: &str) -> Option<&Literal> {
        self.bindings.get(name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Predicate {
    True,
    False,
    Cmp { op: CmpOp, lhs: Expr, rhs: Expr },
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Not(Box<Predicate>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    Lit(Literal),
    Var(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Predicate {
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

    fn depth(&self) -> usize {
        match self {
            Predicate::True | Predicate::False | Predicate::Cmp { .. } => 1,
            Predicate::And(ps) | Predicate::Or(ps) => {
                1 + ps.iter().map(Predicate::depth).max().unwrap_or(0)
            }
            Predicate::Not(p) => 1 + p.depth(),
        }
    }

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
                for p in ps {
                    if !p.eval(ctx, steps)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Predicate::Or(ps) => {
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
    fn apply(self, lhs: &Literal, rhs: &Literal) -> Result<bool, EvalError> {
        match self {
            CmpOp::Eq => literals_eq(lhs, rhs).ok_or(EvalError::TypeError),
            CmpOp::Ne => literals_eq(lhs, rhs)
                .map(|b| !b)
                .ok_or(EvalError::TypeError),
            CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => match (lhs, rhs) {
                (Literal::Int(a), Literal::Int(b)) => Ok(match self {
                    CmpOp::Lt => a < b,
                    CmpOp::Le => a <= b,
                    CmpOp::Gt => a > b,
                    CmpOp::Ge => a >= b,
                    _ => unreachable!("the match arm constrains the op set to the orderings"),
                }),
                _ => Err(EvalError::TypeError),
            },
        }
    }
}

fn literals_eq(lhs: &Literal, rhs: &Literal) -> Option<bool> {
    match (lhs, rhs) {
        (Literal::Bool(a), Literal::Bool(b)) => Some(a == b),
        (Literal::Int(a), Literal::Int(b)) => Some(a == b),
        (Literal::Str(a), Literal::Str(b)) => Some(a == b),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PredicateError {
    TooLarge { nodes: usize },
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvalError {
    MissingContext { name: String },
    TypeError,
    CostExceeded,
    NotCompiled,
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::MissingContext { name } => {
                write!(
                    f,
                    "predicate references unbound variable `{name}` (missing context)"
                )
            }
            EvalError::TypeError => write!(f, "comparison is not defined over the operand types"),
            EvalError::CostExceeded => {
                write!(
                    f,
                    "predicate evaluation exceeded the step ceiling ({MAX_EVAL_STEPS})"
                )
            }
            EvalError::NotCompiled => {
                write!(
                    f,
                    "the QueryAst is the un-parsed placeholder surface (no compiled tree)"
                )
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

    #[test]
    fn raw_placeholder_surface_preserved() {
        let ast = QueryAst::raw("status == 'open'");
        assert_eq!(ast.source(), "status == 'open'");
        assert!(ast.predicate().is_none());
        assert_eq!(ast.eval(&EvalContext::new()), Err(EvalError::NotCompiled));
    }

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

    #[test]
    fn missing_context_is_error_not_silent_true() {
        let ast = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Lt,
            lhs: var("severity"),
            rhs: int(5),
        })
        .unwrap();
        assert_eq!(
            ast.eval(&EvalContext::new()),
            Err(EvalError::MissingContext {
                name: "severity".into()
            })
        );
    }

    #[test]
    fn and_propagates_missing_context() {
        let ast = QueryAst::compiled(Predicate::And(vec![
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: int(1),
                rhs: int(1),
            },
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("x"),
                rhs: int(1),
            },
        ]))
        .unwrap();
        assert_eq!(
            ast.eval(&EvalContext::new()),
            Err(EvalError::MissingContext { name: "x".into() })
        );
    }

    #[test]
    fn or_does_not_mask_missing_context() {
        let ast = QueryAst::compiled(Predicate::Or(vec![
            Predicate::True,
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("x"),
                rhs: int(1),
            },
        ]))
        .unwrap();
        assert_eq!(
            ast.eval(&EvalContext::new()),
            Err(EvalError::MissingContext { name: "x".into() })
        );
    }

    #[test]
    fn boolean_connectives() {
        let ctx = EvalContext::new()
            .bind("a", Literal::Int(1))
            .bind("b", Literal::Str("x".into()));
        let and = QueryAst::compiled(Predicate::And(vec![
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("a"),
                rhs: int(1),
            },
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("b"),
                rhs: lit_str("x"),
            },
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

    #[test]
    fn oversized_tree_rejected_at_construction() {
        let big: Vec<Predicate> = (0..(MAX_PREDICATE_NODES + 10))
            .map(|_| Predicate::True)
            .collect();
        let err = QueryAst::compiled(Predicate::And(big)).unwrap_err();
        assert!(
            matches!(err, PredicateError::TooLarge { .. }),
            "oversized tree rejected"
        );
    }

    #[test]
    fn overdeep_tree_rejected_at_construction() {
        let mut p = Predicate::True;
        for _ in 0..(MAX_PREDICATE_DEPTH + 5) {
            p = Predicate::Not(Box::new(p));
        }
        let err = QueryAst::compiled(p).unwrap_err();
        assert!(
            matches!(err, PredicateError::TooDeep { .. }),
            "over-deep tree rejected"
        );
    }

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
