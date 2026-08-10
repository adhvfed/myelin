use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_events::ArtifactRef;
use myelin_identity::{
    ColRef, Consistency, ListObjectsResult, ObjectId, Permission, Principal, SetExpr, Zookie,
};
use myelin_tenancy::{Region, TenantId};

use crate::edge_builder::{EdgeProjection, EdgeRow};

pub const VIEW_PERMISSION: &str = "view";

pub const SOURCE_ROOT_COLUMN: &str = "source_root";

pub const AUTHZ_VISIBLE_TABLE: &str = "authz_visible";

pub const FILTER_MODE_SPLIT_SIGNAL: &str = "refs.backlink_filter_mode";

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
pub struct SourceRootFilter {
    pub sql_predicate: String,
    pub joins: Vec<AuthzJoin>,
    pub params: Vec<BoundParam>,
}

impl SourceRootFilter {
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

pub fn lower_over_source_root(set_expr: &SetExpr, viewer: &Principal) -> SourceRootFilter {
    let via = source_root_colref();
    let mut ctx = LowerCtx::new(&viewer.principal_id.0, &via);
    let sql_predicate = lower_expr(set_expr, &mut ctx);
    SourceRootFilter {
        sql_predicate,
        joins: ctx.joins,
        params: ctx.params,
    }
}

pub fn source_root_colref() -> ColRef {
    ColRef {
        table: "edge".into(),
        column: SOURCE_ROOT_COLUMN.into(),
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

#[derive(Clone, Default)]
pub struct AuthzVisibleIndex {
    watermark: Arc<std::sync::Mutex<WatermarkMap>>,
    visible: Arc<std::sync::Mutex<VisibleMap>>,
}

type WatermarkMap = std::collections::HashMap<(String, String), String>;
type VisibleMap = std::collections::HashMap<(String, String, String, String), Vec<String>>;

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
        self.visible
            .lock()
            .unwrap()
            .entry(key)
            .or_default()
            .push(object_id.into());
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

    pub fn watermark(&self, tenant: &TenantId, region: &Region) -> String {
        self.watermark
            .lock()
            .unwrap()
            .get(&(tenant.0.clone(), region.0.clone()))
            .cloned()
            .unwrap_or_default()
    }

    fn visible(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
        relation: &str,
        object_id: &str,
    ) -> bool {
        let key = (
            tenant.0.clone(),
            region.0.clone(),
            subject.into(),
            relation.into(),
        );
        self.visible
            .lock()
            .unwrap()
            .get(&key)
            .map(|s| s.iter().any(|o| o == object_id))
            .unwrap_or(false)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatermarkVerdict {
    JoinServes,
    FallBackToCheck { required: String, watermark: String },
}

pub fn watermark_verdict(
    index: &AuthzVisibleIndex,
    tenant: &TenantId,
    region: &Region,
    filter: &SourceRootFilter,
    at: &Consistency,
) -> WatermarkVerdict {
    if !filter.depends_on_reverse_index() {
        return WatermarkVerdict::JoinServes;
    }
    if at.at_least.0.is_empty() {
        return WatermarkVerdict::JoinServes;
    }
    let watermark = index.watermark(tenant, region);
    if watermark >= at.at_least.0 {
        WatermarkVerdict::JoinServes
    } else {
        WatermarkVerdict::FallBackToCheck {
            required: at.at_least.0.clone(),
            watermark,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlink {
    pub source: ArtifactRef,
    pub source_root: ArtifactRef,
    pub rel: String,
    pub rel_class: String,
    pub origin_actor: String,
}

impl Backlink {
    fn from_row(row: &EdgeRow) -> Backlink {
        Backlink {
            source: row.source.clone(),
            source_root: row.source_root.clone(),
            rel: row.rel.clone(),
            rel_class: row.rel_class.as_str().into(),
            origin_actor: row.origin_actor.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacklinkPage {
    pub edges: Vec<Backlink>,
    pub mode: FilterMode,
    pub fell_back_to_check: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BacklinkError {
    InvalidPage,
}

impl core::fmt::Display for BacklinkError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BacklinkError::InvalidPage => write!(f, "page size must be > 0 (always paginated)"),
        }
    }
}

impl std::error::Error for BacklinkError {}

#[derive(Clone)]
pub struct BacklinkRead {
    edges: EdgeProjection,
    authz: AuthzVisibleIndex,
    query_count: Arc<AtomicU64>,
    ids_mode_reads: Arc<AtomicU64>,
    pushed_down_reads: Arc<AtomicU64>,
}

impl BacklinkRead {
    pub fn new(edges: EdgeProjection, authz: AuthzVisibleIndex) -> BacklinkRead {
        BacklinkRead {
            edges,
            authz,
            query_count: Arc::new(AtomicU64::new(0)),
            ids_mode_reads: Arc::new(AtomicU64::new(0)),
            pushed_down_reads: Arc::new(AtomicU64::new(0)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn backlinks(
        &self,
        tenant: &TenantId,
        region: &Region,
        target_root: &ArtifactRef,
        viewer: &Principal,
        list_objects: &ListObjectsResult,
        at: &Consistency,
        page: usize,
    ) -> Result<BacklinkPage, BacklinkError> {
        if page == 0 {
            return Err(BacklinkError::InvalidPage);
        }

        self.query_count.fetch_add(1, Ordering::SeqCst);
        let candidates = self.edges.inbound_live(tenant, region, target_root);

        let (set_expr, mode) = match list_objects {
            ListObjectsResult::Ids { ids, .. } => {
                self.ids_mode_reads.fetch_add(1, Ordering::SeqCst);
                (SetExpr::Ids(ids.clone()), FilterMode::Ids)
            }
            ListObjectsResult::Filter { set_expr, .. } => {
                self.pushed_down_reads.fetch_add(1, Ordering::SeqCst);
                (set_expr.clone(), FilterMode::PushedDown)
            }
        };

        let filter = lower_over_source_root(&set_expr, viewer);

        let verdict = watermark_verdict(&self.authz, tenant, region, &filter, at);
        let fell_back_to_check = matches!(verdict, WatermarkVerdict::FallBackToCheck { .. });

        let admitted: Vec<Backlink> = candidates
            .iter()
            .filter(|row| {
                set_expr_admits(
                    &set_expr,
                    &self.authz,
                    viewer,
                    tenant,
                    region,
                    &row.source_root,
                )
            })
            .map(Backlink::from_row)
            .take(page)
            .collect();

        Ok(BacklinkPage {
            edges: admitted,
            mode,
            fell_back_to_check,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn edges(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_root: &ArtifactRef,
        viewer: &Principal,
        list_objects: &ListObjectsResult,
        at: &Consistency,
        page: usize,
    ) -> Result<BacklinkPage, BacklinkError> {
        self.backlinks(tenant, region, ref_root, viewer, list_objects, at, page)
    }

    pub fn authz_index(&self) -> &AuthzVisibleIndex {
        &self.authz
    }

    pub fn edge_projection(&self) -> &EdgeProjection {
        &self.edges
    }

    pub fn query_count(&self) -> u64 {
        self.query_count.load(Ordering::SeqCst)
    }

    pub fn filter_mode_split(&self) -> (u64, u64) {
        (
            self.ids_mode_reads.load(Ordering::SeqCst),
            self.pushed_down_reads.load(Ordering::SeqCst),
        )
    }
}

pub fn set_expr_admits(
    set_expr: &SetExpr,
    authz: &AuthzVisibleIndex,
    viewer: &Principal,
    tenant: &TenantId,
    region: &Region,
    source_root: &ArtifactRef,
) -> bool {
    match set_expr {
        SetExpr::All => true,
        SetExpr::None => false,
        SetExpr::Ids(ids) => ids.iter().any(|id| id.0 == source_root.0),
        SetExpr::NotIds(ids) => !ids.iter().any(|id| id.0 == source_root.0),
        SetExpr::InRelation { relation, .. } => authz.visible(
            tenant,
            region,
            &viewer.principal_id.0,
            &relation.0,
            &source_root.0,
        ),
        SetExpr::TupleSet { index } => authz.visible(
            tenant,
            region,
            &viewer.principal_id.0,
            &index.0,
            &source_root.0,
        ),
        SetExpr::Union(parts) => parts
            .iter()
            .any(|p| set_expr_admits(p, authz, viewer, tenant, region, source_root)),
        SetExpr::Intersect(parts) => parts
            .iter()
            .all(|p| set_expr_admits(p, authz, viewer, tenant, region, source_root)),
        SetExpr::Difference(a, b) => {
            set_expr_admits(a, authz, viewer, tenant, region, source_root)
                && !set_expr_admits(b, authz, viewer, tenant, region, source_root)
        }
    }
}

pub fn view_permission() -> Permission {
    Permission(VIEW_PERMISSION.into())
}

pub fn ids_result(ids: &[&str], zookie: &str) -> ListObjectsResult {
    ListObjectsResult::Ids {
        ids: ids.iter().map(|s| ObjectId((*s).into())).collect(),
        zookie: Zookie(zookie.into()),
    }
}

#[cfg(test)]
mod tests;
