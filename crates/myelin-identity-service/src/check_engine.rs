use crate::tuple_store::TupleStore;
use myelin_identity::{
    CaveatContext, Consistency, Decision, Literal, Principal, PrincipalStatus, RelName, Zookie,
};
use myelin_query::{CmpOp, EvalContext, EvalError, Expr, Predicate, QueryAst};
use myelin_storage::TenantScope;
use myelin_tenancy::ArtifactRef;
use std::collections::HashMap;

pub const MAX_REWRITE_DEPTH: usize = 16;

pub const USERSET_SEP: char = '#';

#[derive(Clone)]
pub struct CheckEngine {
    tuples: TupleStore,
}

impl CheckEngine {
    pub fn new(tuples: TupleStore) -> CheckEngine {
        CheckEngine { tuples }
    }

    pub fn check(
        &self,
        scope: &TenantScope,
        subject: &Principal,
        permission: &RelName,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&CaveatContext>,
    ) -> Decision {
        if subject.status != PrincipalStatus::Active {
            return Decision::Deny;
        }
        if permission.0.trim().is_empty() {
            return Decision::Deny;
        }
        let object_id = match object_id_of(object) {
            Some(id) => id,
            None => return Decision::Deny,
        };

        let snapshot = self.snapshot_view(scope, &at.at_least);

        let mut memo: HashMap<MemoKey, bool> = HashMap::new();
        let granted = snapshot.has_relation(
            &subject.principal_id.0,
            &permission.0,
            &object_id,
            0,
            &mut memo,
        );

        if !granted {
            return Decision::Deny;
        }

        match caveat {
            None => Decision::Allow,
            Some(cav) => eval_caveat(cav),
        }
    }

    pub fn direct_subjects(
        &self,
        scope: &TenantScope,
        object: &ArtifactRef,
        relation: &RelName,
        at: &Consistency,
    ) -> Vec<String> {
        let object_id = match object_id_of(object) {
            Some(id) => id,
            None => return Vec::new(),
        };
        let snapshot = self.snapshot_view(scope, &at.at_least);
        snapshot
            .by_object
            .get(&object_id)
            .map(|tuples| {
                tuples
                    .iter()
                    .filter(|t| t.relation == relation.0)
                    .map(|t| t.subject.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn snapshot_view(&self, scope: &TenantScope, at_least: &Zookie) -> SnapshotView {
        let mut by_object: HashMap<String, Vec<SnapTuple>> = HashMap::new();
        for st in self.tuples.tuples_in(scope) {
            if !at_least.0.is_empty() && st.zookie.0 > at_least.0 {
                continue;
            }
            let stored_key = myelin_refs::object_key(&ArtifactRef(st.tuple.object.0.clone()))
                .map(|k| k.tuple_key())
                .unwrap_or_else(|| st.tuple.object.0.clone());
            by_object.entry(stored_key).or_default().push(SnapTuple {
                relation: st.tuple.relation.0.clone(),
                subject: st.tuple.subject.0.clone(),
            });
        }
        SnapshotView { by_object }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct MemoKey {
    subject: String,
    relation: String,
    object: String,
}

#[derive(Clone)]
struct SnapTuple {
    relation: String,
    subject: String,
}

struct SnapshotView {
    by_object: HashMap<String, Vec<SnapTuple>>,
}

impl SnapshotView {
    fn has_relation(
        &self,
        subject: &str,
        relation: &str,
        object: &str,
        depth: usize,
        memo: &mut HashMap<MemoKey, bool>,
    ) -> bool {
        if depth >= MAX_REWRITE_DEPTH {
            return false;
        }

        let key = MemoKey {
            subject: subject.to_string(),
            relation: relation.to_string(),
            object: object.to_string(),
        };
        if let Some(&hit) = memo.get(&key) {
            return hit;
        }
        memo.insert(key.clone(), false);

        let mut granted = false;
        if let Some(tuples) = self.by_object.get(object) {
            for t in tuples {
                if t.relation != relation {
                    continue;
                }
                if t.subject == subject {
                    granted = true;
                    break;
                }
                if let Some((obj2, rel2)) = parse_userset(&t.subject) {
                    if self.has_relation(subject, rel2, obj2, depth + 1, memo) {
                        granted = true;
                        break;
                    }
                }
            }
        }

        memo.insert(key, granted);
        granted
    }
}

pub(crate) fn parse_userset(subject: &str) -> Option<(&str, &str)> {
    let (obj, rel) = subject.split_once(USERSET_SEP)?;
    if obj.is_empty() || rel.is_empty() || rel.contains(USERSET_SEP) {
        return None;
    }
    Some((obj, rel))
}

fn object_id_of(object: &ArtifactRef) -> Option<String> {
    myelin_refs::object_key(object).map(|k| k.tuple_key())
}

pub fn eval_caveat_predicate(predicate: &QueryAst, caveat: &CaveatContext) -> Decision {
    let ctx = EvalContext::from_attrs(caveat.attrs.clone());
    match predicate.eval(&ctx) {
        Ok(true) => Decision::Allow,
        Ok(false) => Decision::Deny,
        Err(EvalError::MissingContext { .. }) => Decision::Conditional,
        Err(EvalError::TypeError) | Err(EvalError::NotCompiled) => Decision::Conditional,
        Err(EvalError::CostExceeded) => Decision::Deny,
    }
}

pub fn eval_caveat(caveat: &CaveatContext) -> Decision {
    match lower_legacy_caveat(caveat) {
        Some(predicate) => match QueryAst::compiled(predicate) {
            Ok(ast) => eval_caveat_predicate(&ast, caveat),
            Err(_) => Decision::Conditional,
        },
        None => Decision::Conditional,
    }
}

fn lower_legacy_caveat(caveat: &CaveatContext) -> Option<Predicate> {
    if let Some(b @ Literal::Bool(_)) = caveat.attrs.get("__caveat_bool") {
        return Some(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Lit(b.clone()),
            rhs: Expr::Lit(Literal::Bool(true)),
        });
    }

    let op = match caveat.attrs.get("__caveat_op") {
        Some(Literal::Str(op)) => match op.as_str() {
            "eq" => CmpOp::Eq,
            "ne" => CmpOp::Ne,
            "lt" => CmpOp::Lt,
            "le" => CmpOp::Le,
            "gt" => CmpOp::Gt,
            "ge" => CmpOp::Ge,
            _ => return None,
        },
        _ => return None,
    };
    let lhs = lower_operand(caveat, "__caveat_lhs")?;
    let rhs = lower_operand(caveat, "__caveat_rhs")?;
    Some(Predicate::Cmp { op, lhs, rhs })
}

fn lower_operand(caveat: &CaveatContext, base: &str) -> Option<Expr> {
    if let Some(Literal::Str(var)) = caveat.attrs.get(&format!("{base}_var")) {
        return Some(Expr::Var(var.clone()));
    }
    caveat.attrs.get(base).map(|lit| Expr::Lit(lit.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{OutboxStore, Timestamp};
    use myelin_identity::{
        FieldId, ObjectId, PrincipalId, PrincipalKind, RelationTuple, TupleDelta,
    };
    use myelin_tenancy::{Region, TenantId};
    use std::collections::BTreeMap;

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn subject(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
        TupleDelta::Add(RelationTuple {
            object: ObjectId(object.into()),
            relation: RelName(relation.into()),
            subject: PrincipalId(subject.into()),
            caveat: None,
        })
    }

    fn now() -> Timestamp {
        Timestamp("2026-06-19T00:00:00Z".into())
    }

    fn at(zookie: &str) -> Consistency {
        Consistency {
            at_least: Zookie(zookie.into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        }
    }

    fn latest() -> Consistency {
        at("")
    }

    fn engine_with(scope: &TenantScope, tuples: &[TupleDelta]) -> CheckEngine {
        let store = TupleStore::new(OutboxStore::new());
        let actor = subject("p-admin");
        store
            .write_tuples(scope, &actor, tuples, None, None, now())
            .expect("seed tuples");
        CheckEngine::new(store)
    }

    #[test]
    fn check_direct_grant_allows() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("repo:core", "reader", "p:alice")]);
        let d = eng.check(
            &s,
            &subject("p:alice"),
            &RelName("reader".into()),
            &ArtifactRef("repo:core".into()),
            &latest(),
            None,
        );
        assert_eq!(d, Decision::Allow, "a direct grant allows");
    }

    #[test]
    fn check_missing_grant_denies() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("repo:core", "reader", "p:alice")]);
        assert_eq!(
            eng.check(
                &s,
                &subject("p:bob"),
                &RelName("reader".into()),
                &ArtifactRef("repo:core".into()),
                &latest(),
                None
            ),
            Decision::Deny,
            "no tuple for bob ⇒ deny"
        );
        assert_eq!(
            eng.check(
                &s,
                &subject("p:alice"),
                &RelName("writer".into()),
                &ArtifactRef("repo:core".into()),
                &latest(),
                None
            ),
            Decision::Deny,
            "alice has reader, not writer ⇒ deny"
        );
    }

    #[test]
    fn check_inherited_via_userset_allows() {
        let s = scope("acme");
        let eng = engine_with(
            &s,
            &[
                add("org:acme", "member", "p:alice"),
                add("repo:core", "reader", "org:acme#member"),
            ],
        );
        let d = eng.check(
            &s,
            &subject("p:alice"),
            &RelName("reader".into()),
            &ArtifactRef("repo:core".into()),
            &latest(),
            None,
        );
        assert_eq!(
            d,
            Decision::Allow,
            "alice inherits reader via org membership (userset rewrite)"
        );
        assert_eq!(
            eng.check(
                &s,
                &subject("p:bob"),
                &RelName("reader".into()),
                &ArtifactRef("repo:core".into()),
                &latest(),
                None
            ),
            Decision::Deny,
            "a non-member does not inherit"
        );
    }

    #[test]
    fn check_fail_closed_on_malformed_query() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("repo:core", "reader", "p:alice")]);
        assert_eq!(
            eng.check(
                &s,
                &subject("p:alice"),
                &RelName("reader".into()),
                &ArtifactRef("   ".into()),
                &latest(),
                None
            ),
            Decision::Deny,
            "an unparseable object ref fails closed"
        );
        assert_eq!(
            eng.check(
                &s,
                &subject("p:alice"),
                &RelName("".into()),
                &ArtifactRef("repo:core".into()),
                &latest(),
                None
            ),
            Decision::Deny,
            "an empty permission fails closed"
        );
    }

    #[test]
    fn check_suspended_subject_denied_despite_grant() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("repo:core", "reader", "p:alice")]);
        let mut suspended = subject("p:alice");
        suspended.status = PrincipalStatus::Disabled;
        assert_eq!(
            eng.check(
                &s,
                &suspended,
                &RelName("reader".into()),
                &ArtifactRef("repo:core".into()),
                &latest(),
                None
            ),
            Decision::Deny,
            "a disabled subject is denied despite the grant (ID-D1)"
        );
    }

    #[test]
    fn check_at_older_zookie_does_not_see_newer_tuple() {
        let s = scope("acme");
        let store = TupleStore::new(OutboxStore::new());
        let actor = subject("p-admin");
        let z0 = store.current_zookie();
        let z1 = store
            .write_tuples(
                &s,
                &actor,
                &[add("repo:core", "reader", "p:alice")],
                None,
                None,
                now(),
            )
            .expect("grant");
        let eng = CheckEngine::new(store);

        assert_eq!(
            eng.check(
                &s,
                &subject("p:alice"),
                &RelName("reader".into()),
                &ArtifactRef("repo:core".into()),
                &at(&z0.0),
                None
            ),
            Decision::Deny,
            "a check at the pre-grant zookie does not see the grant written after it"
        );
        assert_eq!(
            eng.check(
                &s,
                &subject("p:alice"),
                &RelName("reader".into()),
                &ArtifactRef("repo:core".into()),
                &at(&z1.0),
                None
            ),
            Decision::Allow,
            "a check at-or-after the grant zookie sees the grant"
        );
    }

    #[test]
    fn check_is_depth_bounded() {
        let s = scope("acme");
        let n = MAX_REWRITE_DEPTH + 4;
        let mut deltas: Vec<TupleDelta> = Vec::new();
        for i in 0..n {
            deltas.push(add(
                &format!("level_{i}"),
                "m",
                &format!("level_{}#m", i + 1),
            ));
        }
        deltas.push(add(&format!("level_{n}"), "m", "p:deep"));
        let eng = engine_with(&s, &deltas);

        let d = eng.check(
            &s,
            &subject("p:deep"),
            &RelName("m".into()),
            &ArtifactRef("level_0".into()),
            &latest(),
            None,
        );
        assert_eq!(
            d,
            Decision::Deny,
            "a chain deeper than the bound fails closed (depth-bounded, never unbounded recursion / never allow-by-exhaustion)"
        );
    }

    #[test]
    fn check_userset_cycle_denies_without_diverging() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("a", "m", "b#m"), add("b", "m", "a#m")]);
        let d = eng.check(
            &s,
            &subject("p:nobody"),
            &RelName("m".into()),
            &ArtifactRef("a".into()),
            &latest(),
            None,
        );
        assert_eq!(
            d,
            Decision::Deny,
            "a userset cycle denies (bounded) rather than diverging"
        );
    }

    #[test]
    fn check_memoises_the_repeated_subproblem() {
        let s = scope("acme");
        let eng = engine_with(
            &s,
            &[
                add("top", "m", "left#m"),
                add("top", "m", "right#m"),
                add("left", "m", "base#m"),
                add("right", "m", "base#m"),
                add("base", "m", "p:alice"),
            ],
        );
        assert_eq!(
            eng.check(
                &s,
                &subject("p:alice"),
                &RelName("m".into()),
                &ArtifactRef("top".into()),
                &latest(),
                None
            ),
            Decision::Allow,
            "the diamond resolves to Allow"
        );

        let view = eng.snapshot_view(&s, &Zookie(String::new()));
        let mut memo: HashMap<MemoKey, bool> = HashMap::new();
        let granted = view.has_relation("p:alice", "m", "top", 0, &mut memo);
        assert!(granted, "the diamond grants");
        let base_key = MemoKey {
            subject: "p:alice".into(),
            relation: "m".into(),
            object: "base".into(),
        };
        assert_eq!(
            memo.get(&base_key),
            Some(&true),
            "the convergent subproblem was memoised (computed once, reused on the second arm)"
        );
    }

    #[test]
    fn check_literal_caveat_gates_correctly() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("issue:PROJ-1", "view_field", "p:alice")]);

        let mut ok = BTreeMap::new();
        ok.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
        ok.insert("__caveat_lhs".to_string(), Literal::Int(3));
        ok.insert("__caveat_rhs".to_string(), Literal::Int(5));
        let cav_ok = CaveatContext {
            object: ArtifactRef("issue:PROJ-1".into()),
            field: Some(FieldId("salary".into())),
            transition: None,
            attrs: ok,
        };
        assert_eq!(
            eng.check(
                &s,
                &subject("p:alice"),
                &RelName("view_field".into()),
                &ArtifactRef("issue:PROJ-1".into()),
                &latest(),
                Some(&cav_ok)
            ),
            Decision::Allow,
            "a satisfied literal caveat keeps the Allow"
        );

        let mut bad = BTreeMap::new();
        bad.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
        bad.insert("__caveat_lhs".to_string(), Literal::Int(7));
        bad.insert("__caveat_rhs".to_string(), Literal::Int(5));
        let cav_bad = CaveatContext {
            object: ArtifactRef("issue:PROJ-1".into()),
            field: Some(FieldId("salary".into())),
            transition: None,
            attrs: bad,
        };
        assert_eq!(
            eng.check(
                &s,
                &subject("p:alice"),
                &RelName("view_field".into()),
                &ArtifactRef("issue:PROJ-1".into()),
                &latest(),
                Some(&cav_bad)
            ),
            Decision::Deny,
            "a violated literal caveat denies (redacts the field)"
        );
    }

    #[test]
    fn check_missing_context_caveat_is_conditional_not_allow() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("issue:PROJ-1", "view_field", "p:alice")]);
        let mut attrs = BTreeMap::new();
        attrs.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
        let cav = CaveatContext {
            object: ArtifactRef("issue:PROJ-1".into()),
            field: Some(FieldId("salary".into())),
            transition: None,
            attrs,
        };
        let d = eng.check(
            &s,
            &subject("p:alice"),
            &RelName("view_field".into()),
            &ArtifactRef("issue:PROJ-1".into()),
            &latest(),
            Some(&cav),
        );
        assert_eq!(
            d,
            Decision::Conditional,
            "a caveat needing missing context is Conditional, NEVER a silent Allow"
        );
        assert_ne!(
            d,
            Decision::Allow,
            "the missing-context branch is mandatory-core: it must not Allow"
        );
    }

    fn field_caveat(object: &str, field: &str, attrs: BTreeMap<String, Literal>) -> CaveatContext {
        CaveatContext {
            object: ArtifactRef(object.into()),
            field: Some(FieldId(field.into())),
            transition: None,
            attrs,
        }
    }

    #[test]
    fn non_literal_field_caveat_redacts_through_query_ast() {
        let predicate = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Lt,
            lhs: Expr::Var("severity".into()),
            rhs: Expr::Var("threshold".into()),
        })
        .unwrap();

        let mut visible = BTreeMap::new();
        visible.insert("severity".to_string(), Literal::Int(3));
        visible.insert("threshold".to_string(), Literal::Int(5));
        let cav_visible = field_caveat("issue:PROJ-1", "salary", visible);
        assert_eq!(
            eval_caveat_predicate(&predicate, &cav_visible),
            Decision::Allow,
            "severity(3) < threshold(5) ⇒ the field is visible (Allow)"
        );

        let mut redacted = BTreeMap::new();
        redacted.insert("severity".to_string(), Literal::Int(7));
        redacted.insert("threshold".to_string(), Literal::Int(5));
        let cav_redacted = field_caveat("issue:PROJ-1", "salary", redacted);
        assert_eq!(
            eval_caveat_predicate(&predicate, &cav_redacted),
            Decision::Deny,
            "severity(7) < threshold(5) is false ⇒ the field is redacted (Deny)"
        );
    }

    #[test]
    fn transition_caveat_gates_through_query_ast() {
        let predicate = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("has_approver".into()),
            rhs: Expr::Lit(Literal::Bool(true)),
        })
        .unwrap();

        let mut approved = BTreeMap::new();
        approved.insert("has_approver".to_string(), Literal::Bool(true));
        let cav_ok = CaveatContext {
            object: ArtifactRef("issue:PROJ-1".into()),
            field: None,
            transition: Some(myelin_identity::TransitionId("close".into())),
            attrs: approved,
        };
        assert_eq!(
            eval_caveat_predicate(&predicate, &cav_ok),
            Decision::Allow,
            "an approver edge permits the transition"
        );

        let mut unapproved = BTreeMap::new();
        unapproved.insert("has_approver".to_string(), Literal::Bool(false));
        let cav_bad = CaveatContext {
            object: ArtifactRef("issue:PROJ-1".into()),
            field: None,
            transition: Some(myelin_identity::TransitionId("close".into())),
            attrs: unapproved,
        };
        assert_eq!(
            eval_caveat_predicate(&predicate, &cav_bad),
            Decision::Deny,
            "no approver edge gates the transition"
        );
    }

    #[test]
    fn promoted_missing_context_is_conditional_not_allow() {
        let predicate = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Lt,
            lhs: Expr::Var("severity".into()),
            rhs: Expr::Lit(Literal::Int(5)),
        })
        .unwrap();
        let cav = field_caveat("issue:PROJ-1", "salary", BTreeMap::new());
        let d = eval_caveat_predicate(&predicate, &cav);
        assert_eq!(
            d,
            Decision::Conditional,
            "a caveat needing missing context is Conditional (the caller supplies it)"
        );
        assert_ne!(
            d,
            Decision::Allow,
            "MANDATORY-CORE: the missing-context branch must NEVER become Allow (mutation-caught)"
        );
    }

    #[test]
    fn promoted_un_evaluable_comparison_is_conditional() {
        let predicate = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Lt,
            lhs: Expr::Var("name".into()),
            rhs: Expr::Lit(Literal::Str("z".into())),
        })
        .unwrap();
        let mut attrs = BTreeMap::new();
        attrs.insert("name".to_string(), Literal::Str("alice".into()));
        let cav = field_caveat("issue:PROJ-1", "salary", attrs);
        assert_eq!(
            eval_caveat_predicate(&predicate, &cav),
            Decision::Conditional,
            "ordering on strings is un-evaluable ⇒ Conditional, never a silent allow"
        );
    }

    #[test]
    fn promoted_predicate_is_cost_bounded() {
        let conjuncts: Vec<Predicate> = (0..(myelin_query::MAX_PREDICATE_NODES / 2))
            .map(|_| Predicate::True)
            .collect();
        let ast = QueryAst::compiled(Predicate::And(conjuncts)).unwrap();
        let cav = field_caveat("issue:PROJ-1", "salary", BTreeMap::new());
        assert_eq!(
            eval_caveat_predicate(&ast, &cav),
            Decision::Allow,
            "a large-but-legal predicate evaluates bounded (no DoS)"
        );

        let oversized: Vec<Predicate> = (0..(myelin_query::MAX_PREDICATE_NODES + 50))
            .map(|_| Predicate::True)
            .collect();
        assert!(
            QueryAst::compiled(Predicate::And(oversized)).is_err(),
            "an adversarial over-budget predicate is rejected at construction (statically cost-bounded)"
        );
    }

    #[test]
    fn caveat_cannot_manufacture_allow_without_a_grant() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("issue:PROJ-1", "view_field", "p:alice")]);
        let mut ok = BTreeMap::new();
        ok.insert("__caveat_bool".to_string(), Literal::Bool(true));
        let cav = CaveatContext {
            object: ArtifactRef("issue:PROJ-1".into()),
            field: None,
            transition: None,
            attrs: ok,
        };
        assert_eq!(
            eng.check(
                &s,
                &subject("p:bob"),
                &RelName("view_field".into()),
                &ArtifactRef("issue:PROJ-1".into()),
                &latest(),
                Some(&cav)
            ),
            Decision::Deny,
            "a satisfied caveat cannot grant access without the underlying relation"
        );
    }

    #[test]
    fn object_id_extracted_from_urn_and_sub_anchor() {
        assert_eq!(
            object_id_of(&ArtifactRef("myelin://acme/issues/issue/PROJ-1".into())),
            Some("issue:PROJ-1".into()),
            "a URN keys type-qualified, never as the bare trailing id"
        );
        assert_eq!(
            object_id_of(&ArtifactRef("repo:core".into())),
            Some("repo:core".into())
        );
        assert_eq!(
            object_id_of(&ArtifactRef("myelin://acme/git/repo/core".into())),
            object_id_of(&ArtifactRef("repo:core".into())),
            "URN and bare spellings of ONE object agree on ONE key"
        );
        assert_eq!(
            object_id_of(&ArtifactRef(
                "myelin://acme/issues/issue/PROJ-1#comment-7".into()
            )),
            Some("issue:PROJ-1".into()),
            "a #sub anchor authorizes at the root object"
        );
        assert_eq!(
            object_id_of(&ArtifactRef("repo:team/app".into())),
            Some("repo:team/app".into()),
            "a namespaced slug (the R2.1a git grammar) is kept whole, never collapsed to `app`"
        );
        assert_eq!(object_id_of(&ArtifactRef("  ".into())), None);
    }

    #[test]
    fn no_cross_type_check() {
        let s = scope("acme");
        let eng = engine_with(
            &s,
            &[
                add("issue:PROJ-1", "reader", "p:alice"),
                add("PROJ-1", "reader", "p:alice"),
            ],
        );
        for repo_spelling in ["myelin://acme/git/repo/PROJ-1", "repo:PROJ-1"] {
            assert_eq!(
                eng.check(
                    &s,
                    &subject("p:alice"),
                    &RelName("reader".into()),
                    &ArtifactRef(repo_spelling.into()),
                    &latest(),
                    None
                ),
                Decision::Deny,
                "a grant on issue:PROJ-1 must not authorize `{repo_spelling}` (cross-type confusion)"
            );
        }
        for issue_spelling in ["issue:PROJ-1", "myelin://acme/issues/issue/PROJ-1"] {
            assert_eq!(
                eng.check(
                    &s,
                    &subject("p:alice"),
                    &RelName("reader".into()),
                    &ArtifactRef(issue_spelling.into()),
                    &latest(),
                    None
                ),
                Decision::Allow,
                "the issue grant authorizes the issue (spelling `{issue_spelling}`)"
            );
        }
    }

    #[test]
    fn grant_and_check_spellings_agree_across_forms() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("repo:core", "reader", "p:alice")]);
        assert_eq!(
            eng.check(
                &s,
                &subject("p:alice"),
                &RelName("reader".into()),
                &ArtifactRef("myelin://acme/git/repo/core".into()),
                &latest(),
                None
            ),
            Decision::Allow,
            "a bare-spelled grant matches a URN-spelled check of the SAME object"
        );
        let eng2 = engine_with(&s, &[add("myelin://acme/git/repo/core", "reader", "p:bob")]);
        assert_eq!(
            eng2.check(
                &s,
                &subject("p:bob"),
                &RelName("reader".into()),
                &ArtifactRef("repo:core".into()),
                &latest(),
                None
            ),
            Decision::Allow,
            "a URN-spelled stored grant matches a bare-spelled check of the SAME object"
        );
    }

    #[test]
    fn namespaced_slug_grant_matches_its_own_check() {
        let s = scope("acme");
        let eng = engine_with(&s, &[add("repo:team/app", "reader", "p:alice")]);
        assert_eq!(
            eng.check(
                &s,
                &subject("p:alice"),
                &RelName("reader".into()),
                &ArtifactRef("repo:team/app".into()),
                &latest(),
                None
            ),
            Decision::Allow,
            "the namespaced-slug grant matches its own check (never collapsed to `app`)"
        );
        assert_eq!(
            eng.check(
                &s,
                &subject("p:alice"),
                &RelName("reader".into()),
                &ArtifactRef("repo:app".into()),
                &latest(),
                None
            ),
            Decision::Deny,
            "the grant on team/app does not alias onto a repo literally named `app`"
        );
    }

    #[test]
    fn no_cross_tenant_check() {
        let acme = scope("acme");
        let globex = scope("globex");
        let store = TupleStore::new(OutboxStore::new());
        store
            .write_tuples(
                &acme,
                &subject("p-admin"),
                &[add("repo:core", "reader", "p:alice")],
                None,
                None,
                now(),
            )
            .expect("acme grant");
        let eng = CheckEngine::new(store);
        assert_eq!(
            eng.check(
                &acme,
                &subject("p:alice"),
                &RelName("reader".into()),
                &ArtifactRef("repo:core".into()),
                &latest(),
                None
            ),
            Decision::Allow
        );
        assert_eq!(
            eng.check(
                &globex,
                &subject("p:alice"),
                &RelName("reader".into()),
                &ArtifactRef("repo:core".into()),
                &latest(),
                None
            ),
            Decision::Deny,
            "a grant in one tenant does not allow a check in another"
        );
    }
}
