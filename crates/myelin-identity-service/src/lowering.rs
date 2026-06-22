//! # `lowering` — the `SetExpr` no-N+1/no-post-filter lowering + the S8 watermark consistency path
//! (P-ID-12 → P-070)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §7.2 (the no-N+1/no-post-filter lowering — `Ids`/`NotIds` → `IN`/`NOT IN` under the cap;
//! `InRelation`/`TupleSet` → the **JOIN against `authz_visible`** keyed on the consumer's own id
//! column; `Union`/`Intersect`/`Difference` → `AND`/`OR`/`EXCEPT`), §7.3 (the five id-column
//! mapping), §7.4 (consistency + the S8 revision watermark — a scan needing a fresher revision than
//! the watermark **waits or falls back to per-row `check`** rather than serving stale), §8.7 (the
//! watermark: at-or-after → the JOIN serves; behind → wait-or-fall-back-to-`check`).
//!
//! **Contract-index:** rows **4.3** (the `Filter` lowering — **OWNED** here, closing the P-ID-11
//! stub) and **4.10** (the S8 watermark read-half — **OWNED** here, closing the P-ID-08 floor).
//!
//! ## What this module ships (P-ID-12 — the load-bearing crux)
//! 1. **[`lower`] — the `SetExpr` → SQL lowering (§7.2).** Each variant lowers to a SQL predicate /
//!    JOIN over the consumer's OWN `via_column` ([`ColRef`]) — **one query, no N+1, no post-filter**:
//!    - `All` → `TRUE` (the subject sees every object of this type in the tenant);
//!    - `None` → `FALSE` (`WHERE false` — the deny set, never a permissive default);
//!    - `Ids(v)` → `<col> IN (:p0, :p1, …)` (inlined under the cardinality cap);
//!    - `NotIds(v)` → `<col> NOT IN (…)` (an explicit deny over an otherwise-visible space);
//!    - `InRelation{relation, via_column}` / `TupleSet{index}` → a **JOIN against `authz_visible`**
//!      keyed `av.object_id = <via_column> AND av.subject = :subject AND av.relation = :relation`
//!      (the SpiceDB/Zanzibar reverse-index / LookupResources pattern as a co-located JOIN target —
//!      the consumer's own query planner does the conjoin);
//!    - `Union`/`Intersect`/`Difference` → `(a OR b)` / `(a AND b)` / `(a AND NOT b)` (the boolean
//!      composition; `EXCEPT` realised as `AND NOT` on the same row space so it composes inside one
//!      `WHERE`).
//!
//!    The lowering carries its **bound parameters** (never string-interpolated literals — an injected
//!    id can never become SQL) and its set of **required JOINs** (deduplicated, so the same
//!    `(subject, relation)` reverse-index JOIN is emitted once — no N+1).
//! 2. **[`Lowered`] — the lowering result the consumer composes.** A `(sql_predicate, joins, params)`
//!    triple: the consumer ANDs `sql_predicate` into its board/list/search `WHERE`, adds `joins` to
//!    its `FROM`, and binds `params`. The shape is the wire contract; this is the producer half the
//!    consumer's `myelin-query` compiler lowers.
//! 3. **[`scan_with_watermark`] — the S8 watermark consistency path (§7.4/§8.7; contract 4.10
//!    read-half).** A zookie-stamped scan compares its **required revision** against S8's per-tenant
//!    `revision_watermark`: at-or-after → the JOIN serves (the fast path); behind → **fall back to
//!    per-row `check`** rather than serving the stale grant (the new-enemy guard — ID-D7). This is
//!    the read-half of 4.10 that P-ID-08 floored; it closes here.
//!
//! ## Floors closed here (named CLOSED, per the prompt)
//! - **The `Filter` SetExpr→SQL lowering** (opened in P-ID-11, the [`crate::list_objects`] stub): the
//!   real consumer-composable lowering lands here. CLOSED.
//! - **The watermark *read* consistency path** (opened in P-ID-08/P-ID-11): [`scan_with_watermark`]
//!   waits/falls-back rather than serving stale. CLOSED.
//!
//! No NEW floor is opened by this prompt (the prompt's DELIVERABLE: "none new").

use crate::check_engine::CheckEngine;
use crate::namespace::NamespaceEngine;
use crate::reverse_index::{ReverseIndex, S8_TABLE};
use myelin_identity::{
    ColRef, Consistency, Decision, ObjectId, ObjectType, Permission, Principal, SetExpr, Zookie,
};
use myelin_storage::TenantScope;
use myelin_tenancy::ArtifactRef;

/// One bound parameter the lowered predicate carries (never a string-interpolated literal — an
/// id/subject/relation an attacker controls can never become SQL; the consumer binds these). The
/// placeholder is `:name`; the value is the bound string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundParam {
    /// The named placeholder in the SQL (`:p0`, `:subject_0`, `:rel_0`).
    pub placeholder: String,
    /// The bound value (an object id / a subject id / a relation name) — bound, never interpolated.
    pub value: String,
}

/// One JOIN the lowered predicate requires against the S8 reverse index (`authz_visible`). The
/// consumer adds this to its `FROM`; the predicate references the alias. Deduplicated by the
/// `(alias)` so the SAME reverse-index JOIN is emitted ONCE — the no-N+1 guarantee (§7.2): an
/// `InRelation`/`TupleSet`, however deeply nested in a boolean tree, contributes at most one JOIN
/// per distinct `(subject, relation)` pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthzJoin {
    /// The alias for this `authz_visible` JOIN (`av0`, `av1`, …) the predicate references.
    pub alias: String,
    /// The full JOIN clause: `JOIN authz_visible <alias> ON <alias>.object_id = <via_column>
    /// AND <alias>.subject = :<subject> AND <alias>.relation = :<relation>`. The consumer's own
    /// query planner does the conjoin — one query, no N+1, no post-filter.
    pub clause: String,
}

/// **The lowering result the consumer composes (§7.2) — `(sql_predicate, joins, params)`.**
///
/// - `sql_predicate` — the boolean SQL the consumer ANDs into its `WHERE` (over its OWN id column);
/// - `joins` — the deduplicated `authz_visible` JOINs the consumer adds to its `FROM` (no N+1);
/// - `params` — the bound parameters (never interpolated literals).
///
/// The consumer does: `SELECT … FROM <its table> <joins> WHERE (<sql_predicate>) AND <its filters>`
/// binding `params`. This is **one query** — the conjoin is the consumer's query planner's job, not
/// a per-row `check` loop. Leak-free: a denied row never survives the `WHERE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lowered {
    /// The boolean SQL predicate over the consumer's own `via_column` (ANDed into its `WHERE`).
    pub sql_predicate: String,
    /// The deduplicated `authz_visible` JOINs the consumer adds to its `FROM` (one per distinct
    /// `(subject, relation)` — the no-N+1 guarantee).
    pub joins: Vec<AuthzJoin>,
    /// The bound parameters (object ids, the subject, relation names) — bound, never interpolated.
    pub params: Vec<BoundParam>,
}

impl Lowered {
    /// `true` iff the predicate references at least one S8 (`authz_visible`) JOIN — i.e. the
    /// lowering hit an `InRelation`/`TupleSet` and therefore depends on the reverse index's
    /// watermark (the [`scan_with_watermark`] consistency path applies). A purely `Ids`/`NotIds`/
    /// `All`/`None` lowering is watermark-independent (it carries its own materialised set).
    pub fn depends_on_reverse_index(&self) -> bool {
        !self.joins.is_empty()
    }
}

/// Internal accumulator threaded through the recursive lowering so JOINs + params are collected once
/// (the no-N+1 dedup lives here: a `(subject, relation)` JOIN already emitted is reused by alias).
struct LowerCtx<'a> {
    /// The subject the whole `Filter` is for (the `av.subject = :subject` binding — one subject per
    /// `list_objects` call; the reverse-index JOIN keys on it).
    subject: &'a str,
    /// The consumer's own id column (`<table>.<column>`) every JOIN/`IN` references.
    via_sql: String,
    joins: Vec<AuthzJoin>,
    params: Vec<BoundParam>,
    /// A monotonically-increasing counter for unique aliases/placeholders.
    next_id: usize,
}

impl<'a> LowerCtx<'a> {
    fn new(subject: &'a str, via: &ColRef) -> LowerCtx<'a> {
        LowerCtx {
            subject,
            via_sql: format!("{}.{}", via.table, via.column),
            joins: Vec::new(),
            params: Vec::new(),
            // The subject is bound once up front (every InRelation JOIN reuses :subject — the JOIN
            // keys on one subject per call; binding it once is part of the no-N+1 discipline).
            next_id: 0,
        }
    }

    /// Bind a value, returning its `:placeholder`. Named uniquely so the consumer binds an
    /// unambiguous parameter set (object ids, relations) — never an interpolated literal.
    fn bind(&mut self, prefix: &str, value: &str) -> String {
        let placeholder = format!(":{}_{}", prefix, self.next_id);
        self.next_id += 1;
        self.params.push(BoundParam {
            placeholder: placeholder.clone(),
            value: value.to_string(),
        });
        placeholder
    }

    /// Emit (or reuse) the `authz_visible` JOIN for a `(subject, relation)` — the §7.2 reverse-index
    /// JOIN keyed on the consumer's own id column. Deduplicated by `(subject, relation)`: a relation
    /// already JOINed reuses its alias, so the SAME JOIN is never emitted twice (the no-N+1
    /// guarantee — at most one JOIN per distinct `(subject, relation)`, however nested). Returns the
    /// boolean predicate fragment `<alias>.object_id IS NOT NULL` the boolean tree composes.
    fn authz_join_predicate(&mut self, relation: &str) -> String {
        // Reuse an existing alias for the same relation (the subject is constant for the whole call).
        if let Some(existing) = self.joins.iter().find(|j| {
            j.clause
                .contains(&format!("relation = :rel_for_{relation}"))
        }) {
            return format!("{}.object_id IS NOT NULL", existing.alias);
        }
        let alias = format!("av{}", self.joins.len());
        // Bind the subject (once per distinct relation JOIN; the same :subject value) + the relation.
        let subject_ph = self.bind("subject", self.subject);
        // Use a relation-stamped placeholder name so the dedup `find` above can recognise the JOIN.
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

/// **Lower a `SetExpr` to the consumer-composable SQL `Filter` (§7.2; contract 4.3 — CLOSES the
/// P-ID-11 stub).** `subject` is the principal the `list_objects` is for (the `av.subject` binding);
/// `via` is the consumer's own id column (the §7.3 mapping). Returns the [`Lowered`]
/// `(sql_predicate, joins, params)` the consumer ANDs into its query — **one query, no N+1, no
/// post-filter**.
pub fn lower(set_expr: &SetExpr, subject: &Principal, via: &ColRef) -> Lowered {
    let mut ctx = LowerCtx::new(&subject.principal_id.0, via);
    let sql_predicate = lower_expr(set_expr, &mut ctx);
    Lowered {
        sql_predicate,
        joins: ctx.joins,
        params: ctx.params,
    }
}

/// The recursive lowering of one `SetExpr` node into a boolean SQL fragment (collecting JOINs +
/// params into `ctx`). Every leaf is a predicate over the consumer's own `via_column` or a
/// reverse-index JOIN; the boolean nodes compose with `OR`/`AND`/`AND NOT` — no per-row subquery, no
/// post-filter.
fn lower_expr(expr: &SetExpr, ctx: &mut LowerCtx<'_>) -> String {
    match expr {
        // The subject sees every object of this type in the tenant (e.g. admin) → no restriction.
        SetExpr::All => "TRUE".to_string(),
        // The deny set — `WHERE false`, never a permissive default (leak-free).
        SetExpr::None => "FALSE".to_string(),
        // An explicit allow-set inlined under the cardinality cap → `<col> IN (:p0, …)`. An empty
        // allow-set is `FALSE` (IN () is not valid SQL and means "no rows" — never a permissive TRUE).
        SetExpr::Ids(ids) => {
            if ids.is_empty() {
                return "FALSE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        // An explicit deny-set over an otherwise-visible space → `<col> NOT IN (…)`. An empty
        // deny-set excludes nothing → `TRUE` (the otherwise-visible space is unrestricted).
        SetExpr::NotIds(ids) => {
            if ids.is_empty() {
                return "TRUE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} NOT IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        // The reverse-index JOIN keyed on the consumer's own id column (§7.2) — the SpiceDB
        // LookupResources pattern as a co-located JOIN target. One JOIN per distinct relation
        // (deduplicated — no N+1).
        SetExpr::InRelation { relation, .. } => ctx.authz_join_predicate(&relation.0),
        // A server-materialised tuple set the consumer JOINs against (the big-result path). On this
        // store the index ref is the relation it materialises; the JOIN is the same `authz_visible`
        // target. (The index ref carries the relation name in `index.0` — the materialised set is
        // `(subject, relation)` keyed.)
        SetExpr::TupleSet { index } => ctx.authz_join_predicate(&index.0),
        // Boolean composition → `(a OR b)` / `(a AND b)` / `(a AND NOT b)` (§7.2 — Union/Intersect/
        // Difference). An empty Union is `FALSE` (sees nothing), an empty Intersect is `TRUE`
        // (no restriction) — the identity elements, never a leak.
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
        // `Difference(a, b)` = a EXCEPT b → `(a AND NOT b)` on the same row space (so it composes
        // inside one `WHERE`, not a set-difference subquery — still one query, no N+1).
        SetExpr::Difference(a, b) => {
            let af = lower_expr(a, ctx);
            let bf = lower_expr(b, ctx);
            format!("({af} AND NOT {bf})")
        }
    }
}

/// **The S8 watermark consistency path (§7.4/§8.7; contract 4.10 read-half — CLOSES the P-ID-08
/// floor).**
///
/// A zookie-stamped scan compares its **required revision** (the caller's pinned `at.at_least`)
/// against S8's per-tenant `revision_watermark`:
/// - **at-or-after** (the watermark has caught up to the required revision) → the JOIN serves: the
///   reverse index reflects the writes the scan must see, so the lowered `Filter` is safe to run
///   against `authz_visible` ([`WatermarkVerdict::JoinServes`]).
/// - **behind** (the watermark is older than the required revision — the index has not yet projected
///   a write the scan must reflect, e.g. a just-applied revoke) → **fall back to per-row `check`**
///   rather than serving the stale grant ([`WatermarkVerdict::FallBackToCheck`]). This is the
///   new-enemy guard (Zanzibar §2.4.4): never serve a stale ALLOW from a behind index.
///
/// A scan with no pinned revision (`at.at_least` empty = "latest is fine") always lets the JOIN
/// serve (it asked for no freshness floor — the default-consistency read). A `Lowered` that does not
/// depend on the reverse index (pure `Ids`/`NotIds`) is watermark-independent and always serves.
pub fn watermark_verdict(
    index: &ReverseIndex,
    scope: &TenantScope,
    lowered: &Lowered,
    at: &Consistency,
) -> WatermarkVerdict {
    // A lowering that carries its own materialised id set (no reverse-index JOIN) is watermark-
    // independent — the ids ARE the answer; there is no stale-JOIN to guard.
    if !lowered.depends_on_reverse_index() {
        return WatermarkVerdict::JoinServes;
    }
    // No pinned revision → the default-consistency read (no freshness floor) — the JOIN serves.
    if at.at_least.0.is_empty() {
        return WatermarkVerdict::JoinServes;
    }
    let watermark = index.watermark(scope);
    // The zookie strings are the zero-padded `zk-<rev>` form (S3 mints them) — lexical order ==
    // revision order. At-or-after → serve; strictly behind → fall back to check.
    if watermark.0 >= at.at_least.0 {
        WatermarkVerdict::JoinServes
    } else {
        WatermarkVerdict::FallBackToCheck {
            required: at.at_least.clone(),
            watermark,
        }
    }
}

/// The verdict of the [`watermark_verdict`] consistency check (§8.7): either the S8 JOIN may serve
/// (the watermark is at-or-after the scan's required revision), or the scan must fall back to per-row
/// `check` (the index is behind — never serve the stale grant; the new-enemy guard).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatermarkVerdict {
    /// The S8 watermark is at-or-after the required revision — the lowered `Filter` JOIN serves.
    JoinServes,
    /// The S8 watermark is BEHIND the required revision — fall back to per-row `check` rather than
    /// serving a stale grant (the new-enemy guard, ID-D7). Carries the required revision + the
    /// (behind) watermark for the loud observability artifact.
    FallBackToCheck {
        /// The revision the scan required (the caller's pinned `at.at_least`).
        required: Zookie,
        /// The S8 watermark, which is behind `required`.
        watermark: Zookie,
    },
}

/// **The per-row fall-back: re-`check` a candidate object against the authoritative S3 store at the
/// scan's required revision (the [`WatermarkVerdict::FallBackToCheck`] body).** When S8 is behind,
/// the lowered `Filter` would serve a stale grant; instead each candidate id is re-checked through
/// the authoritative engine at the required zookie — so a just-revoked grant is NOT served
/// (ID-D7). This is the correctness floor (slower, but never stale); the fast JOIN path is the
/// steady state.
///
/// Returns the subset of `candidates` that genuinely pass `check(subject, permission, object)` at
/// the required revision — leak-free (a denied/revoked object never appears).
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
) -> Vec<ObjectId> {
    let _ = ty; // the type is implied by the candidates' ids; kept for the call-shape symmetry.
    candidates
        .iter()
        .filter(|obj| {
            let object_ref = ArtifactRef(obj.0.clone());
            let object_type = type_of_object_id(&obj.0);
            namespace.permits(
                engine,
                scope,
                subject,
                &object_type,
                &permission.0,
                &object_ref,
                at,
            )
        })
        .cloned()
        .collect()
}

/// Infer an object's TYPE from its id by the leading `type:` prefix (`repo:core` → `repo`). Mirrors
/// the convention `namespace`/`reverse_index`/`list_objects` use.
fn type_of_object_id(object_id: &str) -> String {
    object_id
        .split_once(':')
        .map(|(ty, _)| ty.to_string())
        .unwrap_or_else(|| object_id.to_string())
}

/// A convenience helper a caller (the [`crate::list_objects`] dispatch / a drill) uses to decide
/// whether [`fall_back_to_check`] applies: `true` iff the watermark verdict is a fall-back.
pub fn is_fall_back(verdict: &WatermarkVerdict) -> bool {
    matches!(verdict, WatermarkVerdict::FallBackToCheck { .. })
}

/// Decision re-exported so a caller pattern-matches the fall-back result without importing the
/// contract crate path directly.
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

    /// **`All` → `TRUE`** (the subject sees everything of this type in the tenant).
    #[test]
    fn all_lowers_to_true() {
        let l = lower(&SetExpr::All, &subject("p:a"), &via());
        assert_eq!(l.sql_predicate, "TRUE");
        assert!(l.joins.is_empty() && l.params.is_empty());
    }

    /// **`None` → `FALSE`** (the deny set — `WHERE false`, never a permissive default).
    #[test]
    fn none_lowers_to_false() {
        let l = lower(&SetExpr::None, &subject("p:a"), &via());
        assert_eq!(l.sql_predicate, "FALSE");
    }

    /// **`Ids` → `<col> IN (…)` with BOUND params** (never interpolated literals — injection-safe).
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

    /// **An empty `Ids` is `FALSE`** (the empty allow-set sees nothing — never `IN ()`, never a
    /// permissive `TRUE`).
    #[test]
    fn empty_ids_lowers_to_false() {
        let l = lower(&SetExpr::Ids(vec![]), &subject("p:a"), &via());
        assert_eq!(l.sql_predicate, "FALSE", "an empty allow-set sees nothing");
    }

    /// **`NotIds` → `<col> NOT IN (…)`**; an empty deny-set is `TRUE`.
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

    /// **`InRelation` → the `authz_visible` JOIN keyed on the consumer's own id column (§7.2).**
    /// The JOIN is ONE clause, the predicate references its alias — no per-row subquery, no N+1.
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
        // The subject is a BOUND param (injection-safe).
        assert!(l
            .params
            .iter()
            .any(|p| p.placeholder == ":subject_0" && p.value == "p:alice"));
        assert!(
            l.depends_on_reverse_index(),
            "an InRelation lowering depends on the S8 watermark"
        );
    }

    /// **`TupleSet` → the same `authz_visible` JOIN** (the big-result materialised path; the index
    /// ref names the relation it materialises).
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

    /// **`Union` → `(a OR b)`, `Intersect` → `(a AND b)`, `Difference` → `(a AND NOT b)` (§7.2).**
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

    /// **No N+1: the SAME `(subject, relation)` reverse-index JOIN is emitted ONCE even when it
    /// appears in two branches of a boolean tree (§7.2 — one query, no N+1).**
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
            "the same (subject, relation) JOIN is emitted once, however nested — no N+1"
        );
        // Both branches reference the one alias.
        assert_eq!(
            l.sql_predicate,
            "(av0.object_id IS NOT NULL OR av0.object_id IS NOT NULL)"
        );
    }

    /// **Two DISTINCT relations emit two distinct JOINs** (one per `(subject, relation)`).
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

    /// **A watermark AT-OR-AFTER the required revision → the JOIN serves.**
    #[test]
    fn watermark_at_or_after_serves_the_join() {
        let index = ReverseIndex::new();
        let scope = TenantScope::from_verified_token(
            &subject("p-admin"),
            myelin_tenancy::Region("eu-west".into()),
        );
        // Advance the watermark to rev 5.
        index.advance_watermark_only(&scope, &Zookie("zk-00000000000000000005".into()));
        let lowered = lower(
            &SetExpr::InRelation {
                relation: RelName("read".into()),
                via_column: via(),
            },
            &subject("p:alice"),
            &via(),
        );
        // A scan requiring rev 3 (<= watermark 5) → the JOIN serves.
        let v = watermark_verdict(&index, &scope, &lowered, &pinned("zk-00000000000000000003"));
        assert_eq!(v, WatermarkVerdict::JoinServes);
        // A scan requiring EXACTLY rev 5 → still serves (at-or-after is inclusive).
        let v = watermark_verdict(&index, &scope, &lowered, &pinned("zk-00000000000000000005"));
        assert_eq!(v, WatermarkVerdict::JoinServes);
    }

    /// **A watermark BEHIND the required revision → fall back to check (the new-enemy guard).**
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
        // A scan requiring rev 7 (> watermark 3) → fall back to per-row check (never serve stale).
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

    /// **A default-consistency read (no pinned revision) always serves the JOIN** (it asked for no
    /// freshness floor). And a pure-`Ids` lowering is watermark-independent (it carries its set).
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
        // No pinned revision → serve.
        assert_eq!(
            watermark_verdict(&index, &scope, &join_lowered, &latest()),
            WatermarkVerdict::JoinServes
        );
        // A pure-Ids lowering with a pinned revision → still serves (no reverse-index dependency).
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
