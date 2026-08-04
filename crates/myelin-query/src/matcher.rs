use crate::{EvalContext, EvalError, Predicate, QueryAst};
use myelin_events::{EventEnvelope, Visibility};
use myelin_identity::{Literal, ObjectId, ObjectType, SetExpr};
use serde::{Deserialize, Serialize};

pub const MAX_SETEXPR_NODES: usize = 4096;

pub const MAX_SETEXPR_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMatcher {
    object_type: ObjectType,
    predicate: QueryAst,
}

impl EventMatcher {
    pub fn new(object_type: ObjectType, predicate: QueryAst) -> EventMatcher {
        EventMatcher {
            object_type,
            predicate,
        }
    }

    pub fn compile(
        object_type: ObjectType,
        predicate: Predicate,
    ) -> Result<EventMatcher, crate::PredicateError> {
        Ok(EventMatcher {
            object_type,
            predicate: QueryAst::compiled(predicate)?,
        })
    }

    pub fn object_type(&self) -> &ObjectType {
        &self.object_type
    }

    pub fn predicate(&self) -> &QueryAst {
        &self.predicate
    }

    pub fn compile_subject_filter(&self) -> Option<String> {
        let predicate = self.predicate.predicate()?;
        subject_filter_of(predicate)
    }

    pub fn matches(
        &self,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&RelMembership) -> bool,
    ) -> Result<bool, EvalError> {
        let Some(key) = myelin_refs::object_key(&envelope.subject) else {
            return Ok(false);
        };
        if key.object_type.as_deref() != Some(self.object_type.0.as_str()) {
            return Ok(false);
        }
        if let Some(subject_tenant) = &key.tenant {
            if subject_tenant != &envelope.tenant.0 {
                return Ok(false);
            }
        }
        let mut budget = 0usize;
        let bare = ObjectId(key.id.clone());
        let qualified = ObjectId(key.tuple_key());
        let mut visible_here = setexpr_contains(visible, &bare, member_oracle, &mut budget, 0)
            .ok_or(EvalError::CostExceeded)?;
        if !visible_here && qualified != bare {
            visible_here = setexpr_contains(visible, &qualified, member_oracle, &mut budget, 0)
                .ok_or(EvalError::CostExceeded)?;
        }
        if !visible_here {
            return Ok(false);
        }
        let ctx = project_envelope(envelope);
        self.predicate.eval(&ctx)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelMembership {
    InRelation {
        relation: String,
        object_id: ObjectId,
    },
    InTupleSet { index: String, object_id: ObjectId },
}

fn setexpr_contains(
    expr: &SetExpr,
    object_id: &ObjectId,
    member_oracle: &dyn Fn(&RelMembership) -> bool,
    budget: &mut usize,
    depth: usize,
) -> Option<bool> {
    *budget += 1;
    if *budget > MAX_SETEXPR_NODES || depth > MAX_SETEXPR_DEPTH {
        return None;
    }
    let r = match expr {
        SetExpr::All => true,
        SetExpr::None => false,
        SetExpr::Ids(ids) => ids.contains(object_id),
        SetExpr::NotIds(ids) => !ids.contains(object_id),
        SetExpr::InRelation { relation, .. } => member_oracle(&RelMembership::InRelation {
            relation: relation.0.clone(),
            object_id: object_id.clone(),
        }),
        SetExpr::TupleSet { index } => member_oracle(&RelMembership::InTupleSet {
            index: index.0.clone(),
            object_id: object_id.clone(),
        }),
        SetExpr::Union(xs) => {
            let mut any = false;
            for x in xs {
                if setexpr_contains(x, object_id, member_oracle, budget, depth + 1)? {
                    any = true;
                }
            }
            any
        }
        SetExpr::Intersect(xs) => {
            let mut all = true;
            for x in xs {
                if !setexpr_contains(x, object_id, member_oracle, budget, depth + 1)? {
                    all = false;
                }
            }
            all
        }
        SetExpr::Difference(a, b) => {
            let in_a = setexpr_contains(a, object_id, member_oracle, budget, depth + 1)?;
            let in_b = setexpr_contains(b, object_id, member_oracle, budget, depth + 1)?;
            in_a && !in_b
        }
    };
    Some(r)
}

pub fn project_envelope(envelope: &EventEnvelope) -> EvalContext {
    let mut ctx = EvalContext::new()
        .bind("event.id", Literal::Str(envelope.event_id.0.clone()))
        .bind("event.type", Literal::Str(envelope.type_.0.clone()))
        .bind("event.subject", Literal::Str(envelope.subject.0.clone()))
        .bind("event.tenant", Literal::Str(envelope.tenant.0.clone()))
        .bind("event.region", Literal::Str(envelope.region.0.clone()))
        .bind(
            "event.correlation_id",
            Literal::Str(envelope.correlation_id.0.clone()),
        )
        .bind(
            "event.visibility",
            Literal::Str(
                match envelope.visibility {
                    Visibility::Public => "public",
                    Visibility::Internal => "internal",
                    Visibility::Private => "private",
                }
                .to_string(),
            ),
        )
        .bind(
            "event.contains_personal_data",
            Literal::Bool(envelope.contains_personal_data),
        )
        .bind("event.depth", Literal::Int(i64::from(envelope.depth)));

    if let serde_json::Value::Object(map) = &envelope.payload {
        for (key, value) in map {
            if let Some(lit) = scalar_literal(value) {
                ctx = ctx.bind(format!("payload.{key}"), lit);
            }
        }
    }
    ctx
}

fn scalar_literal(value: &serde_json::Value) -> Option<Literal> {
    match value {
        serde_json::Value::Bool(b) => Some(Literal::Bool(*b)),
        serde_json::Value::Number(n) => n.as_i64().map(Literal::Int),
        serde_json::Value::String(s) => Some(Literal::Str(s.clone())),
        _ => None,
    }
}

fn subject_filter_of(predicate: &Predicate) -> Option<String> {
    use crate::{CmpOp, Expr};
    match predicate {
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs,
            rhs,
        } => match (lhs, rhs) {
            (Expr::Var(name), Expr::Lit(Literal::Str(t)))
            | (Expr::Lit(Literal::Str(t)), Expr::Var(name))
                if name == "event.type" =>
            {
                Some(t.clone())
            }
            _ => None,
        },
        Predicate::And(ps) => ps.iter().find_map(subject_filter_of),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CmpOp, Expr};
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventId, EventType, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind, RelName};
    use myelin_tenancy::{Region, TenantId};

    fn var(name: &str) -> Expr {
        Expr::Var(name.into())
    }
    fn str_(s: &str) -> Expr {
        Expr::Lit(Literal::Str(s.into()))
    }
    fn int(n: i64) -> Expr {
        Expr::Lit(Literal::Int(n))
    }

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("alice".into()),
            PrincipalKind::Human,
            TenantId("t1".into()),
        )
    }

    fn envelope(
        type_: &str,
        id: &str,
        visibility: Visibility,
        payload: serde_json::Value,
    ) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01EVENT".into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: TenantId("t1".into()),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            subject: ArtifactRef(format!("myelin://t1/issues/issue/{id}")),
            aggregate: AggregateKey("agg".into()),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            payload,
        }
    }

    fn no_rel(_m: &RelMembership) -> bool {
        false
    }

    #[test]
    fn unviewable_type_returns_zero_matches() {
        let m = EventMatcher::compile(
            ObjectType("issue".into()),
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("event.type"),
                rhs: str_("issues.issue.transitioned"),
            },
        )
        .unwrap();
        let env = envelope(
            "issues.issue.transitioned",
            "issue-1",
            Visibility::Internal,
            serde_json::json!({}),
        );
        assert_eq!(
            m.matches(&env, &SetExpr::None, &no_rel),
            Ok(false),
            "an unviewable object yields 0 matches regardless of the predicate"
        );
        assert_eq!(
            m.matches(&env, &SetExpr::All, &no_rel),
            Ok(true),
            "visible + predicate holds → match"
        );
    }

    #[test]
    fn same_trailing_id_different_type_is_not_matched() {
        let m = EventMatcher::compile(ObjectType("issue".into()), Predicate::True).unwrap();
        let visible = SetExpr::Ids(vec![ObjectId("X-1".into())]);
        let mut repo_env = envelope(
            "git.ref.updated",
            "X-1",
            Visibility::Internal,
            serde_json::json!({}),
        );
        repo_env.subject = ArtifactRef("myelin://t1/git/repo/X-1".into());
        assert_eq!(
            m.matches(&repo_env, &visible, &no_rel),
            Ok(false),
            "an `issue` matcher never matches a `repo` subject, even on a shared trailing id"
        );
        assert_eq!(
            m.matches(&repo_env, &SetExpr::All, &no_rel),
            Ok(false),
            "the type gate is structural - not a property of the visible set"
        );
        let issue_env = envelope(
            "issues.issue.updated",
            "X-1",
            Visibility::Internal,
            serde_json::json!({}),
        );
        assert_eq!(m.matches(&issue_env, &visible, &no_rel), Ok(true));
    }

    #[test]
    fn cross_tenant_subject_is_not_matched() {
        let m = EventMatcher::compile(ObjectType("issue".into()), Predicate::True).unwrap();
        let mut env = envelope(
            "issues.issue.updated",
            "issue-1",
            Visibility::Internal,
            serde_json::json!({}),
        );
        env.subject = ArtifactRef("myelin://t2/issues/issue/issue-1".into());
        assert_eq!(
            m.matches(&env, &SetExpr::All, &no_rel),
            Ok(false),
            "a subject URN naming a foreign tenant never matches (structural 0-leak)"
        );
    }

    #[test]
    fn malformed_subject_is_not_matched() {
        let m = EventMatcher::compile(ObjectType("issue".into()), Predicate::True).unwrap();
        let mut env = envelope(
            "issues.issue.updated",
            "issue-1",
            Visibility::Internal,
            serde_json::json!({}),
        );
        env.subject = ArtifactRef("myelin://t1/issues/issue".into());
        assert_eq!(m.matches(&env, &SetExpr::All, &no_rel), Ok(false));
    }

    #[test]
    fn visible_set_matches_either_spelling_of_the_same_typed_object() {
        let m = EventMatcher::compile(ObjectType("issue".into()), Predicate::True).unwrap();
        let mut env = envelope(
            "issues.issue.updated",
            "PROJ-1",
            Visibility::Internal,
            serde_json::json!({}),
        );
        env.subject = ArtifactRef("myelin://t1/issues/issue/PROJ-1".into());
        let qualified = SetExpr::Ids(vec![ObjectId("issue:PROJ-1".into())]);
        assert_eq!(m.matches(&env, &qualified, &no_rel), Ok(true));
        let bare = SetExpr::Ids(vec![ObjectId("PROJ-1".into())]);
        assert_eq!(m.matches(&env, &bare, &no_rel), Ok(true));
        let wrong_type = SetExpr::Ids(vec![ObjectId("repo:PROJ-1".into())]);
        assert_eq!(m.matches(&env, &wrong_type, &no_rel), Ok(false));
    }

    #[test]
    fn permission_compose_filters_to_visible_ids() {
        let m = EventMatcher::compile(ObjectType("issue".into()), Predicate::True).unwrap();
        let visible = SetExpr::Ids(vec![ObjectId("issue-visible".into())]);
        let seen = envelope(
            "issues.issue.created",
            "issue-visible",
            Visibility::Internal,
            serde_json::json!({}),
        );
        let unseen = envelope(
            "issues.issue.created",
            "issue-hidden",
            Visibility::Internal,
            serde_json::json!({}),
        );
        assert_eq!(m.matches(&seen, &visible, &no_rel), Ok(true));
        assert_eq!(
            m.matches(&unseen, &visible, &no_rel),
            Ok(false),
            "an id outside the visible set never matches (0-leak)"
        );
    }

    #[test]
    fn permission_compose_relational_arm_consults_oracle() {
        let m = EventMatcher::compile(ObjectType("issue".into()), Predicate::True).unwrap();
        let visible = SetExpr::InRelation {
            relation: RelName("reader".into()),
            via_column: myelin_identity::ColRef {
                table: "issue".into(),
                column: "id".into(),
            },
        };
        let env = envelope(
            "issues.issue.created",
            "issue-7",
            Visibility::Internal,
            serde_json::json!({}),
        );
        let reader_of_7 = |mem: &RelMembership| {
            matches!(mem, RelMembership::InRelation { relation, object_id }
                if relation == "reader" && object_id.0 == "issue-7")
        };
        assert_eq!(m.matches(&env, &visible, &reader_of_7), Ok(true));
        let env_other = envelope(
            "issues.issue.created",
            "issue-99",
            Visibility::Internal,
            serde_json::json!({}),
        );
        assert_eq!(m.matches(&env_other, &visible, &reader_of_7), Ok(false));
    }

    #[test]
    fn projection_state_all_blocked_by_resolved() {
        let m = EventMatcher::compile(
            ObjectType("issue".into()),
            Predicate::And(vec![
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var("event.type"),
                    rhs: str_("issues.issue.transitioned"),
                },
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var("payload.blocked_by_unresolved"),
                    rhs: int(0),
                },
            ]),
        )
        .unwrap();
        let resolved = envelope(
            "issues.issue.transitioned",
            "issue-1",
            Visibility::Internal,
            serde_json::json!({ "blocked_by_unresolved": 0 }),
        );
        assert_eq!(m.matches(&resolved, &SetExpr::All, &no_rel), Ok(true));
        let blocked = envelope(
            "issues.issue.transitioned",
            "issue-1",
            Visibility::Internal,
            serde_json::json!({ "blocked_by_unresolved": 2 }),
        );
        assert_eq!(m.matches(&blocked, &SetExpr::All, &no_rel), Ok(false));
    }

    #[test]
    fn oversized_matcher_rejected_at_compile() {
        let big: Vec<Predicate> = (0..(crate::MAX_PREDICATE_NODES + 10))
            .map(|_| Predicate::True)
            .collect();
        let err = EventMatcher::compile(ObjectType("issue".into()), Predicate::And(big))
            .expect_err("an over-budget matcher must be rejected at subscribe-time");
        assert!(matches!(err, crate::PredicateError::TooLarge { .. }));
    }

    #[test]
    fn unprojected_field_fails_closed() {
        let m = EventMatcher::compile(
            ObjectType("issue".into()),
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("payload.staet"),
                rhs: str_("done"),
            },
        )
        .unwrap();
        let env = envelope(
            "issues.issue.transitioned",
            "issue-1",
            Visibility::Internal,
            serde_json::json!({ "state": "done" }),
        );
        assert_eq!(
            m.matches(&env, &SetExpr::All, &no_rel),
            Err(EvalError::MissingContext {
                name: "payload.staet".into()
            })
        );
    }

    #[test]
    fn compiles_to_jetstream_subject_filter() {
        let pinned = EventMatcher::compile(
            ObjectType("issue".into()),
            Predicate::And(vec![
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var("event.type"),
                    rhs: str_("issues.issue.transitioned"),
                },
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var("payload.state"),
                    rhs: str_("done"),
                },
            ]),
        )
        .unwrap();
        assert_eq!(
            pinned.compile_subject_filter(),
            Some("issues.issue.transitioned".to_string()),
            "the event.type == pin lowers to the exact NATS subject"
        );
        let unpinned = EventMatcher::compile(
            ObjectType("issue".into()),
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("payload.state"),
                rhs: str_("done"),
            },
        )
        .unwrap();
        assert_eq!(unpinned.compile_subject_filter(), None);
    }

    #[test]
    fn matcher_predicate_is_byte_identical_queryast() {
        let predicate = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("event.type"),
            rhs: str_("issues.issue.created"),
        })
        .unwrap();
        let m = EventMatcher::new(ObjectType("issue".into()), predicate.clone());

        let matcher_json = serde_json::to_value(&m).unwrap();
        let predicate_in_matcher = &matcher_json["predicate"];
        let bare_json = serde_json::to_value(&predicate).unwrap();
        assert_eq!(
            predicate_in_matcher, &bare_json,
            "the matcher's QueryAst bytes are byte-identical with the bare QueryAst (no drift)"
        );

        let back: QueryAst = serde_json::from_value(predicate_in_matcher.clone()).unwrap();
        assert_eq!(back, predicate);
    }

    #[test]
    fn matcher_round_trips_stably() {
        let m = EventMatcher::compile(
            ObjectType("pr".into()),
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("event.type"),
                rhs: str_("git.pull_request.opened"),
            },
        )
        .unwrap();
        let json = serde_json::to_string(&m).unwrap();
        let back: EventMatcher = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn setexpr_membership_is_bounded() {
        let mut expr = SetExpr::All;
        for _ in 0..(MAX_SETEXPR_DEPTH + 16) {
            expr = SetExpr::Union(vec![expr]);
        }
        let m = EventMatcher::compile(ObjectType("issue".into()), Predicate::True).unwrap();
        let env = envelope(
            "issues.issue.created",
            "issue-1",
            Visibility::Internal,
            serde_json::json!({}),
        );
        assert_eq!(
            m.matches(&env, &expr, &no_rel),
            Err(EvalError::CostExceeded),
            "an over-budget SetExpr nest is bounded, never a DoS"
        );
    }
}
