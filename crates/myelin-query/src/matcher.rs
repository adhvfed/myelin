//! # `matcher` — the `EventMatcher` = the frozen `myelin-query` `QueryAst`, bounded +
//! permission-aware (contract 3.4; Bus §4.5; P-137 / EB-17)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §4.5 (the `EventMatcher`
//! = the frozen `QueryAst`: the bounded interpreter, no UDFs/loops/recursion, statically
//! cost-bounded, side-effect-free; permission-aware **by construction**; one grammar, the
//! JetStream subject-filter + bounded-interpreter compile targets). Contract-index rows
//! **3.4** (`EventMatcher` = the frozen `QueryAst`, owned), **13.3** (`myelin-query`
//! primitive, frozen byte-identical — the matcher IS the predicate core), **4.3**
//! (`list_objects` `SetExpr` push-down — consumed for permission-awareness).
//!
//! ## Why the matcher lives in `myelin-query`, not `myelin-events` (DOCUMENTED DEVIATION)
//! The EB-17 prompt's DELIVERABLE field says "In `myelin-events`: `matcher.rs`". That is
//! **genuinely unworkable against the frozen crate DAG** (architecture §2.9): the predicate
//! ENGINE — [`QueryAst`](crate::QueryAst) / [`Predicate`](crate::Predicate) — was promoted
//! into `myelin-query` by **P-133 (P-ID-22)** (Identity's `CaveatContext` evaluator is the
//! first real consumer), and `myelin-query` **depends on `myelin-events`** (`Cargo.toml`,
//! §2.9: `…-events → …-query`). Putting the matcher in `myelin-events` would require
//! `…-events → …-query` for the engine, forming the cycle the `no-cross-sync-cycle` lint
//! (E-5) and the events `Cargo.toml` ("MUST NOT depend on -query … such an edge MUST NOT
//! compile") forbid. The EI-01 §7 invariant is **one predicate engine, no second copy** —
//! so the `EventMatcher` is built HERE, ON TOP of the one engine, composing over the
//! `myelin_events::EventEnvelope` (upstream) and the `myelin_identity::SetExpr` push-down
//! (upstream). The Bus consumers (Signals/Automations/Triggers, EB-18/19/20) reference
//! `myelin_query::EventMatcher`. This deviation is recorded here and in the P-137 report,
//! per external-insights/01 §1 (do the right thing; document the deviation).
//!
//! ## What this module adds (it does NOT re-define the engine)
//! - [`EventMatcher`] — a [`QueryAst`](crate::QueryAst) predicate **plus** the
//!   [`ObjectType`] it selects over (so permission composition knows the type), with the
//!   `subscribe`-time compile path.
//! - [`project_envelope`] — the canonical projection of an [`EventEnvelope`] (+ its payload)
//!   into the [`EvalContext`](crate::EvalContext) variable namespace the bounded interpreter
//!   reads (`event.type`, `event.subject`, `event.visibility`, `event.tenant`, `event.id`,
//!   and the flat scalar `payload.*` fields). The `Has`/`Ref` projection-state predicates
//!   ("all `blocked_by` resolved") evaluate against these supplied bindings.
//! - [`EventMatcher::compile_subject_filter`] — compile target (a): the cheap server-side
//!   **JetStream subject filter** where the predicate pins `event.type` by equality/prefix;
//!   the residual predicate falls through to compile target (b), the bounded interpreter.
//! - [`EventMatcher::matches`] — permission-aware **by construction**: it composes the
//!   bounded interpreter result with `list_objects(viewer, read, type)` (the frozen
//!   [`SetExpr`] push-down, 4.3), so a matcher over a type/object the viewer cannot see
//!   returns **zero matches** (the 0-leak property) — the predicate is NEVER consulted for
//!   an unviewable object.

use crate::{EvalContext, EvalError, Predicate, QueryAst};
use myelin_events::{ArtifactRef, EventEnvelope, Visibility};
use myelin_identity::{Literal, ObjectId, ObjectType, SetExpr};
use serde::{Deserialize, Serialize};

/// The maximum number of `SetExpr` nodes a permission-composition pass will walk before
/// aborting (the same defence-in-depth posture as [`crate::MAX_EVAL_STEPS`]: the push-down
/// algebra is finite and tenant-bounded, but the membership walker never trusts that — a
/// pathological `Union`/`Intersect` nest is bounded here too). Generous for legitimate
/// authz reverse-index expansions (which are wide, not deep).
pub const MAX_SETEXPR_NODES: usize = 4096;

/// The maximum **nesting depth** the `SetExpr` membership walk will recurse before aborting.
/// This is the structural stack guard (distinct from the node-count budget): the recursive
/// walker charges one stack frame per `Union`/`Intersect`/`Difference` level, so a deep nest
/// is bounded BELOW the OS stack limit — a pathological `Union(Union(Union(…)))` aborts with
/// [`EvalError::CostExceeded`] long before it can overflow the stack. Legitimate authz
/// expansions are wide (many ids in one `Union`), never deep.
pub const MAX_SETEXPR_DEPTH: usize = 64;

/// The `EventMatcher` (contract 3.4) — **the predicate core of the frozen
/// [`QueryAst`](crate::QueryAst)**, carried alongside the [`ObjectType`] it selects over so
/// the matcher composes with `list_objects(viewer, read, type)` for permission-awareness.
///
/// This is NOT a second predicate language: the boolean logic, the operators, the static
/// cost bound, and the side-effect-free bounded interpreter all come from the one
/// [`QueryAst`]. The matcher only adds (a) the object-type it filters, (b) the
/// envelope→context projection, (c) the JetStream subject-filter compile path, and (d) the
/// permission compose.
///
/// **Serialisation is byte-identical with the bare [`QueryAst`]'s** (the no-drift property,
/// X-3/13.3): the `predicate` field IS a `QueryAst`, serialised exactly as the saved-view /
/// Notif-prefs / Search consumers serialise it — see [`tests::matcher_predicate_is_byte_identical_queryast`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMatcher {
    /// The object type this matcher selects (e.g. `"issue"`, `"pr"`, `"run"`). The
    /// permission compose calls `list_objects(viewer, read, <this type>)`; an event whose
    /// subject is not of this type, or not in the viewer's visible set for it, never matches.
    object_type: ObjectType,
    /// The bounded predicate (the frozen [`QueryAst`]) evaluated over the projected envelope.
    /// Validated against the static cost bounds at construction (an over-budget AST is
    /// rejected, never matched — the DoS-hardening property, §4.5).
    predicate: QueryAst,
}

impl EventMatcher {
    /// Build a matcher from the object type it selects and a **pre-validated**
    /// [`QueryAst`] (the AST's own constructor already enforced the static cost bounds, so an
    /// over-budget tree could never have produced this `QueryAst`). This is the
    /// `subscribe`-time entry point the Bus consumers call.
    pub fn new(object_type: ObjectType, predicate: QueryAst) -> EventMatcher {
        EventMatcher {
            object_type,
            predicate,
        }
    }

    /// Build a matcher directly from a [`Predicate`] tree, validating it against the static
    /// cost bounds **here** (the over-budget AST is rejected at `subscribe`-time, never
    /// evaluated — the GATE (a) DoS-hardening property). Returns
    /// [`PredicateError`](crate::PredicateError) on an over-budget tree.
    pub fn compile(
        object_type: ObjectType,
        predicate: Predicate,
    ) -> Result<EventMatcher, crate::PredicateError> {
        Ok(EventMatcher {
            object_type,
            predicate: QueryAst::compiled(predicate)?,
        })
    }

    /// The object type this matcher selects (the type fed to `list_objects`).
    pub fn object_type(&self) -> &ObjectType {
        &self.object_type
    }

    /// The underlying [`QueryAst`] predicate (the one engine; for the saved-view / Search
    /// compile targets that share it).
    pub fn predicate(&self) -> &QueryAst {
        &self.predicate
    }

    /// **Compile target (a): the JetStream subject filter** (Bus §4.5 — the cheap
    /// server-side prefilter). Where the predicate pins `event.type` (an `==` on the
    /// `event.type` variable, possibly buried in a top-level `And`), we lower it to the
    /// dotted NATS subject token so the broker drops non-matching events before they reach
    /// the bounded interpreter. The residual predicate still runs in target (b); the subject
    /// filter is a **conservative over-approximation** (it never excludes an event the
    /// predicate would match — only narrows the firehose).
    ///
    /// Returns `Some(subject)` (e.g. `"issues.issue.transitioned"`) when an `event.type ==`
    /// equality is present at the top level; otherwise `None` (subscribe to the wildcard and
    /// let the bounded interpreter do all the work). A `starts_with` on `event.type` lowers
    /// to a `<prefix>.>` wildcard subject.
    pub fn compile_subject_filter(&self) -> Option<String> {
        let predicate = self.predicate.predicate()?;
        subject_filter_of(predicate)
    }

    /// **The match decision — permission-aware BY CONSTRUCTION (the 0-leak property).**
    ///
    /// `visible` is the [`SetExpr`] result of `list_objects(viewer, read,
    /// self.object_type)` (contract 4.3) — the leak-free pre-filter of the object ids this
    /// viewer may read. `member_oracle` answers the relational `SetExpr` arms
    /// (`InRelation` / `TupleSet`) for the candidate object id (the consumer's authz
    /// reverse-index lookup; for the in-process / unit path it is an explicit closure).
    ///
    /// The order is the load-bearing invariant: **we test visibility FIRST**. If the
    /// envelope's subject object id is NOT in the viewer's visible set, we return
    /// `Ok(false)` **without ever consulting the predicate** — a matcher can never select an
    /// artifact the subject cannot see, no matter what the predicate says. Only for a
    /// visible object do we project the envelope and run the bounded interpreter.
    ///
    /// Returns:
    /// - `Ok(true)` — visible AND the predicate holds.
    /// - `Ok(false)` — not visible (0-leak) OR the predicate is defined-and-false.
    /// - `Err(EvalError)` — the predicate is un-evaluable over this envelope (missing a
    ///   projected field, a type error, the step ceiling, or an un-parsed placeholder). The
    ///   caller (the Bus consumer) maps this to "no match", never a silent match — but it is
    ///   surfaced, not swallowed, so a mis-authored matcher is observable.
    pub fn matches(
        &self,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&RelMembership) -> bool,
    ) -> Result<bool, EvalError> {
        // Permission compose FIRST (the 0-leak invariant). The candidate object is the
        // envelope's subject; its id is the last path segment of the ArtifactRef.
        let object_id = subject_object_id(&envelope.subject);
        let mut budget = 0usize;
        let visible_here = setexpr_contains(visible, &object_id, member_oracle, &mut budget, 0)
            .ok_or(EvalError::CostExceeded)?;
        if !visible_here {
            // Not in the viewer's read set → zero matches, predicate NEVER consulted.
            return Ok(false);
        }
        // Visible: now (and only now) run the bounded interpreter over the projection.
        let ctx = project_envelope(envelope);
        self.predicate.eval(&ctx)
    }
}

/// A relational-membership question the `member_oracle` answers for a `SetExpr::InRelation`
/// / `SetExpr::TupleSet` arm — "is `object_id` in `relation`/`index` for the viewer?" The
/// consumer resolves it against its per-tenant authz reverse index (the S8 JOIN target,
/// 4.3); the in-process path supplies a closure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelMembership {
    /// `SetExpr::InRelation { relation, .. }` — is the object the object of `relation` for
    /// the viewer?
    InRelation {
        relation: String,
        object_id: ObjectId,
    },
    /// `SetExpr::TupleSet { index }` — is the object in the server-materialised tuple set?
    InTupleSet {
        index: String,
        object_id: ObjectId,
    },
}

/// Extract the object id (the last `/`-segment, sans any `#sub` anchor) from an
/// `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]` [`ArtifactRef`]. This is the id space
/// `list_objects` ranges over (the consumer's own id column, 4.3).
fn subject_object_id(subject: &ArtifactRef) -> ObjectId {
    let s = &subject.0;
    let no_anchor = s.split('#').next().unwrap_or(s);
    let id = no_anchor.rsplit('/').next().unwrap_or(no_anchor);
    ObjectId(id.to_string())
}

/// **Bounded membership over the frozen [`SetExpr`] algebra** (contract 4.3) — does the
/// viewer's visible set contain `object_id`? Returns `Some(bool)` (the membership) or `None`
/// if the walk exceeds [`MAX_SETEXPR_NODES`] (the DoS guard — a pathological nest is bounded
/// exactly like the predicate interpreter). The relational arms defer to `member_oracle`.
fn setexpr_contains(
    expr: &SetExpr,
    object_id: &ObjectId,
    member_oracle: &dyn Fn(&RelMembership) -> bool,
    budget: &mut usize,
    depth: usize,
) -> Option<bool> {
    *budget += 1;
    // Two guards: the node-count budget (total work) AND the nesting depth (stack frames).
    // Either ceiling aborts → CostExceeded, never a stack overflow.
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

/// **The canonical envelope projection** — flatten an [`EventEnvelope`] (and its payload)
/// into the variable namespace the bounded interpreter reads. This is the bridge between the
/// frozen envelope (the names/units authority, §2.10) and the one predicate engine: a
/// matcher predicate references `Expr::Var("event.type")`, `Expr::Var("payload.state")`,
/// etc., and this function binds them.
///
/// Bound variables:
/// - `event.id`, `event.type`, `event.subject`, `event.tenant`, `event.region`,
///   `event.correlation_id` — the dotted envelope identifiers (as `Literal::Str`).
/// - `event.visibility` — `"public" | "internal" | "private"`.
/// - `event.contains_personal_data` — `Literal::Bool`.
/// - `event.depth` — `Literal::Int`.
/// - `payload.<key>` — every **scalar** top-level payload field (`string` / `integer` /
///   `bool`), so `Has`/`Ref` projection-state conditions ("all `blocked_by` resolved",
///   modelled by the emitter as a flat `payload.blocked_by_unresolved = 0` projection field)
///   evaluate against supplied state, never against a join the matcher executes (§4.5: the
///   relational condition is a membership test over projection state).
///
/// A field the predicate references but the projection does not bind surfaces as
/// [`EvalError::MissingContext`] from the interpreter — never a silent match.
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

    // Flatten the scalar payload fields (references-not-payloads: the payload carries
    // ids/refs/projection scalars, never PII bodies). Only scalar leaves are projected; a
    // nested object/array is NOT walked (no field-walk surface — that would be a second,
    // unbounded predicate language, §4.5). A predicate over an un-projected nested field
    // therefore fails closed with MissingContext.
    if let serde_json::Value::Object(map) = &envelope.payload {
        for (key, value) in map {
            if let Some(lit) = scalar_literal(value) {
                ctx = ctx.bind(format!("payload.{key}"), lit);
            }
        }
    }
    ctx
}

/// Convert a scalar JSON leaf into a [`Literal`] (the bounded interpreter's value space:
/// `bool | i64 | String`). Non-scalar (object/array/null) and non-integral numbers return
/// `None` (not projected → a predicate over them fails closed).
fn scalar_literal(value: &serde_json::Value) -> Option<Literal> {
    match value {
        serde_json::Value::Bool(b) => Some(Literal::Bool(*b)),
        serde_json::Value::Number(n) => n.as_i64().map(Literal::Int),
        serde_json::Value::String(s) => Some(Literal::Str(s.clone())),
        _ => None,
    }
}

/// Lower a predicate to a NATS subject filter where it pins `event.type` (compile target a).
/// Walks a top-level `And` for an `event.type ==` equality (→ exact subject) or
/// `starts_with` (→ `<prefix>.>` wildcard). Returns the FIRST such pin found (a conservative
/// over-approximation — if several conjuncts pin the type, any one is a valid narrowing
/// filter; the residual predicate runs in target b regardless). `None` ⇒ no type pin ⇒
/// subscribe to the wildcard.
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
        // A top-level conjunction: the FIRST conjunct that pins the type is a valid
        // narrowing filter (a conservative over-approximation — the residual conjuncts run in
        // the bounded interpreter regardless, so any single type pin is sound).
        Predicate::And(ps) => ps.iter().find_map(subject_filter_of),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CmpOp, Expr};
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp,
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

    /// Build an envelope for `subject = myelin://t1/issues/issue/<id>` of the given type,
    /// visibility, and flat payload.
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

    /// The "see everything" oracle — no relational arm is exercised by the simple tests.
    fn no_rel(_m: &RelMembership) -> bool {
        false
    }

    /// **GATE (c) / 0-leak: a matcher over a type the viewer can't see returns ZERO matches,
    /// even when the predicate would otherwise hold.** Visibility (`SetExpr::None`) is tested
    /// BEFORE the predicate; the predicate is never consulted.
    #[test]
    fn unviewable_type_returns_zero_matches() {
        let m = EventMatcher::compile(
            ObjectType("issue".into()),
            // A predicate that WOULD match (event.type == the event's type).
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
        // The viewer can read NOTHING of this type (list_objects → None).
        assert_eq!(
            m.matches(&env, &SetExpr::None, &no_rel),
            Ok(false),
            "an unviewable object yields 0 matches regardless of the predicate"
        );
        // Sanity: the SAME predicate over the SAME event matches once visible.
        assert_eq!(
            m.matches(&env, &SetExpr::All, &no_rel),
            Ok(true),
            "visible + predicate holds → match"
        );
    }

    /// **0-leak with an explicit allow-set: only the visible id matches.**
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

    /// **The relational `SetExpr` arm (`InRelation`) defers to the member oracle** — the
    /// projection-state / reverse-index lookup. Models "viewer is a `reader` of the issue".
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
        // A different id the viewer is NOT a reader of → no match.
        let env_other = envelope(
            "issues.issue.created",
            "issue-99",
            Visibility::Internal,
            serde_json::json!({}),
        );
        assert_eq!(m.matches(&env_other, &visible, &reader_of_7), Ok(false));
    }

    /// **A `Has`/`Ref` projection-state predicate evaluates correctly: "all `blocked_by`
    /// resolved".** The emitter projects the relational condition as a flat scalar
    /// (`payload.blocked_by_unresolved = 0`); the matcher tests membership over that
    /// projection state — NOT a join it executes (§4.5).
    #[test]
    fn projection_state_all_blocked_by_resolved() {
        // condition: event.type == issue.transitioned AND payload.blocked_by_unresolved == 0
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
        // All blockers resolved (count 0) → match.
        let resolved = envelope(
            "issues.issue.transitioned",
            "issue-1",
            Visibility::Internal,
            serde_json::json!({ "blocked_by_unresolved": 0 }),
        );
        assert_eq!(m.matches(&resolved, &SetExpr::All, &no_rel), Ok(true));
        // Two blockers still open → no match.
        let blocked = envelope(
            "issues.issue.transitioned",
            "issue-1",
            Visibility::Internal,
            serde_json::json!({ "blocked_by_unresolved": 2 }),
        );
        assert_eq!(m.matches(&blocked, &SetExpr::All, &no_rel), Ok(false));
    }

    /// **The cost validator rejects an over-budget AST at `compile` (subscribe-time)** — the
    /// GATE (a) DoS-hardening property. The over-budget matcher is never constructed, so it
    /// can never be evaluated.
    #[test]
    fn oversized_matcher_rejected_at_compile() {
        let big: Vec<Predicate> = (0..(crate::MAX_PREDICATE_NODES + 10))
            .map(|_| Predicate::True)
            .collect();
        let err = EventMatcher::compile(ObjectType("issue".into()), Predicate::And(big))
            .expect_err("an over-budget matcher must be rejected at subscribe-time");
        assert!(matches!(err, crate::PredicateError::TooLarge { .. }));
    }

    /// **A predicate field the projection does not bind fails closed (MissingContext), never
    /// a silent match.** (e.g. a typo'd `payload.staet`.)
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
        // Visible, but the predicate references an unbound field → surfaced, not silent.
        assert_eq!(
            m.matches(&env, &SetExpr::All, &no_rel),
            Err(EvalError::MissingContext {
                name: "payload.staet".into()
            })
        );
    }

    /// **Compile target (a): the JetStream subject filter** — an `event.type ==` pin lowers
    /// to the exact dotted subject; a bare predicate yields `None` (wildcard subscribe).
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
        // No type pin → wildcard subscribe (the bounded interpreter does all the work).
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

    /// **Byte-identical serialisation (the no-drift property, 13.3 / X-3).** The matcher's
    /// `predicate` field IS a [`QueryAst`]; the bytes of that field, deserialised by the bare
    /// `QueryAst` path (the saved-view / Notif-prefs / Search consumers' path), round-trip
    /// equal. There is no second serialisation — one grammar, byte-identical.
    #[test]
    fn matcher_predicate_is_byte_identical_queryast() {
        let predicate = QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("event.type"),
            rhs: str_("issues.issue.created"),
        })
        .unwrap();
        let m = EventMatcher::new(ObjectType("issue".into()), predicate.clone());

        // The matcher serialises its predicate exactly as the bare QueryAst serialises.
        let matcher_json = serde_json::to_value(&m).unwrap();
        let predicate_in_matcher = &matcher_json["predicate"];
        let bare_json = serde_json::to_value(&predicate).unwrap();
        assert_eq!(
            predicate_in_matcher, &bare_json,
            "the matcher's QueryAst bytes are byte-identical with the bare QueryAst (no drift)"
        );

        // And the field deserialises back to an equal QueryAst (the cross-consumer
        // round-trip — saved-views/Search/Notif read the SAME bytes).
        let back: QueryAst = serde_json::from_value(predicate_in_matcher.clone()).unwrap();
        assert_eq!(back, predicate);
    }

    /// **The whole matcher round-trips stably (the wire contract).**
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

    /// **The `SetExpr` membership walk is itself bounded** — a pathological deep nest aborts
    /// (the matcher reports CostExceeded rather than blowing the stack/looping). Defence in
    /// depth over the tenant-bounded algebra: the depth guard fires before the recursion can
    /// overflow the stack.
    #[test]
    fn setexpr_membership_is_bounded() {
        // Build a Union nest deeper than the depth ceiling (modest absolute depth so the
        // fixture itself does not overflow on construction/drop — the point is that the
        // WALK aborts at MAX_SETEXPR_DEPTH, not that the OS stack is the limit).
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
