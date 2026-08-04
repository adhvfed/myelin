use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_identity::{ColRef, ObjectId, Principal, SetExpr, Zookie};
use myelin_tenancy::{Region, TenantId};

pub fn unfurl_candidate_colref() -> ColRef {
    ColRef {
        table: "unfurl_candidate".into(),
        column: "object_id".into(),
    }
}

pub const AUTHZ_VISIBLE_TABLE: &str = "authz_visible";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    Ids,
    PushedDown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundParam {
    pub placeholder: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthzJoin {
    pub alias: String,
    pub relation: String,
    pub clause: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredFilter {
    pub sql_predicate: String,
    pub joins: Vec<AuthzJoin>,
    pub params: Vec<BoundParam>,
}

impl LoweredFilter {
    pub fn depends_on_reverse_index(&self) -> bool {
        !self.joins.is_empty()
    }

    pub fn filter_mode(&self) -> FilterMode {
        if self.depends_on_reverse_index() {
            FilterMode::PushedDown
        } else {
            FilterMode::Ids
        }
    }

    pub fn join_count(&self) -> usize {
        self.joins.len()
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
        if let Some(existing) = self.joins.iter().find(|j| j.relation == relation) {
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
            table = AUTHZ_VISIBLE_TABLE,
            via = self.via_sql,
        );
        self.joins.push(AuthzJoin {
            alias: alias.clone(),
            relation: relation.to_string(),
            clause,
        });
        format!("{alias}.object_id IS NOT NULL")
    }
}

pub fn lower_over(set_expr: &SetExpr, viewer: &Principal, via: &ColRef) -> LoweredFilter {
    let mut ctx = LowerCtx::new(&viewer.principal_id.0, via);
    let sql_predicate = lower_expr(set_expr, &mut ctx);
    LoweredFilter {
        sql_predicate,
        joins: ctx.joins,
        params: ctx.params,
    }
}

pub fn lower_over_unfurl_candidate(set_expr: &SetExpr, viewer: &Principal) -> LoweredFilter {
    lower_over(set_expr, viewer, &unfurl_candidate_colref())
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

#[derive(Clone, Default)]
pub struct AuthzVisibleIndex {
    watermark: Arc<Mutex<WatermarkMap>>,
    visible: Arc<Mutex<VisibleMap>>,
}

type WatermarkMap = HashMap<(String, String), String>;
type VisibleMap = HashMap<(String, String, String, String), Vec<String>>;

impl AuthzVisibleIndex {
    pub fn new() -> AuthzVisibleIndex {
        AuthzVisibleIndex::default()
    }

    pub fn grant(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
        relation: &str,
        object_id: &str,
        at_revision: &str,
    ) {
        let key = (
            tenant.0.clone(),
            region.0.clone(),
            subject.into(),
            relation.into(),
        );
        let mut v = self.visible.lock().unwrap();
        let set = v.entry(key).or_default();
        if !set.iter().any(|o| o == object_id) {
            set.push(object_id.into());
        }
        drop(v);
        self.advance_watermark(tenant, region, at_revision);
    }

    pub fn revoke(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
        relation: &str,
        object_id: &str,
        at_revision: &str,
    ) {
        let key = (
            tenant.0.clone(),
            region.0.clone(),
            subject.into(),
            relation.into(),
        );
        if let Some(set) = self.visible.lock().unwrap().get_mut(&key) {
            set.retain(|o| o != object_id);
        }
        self.advance_watermark(tenant, region, at_revision);
    }

    pub fn advance_watermark(&self, tenant: &TenantId, region: &Region, revision: &str) {
        let key = (tenant.0.clone(), region.0.clone());
        let mut w = self.watermark.lock().unwrap();
        let cur = w.entry(key).or_default();
        if revision > cur.as_str() {
            *cur = revision.into();
        }
    }

    pub fn watermark(&self, tenant: &TenantId, region: &Region) -> Zookie {
        let key = (tenant.0.clone(), region.0.clone());
        Zookie(
            self.watermark
                .lock()
                .unwrap()
                .get(&key)
                .cloned()
                .unwrap_or_default(),
        )
    }

    pub fn serves(&self, tenant: &TenantId, region: &Region, required: &Zookie) -> bool {
        if required.0.is_empty() {
            return true;
        }
        self.watermark(tenant, region).0 >= required.0
    }

    pub fn evaluate(
        &self,
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        lowered: &LoweredFilter,
        candidates: &[ObjectId],
    ) -> Vec<ObjectId> {
        candidates
            .iter()
            .filter(|c| self.row_survives(tenant, region, viewer, lowered, &c.0))
            .cloned()
            .collect()
    }

    fn row_survives(
        &self,
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        lowered: &LoweredFilter,
        candidate: &str,
    ) -> bool {
        eval_predicate(&lowered.sql_predicate, &mut |frag| {
            self.frag_holds(tenant, region, viewer, lowered, frag, candidate)
        })
    }

    fn frag_holds(
        &self,
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        lowered: &LoweredFilter,
        frag: &str,
        candidate: &str,
    ) -> bool {
        let f = frag.trim();
        if f == "TRUE" {
            return true;
        }
        if f == "FALSE" {
            return false;
        }
        if let Some(alias) = f.strip_suffix(".object_id IS NOT NULL") {
            let relation = lowered
                .joins
                .iter()
                .find(|j| j.alias == alias)
                .map(|j| j.relation.as_str())
                .unwrap_or("");
            let key = (
                tenant.0.clone(),
                region.0.clone(),
                viewer.principal_id.0.clone(),
                relation.to_string(),
            );
            return self
                .visible
                .lock()
                .unwrap()
                .get(&key)
                .map(|set| set.iter().any(|o| o == candidate))
                .unwrap_or(false);
        }
        if let Some(rest) = f.split_once(" NOT IN (") {
            let in_set = self.bound_in_set(lowered, rest.1);
            return !in_set.iter().any(|v| v == candidate);
        }
        if let Some(rest) = f.split_once(" IN (") {
            let in_set = self.bound_in_set(lowered, rest.1);
            return in_set.iter().any(|v| v == candidate);
        }
        false
    }

    fn bound_in_set(&self, lowered: &LoweredFilter, in_body: &str) -> Vec<String> {
        let body = in_body.trim_end_matches(')');
        body.split(',')
            .map(|p| p.trim())
            .filter_map(|ph| {
                lowered
                    .params
                    .iter()
                    .find(|p| p.placeholder == ph)
                    .map(|p| p.value.clone())
            })
            .collect()
    }
}

fn eval_predicate(pred: &str, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    let tokens = tokenize(pred);
    let mut pos = 0;
    let v = parse_or(&tokens, &mut pos, leaf);
    debug_assert_eq!(pos, tokens.len(), "the predicate parsed fully: {pred}");
    v
}

fn tokenize(pred: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut depth_in_leaf = 0usize;
    let mut i = 0;
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        let t = cur.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
        cur.clear();
    };
    let chars: Vec<char> = pred.chars().collect();
    while i < chars.len() {
        let rest: String = chars[i..].iter().collect();
        if rest.starts_with("IN (") {
            cur.push_str("IN (");
            i += 4;
            depth_in_leaf += 1;
            continue;
        }
        if depth_in_leaf == 0 {
            if rest.starts_with(" AND NOT ") {
                flush(&mut cur, &mut out);
                out.push("AND NOT".into());
                i += " AND NOT ".chars().count();
                continue;
            }
            if rest.starts_with(" AND ") {
                flush(&mut cur, &mut out);
                out.push("AND".into());
                i += " AND ".chars().count();
                continue;
            }
            if rest.starts_with(" OR ") {
                flush(&mut cur, &mut out);
                out.push("OR".into());
                i += " OR ".chars().count();
                continue;
            }
            if rest.starts_with("NOT ") && cur.trim().is_empty() {
                out.push("NOT".into());
                i += "NOT ".chars().count();
                continue;
            }
            let c = chars[i];
            if c == '(' {
                flush(&mut cur, &mut out);
                out.push("(".into());
                i += 1;
                continue;
            }
            if c == ')' {
                flush(&mut cur, &mut out);
                out.push(")".into());
                i += 1;
                continue;
            }
        } else if chars[i] == ')' {
            cur.push(')');
            i += 1;
            depth_in_leaf -= 1;
            continue;
        }
        cur.push(chars[i]);
        i += 1;
    }
    flush(&mut cur, &mut out);
    out
}

fn parse_or(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    let mut v = parse_and(tokens, pos, leaf);
    while *pos < tokens.len() && tokens[*pos] == "OR" {
        *pos += 1;
        let rhs = parse_and(tokens, pos, leaf);
        v = v || rhs;
    }
    v
}

fn parse_and(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    let mut v = parse_not(tokens, pos, leaf);
    while *pos < tokens.len() && (tokens[*pos] == "AND" || tokens[*pos] == "AND NOT") {
        let negate = tokens[*pos] == "AND NOT";
        *pos += 1;
        let rhs = parse_not(tokens, pos, leaf);
        v = v && (if negate { !rhs } else { rhs });
    }
    v
}

fn parse_not(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    if *pos < tokens.len() && tokens[*pos] == "NOT" {
        *pos += 1;
        return !parse_atom(tokens, pos, leaf);
    }
    parse_atom(tokens, pos, leaf)
}

fn parse_atom(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    if *pos >= tokens.len() {
        return false;
    }
    if tokens[*pos] == "(" {
        *pos += 1;
        let v = parse_or(tokens, pos, leaf);
        if *pos < tokens.len() && tokens[*pos] == ")" {
            *pos += 1;
        }
        return v;
    }
    let frag = &tokens[*pos];
    *pos += 1;
    leaf(frag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{AuthzIndexRef, ObjectId, PrincipalId, PrincipalKind, RelName};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }
    fn in_relation(rel: &str) -> SetExpr {
        SetExpr::InRelation {
            relation: RelName(rel.into()),
            via_column: unfurl_candidate_colref(),
        }
    }

    #[test]
    fn in_relation_lowers_to_one_join_over_candidate_column() {
        let lowered = lower_over_unfurl_candidate(&in_relation("read"), &viewer("alice"));
        assert_eq!(lowered.join_count(), 1, "one JOIN for one relation");
        assert!(lowered.depends_on_reverse_index());
        assert_eq!(lowered.filter_mode(), FilterMode::PushedDown);
        let join = &lowered.joins[0];
        assert!(join.clause.contains("JOIN authz_visible"));
        assert!(join
            .clause
            .contains("ON av0.object_id = unfurl_candidate.object_id"));
        assert_eq!(lowered.sql_predicate, "av0.object_id IS NOT NULL");
    }

    #[test]
    fn repeated_relation_dedups_to_one_join_no_n_plus_1() {
        let expr = SetExpr::Union(vec![
            in_relation("read"),
            SetExpr::Intersect(vec![in_relation("read"), in_relation("read")]),
            in_relation("read"),
        ]);
        let lowered = lower_over_unfurl_candidate(&expr, &viewer("bob"));
        assert_eq!(
            lowered.join_count(),
            1,
            "the same (viewer, relation) JOINs ONCE however nested - no N+1"
        );
    }

    #[test]
    fn distinct_relations_emit_distinct_joins() {
        let expr = SetExpr::Union(vec![in_relation("read"), in_relation("member")]);
        let lowered = lower_over_unfurl_candidate(&expr, &viewer("carol"));
        assert_eq!(lowered.join_count(), 2);
    }

    #[test]
    fn frozen_identity_elements_are_leak_free() {
        let v = viewer("dan");
        assert_eq!(
            lower_over_unfurl_candidate(&SetExpr::None, &v).sql_predicate,
            "FALSE"
        );
        assert_eq!(
            lower_over_unfurl_candidate(&SetExpr::Ids(vec![]), &v).sql_predicate,
            "FALSE"
        );
        assert_eq!(
            lower_over_unfurl_candidate(&SetExpr::NotIds(vec![]), &v).sql_predicate,
            "TRUE"
        );
        assert_eq!(
            lower_over_unfurl_candidate(&SetExpr::All, &v).sql_predicate,
            "TRUE"
        );
    }

    #[test]
    fn join_filters_candidates_leak_free() {
        let index = AuthzVisibleIndex::new();
        let v = viewer("erin");
        index.grant(&tenant(), &region(), "erin", "read", "channel:c1", "zk-01");
        let lowered = lower_over_unfurl_candidate(&in_relation("read"), &v);
        let candidates = vec![ObjectId("channel:c1".into()), ObjectId("channel:c2".into())];
        let visible = index.evaluate(&tenant(), &region(), &v, &lowered, &candidates);
        assert_eq!(visible, vec![ObjectId("channel:c1".into())], "0 leak of c2");
    }

    #[test]
    fn revoke_drops_candidate_new_enemy() {
        let index = AuthzVisibleIndex::new();
        let v = viewer("frank");
        index.grant(&tenant(), &region(), "frank", "read", "channel:c9", "zk-01");
        let lowered = lower_over_unfurl_candidate(&in_relation("read"), &v);
        let candidates = vec![ObjectId("channel:c9".into())];
        assert_eq!(
            index
                .evaluate(&tenant(), &region(), &v, &lowered, &candidates)
                .len(),
            1
        );
        index.revoke(&tenant(), &region(), "frank", "read", "channel:c9", "zk-02");
        assert!(index
            .evaluate(&tenant(), &region(), &v, &lowered, &candidates)
            .is_empty());
        assert!(index.serves(&tenant(), &region(), &Zookie("zk-02".into())));
    }

    #[test]
    fn watermark_is_monotone_stale_never_regresses() {
        let index = AuthzVisibleIndex::new();
        index.advance_watermark(&tenant(), &region(), "zk-05");
        index.advance_watermark(&tenant(), &region(), "zk-02");
        assert_eq!(index.watermark(&tenant(), &region()).0, "zk-05");
    }

    #[test]
    fn difference_excludes_the_deny_set() {
        let index = AuthzVisibleIndex::new();
        let v = viewer("gwen");
        let expr = SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::Ids(vec![ObjectId("channel:secret".into())])),
        );
        let lowered = lower_over_unfurl_candidate(&expr, &v);
        let candidates = vec![
            ObjectId("channel:open".into()),
            ObjectId("channel:secret".into()),
        ];
        let visible = index.evaluate(&tenant(), &region(), &v, &lowered, &candidates);
        assert_eq!(visible, vec![ObjectId("channel:open".into())]);
    }

    #[test]
    fn tuple_set_lowers_to_join() {
        let expr = SetExpr::TupleSet {
            index: AuthzIndexRef("read".into()),
        };
        let lowered = lower_over_unfurl_candidate(&expr, &viewer("hank"));
        assert_eq!(lowered.join_count(), 1);
        assert_eq!(lowered.filter_mode(), FilterMode::PushedDown);
    }
}
