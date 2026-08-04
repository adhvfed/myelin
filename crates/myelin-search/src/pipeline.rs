use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use myelin_identity::{
    ColRef, Consistency, ListObjectsResult, ObjectId, ObjectType, Permission, Principal, RelName,
    Result as AuthzResult, SetExpr,
};
use myelin_query::QueryAst;

use crate::compiler::{self, CompileError, FieldSchema};
use crate::engine::{AclFilter, Hit, IndexBackend, IndexError};

pub const READ_PERMISSION: &str = "read";

pub trait ListObjectsPort {
    fn list_objects(
        &self,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> AuthzResult<ListObjectsResult>;

    fn resolve_relation(
        &self,
        _subject: &Principal,
        _form: &RelationalLeaf,
        _required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        Err(myelin_identity::AuthzError::Unavailable(
            "the authz reverse index is not wired for this query path - a relational SetExpr leaf \
             cannot be resolved (deny-when-unsure, ADR-03; SRCH-P09 needs a reverse-index resolver)"
                .into(),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelationalLeaf {
    InRelation {
        relation: RelName,
        via_column: ColRef,
    },
    TupleSet {
        index: myelin_identity::AuthzIndexRef,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RevisionWatermark(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseIndexAnswer {
    pub object_ids: Vec<String>,
    pub revision: RevisionWatermark,
}

pub struct ScopedEngine<'a, B: IndexBackend> {
    backend: &'a B,
    tenant: String,
    region: String,
    schema: FieldSchema,
}

impl<'a, B: IndexBackend> ScopedEngine<'a, B> {
    pub fn new(
        backend: &'a B,
        tenant: impl Into<String>,
        region: impl Into<String>,
        schema: FieldSchema,
    ) -> ScopedEngine<'a, B> {
        ScopedEngine {
            backend,
            tenant: tenant.into(),
            region: region.into(),
            schema,
        }
    }

    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    pub fn region(&self) -> &str {
        &self.region
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RankedResult {
    pub doc_id: String,
    pub score: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RankedResults {
    pub hits: Vec<RankedResult>,
    pub zookie: String,
    pub post_fetch_fields: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    pub offset: usize,
    pub limit: usize,
}

impl Page {
    pub const MAX_LIMIT: usize = 1_000;

    pub const FIRST: Page = Page {
        offset: 0,
        limit: 50,
    };

    fn effective_limit(self) -> usize {
        self.limit.clamp(1, Page::MAX_LIMIT)
    }
}

impl Default for Page {
    fn default() -> Page {
        Page::FIRST
    }
}

#[derive(Debug, Default)]
pub struct QueryStats {
    list_objects_calls: AtomicU64,
    engine_branches: AtomicU64,
    reverse_index_joins: AtomicU64,
    ids_mode_count: AtomicU64,
    filter_mode_count: AtomicU64,
}

impl QueryStats {
    pub fn new() -> QueryStats {
        QueryStats::default()
    }

    pub fn list_objects_calls(&self) -> u64 {
        self.list_objects_calls.load(Ordering::Relaxed)
    }

    pub fn engine_branches(&self) -> u64 {
        self.engine_branches.load(Ordering::Relaxed)
    }

    pub fn reverse_index_joins(&self) -> u64 {
        self.reverse_index_joins.load(Ordering::Relaxed)
    }

    pub fn ids_mode_count(&self) -> u64 {
        self.ids_mode_count.load(Ordering::Relaxed)
    }

    pub fn filter_mode_count(&self) -> u64 {
        self.filter_mode_count.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub enum QueryError {
    Compile(CompileError),
    Engine(IndexError),
    Authz(myelin_identity::AuthzError),
    TenantMismatch {
        viewer_tenant: String,
        engine_tenant: String,
    },
    StaleReverseIndex {
        required: u64,
        served: u64,
        form: &'static str,
    },
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::Compile(e) => write!(f, "query did not compile: {e}"),
            QueryError::Engine(e) => write!(f, "search engine error: {e}"),
            QueryError::Authz(e) => write!(f, "list_objects (authz) failed: {e:?}"),
            QueryError::TenantMismatch {
                viewer_tenant,
                engine_tenant,
            } => write!(
                f,
                "cross-tenant query rejected: viewer tenant `{viewer_tenant}` != engine tenant \
                 `{engine_tenant}` (SRCH-D3 - tenant from the verified token, the engine is the \
                 wrong tenant's index)"
            ),
            QueryError::StaleReverseIndex {
                required,
                served,
                form,
            } => write!(
                f,
                "the authz reverse-index JOIN for the relational form `{form}` served revision \
                 {served} but the list_objects watermark requires >= {required} (contract 4.10) - \
                 the JOIN refuses to compose a stale reverse-index revision (SRCH-P09; a stale \
                 revision could re-admit a revoked grant - the new-enemy problem); the full \
                 no-stale-grant + fail-static path is SRCH-P10"
            ),
        }
    }
}

impl std::error::Error for QueryError {}

impl From<CompileError> for QueryError {
    fn from(e: CompileError) -> Self {
        QueryError::Compile(e)
    }
}

impl From<IndexError> for QueryError {
    fn from(e: IndexError) -> Self {
        QueryError::Engine(e)
    }
}

pub(crate) fn watermark_from_zookie(zookie: &str) -> RevisionWatermark {
    let rev = zookie
        .rsplit_once('@')
        .and_then(|(_, suffix)| suffix.parse::<u64>().ok())
        .unwrap_or(0);
    RevisionWatermark(rev)
}

fn lower_acl(
    result: &ListObjectsResult,
    subject: &Principal,
    identity: &dyn ListObjectsPort,
    stats: &QueryStats,
) -> Result<(AclFilter, String), QueryError> {
    match result {
        ListObjectsResult::Ids { ids, zookie } => {
            stats.ids_mode_count.fetch_add(1, Ordering::Relaxed);
            let ids: Vec<String> = ids.iter().map(|o| o.0.clone()).collect();
            let filter = if ids.is_empty() {
                AclFilter::None
            } else {
                AclFilter::Ids(ids)
            };
            Ok((filter, zookie.0.clone()))
        }
        ListObjectsResult::Filter { set_expr, zookie } => {
            stats.filter_mode_count.fetch_add(1, Ordering::Relaxed);
            let required = watermark_from_zookie(&zookie.0);
            let filter = lower_set_expr(set_expr, subject, identity, &required, stats)?;
            Ok((filter, zookie.0.clone()))
        }
    }
}

pub(crate) fn lower_set_expr(
    set_expr: &SetExpr,
    subject: &Principal,
    identity: &dyn ListObjectsPort,
    required: &RevisionWatermark,
    stats: &QueryStats,
) -> Result<AclFilter, QueryError> {
    match set_expr {
        SetExpr::All => Ok(AclFilter::All),
        SetExpr::None => Ok(AclFilter::None),
        SetExpr::Ids(ids) => {
            let ids: Vec<String> = ids.iter().map(|o| o.0.clone()).collect();
            Ok(if ids.is_empty() {
                AclFilter::None
            } else {
                AclFilter::Ids(ids)
            })
        }
        SetExpr::NotIds(ids) => {
            let ids: Vec<String> = ids.iter().map(|o| o.0.clone()).collect();
            Ok(if ids.is_empty() {
                AclFilter::All
            } else {
                AclFilter::NotIds(ids)
            })
        }
        SetExpr::InRelation {
            relation,
            via_column,
        } => resolve_relational_leaf(
            &RelationalLeaf::InRelation {
                relation: relation.clone(),
                via_column: via_column.clone(),
            },
            "InRelation",
            subject,
            identity,
            required,
            stats,
        ),
        SetExpr::TupleSet { index } => resolve_relational_leaf(
            &RelationalLeaf::TupleSet {
                index: index.clone(),
            },
            "TupleSet",
            subject,
            identity,
            required,
            stats,
        ),
        SetExpr::Union(subs) => {
            let mut clauses = Vec::with_capacity(subs.len());
            for s in subs {
                clauses.push(lower_set_expr(s, subject, identity, required, stats)?);
            }
            Ok(AclFilter::Or(clauses))
        }
        SetExpr::Intersect(subs) => {
            let mut clauses = Vec::with_capacity(subs.len());
            for s in subs {
                clauses.push(lower_set_expr(s, subject, identity, required, stats)?);
            }
            Ok(AclFilter::And(clauses))
        }
        SetExpr::Difference(left, right) => {
            let l = lower_set_expr(left, subject, identity, required, stats)?;
            let r = lower_set_expr(right, subject, identity, required, stats)?;
            Ok(AclFilter::And(vec![l, AclFilter::Not(Box::new(r))]))
        }
    }
}

fn resolve_relational_leaf(
    leaf: &RelationalLeaf,
    form: &'static str,
    subject: &Principal,
    identity: &dyn ListObjectsPort,
    required: &RevisionWatermark,
    stats: &QueryStats,
) -> Result<AclFilter, QueryError> {
    let answer = identity
        .resolve_relation(subject, leaf, required)
        .map_err(QueryError::Authz)?;
    stats.reverse_index_joins.fetch_add(1, Ordering::Relaxed);

    if answer.revision < *required {
        return Err(QueryError::StaleReverseIndex {
            required: required.0,
            served: answer.revision.0,
            form,
        });
    }

    Ok(if answer.object_ids.is_empty() {
        AclFilter::None
    } else {
        AclFilter::Ids(answer.object_ids)
    })
}

#[allow(clippy::too_many_arguments)]
pub fn query<B: IndexBackend>(
    engine: &ScopedEngine<'_, B>,
    identity: &dyn ListObjectsPort,
    ast: &QueryAst,
    viewer: &Principal,
    ty: &ObjectType,
    at: &Consistency,
    page: Page,
    stats: &QueryStats,
) -> Result<RankedResults, QueryError> {
    let consistency_stats = crate::consistency::ConsistencyStats::new();
    query_consistent(
        engine,
        identity,
        None,
        ast,
        viewer,
        ty,
        at,
        page,
        stats,
        &consistency_stats,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn query_consistent<B: IndexBackend>(
    engine: &ScopedEngine<'_, B>,
    identity: &dyn ListObjectsPort,
    check: Option<&dyn crate::consistency::BoundedCheckPort>,
    ast: &QueryAst,
    viewer: &Principal,
    ty: &ObjectType,
    at: &Consistency,
    page: Page,
    stats: &QueryStats,
    cstats: &crate::consistency::ConsistencyStats,
) -> Result<RankedResults, QueryError> {
    query_consistent_with_vector(
        engine, identity, check, ast, viewer, ty, at, None, page, stats, cstats,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn semantic<B: IndexBackend>(
    engine: &ScopedEngine<'_, B>,
    identity: &dyn ListObjectsPort,
    check: Option<&dyn crate::consistency::BoundedCheckPort>,
    ast: &QueryAst,
    viewer: &Principal,
    ty: &ObjectType,
    at: &Consistency,
    vec: &VectorQuery<'_>,
    page: Page,
    stats: &QueryStats,
    cstats: &crate::consistency::ConsistencyStats,
) -> Result<RankedResults, QueryError> {
    query_consistent_with_vector(
        engine,
        identity,
        check,
        ast,
        viewer,
        ty,
        at,
        Some(vec),
        page,
        stats,
        cstats,
    )
}

#[allow(clippy::too_many_arguments)]
fn query_consistent_with_vector<B: IndexBackend>(
    engine: &ScopedEngine<'_, B>,
    identity: &dyn ListObjectsPort,
    check: Option<&dyn crate::consistency::BoundedCheckPort>,
    ast: &QueryAst,
    viewer: &Principal,
    ty: &ObjectType,
    at: &Consistency,
    vector_query: Option<&VectorQuery<'_>>,
    page: Page,
    stats: &QueryStats,
    cstats: &crate::consistency::ConsistencyStats,
) -> Result<RankedResults, QueryError> {
    if viewer.tenant.0 != engine.tenant {
        return Err(QueryError::TenantMismatch {
            viewer_tenant: viewer.tenant.0.clone(),
            engine_tenant: engine.tenant.clone(),
        });
    }

    if crate::consistency::fail_static_bypass(at) {
        cstats.record_fail_static_bypass();
    } else {
        cstats.record_fail_static_served();
    }

    let permission = Permission(READ_PERMISSION.to_string());
    let lo = identity
        .list_objects(viewer, &permission, ty, at)
        .map_err(QueryError::Authz)?;
    stats.list_objects_calls.fetch_add(1, Ordering::Relaxed);

    let (acl, zookie) = lower_acl(&lo, viewer, identity, stats)?;

    let plan = compiler::compile(ast, &engine.schema)?;
    let post_fetch_fields: Vec<String> = plan.post_fetch.iter().map(|p| p.field.clone()).collect();

    let conjoined: crate::compiler::ConjoinedPlan<AclFilter> = plan.with_acl(acl);
    if matches!(conjoined.acl, AclFilter::None) {
        return Ok(RankedResults {
            hits: Vec::new(),
            zookie,
            post_fetch_fields,
        });
    }

    let hits = execute(engine.backend, &conjoined, vector_query, page, stats)?;

    let hits = revalidate_stale_candidates(
        engine.backend,
        hits,
        identity_subject(viewer),
        &permission,
        &at.at_least.0,
        at,
        check,
        cstats,
    )?;

    let paged = paginate(hits, page);
    Ok(RankedResults {
        hits: paged
            .into_iter()
            .map(|h| RankedResult {
                doc_id: h.doc_id,
                score: h.score,
            })
            .collect(),
        zookie,
        post_fetch_fields,
    })
}

fn identity_subject(viewer: &Principal) -> &Principal {
    viewer
}

#[allow(clippy::too_many_arguments)]
fn revalidate_stale_candidates<B: IndexBackend>(
    backend: &B,
    hits: Vec<Hit>,
    subject: &Principal,
    permission: &Permission,
    zookie: &str,
    at: &Consistency,
    check: Option<&dyn crate::consistency::BoundedCheckPort>,
    cstats: &crate::consistency::ConsistencyStats,
) -> Result<Vec<Hit>, QueryError> {
    let mut out: Vec<Hit> = Vec::with_capacity(hits.len());
    for hit in hits {
        let indexed = backend.indexed_zookie_of(&hit.doc_id);
        match crate::consistency::disposition(indexed.as_deref(), zookie) {
            crate::consistency::CandidateDisposition::Fresh => out.push(hit),
            crate::consistency::CandidateDisposition::StaleNeedsRevalidation => {
                match check {
                    Some(port) => {
                        cstats.record_revalidation();
                        let object = ObjectId(hit.doc_id.clone());
                        let still_allowed = port
                            .check(subject, permission, &object, at)
                            .map_err(QueryError::Authz)?;
                        if still_allowed {
                            out.push(hit);
                        } else {
                            cstats.record_excluded_stale();
                        }
                    }
                    None => {
                        cstats.record_excluded_stale();
                    }
                }
            }
        }
    }
    Ok(out)
}

pub enum VectorQuery<'a> {
    Vec(crate::vector::Embedding),
    Text {
        text: String,
        embedder: &'a dyn crate::indexer::EmbeddingAdapter,
    },
}

impl VectorQuery<'_> {
    fn resolve(&self) -> Option<crate::vector::Embedding> {
        match self {
            VectorQuery::Vec(e) => Some(e.clone()),
            VectorQuery::Text { text, embedder } => embedder.embed(text),
        }
    }
}

fn execute<B: IndexBackend>(
    backend: &B,
    conjoined: &crate::compiler::ConjoinedPlan<AclFilter>,
    vector_query: Option<&VectorQuery<'_>>,
    page: Page,
    stats: &QueryStats,
) -> Result<Vec<Hit>, QueryError> {
    let acl_filter = &conjoined.acl;
    let plan = &conjoined.plan;
    let fetch = page.offset.saturating_add(page.effective_limit());

    let mut ft_ranked: Vec<Hit> = Vec::new();
    for ft in &plan.ft {
        for h in backend.search(acl_filter, &ft.query, fetch)? {
            if !ft_ranked.iter().any(|e| e.doc_id == h.doc_id) {
                ft_ranked.push(h);
            }
        }
        stats.engine_branches.fetch_add(1, Ordering::Relaxed);
    }

    let mut vector_ranked: Vec<crate::vector::VectorHit> = Vec::new();
    if plan.vector.is_some() {
        stats.engine_branches.fetch_add(1, Ordering::Relaxed);
        if let Some(vq) = vector_query {
            if let Some(query_embedding) = vq.resolve() {
                vector_ranked = backend.semantic(acl_filter, &query_embedding, fetch)?;
            }
        }
    }

    let mut fusion_inputs: Vec<crate::fusion::RankedList> = Vec::new();
    if !ft_ranked.is_empty() {
        fusion_inputs.push(crate::fusion::RankedList::from_ranked(
            ft_ranked.iter().map(|h| h.doc_id.clone()),
        ));
    }
    if !vector_ranked.is_empty() {
        fusion_inputs.push(crate::fusion::RankedList::from_ranked(
            vector_ranked.iter().map(|h| h.doc_id.clone()),
        ));
    }
    let fused = crate::fusion::reciprocal_rank_fusion(&fusion_inputs);

    let mut merged: BTreeMap<String, f32> = BTreeMap::new();
    let mut record = |hits: Vec<Hit>| {
        for h in hits {
            let e = merged.entry(h.doc_id).or_insert(f32::MIN);
            if h.score > *e {
                *e = h.score;
            }
        }
    };
    record(
        fused
            .into_iter()
            .map(|f| Hit {
                doc_id: f.doc_id,
                score: f.score,
            })
            .collect(),
    );

    for sc in &plan.structured {
        match sc {
            crate::compiler::StructuredClause::Cmp { field, value, .. } => {
                record(backend.search_structured(
                    acl_filter,
                    field,
                    &to_field_value(value),
                    fetch,
                )?);
                stats.engine_branches.fetch_add(1, Ordering::Relaxed);
            }
            crate::compiler::StructuredClause::In { field, values, .. } => {
                for v in values {
                    record(backend.search_structured(
                        acl_filter,
                        field,
                        &to_field_value(v),
                        fetch,
                    )?);
                    stats.engine_branches.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    if plan.ft.is_empty() && plan.structured.is_empty() && vector_ranked.is_empty() {
        record(backend.search(acl_filter, "*", fetch)?);
        stats.engine_branches.fetch_add(1, Ordering::Relaxed);
    }

    let mut hits: Vec<Hit> = merged
        .into_iter()
        .map(|(doc_id, score)| Hit { doc_id, score })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.doc_id.cmp(&b.doc_id))
    });
    Ok(hits)
}

fn paginate(hits: Vec<Hit>, page: Page) -> Vec<Hit> {
    hits.into_iter()
        .skip(page.offset)
        .take(page.effective_limit())
        .collect()
}

fn to_field_value(value: &myelin_identity::Literal) -> myelin_query::FieldValue {
    use myelin_identity::Literal;
    use myelin_query::FieldValue;
    match value {
        Literal::Int(n) => FieldValue::Int(*n),
        Literal::Bool(b) => FieldValue::Bool(*b),
        Literal::Str(s) => FieldValue::Select(s.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{ConsistencyMode, Literal, ObjectId, PrincipalId, PrincipalKind, Zookie};
    use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate};
    use myelin_tenancy::TenantId;

    use crate::compiler::{FieldDecl, FT_BODY_FIELD};
    use crate::engine::{IndexDocument, TantivyBackend, ORDER_KEY_FIELD};

    fn schema() -> FieldSchema {
        FieldSchema::new()
            .with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
            .with("status", FieldDecl::stored(FieldType::Select))
            .with("severity", FieldDecl::stored(FieldType::Int))
            .with(ORDER_KEY_FIELD, FieldDecl::stored(FieldType::OrderKey))
            .with("progress", FieldDecl::read_time(FieldType::Int))
    }

    fn facet_decl() -> BTreeMap<String, FieldType> {
        let mut m = BTreeMap::new();
        m.insert("status".to_string(), FieldType::Select);
        m.insert("severity".to_string(), FieldType::Int);
        m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
        m
    }

    fn doc(id: &str, text: &str, status: &str, severity: i64) -> IndexDocument {
        let k = OrderKey::bisect(None, None);
        IndexDocument::new(id, text)
            .with_field("status", FieldValue::Select(status.into()))
            .with_field("severity", FieldValue::Int(severity))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
    }

    fn viewer(tenant: &str) -> Principal {
        Principal::stub(
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )
    }

    fn consistency() -> Consistency {
        Consistency {
            at_least: Zookie("z0".into()),
            mode: ConsistencyMode::BoundedStale,
        }
    }

    fn ast(p: Predicate) -> QueryAst {
        QueryAst::compiled(p).expect("within cost bounds")
    }

    fn var(name: &str) -> Expr {
        Expr::Var(name.into())
    }
    fn s(v: &str) -> Expr {
        Expr::Lit(Literal::Str(v.into()))
    }

    struct FakeAuthz {
        answer: ListObjectsResult,
        calls: AtomicU64,
        reverse: Option<ReverseIndexAnswer>,
        resolve_calls: AtomicU64,
    }
    impl FakeAuthz {
        fn new(answer: ListObjectsResult) -> FakeAuthz {
            FakeAuthz {
                answer,
                calls: AtomicU64::new(0),
                reverse: None,
                resolve_calls: AtomicU64::new(0),
            }
        }
        fn ids(ids: &[&str]) -> FakeAuthz {
            FakeAuthz::new(ListObjectsResult::Ids {
                ids: ids.iter().map(|i| ObjectId((*i).into())).collect(),
                zookie: Zookie("z-acl".into()),
            })
        }
        fn filter(set_expr: SetExpr) -> FakeAuthz {
            FakeAuthz::new(ListObjectsResult::Filter {
                set_expr,
                zookie: Zookie("z-acl".into()),
            })
        }
        fn filter_with(set_expr: SetExpr, zookie: &str, reverse: ReverseIndexAnswer) -> FakeAuthz {
            FakeAuthz {
                answer: ListObjectsResult::Filter {
                    set_expr,
                    zookie: Zookie(zookie.into()),
                },
                calls: AtomicU64::new(0),
                reverse: Some(reverse),
                resolve_calls: AtomicU64::new(0),
            }
        }
    }
    impl ListObjectsPort for FakeAuthz {
        fn list_objects(
            &self,
            _subject: &Principal,
            _permission: &Permission,
            _ty: &ObjectType,
            _at: &Consistency,
        ) -> AuthzResult<ListObjectsResult> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.answer.clone())
        }
        fn resolve_relation(
            &self,
            _subject: &Principal,
            _form: &RelationalLeaf,
            _required: &RevisionWatermark,
        ) -> AuthzResult<ReverseIndexAnswer> {
            self.resolve_calls.fetch_add(1, Ordering::Relaxed);
            self.reverse
                .clone()
                .ok_or_else(|| myelin_identity::AuthzError::Unavailable("no reverse index".into()))
        }
    }

    fn corpus() -> TantivyBackend {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        be.upsert(&doc(
            "acme/issue/PUB-1",
            "deadlock in the scheduler",
            "open",
            5,
        ))
        .unwrap();
        be.upsert(&doc(
            "acme/issue/SECRET-9",
            "deadlock secret incident",
            "open",
            9,
        ))
        .unwrap();
        be.upsert(&doc("acme/issue/OTHER-2", "typo in readme", "closed", 1))
            .unwrap();
        be
    }

    fn no_reverse() -> FakeAuthz {
        FakeAuthz::ids(&[])
    }
    fn lower_bounded(set_expr: &SetExpr) -> Result<AclFilter, QueryError> {
        let v = viewer("acme");
        let authz = no_reverse();
        lower_set_expr(
            set_expr,
            &v,
            &authz,
            &RevisionWatermark(0),
            &QueryStats::new(),
        )
    }

    #[test]
    fn lower_all_and_none() {
        assert_eq!(lower_bounded(&SetExpr::All).unwrap(), AclFilter::All);
        assert_eq!(lower_bounded(&SetExpr::None).unwrap(), AclFilter::None);
    }

    #[test]
    fn lower_ids_and_empty_ids() {
        let f = lower_bounded(&SetExpr::Ids(vec![
            ObjectId("a".into()),
            ObjectId("b".into()),
        ]))
        .unwrap();
        assert_eq!(f, AclFilter::Ids(vec!["a".into(), "b".into()]));
        assert_eq!(
            lower_bounded(&SetExpr::Ids(vec![])).unwrap(),
            AclFilter::None
        );
    }

    #[test]
    fn lower_not_ids_and_empty_not_ids() {
        let f = lower_bounded(&SetExpr::NotIds(vec![ObjectId("x".into())])).unwrap();
        assert_eq!(f, AclFilter::NotIds(vec!["x".into()]));
        assert_eq!(
            lower_bounded(&SetExpr::NotIds(vec![])).unwrap(),
            AclFilter::All
        );
    }

    fn rev_answer(ids: &[&str], revision: u64) -> ReverseIndexAnswer {
        ReverseIndexAnswer {
            object_ids: ids.iter().map(|s| (*s).to_string()).collect(),
            revision: RevisionWatermark(revision),
        }
    }

    #[test]
    fn relational_in_relation_lowers_via_reverse_index_join() {
        use myelin_identity::{ColRef, RelName};
        let authz = FakeAuthz::filter_with(
            SetExpr::InRelation {
                relation: RelName("reader".into()),
                via_column: ColRef {
                    table: "issue".into(),
                    column: "id".into(),
                },
            },
            "z@7",
            rev_answer(&["acme/issue/PUB-1", "acme/issue/PUB-2"], 7),
        );
        let v = viewer("acme");
        let stats = QueryStats::new();
        let (f, z) = lower_acl(&authz.answer.clone(), &v, &authz, &stats).unwrap();
        assert_eq!(
            f,
            AclFilter::Ids(vec!["acme/issue/PUB-1".into(), "acme/issue/PUB-2".into()]),
            "the JOIN resolves to the visible-id set as an Ids membership clause (not All)"
        );
        assert_eq!(z, "z@7", "the list_objects zookie is threaded through");
        assert_eq!(
            stats.reverse_index_joins(),
            1,
            "exactly ONE reverse-index JOIN (no N+1)"
        );
    }

    #[test]
    fn relational_tuple_set_empty_resolved_is_deny_not_widen() {
        use myelin_identity::AuthzIndexRef;
        let authz = FakeAuthz::filter_with(
            SetExpr::TupleSet {
                index: AuthzIndexRef("authz_visible".into()),
            },
            "z@3",
            rev_answer(&[], 3),
        );
        let v = viewer("acme");
        let (f, _) = lower_acl(&authz.answer.clone(), &v, &authz, &QueryStats::new()).unwrap();
        assert_eq!(
            f,
            AclFilter::None,
            "an empty resolved set ⇒ deny, never widened to All"
        );
    }

    #[test]
    fn relational_stale_reverse_index_revision_is_refused() {
        use myelin_identity::AuthzIndexRef;
        let authz = FakeAuthz::filter_with(
            SetExpr::TupleSet {
                index: AuthzIndexRef("ix".into()),
            },
            "z@9",
            rev_answer(&["acme/issue/PUB-1"], 4),
        );
        let v = viewer("acme");
        let err = lower_acl(&authz.answer.clone(), &v, &authz, &QueryStats::new())
            .expect_err("a stale reverse-index revision is refused");
        match err {
            QueryError::StaleReverseIndex {
                required, served, ..
            } => {
                assert_eq!(required, 9);
                assert_eq!(served, 4);
            }
            other => panic!("expected StaleReverseIndex, got {other}"),
        }
    }

    #[test]
    fn relational_revision_at_watermark_is_accepted() {
        use myelin_identity::AuthzIndexRef;
        let authz = FakeAuthz::filter_with(
            SetExpr::TupleSet {
                index: AuthzIndexRef("ix".into()),
            },
            "z@5",
            rev_answer(&["acme/issue/PUB-1"], 5),
        );
        let v = viewer("acme");
        let (f, _) = lower_acl(&authz.answer.clone(), &v, &authz, &QueryStats::new()).unwrap();
        assert_eq!(
            f,
            AclFilter::Ids(vec!["acme/issue/PUB-1".into()]),
            "revision == watermark is fresh"
        );
    }

    #[test]
    fn boolean_composition_lowers_to_engine_and_or_not() {
        let u = lower_bounded(&SetExpr::Union(vec![
            SetExpr::Ids(vec![ObjectId("a".into())]),
            SetExpr::Ids(vec![ObjectId("b".into())]),
        ]))
        .unwrap();
        assert_eq!(
            u,
            AclFilter::Or(vec![
                AclFilter::Ids(vec!["a".into()]),
                AclFilter::Ids(vec!["b".into()])
            ])
        );

        let i = lower_bounded(&SetExpr::Intersect(vec![
            SetExpr::All,
            SetExpr::NotIds(vec![ObjectId("x".into())]),
        ]))
        .unwrap();
        assert_eq!(
            i,
            AclFilter::And(vec![AclFilter::All, AclFilter::NotIds(vec!["x".into()])])
        );

        let d = lower_bounded(&SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::Ids(vec![ObjectId("secret".into())])),
        ))
        .unwrap();
        assert_eq!(
            d,
            AclFilter::And(vec![
                AclFilter::All,
                AclFilter::Not(Box::new(AclFilter::Ids(vec!["secret".into()])))
            ])
        );
    }

    #[test]
    fn relational_without_reverse_index_fails_closed() {
        use myelin_identity::AuthzIndexRef;
        let authz = no_reverse();
        let v = viewer("acme");
        let err = lower_set_expr(
            &SetExpr::TupleSet {
                index: AuthzIndexRef("ix".into()),
            },
            &v,
            &authz,
            &RevisionWatermark(0),
            &QueryStats::new(),
        )
        .expect_err("no reverse index ⇒ fail closed, never widen");
        assert!(
            matches!(err, QueryError::Authz(_)),
            "unavailable surfaces, never widens to All"
        );
    }

    #[test]
    fn lower_materialised_ids_result() {
        let v = viewer("acme");
        let authz = no_reverse();
        let stats = QueryStats::new();
        let (f, z) = lower_acl(
            &ListObjectsResult::Ids {
                ids: vec![ObjectId("d1".into())],
                zookie: Zookie("zX".into()),
            },
            &v,
            &authz,
            &stats,
        )
        .unwrap();
        assert_eq!(f, AclFilter::Ids(vec!["d1".into()]));
        assert_eq!(z, "zX");
        let (empty, _) = lower_acl(
            &ListObjectsResult::Ids {
                ids: vec![],
                zookie: Zookie("z".into()),
            },
            &v,
            &authz,
            &stats,
        )
        .unwrap();
        assert_eq!(empty, AclFilter::None, "empty materialised set ⇒ deny");
    }

    #[test]
    fn exactly_one_list_objects_call_per_query_no_n_plus_1() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::ids(&["acme/issue/PUB-1", "acme/issue/OTHER-2"]);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        assert_eq!(
            stats.list_objects_calls(),
            1,
            "EXACTLY one list_objects per query (no N+1)"
        );
        assert_eq!(
            authz.calls.load(Ordering::Relaxed),
            1,
            "the port saw exactly one call"
        );
        assert_eq!(
            stats.reverse_index_joins(),
            0,
            "a bounded-set (Ids) query does NO reverse-index JOIN"
        );
        assert_eq!(
            res.hits
                .iter()
                .map(|h| h.doc_id.as_str())
                .collect::<Vec<_>>(),
            ["acme/issue/PUB-1"]
        );
        assert_eq!(
            res.zookie, "z-acl",
            "the list_objects zookie is threaded onto the result"
        );
    }

    #[test]
    fn acl_conjoins_into_branch_unauthorized_then_granted() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let q = ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(FT_BODY_FIELD),
            rhs: s("deadlock"),
        });

        let unauth = FakeAuthz::ids(&["acme/issue/PUB-1"]);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &unauth,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("q");
        let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            ["acme/issue/PUB-1"],
            "the confidential doc is excluded (pre-filter, no leak)"
        );

        let granted = FakeAuthz::ids(&["acme/issue/PUB-1", "acme/issue/SECRET-9"]);
        let stats2 = QueryStats::new();
        let res2 = query(
            &eng,
            &granted,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats2,
        )
        .expect("q2");
        let ids2: std::collections::BTreeSet<&str> =
            res2.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            ids2.contains("acme/issue/SECRET-9"),
            "after grant the confidential doc is visible"
        );
        assert!(ids2.contains("acme/issue/PUB-1"));
    }

    #[test]
    fn none_short_circuits_to_empty_without_touching_the_engine() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::filter(SetExpr::None);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        assert!(res.hits.is_empty(), "None ⇒ empty result");
        assert_eq!(
            stats.engine_branches(),
            0,
            "no engine branch ran (short-circuit, no count leak)"
        );
        assert_eq!(
            stats.list_objects_calls(),
            1,
            "still exactly one list_objects call"
        );
    }

    #[test]
    fn all_admits_every_matching_doc() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::filter(SetExpr::All);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        let ids: std::collections::BTreeSet<&str> =
            res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            ids.contains("acme/issue/PUB-1") && ids.contains("acme/issue/SECRET-9"),
            "admin sees both `deadlock` docs: {ids:?}"
        );
    }

    #[test]
    fn not_ids_deny_set_hides_only_the_denied() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::filter(SetExpr::NotIds(vec![ObjectId(
            "acme/issue/SECRET-9".into(),
        )]));
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            ["acme/issue/PUB-1"],
            "the denied doc is excluded, the rest surface"
        );
    }

    #[test]
    fn structured_branch_conjoins_acl() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::ids(&["acme/issue/PUB-1"]);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("status"),
                rhs: s("open"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        assert_eq!(
            res.hits
                .iter()
                .map(|h| h.doc_id.as_str())
                .collect::<Vec<_>>(),
            ["acme/issue/PUB-1"],
            "the structured branch excludes the ACL-denied doc"
        );
        assert!(stats.engine_branches() >= 1, "a structured branch ran");
    }

    #[test]
    fn relational_tuple_set_chained_grant_through_query_no_leak() {
        use myelin_identity::AuthzIndexRef;
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let q = ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(FT_BODY_FIELD),
            rhs: s("deadlock"),
        });
        let tuple_set = SetExpr::TupleSet {
            index: AuthzIndexRef("authz_visible".into()),
        };

        let unauth = FakeAuthz::filter_with(
            tuple_set.clone(),
            "z@5",
            rev_answer(&["acme/issue/PUB-1"], 5),
        );
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &unauth,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("q");
        let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            ["acme/issue/PUB-1"],
            "the confidential doc is excluded (no leak, big-result path)"
        );
        assert_eq!(
            stats.reverse_index_joins(),
            1,
            "exactly ONE reverse-index JOIN (no N+1)"
        );
        assert_eq!(
            stats.list_objects_calls(),
            1,
            "and exactly one list_objects"
        );

        let granted = FakeAuthz::filter_with(
            tuple_set,
            "z@6",
            rev_answer(&["acme/issue/PUB-1", "acme/issue/SECRET-9"], 6),
        );
        let stats2 = QueryStats::new();
        let res2 = query(
            &eng,
            &granted,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats2,
        )
        .expect("q2");
        let ids2: std::collections::BTreeSet<&str> =
            res2.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            ids2.contains("acme/issue/SECRET-9"),
            "after grant the confidential doc is visible"
        );
        assert!(ids2.contains("acme/issue/PUB-1"));
    }

    #[test]
    fn relational_difference_through_query_excludes_the_relation_set() {
        use myelin_identity::AuthzIndexRef;
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let q = ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(FT_BODY_FIELD),
            rhs: s("deadlock"),
        });
        let set_expr = SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::TupleSet {
                index: AuthzIndexRef("blocked".into()),
            }),
        );
        let authz =
            FakeAuthz::filter_with(set_expr, "z@2", rev_answer(&["acme/issue/SECRET-9"], 2));
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("q");
        let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            ["acme/issue/PUB-1"],
            "the relation-reached doc is excluded by the Difference"
        );
    }

    #[test]
    fn srch_d3_cross_tenant_zero_results() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let evil = viewer("evil");
        let authz = FakeAuthz::filter(SetExpr::All);
        let stats = QueryStats::new();
        let err = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &evil,
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect_err("a cross-tenant query is rejected (SRCH-D3)");
        assert!(
            matches!(err, QueryError::TenantMismatch { .. }),
            "cross-tenant ⇒ TenantMismatch"
        );
        assert_eq!(stats.engine_branches(), 0, "0 cross-tenant engine touches");
        assert_eq!(
            stats.list_objects_calls(),
            0,
            "rejected before any authz/engine work"
        );
        assert!(
            err.to_string().contains("SRCH-D3"),
            "the error names the drill"
        );
    }

    #[test]
    fn same_tenant_viewer_is_accepted() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::ids(&["acme/issue/PUB-1"]);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        );
        assert!(res.is_ok(), "the same-tenant viewer is admitted");
    }

    #[test]
    fn read_time_predicate_is_carried_post_fetch() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::filter(SetExpr::All);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp {
                op: CmpOp::Ge,
                lhs: var("progress"),
                rhs: Expr::Lit(Literal::Int(80)),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        assert_eq!(
            res.post_fetch_fields,
            vec!["progress".to_string()],
            "the read-time predicate is carried for post-fetch evaluation by the view"
        );
    }

    #[test]
    fn authz_failure_surfaces_never_widens() {
        struct FailingAuthz;
        impl ListObjectsPort for FailingAuthz {
            fn list_objects(
                &self,
                _s: &Principal,
                _p: &Permission,
                _t: &ObjectType,
                _a: &Consistency,
            ) -> AuthzResult<ListObjectsResult> {
                Err(myelin_identity::AuthzError::Unavailable("id down".into()))
            }
        }
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let stats = QueryStats::new();
        let err = query(
            &eng,
            &FailingAuthz,
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect_err("an authz failure is surfaced, not widened");
        assert!(
            matches!(err, QueryError::Authz(_)),
            "the authz error surfaces loudly"
        );
        assert_eq!(
            stats.engine_branches(),
            0,
            "no engine query ran on an authz failure"
        );
    }

    #[test]
    fn pagination_slices_and_clamps_limit() {
        let hits: Vec<Hit> = (0..10)
            .map(|i| Hit {
                doc_id: format!("d{i:02}"),
                score: (10 - i) as f32,
            })
            .collect();
        let page = Page {
            offset: 2,
            limit: 3,
        };
        let sliced = paginate(hits.clone(), page);
        assert_eq!(
            sliced.iter().map(|h| h.doc_id.clone()).collect::<Vec<_>>(),
            ["d02", "d03", "d04"],
            "the page window is offset..offset+limit"
        );
        let huge = Page {
            offset: 0,
            limit: usize::MAX,
        };
        assert_eq!(
            huge.effective_limit(),
            Page::MAX_LIMIT,
            "a crafted limit is clamped"
        );
    }

    #[test]
    fn scoped_engine_exposes_its_partition_key() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        assert_eq!(
            eng.tenant(),
            "acme",
            "the tenant accessor returns the opened tenant verbatim"
        );
        assert_eq!(
            eng.region(),
            "eu-west",
            "the region accessor returns the opened region verbatim"
        );
    }

    #[test]
    fn fusion_keeps_the_max_score_across_branches() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::filter(SetExpr::All);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::And(vec![
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
            ])),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        let pub1 = res
            .hits
            .iter()
            .find(|h| h.doc_id == "acme/issue/PUB-1")
            .expect("PUB-1 surfaces");
        assert!(
            pub1.score > 0.0,
            "the fused score is the MAX (the BM25 FT score), not the structured branch's 0.0: {}",
            pub1.score
        );
        assert!(
            stats.engine_branches() >= 2,
            "both the FT and structured branches ran"
        );
    }

    use crate::compiler::SEMANTIC_FIELD;
    use crate::indexer::MockEmbeddingAdapter;
    use crate::vector::Embedding;
    use crate::EmbeddingAdapter;

    fn embedded_corpus(embedder: &MockEmbeddingAdapter) -> TantivyBackend {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let mut emb = |id: &str, body: &str, status: &str| {
            let v = embedder.embed(body).expect("non-empty body embeds");
            let k = OrderKey::bisect(None, None);
            let d = IndexDocument::new(id, body)
                .with_field("status", FieldValue::Select(status.into()))
                .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
                .with_embedding(v, embedder.model_ref());
            be.upsert(&d).unwrap();
        };
        emb("acme/issue/PUB-1", "deadlock in the scheduler", "open");
        emb("acme/issue/PUB-2", "deadlock in the indexer", "open");
        emb("acme/issue/SECRET-9", "deadlock secret ops runbook", "open");
        be
    }

    fn semantic_ast(query_text: &str) -> QueryAst {
        ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(SEMANTIC_FIELD),
            rhs: s(query_text),
        })
    }

    fn hybrid_ast(ft_text: &str, semantic_text: &str) -> QueryAst {
        ast(Predicate::And(vec![
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s(ft_text),
            },
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(SEMANTIC_FIELD),
                rhs: s(semantic_text),
            },
        ]))
    }

    #[test]
    fn semantic_filter_during_traversal_excludes_confidential_then_grant_makes_visible() {
        let embedder = MockEmbeddingAdapter::new(16);
        let be = embedded_corpus(&embedder);
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let vq = VectorQuery::Text {
            text: "deadlock secret ops runbook".into(),
            embedder: &embedder,
        };
        let q = semantic_ast("deadlock secret ops runbook");

        let unauth = FakeAuthz::ids(&["acme/issue/PUB-1", "acme/issue/PUB-2"]);
        let stats = QueryStats::new();
        let cstats = crate::consistency::ConsistencyStats::new();
        let res = semantic(
            &eng,
            &unauth,
            None,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            &vq,
            Page::FIRST,
            &stats,
            &cstats,
        )
        .expect("semantic");
        let ids: std::collections::BTreeSet<&str> =
            res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            !ids.contains("acme/issue/SECRET-9"),
            "the confidential doc NEVER surfaces in the semantic/RAG result (SRCH-D1 vector half: 0 leak)"
        );
        assert!(
            ids.contains("acme/issue/PUB-1") && ids.contains("acme/issue/PUB-2"),
            "visible neighbours"
        );
        assert_eq!(
            stats.list_objects_calls(),
            1,
            "exactly ONE list_objects (no N+1 on the semantic path)"
        );

        let granted = FakeAuthz::ids(&[
            "acme/issue/PUB-1",
            "acme/issue/PUB-2",
            "acme/issue/SECRET-9",
        ]);
        let stats2 = QueryStats::new();
        let cstats2 = crate::consistency::ConsistencyStats::new();
        let res2 = semantic(
            &eng,
            &granted,
            None,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            &vq,
            Page::FIRST,
            &stats2,
            &cstats2,
        )
        .expect("semantic after grant");
        let ids2: std::collections::BTreeSet<&str> =
            res2.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            ids2.contains("acme/issue/SECRET-9"),
            "after grant the doc is in the visible neighbours"
        );
    }

    #[test]
    fn semantic_accepts_a_directly_supplied_query_vector() {
        let embedder = MockEmbeddingAdapter::new(16);
        let be = embedded_corpus(&embedder);
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let query_vec: Embedding = embedder.embed("deadlock in the scheduler").unwrap();
        let vq = VectorQuery::Vec(query_vec);
        let q = semantic_ast("ignored - the vec form supplies the embedding");

        let authz = FakeAuthz::ids(&["acme/issue/PUB-1", "acme/issue/PUB-2"]);
        let stats = QueryStats::new();
        let cstats = crate::consistency::ConsistencyStats::new();
        let res = semantic(
            &eng,
            &authz,
            None,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            &vq,
            Page::FIRST,
            &stats,
            &cstats,
        )
        .expect("semantic");
        assert_eq!(
            res.hits[0].doc_id, "acme/issue/PUB-1",
            "the exact-text doc is the nearest visible vector"
        );
    }

    #[test]
    fn hybrid_rrf_fusion_no_hidden_doc_and_fuses_both_branches() {
        let embedder = MockEmbeddingAdapter::new(16);
        let be = embedded_corpus(&embedder);
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let vq = VectorQuery::Text {
            text: "deadlock in the scheduler".into(),
            embedder: &embedder,
        };
        let q = hybrid_ast("deadlock", "deadlock in the scheduler");

        let authz = FakeAuthz::ids(&["acme/issue/PUB-1", "acme/issue/PUB-2"]);
        let stats = QueryStats::new();
        let cstats = crate::consistency::ConsistencyStats::new();
        let res = semantic(
            &eng,
            &authz,
            None,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            &vq,
            Page::FIRST,
            &stats,
            &cstats,
        )
        .expect("hybrid");
        let ids: std::collections::BTreeSet<&str> =
            res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            !ids.contains("acme/issue/SECRET-9"),
            "RRF introduces no hidden doc (SRCH-D1 vector half)"
        );
        assert_eq!(
            res.hits[0].doc_id, "acme/issue/PUB-1",
            "the doc both branches rank fuses to the top (RRF)"
        );
        assert!(
            stats.engine_branches() >= 2,
            "the FT and the vector branch both executed"
        );
        assert_eq!(
            stats.list_objects_calls(),
            1,
            "ONE list_objects for the hybrid query (no N+1)"
        );
    }

    #[test]
    fn semantic_reuses_the_no_stale_grant_zookie_path() {
        use myelin_identity::ConsistencyMode;
        let embedder = MockEmbeddingAdapter::new(16);
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let v = embedder.embed("deadlock in the scheduler").unwrap();
        let k = OrderKey::bisect(None, None);
        let d = IndexDocument::new("acme/issue/PUB-1", "deadlock in the scheduler")
            .with_field("status", FieldValue::Select("open".into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
            .with_embedding(v, embedder.model_ref());
        be.upsert_stamped(&d, "z@1", 1).unwrap();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());

        let vq = VectorQuery::Text {
            text: "deadlock in the scheduler".into(),
            embedder: &embedder,
        };
        let q = semantic_ast("deadlock in the scheduler");
        let authz = FakeAuthz::ids(&["acme/issue/PUB-1"]);
        let strong = Consistency {
            at_least: myelin_identity::Zookie("z@9".into()),
            mode: ConsistencyMode::Strong,
        };
        let stats = QueryStats::new();
        let cstats = crate::consistency::ConsistencyStats::new();
        let res = semantic(
            &eng,
            &authz,
            None,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &strong,
            &vq,
            Page::FIRST,
            &stats,
            &cstats,
        )
        .expect("semantic strong");
        assert!(
            res.hits.is_empty(),
            "the stale-indexed vector candidate is excluded pending re-index (no-stale-grant for RAG; fail closed)"
        );
    }

    #[test]
    fn query_error_messages_are_loud() {
        let tm = QueryError::TenantMismatch {
            viewer_tenant: "evil".into(),
            engine_tenant: "acme".into(),
        };
        let s = tm.to_string();
        assert!(s.contains("evil") && s.contains("acme") && s.contains("SRCH-D3"));
        let stale = QueryError::StaleReverseIndex {
            required: 9,
            served: 4,
            form: "TupleSet",
        };
        let sm = stale.to_string();
        assert!(
            sm.contains("TupleSet") && sm.contains("SRCH-P09"),
            "the stale-revision error is loud"
        );
        assert!(
            sm.contains('9') && sm.contains('4'),
            "it names the required + served revisions"
        );
    }
}
