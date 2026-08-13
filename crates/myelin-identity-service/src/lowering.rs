use crate::check_engine::CheckEngine;
use crate::namespace::NamespaceEngine;
use crate::reverse_index::{ReverseIndex, S8_TABLE};
use myelin_identity::{
    ColRef, Consistency, Decision, ObjectId, ObjectType, Permission, Principal, SetExpr, Zookie,
};
use myelin_storage::TenantScope;
use myelin_tenancy::ArtifactRef;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundParam {
    pub placeholder: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthzJoin {
    pub alias: String,
    pub clause: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lowered {
    pub sql_predicate: String,
    pub joins: Vec<AuthzJoin>,
    pub params: Vec<BoundParam>,
}

impl Lowered {
    pub fn depends_on_reverse_index(&self) -> bool {
        !self.joins.is_empty()
    }
}

struct LowerCtx<'a> {
    subject: &'a str,
    via_sql: String,
    joins: Vec<AuthzJoin>,
    params: Vec<BoundParam>,
    next_id: usize,
}

impl<'a> LowerCtx<'a> {
    fn new(subject: &'a str, via: &ColRef) -> LowerCtx<'a> {
        LowerCtx {
            subject,
            via_sql: format!("{}.{}", via.table, via.column),
            joins: Vec::new(),
            params: Vec::new(),
            next_id: 0,
        }
    }

    fn bind(&mut self, prefix: &str, value: &str) -> String {
        let placeholder = format!(":{}_{}", prefix, self.next_id);
        self.next_id += 1;
        self.params.push(BoundParam {
            placeholder: placeholder.clone(),
            value: value.to_string(),
        });
        placeholder
    }

    fn authz_join_predicate(&mut self, relation: &str) -> String {
        if let Some(existing) = self.joins.iter().find(|j| {
            j.clause
                .contains(&format!("relation = :rel_for_{relation}"))
        }) {
            return format!("{}.object_id IS NOT NULL", existing.alias);
        }
        let alias = format!("av{}", self.joins.len());
        let subject_ph = self.bind("subject", self.subject);
        let rel_ph = format!(":rel_for_{relation}");
        self.params.push(BoundParam {
            placeholder: rel_ph.clone(),
            value: relation.to_string(),
        });
        let clause = format!(
            "JOIN {table} {alias} ON {alias}.object_id = {via} \
             AND {alias}.subject = {subject_ph} AND {alias}.relation = {rel_ph}",
            table = S8_TABLE,
            via = self.via_sql,
        );
        self.joins.push(AuthzJoin {
            alias: alias.clone(),
            clause,
        });
        format!("{alias}.object_id IS NOT NULL")
    }
}

pub fn lower(set_expr: &SetExpr, subject: &Principal, via: &ColRef) -> Lowered {
    let mut ctx = LowerCtx::new(&subject.principal_id.0, via);
    let sql_predicate = lower_expr(set_expr, &mut ctx);
    Lowered {
        sql_predicate,
        joins: ctx.joins,
        params: ctx.params,
    }
}

fn lower_expr(expr: &SetExpr, ctx: &mut LowerCtx<'_>) -> String {
    match expr {
        SetExpr::All => "TRUE".to_string(),
        SetExpr::None => "FALSE".to_string(),
        SetExpr::Ids(ids) => {
            if ids.is_empty() {
                return "FALSE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        SetExpr::NotIds(ids) => {
            if ids.is_empty() {
                return "TRUE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} NOT IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        SetExpr::InRelation { relation, .. } => ctx.authz_join_predicate(&relation.0),
        SetExpr::TupleSet { index } => ctx.authz_join_predicate(&index.0),
        SetExpr::Union(parts) => {
            if parts.is_empty() {
                return "FALSE".to_string();
            }
            let frags: Vec<String> = parts.iter().map(|p| lower_expr(p, ctx)).collect();
            format!("({})", frags.join(" OR "))
        }
        SetExpr::Intersect(parts) => {
            if parts.is_empty() {
                return "TRUE".to_string();
            }
            let frags: Vec<String> = parts.iter().map(|p| lower_expr(p, ctx)).collect();
            format!("({})", frags.join(" AND "))
        }
        SetExpr::Difference(a, b) => {
            let af = lower_expr(a, ctx);
            let bf = lower_expr(b, ctx);
            format!("({af} AND NOT {bf})")
        }
    }
}

pub fn watermark_verdict(
    index: &ReverseIndex,
    scope: &TenantScope,
    lowered: &Lowered,
    at: &Consistency,
) -> WatermarkVerdict {
    if !lowered.depends_on_reverse_index() {
        return WatermarkVerdict::JoinServes;
    }
    if at.at_least.0.is_empty() {
        return WatermarkVerdict::JoinServes;
    }
    let watermark = index.watermark(scope);
    if watermark.0 >= at.at_least.0 {
        WatermarkVerdict::JoinServes
    } else {
        WatermarkVerdict::FallBackToCheck {
            required: at.at_least.clone(),
            watermark,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatermarkVerdict {
    JoinServes,
    FallBackToCheck { required: Zookie, watermark: Zookie },
}

#[allow(clippy::too_many_arguments)]
pub fn fall_back_to_check(
    engine: &CheckEngine,
    namespace: &NamespaceEngine,
    scope: &TenantScope,
    subject: &Principal,
    permission: &Permission,
    ty: &ObjectType,
    candidates: &[ObjectId],
    at: &Consistency,
) -> myelin_identity::Result<Vec<ObjectId>> {
    let _ = ty;
    let snapshot = engine.snapshot(scope, &at.at_least)?;
    Ok(candidates
        .iter()
        .filter(|obj| {
            let object_ref = ArtifactRef(obj.0.clone());
            let object_type = type_of_object_id(&obj.0);
            namespace.permits_snapshot(&snapshot, subject, &object_type, &permission.0, &object_ref)
        })
        .cloned()
        .collect())
}

fn type_of_object_id(object_id: &str) -> String {
    object_id
        .split_once(':')
        .map(|(ty, _)| ty.to_string())
        .unwrap_or_else(|| object_id.to_string())
}

pub fn is_fall_back(verdict: &WatermarkVerdict) -> bool {
    matches!(verdict, WatermarkVerdict::FallBackToCheck { .. })
}

pub type CheckDecision = Decision;

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{AuthzIndexRef, ConsistencyMode, PrincipalId, PrincipalKind, RelName};
    use myelin_tenancy::TenantId;

    fn subject(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn via() -> ColRef {
        ColRef {
            table: "repo".into(),
            column: "id".into(),
        }
    }

    fn pinned(rev: &str) -> Consistency {
        Consistency {
            at_least: Zookie(rev.into()),
            mode: ConsistencyMode::Strong,
        }
    }

    fn latest() -> Consistency {
        Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        }
    }

    #[test]
    fn all_lowers_to_true() {
        let l = lower(&SetExpr::All, &subject("p:a"), &via());
        assert_eq!(l.sql_predicate, "TRUE");
        assert!(l.joins.is_empty() && l.params.is_empty());
    }

    #[test]
    fn none_lowers_to_false() {
        let l = lower(&SetExpr::None, &subject("p:a"), &via());
        assert_eq!(l.sql_predicate, "FALSE");
    }

    #[test]
    fn ids_lowers_to_in_with_bound_params() {
        let l = lower(
            &SetExpr::Ids(vec![ObjectId("repo:a".into()), ObjectId("repo:b".into())]),
            &subject("p:a"),
            &via(),
        );
        assert_eq!(l.sql_predicate, "repo.id IN (:id_0, :id_1)");
        assert_eq!(
            l.params,
            vec![
                BoundParam {
                    placeholder: ":id_0".into(),
                    value: "repo:a".into()
                },
                BoundParam {
                    placeholder: ":id_1".into(),
                    value: "repo:b".into()
                },
            ],
            "the ids are BOUND params, never interpolated into the SQL"
        );
        assert!(
            l.joins.is_empty(),
            "an Ids lowering needs no reverse-index JOIN"
        );
    }

    #[test]
    fn empty_ids_lowers_to_false() {
        let l = lower(&SetExpr::Ids(vec![]), &subject("p:a"), &via());
        assert_eq!(l.sql_predicate, "FALSE", "an empty allow-set sees nothing");
    }

    #[test]
    fn not_ids_lowers_to_not_in() {
        let l = lower(
            &SetExpr::NotIds(vec![ObjectId("repo:secret".into())]),
            &subject("p:a"),
            &via(),
        );
        assert_eq!(l.sql_predicate, "repo.id NOT IN (:id_0)");
        let empty = lower(&SetExpr::NotIds(vec![]), &subject("p:a"), &via());
        assert_eq!(
            empty.sql_predicate, "TRUE",
            "an empty deny-set excludes nothing"
        );
    }

    #[test]
    fn in_relation_lowers_to_the_authz_visible_join() {
        let l = lower(
            &SetExpr::InRelation {
                relation: RelName("read".into()),
                via_column: via(),
            },
            &subject("p:alice"),
            &via(),
        );
        assert_eq!(l.joins.len(), 1, "exactly one reverse-index JOIN (no N+1)");
        let j = &l.joins[0];
        assert!(
            j.clause
                .contains("JOIN authz_visible av0 ON av0.object_id = repo.id"),
            "the JOIN keys on the consumer's own id column: {}",
            j.clause
        );
        assert!(
            j.clause.contains("av0.subject = :subject_0"),
            "the JOIN binds the subject: {}",
            j.clause
        );
        assert!(
            j.clause.contains("av0.relation = :rel_for_read"),
            "the JOIN binds the relation: {}",
            j.clause
        );
        assert_eq!(l.sql_predicate, "av0.object_id IS NOT NULL");
        assert!(l
            .params
            .iter()
            .any(|p| p.placeholder == ":subject_0" && p.value == "p:alice"));
        assert!(
            l.depends_on_reverse_index(),
            "an InRelation lowering depends on the S8 watermark"
        );
    }

    #[test]
    fn tuple_set_lowers_to_the_authz_visible_join() {
        let l = lower(
            &SetExpr::TupleSet {
                index: AuthzIndexRef("watcher".into()),
            },
            &subject("p:alice"),
            &via(),
        );
        assert_eq!(l.joins.len(), 1);
        assert!(l.joins[0]
            .clause
            .contains("av0.relation = :rel_for_watcher"));
        assert!(l.depends_on_reverse_index());
    }

    #[test]
    fn boolean_composition_lowers_to_or_and_and_not() {
        let u = lower(
            &SetExpr::Union(vec![
                SetExpr::Ids(vec![ObjectId("repo:a".into())]),
                SetExpr::Ids(vec![ObjectId("repo:b".into())]),
            ]),
            &subject("p:a"),
            &via(),
        );
        assert_eq!(
            u.sql_predicate,
            "(repo.id IN (:id_0) OR repo.id IN (:id_1))"
        );

        let i = lower(
            &SetExpr::Intersect(vec![
                SetExpr::All,
                SetExpr::NotIds(vec![ObjectId("repo:x".into())]),
            ]),
            &subject("p:a"),
            &via(),
        );
        assert_eq!(i.sql_predicate, "(TRUE AND repo.id NOT IN (:id_0))");

        let d = lower(
            &SetExpr::Difference(
                Box::new(SetExpr::All),
                Box::new(SetExpr::Ids(vec![ObjectId("repo:secret".into())])),
            ),
            &subject("p:a"),
            &via(),
        );
        assert_eq!(d.sql_predicate, "(TRUE AND NOT repo.id IN (:id_0))");
    }

    #[test]
    fn repeated_relation_emits_one_join_no_n_plus_1() {
        let l = lower(
            &SetExpr::Union(vec![
                SetExpr::InRelation {
                    relation: RelName("read".into()),
                    via_column: via(),
                },
                SetExpr::InRelation {
                    relation: RelName("read".into()),
                    via_column: via(),
                },
            ]),
            &subject("p:alice"),
            &via(),
        );
        assert_eq!(
            l.joins.len(),
            1,
            "the same (subject, relation) JOIN is emitted once, however nested - no N+1"
        );
        assert_eq!(
            l.sql_predicate,
            "(av0.object_id IS NOT NULL OR av0.object_id IS NOT NULL)"
        );
    }

    #[test]
    fn distinct_relations_emit_distinct_joins() {
        let l = lower(
            &SetExpr::Union(vec![
                SetExpr::InRelation {
                    relation: RelName("read".into()),
                    via_column: via(),
                },
                SetExpr::InRelation {
                    relation: RelName("write".into()),
                    via_column: via(),
                },
            ]),
            &subject("p:alice"),
            &via(),
        );
        assert_eq!(l.joins.len(), 2, "two distinct relations → two JOINs");
        assert_eq!(
            l.sql_predicate,
            "(av0.object_id IS NOT NULL OR av1.object_id IS NOT NULL)"
        );
    }

    #[test]
    fn watermark_at_or_after_serves_the_join() {
        let index = ReverseIndex::new();
        let scope = TenantScope::from_verified_token(
            &subject("p-admin"),
            myelin_tenancy::Region("eu-west".into()),
        );
        index.advance_watermark_only(&scope, &Zookie("zk-00000000000000000005".into()));
        let lowered = lower(
            &SetExpr::InRelation {
                relation: RelName("read".into()),
                via_column: via(),
            },
            &subject("p:alice"),
            &via(),
        );
        let v = watermark_verdict(&index, &scope, &lowered, &pinned("zk-00000000000000000003"));
        assert_eq!(v, WatermarkVerdict::JoinServes);
        let v = watermark_verdict(&index, &scope, &lowered, &pinned("zk-00000000000000000005"));
        assert_eq!(v, WatermarkVerdict::JoinServes);
    }

    #[test]
    fn watermark_behind_falls_back_to_check() {
        let index = ReverseIndex::new();
        let scope = TenantScope::from_verified_token(
            &subject("p-admin"),
            myelin_tenancy::Region("eu-west".into()),
        );
        index.advance_watermark_only(&scope, &Zookie("zk-00000000000000000003".into()));
        let lowered = lower(
            &SetExpr::InRelation {
                relation: RelName("read".into()),
                via_column: via(),
            },
            &subject("p:alice"),
            &via(),
        );
        let v = watermark_verdict(&index, &scope, &lowered, &pinned("zk-00000000000000000007"));
        assert!(
            is_fall_back(&v),
            "a behind index must fall back to check, not serve stale: {v:?}"
        );
        match v {
            WatermarkVerdict::FallBackToCheck {
                required,
                watermark,
            } => {
                assert_eq!(required, Zookie("zk-00000000000000000007".into()));
                assert_eq!(watermark, Zookie("zk-00000000000000000003".into()));
            }
            other => panic!("expected fall-back, got {other:?}"),
        }
    }

    #[test]
    fn default_consistency_and_pure_ids_always_serve() {
        let index = ReverseIndex::new();
        let scope = TenantScope::from_verified_token(
            &subject("p-admin"),
            myelin_tenancy::Region("eu-west".into()),
        );
        let join_lowered = lower(
            &SetExpr::InRelation {
                relation: RelName("read".into()),
                via_column: via(),
            },
            &subject("p:a"),
            &via(),
        );
        assert_eq!(
            watermark_verdict(&index, &scope, &join_lowered, &latest()),
            WatermarkVerdict::JoinServes
        );
        let ids_lowered = lower(
            &SetExpr::Ids(vec![ObjectId("repo:a".into())]),
            &subject("p:a"),
            &via(),
        );
        assert_eq!(
            watermark_verdict(
                &index,
                &scope,
                &ids_lowered,
                &pinned("zk-00000000000000000099")
            ),
            WatermarkVerdict::JoinServes,
            "a materialised Ids set is watermark-independent"
        );
    }
}
