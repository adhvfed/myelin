use crate::deployment::DeployState;
use myelin_identity::{
    ColRef, Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission,
    Principal, SetExpr, Zookie,
};
use myelin_refs::{ArtifactRef, Sub};
use myelin_tenancy::{Region, TenantId};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub const CI_SUBSYSTEM: &str = "ci";

pub const VIEW: &str = "view";

pub const RUN_LIST_PERMISSION: &str = "read";

pub fn ci_run_id_colref() -> ColRef {
    ColRef {
        table: "ci_run".into(),
        column: "run_id".into(),
    }
}

pub const AUTHZ_VISIBLE_TABLE: &str = "authz_visible";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiArtifactType {
    Run,
    Deployment,
    Pipeline,
    Runner,
    Artifact,
}

impl CiArtifactType {
    pub const fn token(self) -> &'static str {
        match self {
            CiArtifactType::Run => "run",
            CiArtifactType::Deployment => "deployment",
            CiArtifactType::Pipeline => "pipeline",
            CiArtifactType::Runner => "runner",
            CiArtifactType::Artifact => "artifact",
        }
    }
}

pub fn ci_run_ref(tenant: &str, run_id: &str) -> ArtifactRef {
    mint_root(tenant, CiArtifactType::Run, run_id)
}

pub fn ci_deployment_ref(tenant: &str, dep_id: &str) -> ArtifactRef {
    mint_root(tenant, CiArtifactType::Deployment, dep_id)
}

pub fn ci_pipeline_ref(tenant: &str, pipeline_id: &str) -> ArtifactRef {
    mint_root(tenant, CiArtifactType::Pipeline, pipeline_id)
}

pub fn ci_runner_ref(tenant: &str, runner_id: &str) -> ArtifactRef {
    mint_root(tenant, CiArtifactType::Runner, runner_id)
}

pub fn ci_artifact_ref(tenant: &str, artifact_id: &str) -> ArtifactRef {
    mint_root(tenant, CiArtifactType::Artifact, artifact_id)
}

fn mint_root(tenant: &str, ty: CiArtifactType, id: &str) -> ArtifactRef {
    myelin_refs::parse(&format!(
        "myelin://{tenant}/{CI_SUBSYSTEM}/{}/{id}",
        ty.token()
    ))
    .expect("CI mints a grammatical canonical ArtifactRef (contract 5.1)")
}

pub fn run_step_ref(
    run_ref: &ArtifactRef,
    step: u64,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(run_ref, Sub::Step(step))
}

pub fn run_step_line_ref(
    run_ref: &ArtifactRef,
    start: u64,
    end: u64,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(run_ref, Sub::LineRange { start, end })
}

pub fn commit_check_ref(
    root: &ArtifactRef,
    context: &str,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(root, Sub::Check(context.to_string()))
}

fn classify(r: &ArtifactRef) -> Result<CiArtifactType, ProjectError> {
    let rest =
        r.0.strip_prefix("myelin://")
            .ok_or_else(|| ProjectError::NotACiArtifact {
                reference: r.0.clone(),
            })?;
    let scope = rest.split('#').next().unwrap_or(rest);
    let segments: Vec<&str> = scope.split('/').collect();
    if segments.len() != 4 || segments[1] != CI_SUBSYSTEM {
        return Err(ProjectError::NotACiArtifact {
            reference: r.0.clone(),
        });
    }
    match segments[2] {
        "run" => Ok(CiArtifactType::Run),
        "deployment" => Ok(CiArtifactType::Deployment),
        "pipeline" => Ok(CiArtifactType::Pipeline),
        "runner" => Ok(CiArtifactType::Runner),
        "artifact" => Ok(CiArtifactType::Artifact),
        other => Err(ProjectError::UnknownCiType {
            ty: other.to_string(),
        }),
    }
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

pub fn lower_over_run_id(set_expr: &SetExpr, viewer: &Principal) -> LoweredFilter {
    let via = ci_run_id_colref();
    let mut ctx = LowerCtx::new(&viewer.principal_id.0, &via);
    let sql_predicate = lower_expr(set_expr, &mut ctx);
    LoweredFilter {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposedRunListQuery {
    pub sql: String,
    pub params: Vec<BoundParam>,
}

impl ComposedRunListQuery {
    pub fn statement_count(&self) -> usize {
        self.sql.split(';').filter(|s| !s.trim().is_empty()).count()
    }
}

pub fn compose_run_list_query(
    set_expr: &SetExpr,
    viewer: &Principal,
    scope_tenant: &TenantId,
    scope_region: &Region,
) -> ComposedRunListQuery {
    let lowered = lower_over_run_id(set_expr, viewer);
    let joins: String = lowered
        .joins
        .iter()
        .map(|j| format!(" {}", j.clause))
        .collect();
    let sql = format!(
        "SELECT ci_run.run_id FROM ci_run{joins} \
         WHERE ci_run.tenant_id = :tenant AND ci_run.region = :region \
         AND ({acl}) ORDER BY ci_run.run_id LIMIT :page",
        acl = lowered.sql_predicate,
    );
    let mut params = vec![
        BoundParam {
            placeholder: ":tenant".into(),
            value: scope_tenant.0.clone(),
        },
        BoundParam {
            placeholder: ":region".into(),
            value: scope_region.0.clone(),
        },
    ];
    params.extend(lowered.params);
    ComposedRunListQuery { sql, params }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiSearchPreFilter {
    pub acl_filter: LoweredFilter,
}

pub fn run_search_pre_filter(set_expr: &SetExpr, viewer: &Principal) -> CiSearchPreFilter {
    CiSearchPreFilter {
        acl_filter: lower_over_run_id(set_expr, viewer),
    }
}

#[derive(Clone, Default)]
pub struct AuthzVisibleIndex {
    visible: Arc<Mutex<VisibleMap>>,
}

type VisibleMap = HashMap<(String, String, String, String), Vec<String>>;

impl AuthzVisibleIndex {
    pub fn new() -> AuthzVisibleIndex {
        AuthzVisibleIndex::default()
    }

    fn visible(&self) -> std::sync::MutexGuard<'_, VisibleMap> {
        self.visible
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn grant(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
        relation: &str,
        object_id: &str,
    ) {
        let key = (
            tenant.0.clone(),
            region.0.clone(),
            subject.into(),
            relation.into(),
        );
        let mut v = self.visible();
        let set = v.entry(key).or_default();
        if !set.iter().any(|o| o == object_id) {
            set.push(object_id.into());
        }
    }

    pub fn revoke(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
        relation: &str,
        object_id: &str,
    ) {
        let key = (
            tenant.0.clone(),
            region.0.clone(),
            subject.into(),
            relation.into(),
        );
        if let Some(set) = self.visible().get_mut(&key) {
            set.retain(|o| o != object_id);
        }
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
                .visible()
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
                i += 4;
                continue;
            }
        }
        let c = chars[i];
        if c == '(' && depth_in_leaf == 0 && cur.trim().is_empty() {
            out.push("(".into());
            i += 1;
            continue;
        }
        if c == ')' {
            if depth_in_leaf > 0 {
                depth_in_leaf -= 1;
                cur.push(')');
                i += 1;
                continue;
            }
            flush(&mut cur, &mut out);
            out.push(")".into());
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    flush(&mut cur, &mut out);
    out
}

fn parse_or(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    let mut v = parse_and(tokens, pos, leaf);
    while *pos < tokens.len() && tokens[*pos] == "OR" {
        *pos += 1;
        let r = parse_and(tokens, pos, leaf);
        v = v || r;
    }
    v
}

fn parse_and(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    let mut v = parse_unary(tokens, pos, leaf);
    while *pos < tokens.len() && (tokens[*pos] == "AND" || tokens[*pos] == "AND NOT") {
        let negate = tokens[*pos] == "AND NOT";
        *pos += 1;
        let mut r = parse_unary(tokens, pos, leaf);
        if negate {
            r = !r;
        }
        v = v && r;
    }
    v
}

fn parse_unary(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    if *pos < tokens.len() && tokens[*pos] == "NOT" {
        *pos += 1;
        return !parse_unary(tokens, pos, leaf);
    }
    parse_primary(tokens, pos, leaf)
}

fn parse_primary(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
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
    let frag = tokens[*pos].clone();
    *pos += 1;
    leaf(&frag)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    pub title: String,
    pub state: String,
    pub icon: String,
    pub render_hint: Option<RenderHint>,
    pub sub_anchor: Option<SubAnchor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderHint {
    Run {
        dag_summary: String,
        failed_step: Option<u64>,
        duration_secs: Option<u64>,
    },
    Deployment {
        env: String,
        risk: String,
        rollback_available: bool,
    },
    Pipeline {
        last_run: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubAnchor {
    pub kind: String,
    pub step: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    Unauthorized,
    Erased,
    Restricted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    pub reason: TombstoneReason,
}

impl Tombstone {
    pub fn display_text(&self) -> &'static str {
        "(not available)"
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Projected {
    Visible(Projection),
    Tombstoned(Tombstone),
}

impl Projected {
    pub fn is_visible(&self) -> bool {
        matches!(self, Projected::Visible(_))
    }

    pub fn is_tombstone(&self) -> bool {
        matches!(self, Projected::Tombstoned(_))
    }

    pub fn title(&self) -> Option<&str> {
        match self {
            Projected::Visible(p) => Some(&p.title),
            Projected::Tombstoned(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    NotACiArtifact { reference: String },
    UnknownCiType { ty: String },
    NotFound { reference: String },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::NotACiArtifact { reference } => write!(
                f,
                "not a CI artifact: `{reference}` - CI's projector does not own this ref"
            ),
            ProjectError::UnknownCiType { ty } => write!(f, "unknown CI artifact type `{ty}`"),
            ProjectError::NotFound { reference } => {
                write!(f, "no CI artifact found for `{reference}` (dangling ref)")
            }
        }
    }
}

impl std::error::Error for ProjectError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunMeta {
    pub number: u64,
    pub pipeline: String,
    pub state: String,
    pub dag_summary: String,
    pub failed_step: Option<u64>,
    pub duration_secs: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeploymentMeta {
    pub env: String,
    pub version: String,
    pub state: DeployState,
    pub risk: String,
    pub rollback_available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineMeta {
    pub name: String,
    pub valid: bool,
    pub last_run: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ArtifactStore {
    runs: HashMap<String, RunMeta>,
    deployments: HashMap<String, DeploymentMeta>,
    pipelines: HashMap<String, PipelineMeta>,
    erased: HashSet<String>,
    restricted: HashSet<String>,
}

impl ArtifactStore {
    pub fn new() -> ArtifactStore {
        ArtifactStore::default()
    }

    pub fn put_run(&mut self, canonical_ref: &ArtifactRef, meta: RunMeta) {
        self.runs.insert(canonical_ref.0.clone(), meta);
    }

    pub fn put_deployment(&mut self, canonical_ref: &ArtifactRef, meta: DeploymentMeta) {
        self.deployments.insert(canonical_ref.0.clone(), meta);
    }

    pub fn put_pipeline(&mut self, canonical_ref: &ArtifactRef, meta: PipelineMeta) {
        self.pipelines.insert(canonical_ref.0.clone(), meta);
    }

    pub fn mark_erased(&mut self, canonical_ref: &ArtifactRef) {
        self.erased.insert(canonical_ref.0.clone());
    }

    pub fn mark_restricted(&mut self, canonical_ref: &ArtifactRef) {
        self.restricted.insert(canonical_ref.0.clone());
    }

    pub fn is_erased(&self, canonical_ref: &ArtifactRef) -> bool {
        self.erased.contains(&canonical_ref.0)
    }
}

pub struct Projector<I: IdentityService> {
    id: I,
    store: ArtifactStore,
}

impl<I: IdentityService> Projector<I> {
    pub fn new(id: I, store: ArtifactStore) -> Projector<I> {
        Projector { id, store }
    }

    pub fn store_mut(&mut self) -> &mut ArtifactStore {
        &mut self.store
    }

    pub fn project(
        &self,
        reference: &ArtifactRef,
        viewer: &Principal,
        zookie: Zookie,
    ) -> Result<Projected, ProjectError> {
        let ty = classify(reference)?;

        let acl_object = myelin_refs::strip_sub(reference);
        let at = Consistency {
            at_least: zookie,
            mode: ConsistencyMode::Strong,
        };
        let permission = Permission(VIEW.to_string());
        match self.id.check(viewer, &permission, &acl_object, &at, None) {
            Ok(Decision::Allow) => {}
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::Unauthorized,
                }));
            }
        }

        if self.store.erased.contains(&acl_object.0) || self.store.erased.contains(&reference.0) {
            return Ok(Projected::Tombstoned(Tombstone {
                reason: TombstoneReason::Erased,
            }));
        }
        if self.store.restricted.contains(&acl_object.0)
            || self.store.restricted.contains(&reference.0)
        {
            return Ok(Projected::Tombstoned(Tombstone {
                reason: TombstoneReason::Restricted,
            }));
        }

        let projection = match ty {
            CiArtifactType::Run => self.project_run(&acl_object, reference)?,
            CiArtifactType::Deployment => self.project_deployment(&acl_object)?,
            CiArtifactType::Pipeline => self.project_pipeline(&acl_object)?,
            CiArtifactType::Runner | CiArtifactType::Artifact => {
                self.project_minimal(&acl_object, ty)
            }
        };
        Ok(Projected::Visible(projection))
    }

    fn project_run(
        &self,
        root: &ArtifactRef,
        reference: &ArtifactRef,
    ) -> Result<Projection, ProjectError> {
        let meta = self
            .store
            .runs
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound {
                reference: root.0.clone(),
            })?;
        let sub_anchor = match myelin_refs::sub_kind(reference) {
            Some(Sub::Step(n)) => Some(SubAnchor {
                kind: "step".to_string(),
                step: n,
            }),
            _ => None,
        };
        Ok(Projection {
            title: format!("Run #{} · {}", meta.number, meta.pipeline),
            state: meta.state.clone(),
            icon: CiArtifactType::Run.token().to_string(),
            render_hint: Some(RenderHint::Run {
                dag_summary: meta.dag_summary.clone(),
                failed_step: meta.failed_step,
                duration_secs: meta.duration_secs,
            }),
            sub_anchor,
        })
    }

    fn project_deployment(&self, root: &ArtifactRef) -> Result<Projection, ProjectError> {
        let meta = self
            .store
            .deployments
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound {
                reference: root.0.clone(),
            })?;
        Ok(Projection {
            title: format!("Deploy {} · {}", meta.env, meta.version),
            state: meta.state.as_token().to_string(),
            icon: CiArtifactType::Deployment.token().to_string(),
            render_hint: Some(RenderHint::Deployment {
                env: meta.env.clone(),
                risk: meta.risk.clone(),
                rollback_available: meta.rollback_available,
            }),
            sub_anchor: None,
        })
    }

    fn project_pipeline(&self, root: &ArtifactRef) -> Result<Projection, ProjectError> {
        let meta = self
            .store
            .pipelines
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound {
                reference: root.0.clone(),
            })?;
        Ok(Projection {
            title: meta.name.clone(),
            state: if meta.valid { "valid" } else { "invalid" }.to_string(),
            icon: CiArtifactType::Pipeline.token().to_string(),
            render_hint: Some(RenderHint::Pipeline {
                last_run: meta.last_run.clone(),
            }),
            sub_anchor: None,
        })
    }

    fn project_minimal(&self, root: &ArtifactRef, ty: CiArtifactType) -> Projection {
        let id = canonical_id(root).unwrap_or_default();
        Projection {
            title: format!("{} {}", ty.token(), id),
            state: "present".to_string(),
            icon: ty.token().to_string(),
            render_hint: None,
            sub_anchor: None,
        }
    }
}

fn canonical_id(r: &ArtifactRef) -> Option<String> {
    let rest = r.0.strip_prefix("myelin://")?;
    let scope = rest.split('#').next().unwrap_or(rest);
    let segments: Vec<&str> = scope.split('/').collect();
    if segments.len() != 4 || segments[1] != CI_SUBSYSTEM {
        return None;
    }
    Some(segments[3].to_string())
}

#[cfg(test)]
#[path = "surfacing_tests.rs"]
mod tests;
