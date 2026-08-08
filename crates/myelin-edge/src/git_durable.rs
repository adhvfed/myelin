use crate::catalogue::{page_envelope, Handler, HandlerCtx, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::git_edge::{map_method, num_param, param, reroot, tenant_of};
use crate::repo_authz::{DenyAllRepos, RepoAuthorizer, RepoPermission};
#[cfg(any(test, feature = "test-support"))]
use crate::repo_authz::AllowAllRepos;
use crate::repo_authz_live::{NoRepoBootstrap, RepoBootstrapGrants};
use crate::request::{EdgeRequest, EdgeResponse};
use myelin_events::{Actor, EmitContextBase, IdMinter, OutboxStore, Region, TenantId, Timestamp};
#[cfg(any(test, feature = "test-support"))]
use myelin_events::MonotonicMinter;
use myelin_git::api::{
    http_catalogue, valid_code_search_query, valid_code_search_repo, Method as GitMethod,
};
use myelin_git::check_status::GitOid;
use myelin_git::core::{Oid as CoreOid, RepoLoc};
use myelin_git::durable::{
    BlobPathLookup, CommitDetail, CommitMeta, FileLinesLookup, PrCommitPageError, PrCommitSnapshot,
    PrDiff, TreePage, TreePageError, TreePageLookup, TreePageRequest, COMMIT_LOG_MAX_OFFSET,
    FILE_LINES_MAX_RANGE, REFS_PAGE_DEFAULT_LIMIT, REFS_PAGE_MAX_LIMIT, REFS_PAGE_MAX_QUERY_BYTES,
    TREE_PAGE_DEFAULT_LIMIT, TREE_PAGE_MAX_LIMIT, TREE_PAGE_MAX_QUERY_BYTES, WIRE_MAX_REFS,
};
use myelin_git::durable::{
    CatalogueRepoState, DurableError, DurableGitRepo, DurableGitStore, RefKind, RefsPageError,
    RefsPageRequest,
};
use myelin_git::events::pseudonymized_event_principal;
use myelin_git::lifecycle::{
    BranchProtectionRuleset, PrState, PullRequest, ReviewState, ReviewVerdict,
};
use myelin_git::pg_pr_store::{PgPrStore, PrMutation, PrOperationId};
use myelin_git::pr_list_pagination::{
    pr_list_static_scope, pr_list_visible_scope, PrListCursor, PrListCursorEndpoint,
    PrListDirection, PrListKey, PrListPage, PR_LIST_CURSOR_PREFIX,
};
use myelin_git::pr_store::{
    effective_ruleset, merge_pr, BranchProtectionConfig, ChecksSummary, DurablePrStore,
    MergeAttempt, PrCrossListQuery, PrListBucket, PrListCounts, PrListQuery, PrListSlice,
    PrListSort, PrListState, PrRecord, ReviewRecord, PR_LIST_OFFSET_MAX,
};
use myelin_git::pr_threads::{
    AnchorSide, AnchorState, BatchVerdict, CommentRecord, CommentState, DurablePrThreadStore,
    PendingCommentRequest, PrincipalRole, ReviewBatch, SubmitReviewRequest, ThreadAnchor,
    ThreadPrincipal, ThreadRecord, ViewedThreads,
};
use myelin_git::receive_pack::{
    evaluate_protected_ref_push, CrashPoint, InMemoryObjectDb, Oid as PushOid, ProposedRefUpdate,
    PushOutcome, PushSession, Pusher, QuarantineMigration, QuarantineObject, RefName, RefStore,
};
use myelin_git::refs_pagination::WIRE_MAX_REF_NAME_BYTES;
use myelin_git::web::{
    CommitDiff, CommitRow, DiffFile, DiffLineView, PrCommitCursor, PrDiffFile, PrDiffHunk,
    PrDiffLine, PrDiffVM, RepoHome, RepoListCursor, RepoListRow, WebEditOutcome,
    PR_COMMIT_CURSOR_MAX_POSITION, REPO_LIST_CURSOR_MAX_BYTES, REPO_LIST_CURSOR_PREFIX,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{DurablePlacementBacking, KmsEngine, SubstrateProvider, TenantScope};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct RepoActorContext<'a> {
    tenant: &'a str,
    region: &'a str,
    slug: &'a str,
    principal: &'a Principal,
}

impl<'a> RepoActorContext<'a> {
    pub fn new(tenant: &'a str, region: &'a str, slug: &'a str, principal: &'a Principal) -> Self {
        Self {
            tenant,
            region,
            slug,
            principal,
        }
    }

    pub fn for_pr(self, number: u64) -> PrActorContext<'a> {
        PrActorContext { repo: self, number }
    }
}

impl core::fmt::Debug for RepoActorContext<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RepoActorContext")
            .field("tenant", &self.tenant)
            .field("region", &self.region)
            .field("slug", &self.slug)
            .field("principal", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy)]
pub struct PrActorContext<'a> {
    repo: RepoActorContext<'a>,
    number: u64,
}

impl core::fmt::Debug for PrActorContext<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PrActorContext")
            .field("repo", &self.repo)
            .field("number", &self.number)
            .finish()
    }
}

pub struct DurableGitBackend {
    store: DurableGitStore,
    prs: DurablePrStore,
    pg_prs: Option<PgPrStore>,
    checks: Option<myelin_git::check_status_store::PgCheckStatusProjection>,
    threads: DurablePrThreadStore,
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
    clone_base: String,
    root: PathBuf,
    git_shutdown: Arc<AtomicBool>,
    git_wire_credentials: Arc<dyn crate::git_wire_exec::GitWireCredentialIssuerFactory>,
    repo_authz: Arc<dyn RepoAuthorizer>,
    bootstrap: Arc<dyn RepoBootstrapGrants>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitBootRecoveryReport {
    pub repos_reconciled: usize,
    pub refs_reapplied: usize,
    pub merges_recovered: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitCellBootRecoveryReport {
    pub tenants_recovered: usize,
    pub repos_reconciled: usize,
    pub refs_reapplied: usize,
    pub merges_recovered: usize,
}

pub async fn recover_placed_git_at_boot(
    backend: &DurableGitBackend,
    provider: &SubstrateProvider,
    cell_id: &str,
) -> Result<GitCellBootRecoveryReport, DurableError> {
    if cell_id.trim().is_empty() {
        return Err(DurableError::Io("Git recovery cell id is empty".into()));
    }
    let placements = DurablePlacementBacking::new(provider.db_pool().clone());
    let local = placements
        .local_tenants(cell_id)
        .await
        .map_err(|_| DurableError::Io("read local tenant recovery directory".into()))?;
    let mut report = GitCellBootRecoveryReport::default();
    for entry in local.into_iter().filter(|entry| entry.active) {
        if entry.cell_id != cell_id {
            return Err(DurableError::Io(
                "local tenant recovery directory returned a foreign cell".into(),
            ));
        }
        let placement = placements
            .get_placement(&entry.tenant_id)
            .await
            .map_err(|_| DurableError::Io("read tenant recovery placement".into()))?
            .ok_or_else(|| {
                DurableError::Io("active local tenant has no canonical placement".into())
            })?;
        if placement.status != "Active" {
            return Err(DurableError::Io(
                "active local tenant has a non-active canonical placement".into(),
            ));
        }
        if placement.tenant_id != entry.tenant_id
            || placement.region != provider.config().region
            || (placement.home_cell != cell_id
                && !placement
                    .member_cells
                    .iter()
                    .any(|member| member == cell_id))
        {
            return Err(DurableError::Io(
                "active local tenant recovery placement does not match this cell/region".into(),
            ));
        }
        let principal = Principal::new(
            TenantId(entry.tenant_id),
            Region(placement.region),
            PrincipalId(format!("git-recovery:{cell_id}")),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
        let tenant_report = backend.recover_tenant_at_boot(&scope, &principal)?;
        report.tenants_recovered += 1;
        report.repos_reconciled += tenant_report.repos_reconciled;
        report.refs_reapplied += tenant_report.refs_reapplied;
        report.merges_recovered += tenant_report.merges_recovered;
    }
    Ok(report)
}

struct EnrichedPr {
    rec: PrRecord,
    summary: ChecksSummary,
    you_requested: bool,
    repo_slug: Option<String>,
}

struct EnrichedPrSlice {
    rows: Vec<EnrichedPr>,
    counts: PrListCounts,
    total: usize,
    offset: usize,
    limit: usize,
    next_cursor: Option<String>,
    prev_cursor: Option<String>,
}

struct EnrichedCrossPrSlice {
    rows: Vec<EnrichedPr>,
    total: usize,
    offset: usize,
    limit: usize,
    next_cursor: Option<String>,
    prev_cursor: Option<String>,
}

#[derive(Clone, Copy)]
struct CrossPrListLimits {
    maximum_records: usize,
    maximum_bytes: usize,
}

impl CrossPrListLimits {
    const fn production() -> Self {
        Self {
            maximum_records: CROSS_PR_LIST_MAX_RECORDS,
            maximum_bytes: CROSS_PR_LIST_MAX_BYTES,
        }
    }
}

impl DurableGitBackend {
    pub fn rooted(
        root: impl Into<PathBuf>,
        public_clone_base: impl Into<String>,
        providers: GitDatabaseProviders,
        kms: Arc<KmsEngine>,
        runtime: tokio::runtime::Handle,
        outbox: OutboxStore,
        minter: Arc<dyn IdMinter>,
    ) -> Result<DurableGitBackend, DurableError> {
        let root = root.into();
        let (provider, checks) = providers.into_projection(runtime.clone());
        Ok(DurableGitBackend {
            store: DurableGitStore::rooted(root.clone()),
            prs: DurablePrStore::rooted(root.clone()),
            pg_prs: Some(PgPrStore::new(provider, kms, runtime)?),
            checks: Some(checks),
            threads: DurablePrThreadStore::rooted(root.clone()),
            outbox,
            minter,
            clone_base: public_clone_base.into().trim_end_matches('/').to_string(),
            root,
            git_shutdown: Arc::new(AtomicBool::new(false)),
            git_wire_credentials:
                crate::git_wire_exec::unavailable_git_wire_credential_issuer_factory(),
            repo_authz: Arc::new(DenyAllRepos),
            bootstrap: Arc::new(NoRepoBootstrap),
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rooted_inmem_for_test(root: impl Into<PathBuf>) -> DurableGitBackend {
        let root = root.into();
        DurableGitBackend {
            store: DurableGitStore::rooted(root.clone()),
            prs: DurablePrStore::rooted(root.clone()),
            pg_prs: None,
            checks: None,
            threads: DurablePrThreadStore::rooted(root.clone()),
            outbox: OutboxStore::new(),
            minter: Arc::new(MonotonicMinter::new()),
            clone_base: String::new(),
            root,
            git_shutdown: Arc::new(AtomicBool::new(false)),
            git_wire_credentials: crate::git_wire_exec::test_git_wire_credential_issuer_factory(),
            repo_authz: Arc::new(AllowAllRepos),
            bootstrap: Arc::new(NoRepoBootstrap),
        }
    }

    pub fn with_repo_authorizer(
        mut self,
        repo_authz: Arc<dyn RepoAuthorizer>,
    ) -> DurableGitBackend {
        self.repo_authz = repo_authz;
        self
    }

    pub fn repo_authorizer(&self) -> &Arc<dyn RepoAuthorizer> {
        &self.repo_authz
    }

    pub fn with_repo_bootstrap(
        mut self,
        bootstrap: Arc<dyn RepoBootstrapGrants>,
    ) -> DurableGitBackend {
        self.bootstrap = bootstrap;
        self
    }

    pub fn with_git_shutdown_signal(mut self, shutdown: Arc<AtomicBool>) -> DurableGitBackend {
        self.git_shutdown = shutdown;
        self
    }

    pub fn with_git_wire_credential_issuer(
        mut self,
        issuer: Arc<dyn crate::git_wire_exec::GitWireCredentialIssuerFactory>,
    ) -> DurableGitBackend {
        self.git_wire_credentials = issuer;
        self
    }

    pub fn wire_serving(
        &self,
        principal: &Principal,
    ) -> myelin_git::core::RoutedGitCore<
        crate::git_wire_exec::GitWireExecutor,
        myelin_git::gix_backend::GixCore<myelin_git::gix_backend::RootedResolver>,
    > {
        crate::git_wire_exec::production_git_core_with_shutdown_and_issuer(
            self.root.clone(),
            crate::git_wire_exec::GitWireExecutor::default_limits(),
            crate::git_wire_exec::GitWireExecutor::serving_hooks(),
            self.git_shutdown.clone(),
            self.git_wire_credentials.bind(principal),
        )
    }

    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    pub fn reconcile_repo(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
    ) -> Result<myelin_git::reconcile::ReconcileReport, DurableError> {
        let records = myelin_git::reconcile::refs_from_outbox_scoped_bounded(
            &self.outbox,
            tenant,
            region,
            slug,
            BOOT_RECOVERY_MAX_RETAINED_OUTBOX_ROWS,
            BOOT_RECOVERY_MAX_RETAINED_OUTBOX_BYTES,
        )?;
        self.reconcile_repo_records(tenant, region, slug, &records)
    }

    fn reconcile_repo_records(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        records: &[myelin_git::reconcile::GitRefUpdatedRecord],
    ) -> Result<myelin_git::reconcile::ReconcileReport, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        myelin_git::reconcile::reconcile_refs(&repo, records)
    }

    pub fn recover_tenant_at_boot(
        &self,
        scope: &TenantScope,
        recovery_principal: &Principal,
    ) -> Result<GitBootRecoveryReport, DurableError> {
        if recovery_principal.tenant != *scope.tenant()
            || recovery_principal.region != *scope.region()
        {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        let tenant = scope.tenant().0.as_str();
        let region = scope.region().0.as_str();
        let pending = match &self.pg_prs {
            Some(store) => store.list_pending_merges_bounded(
                scope,
                BOOT_RECOVERY_MAX_PENDING_MERGES,
                BOOT_RECOVERY_MAX_PENDING_MERGE_BYTES,
            )?,
            None => Vec::new(),
        };
        let mut ref_records = myelin_git::reconcile::refs_by_repo_from_outbox_scoped_bounded(
            &self.outbox,
            tenant,
            region,
            BOOT_RECOVERY_MAX_RETAINED_OUTBOX_ROWS,
            BOOT_RECOVERY_MAX_RETAINED_OUTBOX_BYTES,
        )?;
        let mut repos = ref_records
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        repos.extend(pending.iter().map(|item| item.repo_slug.clone()));

        let mut report = GitBootRecoveryReport::default();
        for slug in &repos {
            let records = ref_records.remove(slug).unwrap_or_default();
            let reconciled = self.reconcile_repo_records(tenant, region, slug, &records)?;
            report.repos_reconciled += 1;
            report.refs_reapplied += reconciled.reapplied.len();
        }

        if let Some(store) = &self.pg_prs {
            for item in pending {
                let loc = Self::loc(tenant, region, &item.repo_slug);
                let repo = Arc::new(self.store.open_repo(&loc)?);
                let ref_store = self.open_durable_refstore(
                    repo.clone(),
                    &item.repo_slug,
                    tenant,
                    region,
                    recovery_principal,
                );
                if store
                    .recover_pending_merge_target(
                        scope,
                        &item.repo_slug,
                        item.number,
                        recovery_principal,
                        &loc,
                        &repo,
                        &ref_store,
                    )?
                    .is_some()
                {
                    report.merges_recovered += 1;
                }
            }
        }
        Ok(report)
    }

    fn loc(tenant: &str, region: &str, slug: &str) -> RepoLoc {
        RepoLoc::new(tenant, region, slug)
    }

    fn verified_pr_scope(
        principal: &Principal,
        loc: &RepoLoc,
    ) -> Result<TenantScope, DurableError> {
        if principal.tenant.0 != loc.tenant || principal.region.0 != loc.region {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        Ok(TenantScope::from_verified_token(
            principal,
            principal.region.clone(),
        ))
    }

    fn pr_get(
        &self,
        loc: &RepoLoc,
        number: u64,
        principal: &Principal,
    ) -> Result<Option<PrRecord>, DurableError> {
        match &self.pg_prs {
            Some(store) => store.get(&Self::verified_pr_scope(principal, loc)?, &loc.repo, number),
            None => self.prs.get(loc, number),
        }
    }

    pub(crate) fn authorize_pr_review(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        principal: &Principal,
    ) -> bool {
        let loc = Self::loc(tenant, region, slug);
        if self
            .repo_authz
            .authorize_repo_permission(principal, &loc, RepoPermission::Push)
        {
            return true;
        }
        let viewer = Self::pseudonym(tenant, principal);
        matches!(
            self.pr_get(&loc, number, principal),
            Ok(Some(record)) if record.is_review_requested_of(&viewer)
        )
    }

    fn pr_list(&self, loc: &RepoLoc, principal: &Principal) -> Result<Vec<PrRecord>, DurableError> {
        match &self.pg_prs {
            Some(store) => store.list_bounded(
                &Self::verified_pr_scope(principal, loc)?,
                &loc.repo,
                PR_LIST_PER_REPO_MAX_RECORDS,
                PR_LIST_PER_REPO_MAX_BYTES,
            ),
            None => self.prs.list_bounded(
                loc,
                PR_LIST_PER_REPO_MAX_RECORDS,
                PR_LIST_PER_REPO_MAX_BYTES,
            ),
        }
    }

    fn pr_list_page(
        &self,
        loc: &RepoLoc,
        principal: &Principal,
        query: &PrListQuery,
    ) -> Result<PrListSlice, DurableError> {
        match &self.pg_prs {
            Some(store) => {
                store.list_page(&Self::verified_pr_scope(principal, loc)?, &loc.repo, query)
            }
            None => self.prs.list_page_bounded(
                loc,
                query,
                PR_LIST_PER_REPO_MAX_RECORDS,
                PR_LIST_PER_REPO_MAX_BYTES,
            ),
        }
    }

    fn pr_open(
        &self,
        loc: &RepoLoc,
        record: PrRecord,
        operation_id: &PrOperationId,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        match &self.pg_prs {
            Some(store) => store.open(
                &Self::verified_pr_scope(principal, loc)?,
                &loc.repo,
                record,
                operation_id,
                principal,
            ),
            None => {
                self.prs.open_pr(loc, &record)?;
                Ok(record)
            }
        }
    }

    fn pr_mutate(
        &self,
        loc: &RepoLoc,
        number: u64,
        mutation: PrMutation,
        operation_id: &PrOperationId,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        if let Some(store) = &self.pg_prs {
            return store.apply_mutation(
                &Self::verified_pr_scope(principal, loc)?,
                &loc.repo,
                number,
                mutation,
                operation_id,
                principal,
            );
        }
        self.prs.update(loc, number, |record| {
            match mutation {
                PrMutation::ReportChecks {
                    green_contexts,
                    fork_unendorsed_contexts,
                    codeowner_review_satisfied,
                    outstanding_conversations,
                } => {
                    if let Some(value) = green_contexts {
                        record.green_contexts = value;
                    }
                    if let Some(value) = fork_unendorsed_contexts {
                        record.fork_unendorsed_contexts = value;
                    }
                    if let Some(value) = codeowner_review_satisfied {
                        record.codeowner_review_satisfied = value;
                    }
                    if let Some(value) = outstanding_conversations {
                        record.outstanding_conversations = value;
                    }
                }
                PrMutation::SubmitReview(review) => record.reviews.push(review),
                PrMutation::EndorseContexts(contexts) => {
                    for context in contexts {
                        if !record.endorsed_contexts.contains(&context) {
                            record.endorsed_contexts.push(context);
                        }
                    }
                }
                PrMutation::Touch => {}
            }
            record.updated_at = Some(now_unix());
            Ok(record.clone())
        })
    }

    fn emit_ctx(tenant: &str, region: &str, principal: &Principal) -> EmitContextBase {
        let now = chrono::DateTime::from_timestamp(now_unix(), 0).unwrap_or_default();
        let now = Timestamp(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        let event_principal = pseudonymized_event_principal(tenant, principal);
        EmitContextBase {
            tenant: TenantId(tenant.into()),
            region: Region(region.into()),
            actor: Actor(event_principal),
            schema_ver: 1,
            occurred_at: now.clone(),
            recorded_at: now,
            caused_by: None,
        }
    }

    fn fresh_operation_id(&self) -> Result<PrOperationId, DurableError> {
        PrOperationId::parse(&format!("internal-{}", self.minter.mint().0))
    }

    fn request_operation_id(&self, request: &EdgeRequest) -> Result<PrOperationId, EdgeError> {
        if self.pg_prs.is_none() {
            return self.fresh_operation_id().map_err(map_durable_err);
        }
        let value = request.header("idempotency-key").ok_or_else(|| {
            EdgeError::BadRequest("production PR writes require an `Idempotency-Key` header".into())
        })?;
        PrOperationId::parse(value)
            .map_err(|_| EdgeError::BadRequest("invalid `Idempotency-Key` header".into()))
    }

    fn pseudonym(tenant: &str, principal: &Principal) -> String {
        format!("{}@{}.noreply", principal.principal_id.0, tenant)
    }

    fn is_agent(principal: &Principal) -> bool {
        matches!(principal.kind, PrincipalKind::Agent { .. })
    }

    fn open_durable_refstore(
        &self,
        repo: Arc<DurableGitRepo>,
        slug: &str,
        tenant: &str,
        region: &str,
        principal: &Principal,
    ) -> RefStore {
        RefStore::open_durable(
            repo,
            slug.to_string(),
            Self::emit_ctx(tenant, region, principal),
            self.outbox.clone(),
            self.minter.clone(),
        )
    }

    pub fn create_repo(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
    ) -> Result<bool, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        if self.store.repo_exists(&loc) {
            return Ok(false);
        }
        self.store.create_repo(&loc)?;
        Ok(true)
    }

    pub fn create_repo_as(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        creator: &Principal,
    ) -> Result<bool, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        if self.store.repo_exists(&loc) {
            return Ok(false);
        }
        self.bootstrap.grant_creator(creator, &loc).map_err(|e| {
            DurableError::Git(format!(
                "creator bootstrap grant refused (repo NOT created - fail-closed): {e}"
            ))
        })?;
        match self.store.create_repo(&loc) {
            Ok(_repo) => Ok(true),
            Err(create_err) => match self.bootstrap.revoke_creator(creator, &loc) {
                Ok(()) => Err(create_err),
                Err(revoke_err) => Err(DurableError::Git(format!(
                    "repo create FAILED and the compensating bootstrap-grant removal ALSO failed - \
                     an admin grant on `{slug}` is ORPHANED (reachable by slug reuse; a reconciler \
                     must revoke it): create error: {create_err}; compensation error: {revoke_err}"
                ))),
            },
        }
    }

    fn scan_repo_slugs(&self, tenant: &str, region: &str) -> Result<Vec<String>, DurableError> {
        self.scan_repo_slugs_bounded(tenant, region, REPO_SCAN_MAX_CANDIDATES)
    }

    fn scan_repo_slugs_bounded(
        &self,
        tenant: &str,
        region: &str,
        maximum: usize,
    ) -> Result<Vec<String>, DurableError> {
        let mut slugs: Vec<String> = Vec::new();
        let probe = Self::loc(tenant, region, "_probe");
        let probe_path = self.store.repo_path(&probe)?;
        let dir = probe_path.parent().ok_or_else(|| {
            DurableError::Git("repository locator has no tenant-region parent".into())
        })?;
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(slugs),
            Err(e) => {
                return Err(DurableError::Git(format!(
                    "scan repository directory {}: {e}",
                    dir.display()
                )))
            }
        };
        for entry in rd {
            let entry = entry.map_err(|e| {
                DurableError::Git(format!(
                    "read repository directory entry in {}: {e}",
                    dir.display()
                ))
            })?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(slug) = name.strip_suffix(".git") {
                if slugs.len() >= maximum {
                    return Err(DurableError::Git(
                        "browse response limit exceeded: repository candidate count".into(),
                    ));
                }
                slugs.push(slug.to_string());
            }
        }
        slugs.sort();
        Ok(slugs)
    }

    fn list_repos_visible(
        &self,
        tenant: &str,
        region: &str,
        principal: &Principal,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<RepoHome>, bool), DurableError> {
        let candidates = self.scan_repo_slugs(tenant, region)?;
        let visible = self
            .repo_authz
            .visible_repos(principal, tenant, region, &candidates);
        let mut page: Vec<String> = visible
            .into_iter()
            .skip(offset)
            .take(limit.saturating_add(1))
            .collect();
        let has_more = page.len() > limit;
        page.truncate(limit);
        let mut out = Vec::new();
        for slug in page {
            let loc = Self::loc(tenant, region, &slug);
            let repo = self.store.open_repo(&loc)?;
            out.push(self.repo_home(tenant, region, &slug, &repo)?);
        }
        Ok((out, has_more))
    }

    fn list_repo_summaries_visible(
        &self,
        tenant: &str,
        region: &str,
        principal: &Principal,
        after: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<RepoListRow>, Option<String>), DurableError> {
        let candidates = self.scan_repo_slugs(tenant, region)?;
        let visible = self
            .repo_authz
            .visible_repos(principal, tenant, region, &candidates);
        let mut page: Vec<String> = visible
            .into_iter()
            .filter(|slug| after.is_none_or(|last| slug.as_str() > last))
            .take(limit.saturating_add(1))
            .collect();
        let has_more = page.len() > limit;
        page.truncate(limit);
        let next_slug = if has_more { page.last().cloned() } else { None };

        let mut rows = Vec::with_capacity(page.len());
        for slug in page {
            let loc = Self::loc(tenant, region, &slug);
            let repo = self.store.open_repo(&loc)?;
            let full_slug = format!("{tenant}/{slug}");
            let row = match repo.catalogue_repo_state()? {
                CatalogueRepoState::Populated => {
                    RepoListRow::populated(full_slug, self.clone_url(tenant, region, &slug))
                }
                CatalogueRepoState::Empty => RepoListRow::empty(full_slug),
            }
            .map_err(|error| {
                DurableError::Git(format!(
                    "repository catalogue projection invalid ({error:?})"
                ))
            })?;
            rows.push(row);
        }
        Ok((rows, next_slug))
    }

    fn clone_url(&self, tenant: &str, region: &str, slug: &str) -> String {
        format!("{}/{tenant}/{region}/{slug}.git", self.clone_base)
    }

    fn repo_home(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        repo: &DurableGitRepo,
    ) -> Result<RepoHome, DurableError> {
        let full_slug = format!("{tenant}/{slug}");
        let clone_url = self.clone_url(tenant, region, slug);
        let refs = repo.refs_summary()?;
        if refs.default_tip.is_none() {
            Ok(RepoHome::Empty {
                slug: full_slug,
                clone_url,
            })
        } else {
            let branch_ref = format!("refs/heads/{}", refs.default_branch);
            let page = first_root_tree_page(repo, &branch_ref)?;
            let entries = page
                .entries
                .iter()
                .map(|entry| (entry.name.clone(), entry.is_dir))
                .collect();
            let readme = read_text_blob_at_snapshot_bounded(
                repo,
                &page.snapshot_oid,
                "README.md",
                64 * 1024,
            )?
            .unwrap_or_default()
            .chars()
            .take(400)
            .collect();
            Ok(RepoHome::Populated {
                slug: full_slug,
                readme_excerpt: readme,
                entries,
                clone_url,
            })
        }
    }

    fn commit_log(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        gitref: &str,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<CommitRow>, bool), DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        let full = qualify_ref(gitref);
        let (metas, has_more) = repo.commit_log(&full, offset, limit)?;
        Ok((metas.into_iter().map(commit_row).collect(), has_more))
    }

    fn commit_diff(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        oid: &str,
    ) -> Result<Option<CommitDiff>, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        Ok(repo.commit_detail(oid)?.map(commit_diff_vm))
    }

    fn pr_diff(
        &self,
        target: PrActorContext<'_>,
        offset: usize,
        limit: usize,
    ) -> Result<Option<PrDiffVM>, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
        let loc = Self::loc(tenant, region, slug);
        let Some(rec) = self.pr_get(&loc, number, principal)? else {
            return Ok(None);
        };
        let repo = self.store.open_repo(&loc)?;
        let Some(diff) = repo.pr_diff(&rec.base_ref, &rec.head_oid, PR_DIFF_PER_FILE_LINE_CAP)?
        else {
            return Ok(Some(PrDiffVM {
                number,
                base_ref: rec.base_ref.clone(),
                base_oid: String::new(),
                head_oid: rec.head_oid.clone(),
                three_dot: false,
                files: Vec::new(),
                restricted_files: 0,
                total_files: 0,
                total_additions: 0,
                total_deletions: 0,
                next_cursor: None,
                limit,
            }));
        };
        Ok(Some(pr_diff_vm(number, &rec.base_ref, diff, offset, limit)))
    }

    fn file_lines(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        oid: &str,
        start: usize,
        end: usize,
    ) -> Result<FileLinesLookup, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        repo.file_lines(oid, start, end)
    }

    fn refs_json(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        request: RefsPageRequest,
    ) -> Result<Value, RefsPageError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        let limit = request.limit;
        let page = repo.refs_page(request)?;
        let default_branch = page.summary.default_branch.clone();
        let mut branches = Vec::new();
        let mut tags = Vec::new();
        for item in page.items {
            match item.kind {
                RefKind::Branch => {
                    let is_default = item.name == default_branch;
                    branches.push(json!({
                        "name": item.name,
                        "oid": item.tip.0,
                        "is_default": is_default,
                    }));
                }
                RefKind::Tag => tags.push(json!({ "name": item.name, "oid": item.tip.0 })),
            }
        }
        let pinned: Vec<Value> = page
            .pins
            .into_iter()
            .map(|item| {
                let is_default = item.kind == RefKind::Branch && item.name == default_branch;
                let kind = match item.kind {
                    RefKind::Branch => "branch",
                    RefKind::Tag => "tag",
                };
                json!({
                    "kind": kind,
                    "full_name": item.qualified_name,
                    "name": item.name,
                    "oid": item.tip.0,
                    "is_default": is_default,
                })
            })
            .collect();
        Ok(json!({
            "branches": branches,
            "tags": tags,
            "default_branch": default_branch,
            "pinned": pinned,
            "page": { "next_cursor": page.next_cursor, "limit": limit },
        }))
    }

    fn repo_home_json(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        let full_slug = format!("{tenant}/{slug}");
        let clone_url = self.clone_url(tenant, region, slug);
        let refs = repo.refs_summary()?;
        let default_branch = refs.default_branch.clone();
        let counts = json!({ "branches": refs.branch_count, "tags": refs.tag_count });
        if refs.default_tip.is_none() {
            return Ok(json!({
                "state": "empty",
                "slug": full_slug,
                "clone_url": clone_url,
                "default_branch": default_branch,
                "counts": counts,
            }));
        }
        let branch_ref = format!("refs/heads/{default_branch}");
        let page = first_root_tree_page(&repo, &branch_ref)?;
        let readme = read_text_blob_at_snapshot_bounded(
            &repo,
            &page.snapshot_oid,
            "README.md",
            README_MAX_BYTES,
        )?;
        let latest = repo.commit_meta_at_oid(&page.snapshot_oid)?;
        let per_entry = repo.latest_commits_for_entries_at_snapshot(
            &page.snapshot_oid,
            "",
            &page.entries,
            LATEST_COMMIT_WALK_CAP,
        )?;
        let entries_json = tree_entries_json(&page.entries, "", &per_entry);
        let entries_page = json!({
            "next_cursor": page.next_cursor,
            "limit": TREE_PAGE_DEFAULT_LIMIT,
            "ref": branch_ref,
            "snapshot_oid": page.snapshot_oid.as_str(),
        });
        Ok(json!({
            "state": "populated",
            "slug": full_slug,
            "clone_url": clone_url,
            "default_branch": default_branch,
            "readme": readme,
            "readme_excerpt": readme.as_ref().map(|r| r.chars().take(400).collect::<String>()),
            "latest_commit": latest.as_ref().map(commit_brief_json),
            "counts": counts,
            "entries": entries_json,
            "entries_page": entries_page,
            "snapshot_oid": page.snapshot_oid.0,
        }))
    }

    fn tree_json(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        gitref: &str,
        path: &str,
        request: TreePageRequest,
    ) -> Result<Value, TreePageError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        let limit = request.limit;
        let include_readme = request.cursor.is_none()
            && request
                .query
                .as_deref()
                .is_none_or(|query| query.trim().is_empty());
        match repo.tree_page(gitref, path, request)? {
            TreePageLookup::IsFile => {
                Ok(json!({ "redirect_to_blob": true, "ref": gitref, "path": path }))
            }
            TreePageLookup::Missing => Err(TreePageError::Durable(DurableError::NotFound(
                format!("no such path `{path}` at `{gitref}`"),
            ))),
            TreePageLookup::Dir(page) => {
                let base = path.trim_matches('/');
                let snapshot_oid = page.snapshot_oid.clone();
                let per_entry = repo.latest_commits_for_entries_at_snapshot(
                    &snapshot_oid,
                    base,
                    &page.entries,
                    LATEST_COMMIT_WALK_CAP,
                )?;
                let entries_json = tree_entries_json(&page.entries, base, &per_entry);
                let mut response = json!({
                    "ref": gitref,
                    "path": base,
                    "entries": entries_json,
                    "snapshot_oid": snapshot_oid.as_str(),
                    "page": { "next_cursor": page.next_cursor, "limit": limit },
                });
                if include_readme {
                    let readme_path = if base.is_empty() {
                        "README.md".to_string()
                    } else {
                        format!("{base}/README.md")
                    };
                    response["readme"] = json!(read_text_blob_at_snapshot_bounded(
                        &repo,
                        &snapshot_oid,
                        &readme_path,
                        README_MAX_BYTES,
                    )?);
                }
                Ok(response)
            }
        }
    }

    fn code_search_json(
        &self,
        tenant: &str,
        region: &str,
        principal: &Principal,
        query: &str,
        repo_filter: Option<&str>,
    ) -> Result<Value, DurableError> {
        let mut repos = self.visible_pr_repo_slugs(tenant, region, principal)?;
        if let Some(filter) = repo_filter {
            repos.retain(|repo| repo == filter);
        }
        let mut budget = CodeSearchBudget::new();
        if repos.len() > CODE_SEARCH_MAX_REPOS {
            repos.truncate(CODE_SEARCH_MAX_REPOS);
            budget.incomplete = true;
        }
        let mut hits = Vec::new();
        for slug in repos {
            let repo = self.store.open_repo(&Self::loc(tenant, region, &slug))?;
            let summary = repo.refs_summary()?;
            if summary.default_tip.is_none() {
                continue;
            }
            let branch_ref = format!("refs/heads/{}", summary.default_branch);
            search_repo_code(
                &repo,
                &slug,
                &branch_ref,
                query,
                &mut hits,
                &mut budget,
            )?;
            if hits.len() >= CODE_SEARCH_MAX_RESULTS || budget.exhausted {
                break;
            }
        }
        Ok(json!({
            "items": hits,
            "page": { "next_cursor": Value::Null, "limit": CODE_SEARCH_MAX_RESULTS },
            "complete": !budget.incomplete,
        }))
    }

    fn blob_json(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        gitref: &str,
        path: &str,
    ) -> Result<Value, DurableError> {
        self.blob_json_bounded(
            tenant,
            region,
            slug,
            gitref,
            path,
            BlobViewOptions {
                maximum_preview_bytes: BLOB_INLINE_CAP,
                maximum_transfer_bytes: RAW_BLOB_MAX_BYTES,
            },
        )
    }

    fn blob_json_bounded(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        gitref: &str,
        path: &str,
        options: BlobViewOptions,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        let raw = format!(
            "/{}/git/repos/{slug}/raw/{gitref}/{path}",
            crate::catalogue::API_VERSION
        );
        let download = format!(
            "/{}/git/repos/{slug}/download/{gitref}/{path}",
            crate::catalogue::API_VERSION
        );
        match repo.read_blob_at_path_bounded(gitref, path, options.maximum_preview_bytes)? {
            BlobPathLookup::IsDir => {
                Ok(json!({ "redirect_to_tree": true, "ref": gitref, "path": path }))
            }
            BlobPathLookup::Missing => Err(DurableError::NotFound(format!(
                "no such file `{path}` at `{gitref}`"
            ))),
            BlobPathLookup::TooLarge { size, oid, .. } => Ok(json!({
                "path": path,
                "contents": "",
                "base_oid": oid.0,
                "viewer_may_edit": false,
                "size_bytes": size,
                "preview_unavailable": true,
                "download_available": size <= options.maximum_transfer_bytes as u64,
                "raw_url": raw,
                "download_url": download,
            })),
            BlobPathLookup::Found {
                bytes,
                oid,
                is_binary,
                size,
            } => {
                let contents = if is_binary {
                    String::new()
                } else {
                    String::from_utf8_lossy(&bytes).to_string()
                };
                Ok(json!({
                    "path": path,
                    "contents": contents,
                    "base_oid": oid.0,
                    "viewer_may_edit": false,
                    "is_binary": is_binary,
                    "size_bytes": size,
                    "is_truncated": false,
                    "preview_unavailable": false,
                    "download_available": true,
                    "raw_url": raw,
                    "download_url": download,
                }))
            }
        }
    }

    fn raw_response(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        gitref: &str,
        path: &str,
        attachment: bool,
    ) -> Result<EdgeResponse, EdgeError> {
        self.raw_response_bounded(
            tenant,
            region,
            slug,
            gitref,
            path,
            RawResponseOptions {
                attachment,
                maximum_bytes: RAW_BLOB_MAX_BYTES,
            },
        )
    }

    fn raw_response_bounded(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        gitref: &str,
        path: &str,
        options: RawResponseOptions,
    ) -> Result<EdgeResponse, EdgeError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc).map_err(map_durable_err)?;
        let (bytes, is_binary) = match repo
            .read_blob_at_path_bounded(gitref, path, options.maximum_bytes)
            .map_err(map_durable_err)?
        {
            BlobPathLookup::Found {
                bytes, is_binary, ..
            } => (bytes, is_binary),
            BlobPathLookup::IsDir => {
                return Err(EdgeError::BadRequest(
                    "path is a directory, not a file".into(),
                ))
            }
            BlobPathLookup::Missing => {
                return Err(EdgeError::NotFound("no such file at that ref".into()))
            }
            BlobPathLookup::TooLarge { maximum, .. } => {
                return Err(EdgeError::PayloadTooLarge(format!(
                    "raw file exceeds the {maximum}-byte transfer limit"
                )))
            }
        };
        let content_type = if is_binary {
            "application/octet-stream".to_string()
        } else {
            "text/plain; charset=utf-8".to_string()
        };
        let filename = path.rsplit('/').next().unwrap_or("download");
        let mut headers = vec![(
            "content-disposition".to_string(),
            if options.attachment {
                format!("attachment; filename=\"{}\"", sanitize_filename(filename))
            } else {
                format!("inline; filename=\"{}\"", sanitize_filename(filename))
            },
        )];
        headers.push(("x-content-type-options".to_string(), "nosniff".to_string()));
        Ok(EdgeResponse::Bytes {
            status: 200,
            content_type,
            headers,
            body: bytes,
        })
    }

    fn web_edit_commit(
        &self,
        target: RepoActorContext<'_>,
        gitref: &str,
        path: &str,
        expected_base: &str,
        contents: &str,
    ) -> Result<WebEditOutcome, DurableError> {
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = target;
        let loc = Self::loc(tenant, region, slug);
        let repo = Arc::new(self.store.open_repo(&loc)?);
        let full = format!("refs/heads/{gitref}");

        let current_base = repo
            .blob_oid_at_path(&full, path)?
            .map(|oid| oid.0)
            .unwrap_or_default();

        let probe = WebEditOutcome::evaluate(expected_base, &current_base, "pending", true);
        if let WebEditOutcome::StaleBase { current_oid } = probe {
            return Ok(WebEditOutcome::StaleBase { current_oid });
        }
        if let WebEditOutcome::Denied = probe {
            return Ok(WebEditOutcome::Denied);
        }

        let psn = Self::pseudonym(tenant, principal);
        let (new_commit, _new_blob, parent) =
            repo.build_file_commit(&full, path, contents.as_bytes(), "web edit", &psn, &psn)?;

        let ref_store = self.open_durable_refstore(repo, slug, tenant, region, principal);
        let expected_old = parent
            .map(|p| PushOid::new(p.0))
            .unwrap_or_else(PushOid::zero);
        let push = PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name: RefName::new(full.clone()),
                expected_old,
                new_oid: PushOid::new(new_commit.0.clone()),
                forced: false,
                commit_oids: vec![PushOid::new(new_commit.0.clone())],
            }],
            quarantine: Vec::new(),
            pusher: Pusher {
                pseudonym: psn,
                is_agent: false,
            },
        };
        match ref_store
            .receive(&push, &InMemoryObjectDb::new(), CrashPoint::None)
            .map_err(|e| DurableError::Git(format!("ref-CAS: {e:?}")))?
        {
            PushOutcome::Accepted { .. } => Ok(WebEditOutcome::Committed {
                new_oid: new_commit.0,
            }),
            PushOutcome::Rejected(_) => Ok(WebEditOutcome::StaleBase {
                current_oid: current_base,
            }),
            PushOutcome::Crashed(_) => Err(DurableError::Git("web-edit ref-CAS crashed".into())),
        }
    }

    pub fn get_pr(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        principal: &Principal,
    ) -> Result<Option<PrRecord>, DurableError> {
        self.pr_get(&Self::loc(tenant, region, slug), number, principal)
    }

    fn next_pr_number(&self, loc: &RepoLoc) -> Result<u64, DurableError> {
        Self::next_pr_number_after(self.prs.max_pr_number(loc)?)
    }

    fn next_pr_number_after(max: Option<u64>) -> Result<u64, DurableError> {
        match max {
            None => Ok(1),
            Some(number) => number.checked_add(1).ok_or_else(|| {
                DurableError::Git("pull-request number space exhausted at u64::MAX".into())
            }),
        }
    }

    pub fn open_pr(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        body: &Value,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        let operation_id = self.fresh_operation_id()?;
        self.open_pr_with_operation(tenant, region, slug, body, principal, &operation_id)
    }

    pub fn open_pr_with_operation(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        body: &Value,
        principal: &Principal,
        operation_id: &PrOperationId,
    ) -> Result<PrRecord, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.store.open_repo(&loc)?;
        let base_ref = body
            .get("base_ref")
            .and_then(Value::as_str)
            .unwrap_or("refs/heads/main")
            .to_string();
        let head_ref = body
            .get("head_ref")
            .and_then(Value::as_str)
            .unwrap_or("refs/heads/feature")
            .to_string();
        let head_repo_slug = body
            .get("head_repo")
            .or_else(|| body.get("head_repo_slug"))
            .and_then(Value::as_str)
            .unwrap_or(slug);
        if head_repo_slug.is_empty() {
            return Err(DurableError::Git(
                "open-PR head repository slug must be non-empty".into(),
            ));
        }
        let head_loc = Self::loc(&principal.tenant.0, &principal.region.0, head_repo_slug);
        if !self
            .repo_authz
            .authorize_repo_permission(principal, &head_loc, RepoPermission::Pull)
        {
            return Err(DurableError::NotFound("repository not found".into()));
        }
        let head_repo = self.store.open_repo(&head_loc)?;
        let head_oid = body
            .get("head_oid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let qualified_head_ref = if head_ref.starts_with("refs/") {
            head_ref.clone()
        } else {
            format!("refs/heads/{head_ref}")
        };
        let source_tip = head_repo.read_ref(&qualified_head_ref)?;
        let head_oid = if head_oid.is_empty() {
            match source_tip {
                Some(tip) => tip.0,
                None => {
                    return Err(DurableError::Git(format!(
                    "open-PR head_ref `{head_ref}` does not exist in the repo - no branch tip to \
                         open against (missing head)"
                )))
                }
            }
        } else if self.pg_prs.is_some()
            && source_tip.as_ref().map(|tip| tip.0.as_str()) != Some(head_oid.as_str())
        {
            return Err(DurableError::Git(format!(
                "open-PR head_oid does not match the current `{head_repo_slug}` source ref tip"
            )));
        } else {
            head_oid
        };
        let title = body
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| DurableError::Git("open-PR body missing a non-empty `title`".into()))?
            .to_string();
        if title.len() > 512 {
            return Err(DurableError::Git(
                "open-PR `title` exceeds 512 bytes".into(),
            ));
        }
        let body_md = body
            .get("body_md")
            .or_else(|| body.get("body"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let number = if self.pg_prs.is_some() {
            0
        } else {
            self.next_pr_number(&loc)?
        };
        let pr = PullRequest::open(
            number,
            base_ref,
            head_ref,
            Self::pseudonym(tenant, principal),
            body.get("draft").and_then(Value::as_bool).unwrap_or(false),
        );
        let mut rec = PrRecord::open(&pr, head_oid);
        rec.head_repo_slug = head_repo_slug.to_string();
        rec.title = title;
        rec.body_md = body_md;
        rec.author_is_agent = Self::is_agent(principal);
        let now = now_unix();
        rec.created_at = Some(now);
        rec.updated_at = Some(now);
        self.pr_open(&loc, rec, operation_id, principal)
    }

    fn list_prs_for_repo(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        principal: &Principal,
        query: &PrListQuery,
    ) -> Result<EnrichedPrSlice, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.store.open_repo(&loc)?;
        let page = self.pr_list_page(&loc, principal, query)?;
        let rows =
            self.enrich_pr_records(&loc, principal, &query.viewer_pseudonym, None, page.records);
        let endpoint = PrListCursorEndpoint::Repository(query.state);
        let static_scope = pr_list_static_scope(
            tenant,
            region,
            &query.viewer_pseudonym,
            endpoint,
            Some(slug),
            query.sort,
            query.limit,
        );
        let (next_cursor, prev_cursor) = mint_pr_list_cursors(
            &rows,
            endpoint,
            query.sort,
            query.limit,
            page.offset,
            page.has_older,
            page.has_newer,
            static_scope,
            [0; 32],
            &query.page,
        )?;
        Ok(EnrichedPrSlice {
            rows,
            counts: page.counts,
            total: page.total,
            offset: page.offset,
            limit: query.limit,
            next_cursor,
            prev_cursor,
        })
    }

    pub(crate) fn visible_pr_repo_slugs(
        &self,
        tenant: &str,
        region: &str,
        principal: &Principal,
    ) -> Result<Vec<String>, DurableError> {
        let candidates = self.scan_repo_slugs(tenant, region)?;
        Ok(self
            .repo_authz
            .visible_repos(principal, tenant, region, &candidates))
    }

    #[cfg(test)]
    fn list_prs_cross_bounded(
        &self,
        tenant: &str,
        region: &str,
        principal: &Principal,
        limits: CrossPrListLimits,
    ) -> Result<Vec<EnrichedPr>, DurableError> {
        let visible = self.visible_pr_repo_slugs(tenant, region, principal)?;
        self.list_visible_prs_cross_bounded(tenant, region, principal, &visible, limits)
    }

    fn list_visible_prs_cross_bounded(
        &self,
        tenant: &str,
        region: &str,
        principal: &Principal,
        visible: &[String],
        limits: CrossPrListLimits,
    ) -> Result<Vec<EnrichedPr>, DurableError> {
        let viewer = Self::pseudonym(tenant, principal);
        let mut out = Vec::new();
        let mut aggregate_records = 0usize;
        let mut aggregate_bytes = 0usize;
        for slug in visible {
            let loc = Self::loc(tenant, region, slug);
            self.store.open_repo(&loc)?;
            let records = self.pr_list(&loc, principal)?;
            aggregate_records = checked_cross_pr_list_total(
                aggregate_records,
                records.len(),
                limits.maximum_records,
                "cross-repository record count",
            )?;
            let repo_bytes = serialized_pr_records_bytes(&records)?;
            aggregate_bytes = checked_cross_pr_list_total(
                aggregate_bytes,
                repo_bytes,
                limits.maximum_bytes,
                "cross-repository serialized bytes",
            )?;
            out.extend(self.enrich_pr_records(&loc, principal, &viewer, Some(slug), records));
        }
        Ok(out)
    }

    fn list_prs_cross_page(
        &self,
        tenant: &str,
        region: &str,
        principal: &Principal,
        query: &PrCrossListQuery,
    ) -> Result<EnrichedCrossPrSlice, DurableError> {
        let visible = self.visible_pr_repo_slugs(tenant, region, principal)?;
        let visible_scope = pr_list_visible_scope(&visible);
        if let PrListPage::Keyset(cursor) = &query.page {
            if cursor.visible_scope() != visible_scope
                || cursor
                    .key()
                    .repo_slug
                    .as_ref()
                    .is_none_or(|slug| !visible.contains(slug))
            {
                return Err(DurableError::Git(
                    "pull request list cursor visible set changed".into(),
                ));
            }
        }
        let endpoint = PrListCursorEndpoint::CrossRepository(query.bucket);
        let static_scope = pr_list_static_scope(
            tenant,
            region,
            &query.viewer_pseudonym,
            endpoint,
            None,
            query.sort,
            query.limit,
        );
        if let Some(store) = &self.pg_prs {
            let page = store.list_cross_page(
                &Self::verified_pr_scope(principal, &Self::loc(tenant, region, "_visible"))?,
                &visible,
                query,
            )?;
            let rows = self.enrich_cross_pr_records(
                tenant,
                region,
                principal,
                &query.viewer_pseudonym,
                page.records,
            );
            let (next_cursor, prev_cursor) = mint_pr_list_cursors(
                &rows,
                endpoint,
                query.sort,
                query.limit,
                page.offset,
                page.has_older,
                page.has_newer,
                static_scope,
                visible_scope,
                &query.page,
            )?;
            return Ok(EnrichedCrossPrSlice {
                rows,
                total: page.total,
                offset: page.offset,
                limit: query.limit,
                next_cursor,
                prev_cursor,
            });
        }

        let mut rows = self.list_visible_prs_cross_bounded(
            tenant,
            region,
            principal,
            &visible,
            CrossPrListLimits::production(),
        )?;
        rows.retain(|item| match query.bucket {
            PrListBucket::Yours => item.rec.author_pseudonym == query.viewer_pseudonym,
            PrListBucket::NeedsReview => {
                item.you_requested
                    && item.rec.author_pseudonym != query.viewer_pseudonym
                    && matches!(item.rec.state, PrState::Open | PrState::Draft)
            }
        });
        match query.sort {
            PrListSort::Created => rows.sort_by(|a, b| {
                b.rec
                    .number
                    .cmp(&a.rec.number)
                    .then(a.repo_slug.as_deref().cmp(&b.repo_slug.as_deref()))
            }),
            PrListSort::Updated => rows.sort_by(|a, b| {
                b.rec
                    .updated_at
                    .cmp(&a.rec.updated_at)
                    .then(b.rec.number.cmp(&a.rec.number))
                    .then(a.repo_slug.as_deref().cmp(&b.repo_slug.as_deref()))
            }),
        }
        let total = rows.len();
        let mut selected: Vec<(usize, EnrichedPr)> = match &query.page {
            PrListPage::Initial => rows.into_iter().enumerate().collect(),
            PrListPage::LegacyOffset(offset) => {
                rows.into_iter().enumerate().skip(*offset).collect()
            }
            PrListPage::Keyset(cursor) => {
                let mut selected: Vec<_> = rows
                    .into_iter()
                    .enumerate()
                    .filter(|(_, row)| {
                        let before = cross_pr_before_key(row, cursor.key(), query.sort);
                        let equal = row.rec.number == cursor.key().number
                            && row.repo_slug.as_deref() == cursor.key().repo_slug.as_deref()
                            && (query.sort == PrListSort::Created
                                || row.rec.updated_at == cursor.key().updated_at);
                        match cursor.direction() {
                            PrListDirection::Newer => before,
                            PrListDirection::Older => !before && !equal,
                        }
                    })
                    .collect();
                if cursor.direction() == PrListDirection::Newer {
                    selected.reverse();
                }
                selected
            }
        };
        selected.truncate(query.limit);
        if matches!(&query.page, PrListPage::Keyset(cursor) if cursor.direction() == PrListDirection::Newer)
        {
            selected.reverse();
        }
        let has_newer = selected.first().is_some_and(|(position, _)| *position > 0);
        let has_older = selected
            .last()
            .is_some_and(|(position, _)| position.saturating_add(1) < total);
        let rows: Vec<_> = selected.into_iter().map(|(_, row)| row).collect();
        let offset = query.page.display_offset();
        let (next_cursor, prev_cursor) = mint_pr_list_cursors(
            &rows,
            endpoint,
            query.sort,
            query.limit,
            offset,
            has_older,
            has_newer,
            static_scope,
            visible_scope,
            &query.page,
        )?;
        Ok(EnrichedCrossPrSlice {
            rows,
            total,
            offset,
            limit: query.limit,
            next_cursor,
            prev_cursor,
        })
    }

    fn pr_state_token(state: PrState) -> &'static str {
        match state {
            PrState::Draft => "draft",
            PrState::Open => "open",
            PrState::Merged => "merged",
            PrState::Closed => "closed",
        }
    }

    fn pr_list_row_json(e: &EnrichedPr) -> Value {
        let rec = &e.rec;
        json!({
            "number": rec.number,
            "title": if rec.title.is_empty() { Value::Null } else { json!(rec.title) },
            "pr_state": Self::pr_state_token(rec.state),
            "base_ref": rec.base_ref,
            "head_ref": rec.head_ref,
            "author": rec.author_pseudonym,
            "author_is_agent": rec.author_is_agent,
            "reviews": rec.reviews.len(),
            "review_state": rec.review_state_label(),
            "you_are_requested": e.you_requested,
            "checks_summary": {
                "verdict": e.summary.verdict.as_str(),
                "passing": e.summary.passing,
                "failing": e.summary.failing,
                "total": e.summary.total,
            },
            "updated_at": rec.updated_at,
            "repo": e.repo_slug,
        })
    }

    pub fn set_branch_protection(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        body: &Value,
    ) -> Result<usize, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.store.open_repo(&loc)?;
        let rulesets: Vec<BranchProtectionRuleset> = body
            .get("rulesets")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .map(|r| BranchProtectionRuleset {
                        ref_pattern: r
                            .get("ref_pattern")
                            .and_then(Value::as_str)
                            .unwrap_or("refs/heads/main")
                            .to_string(),
                        required_contexts: r
                            .get("required_contexts")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        required_approvals: r
                            .get("required_approvals")
                            .and_then(Value::as_u64)
                            .unwrap_or(0) as u32,
                        require_codeowner_review: r
                            .get("require_codeowner_review")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        require_conversation_resolution: r
                            .get("require_conversation_resolution")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        allow_force_push: r
                            .get("allow_force_push")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let n = rulesets.len();
        self.prs
            .put_protection(&loc, &BranchProtectionConfig { rulesets })?;
        Ok(n)
    }

    pub fn report_checks(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        principal: &Principal,
        body: &Value,
    ) -> Result<PrRecord, DurableError> {
        let operation_id = self.fresh_operation_id()?;
        self.report_checks_with_operation(
            tenant,
            region,
            slug,
            number,
            principal,
            body,
            &operation_id,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn report_checks_with_operation(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        principal: &Principal,
        body: &Value,
        operation_id: &PrOperationId,
    ) -> Result<PrRecord, DurableError> {
        if !matches!(principal.kind, PrincipalKind::Service) {
            return Err(DurableError::Forbidden(format!(
                "git.checks.report is a CI-producer capability: principal `{}` (kind {:?}) is not a CI \
                 service producer - a human/agent writer cannot attest CI check facts on a PR",
                principal.principal_id.0, principal.kind
            )));
        }
        let loc = Self::loc(tenant, region, slug);
        let green_contexts = body
            .get("green_contexts")
            .and_then(Value::as_array)
            .map(|g| {
                g.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });
        let fork_unendorsed_contexts = body
            .get("fork_unendorsed_contexts")
            .and_then(Value::as_array)
            .map(|g| {
                g.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });
        let outstanding_conversations = body
            .get("outstanding_conversations")
            .and_then(Value::as_u64)
            .map(u32::try_from)
            .transpose()
            .map_err(|_| DurableError::Git("outstanding conversation count exceeds u32".into()))?;
        self.pr_mutate(
            &loc,
            number,
            PrMutation::ReportChecks {
                green_contexts,
                fork_unendorsed_contexts,
                codeowner_review_satisfied: body
                    .get("codeowner_review_satisfied")
                    .and_then(Value::as_bool),
                outstanding_conversations,
            },
            operation_id,
            principal,
        )
    }

    pub fn submit_review(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        verdict: &str,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        let operation_id = self.fresh_operation_id()?;
        self.submit_review_with_operation(
            tenant,
            region,
            slug,
            number,
            verdict,
            principal,
            &operation_id,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn submit_review_with_operation(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        verdict: &str,
        principal: &Principal,
        operation_id: &PrOperationId,
    ) -> Result<PrRecord, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let v = match verdict {
            "approve" => ReviewVerdict::Approve,
            "request-changes" | "request_changes" => ReviewVerdict::RequestChanges,
            "comment" => ReviewVerdict::Comment,
            other => {
                return Err(DurableError::Git(format!(
                    "unknown review verdict `{other}`"
                )))
            }
        };
        self.pr_mutate(
            &loc,
            number,
            PrMutation::SubmitReview(ReviewRecord {
                reviewer_pseudonym: Self::pseudonym(tenant, principal),
                state: ReviewState::Submitted(v),
                is_agent: Self::is_agent(principal),
            }),
            operation_id,
            principal,
        )
    }

    pub fn endorse_fork_ci(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        body: &Value,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        let operation_id = self.fresh_operation_id()?;
        self.endorse_fork_ci_with_operation(
            tenant,
            region,
            slug,
            number,
            body,
            principal,
            &operation_id,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn endorse_fork_ci_with_operation(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        body: &Value,
        principal: &Principal,
        operation_id: &PrOperationId,
    ) -> Result<PrRecord, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let rec = self
            .pr_get(&loc, number, principal)?
            .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))?;
        let to_endorse: Vec<String> = match body.get("contexts").and_then(Value::as_array) {
            Some(a) => a
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            None => rec.fork_unendorsed_contexts.clone(),
        };
        self.pr_mutate(
            &loc,
            number,
            PrMutation::EndorseContexts(to_endorse),
            operation_id,
            principal,
        )
    }

    pub fn merge(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        principal: &Principal,
    ) -> Result<MergeAttempt, DurableError> {
        let operation_id = self.fresh_operation_id()?;
        self.merge_with_operation(tenant, region, slug, number, principal, &operation_id)
    }

    pub fn merge_with_operation(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        principal: &Principal,
        operation_id: &PrOperationId,
    ) -> Result<MergeAttempt, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = Arc::new(self.store.open_repo(&loc)?);
        let ref_store = self.open_durable_refstore(repo.clone(), slug, tenant, region, principal);
        if let Some(store) = &self.pg_prs {
            let scope = Self::verified_pr_scope(principal, &loc)?;
            if let Some(intent) = store.pending_merge_intent(&scope, slug, number)? {
                if intent.operation_id != operation_id.digest()
                    || intent.actor_subject_id != principal.principal_id.0.trim()
                {
                    return Err(DurableError::Git(
                        "a different merge operation is already pending".into(),
                    ));
                }
                return store
                    .recover_pending_merge_target(
                        &scope, slug, number, principal, &loc, &repo, &ref_store,
                    )?
                    .ok_or_else(|| {
                        DurableError::Io("pending merge disappeared during recovery".into())
                    });
            }
            let rec = self
                .pr_get(&loc, number, principal)?
                .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))?;
            let source_loc = Self::loc(
                &principal.tenant.0,
                &principal.region.0,
                &rec.head_repo_slug,
            );
            if !self.repo_authz.authorize_repo_permission(
                principal,
                &source_loc,
                RepoPermission::Pull,
            ) {
                return Err(DurableError::NotFound("repository not found".into()));
            }
            let source_repo = self.store.open_repo(&source_loc)?;
            repo.import_commit_closure_from(&source_repo, &CoreOid::new(rec.head_oid.clone()))
                .map_err(sanitize_fork_import_error)?;
            return store.merge_pr_durable(
                &scope,
                slug,
                number,
                operation_id,
                principal,
                &self.prs,
                &loc,
                &repo,
                &source_repo,
                &ref_store,
                &Self::pseudonym(tenant, principal),
                self.checks.is_some(),
            );
        }
        merge_pr(
            &self.prs,
            &loc,
            number,
            &ref_store,
            &repo,
            &Self::pseudonym(tenant, principal),
        )
    }

    fn pr_object_key(slug: &str, number: u64) -> String {
        format!("pr:{slug}:{number}")
    }

    fn thread_principal(tenant: &str, principal: &Principal) -> ThreadPrincipal {
        let kind = match principal.kind {
            PrincipalKind::Agent { .. } => PrincipalRole::Agent,
            PrincipalKind::Service => PrincipalRole::Service,
            _ => PrincipalRole::Human,
        };
        ThreadPrincipal::plain(kind, Self::pseudonym(tenant, principal))
    }

    fn require_pr(
        &self,
        loc: &RepoLoc,
        number: u64,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        self.pr_get(loc, number, principal)?
            .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))
    }

    fn resolve_thread_anchor(
        &self,
        loc: &RepoLoc,
        rec: &PrRecord,
        mut anchor: ThreadAnchor,
    ) -> Result<ThreadAnchor, DurableError> {
        let side = anchor
            .side
            .ok_or_else(|| DurableError::Git("anchor side is missing".into()))?;
        let line = anchor
            .line
            .and_then(|line| u32::try_from(line).ok())
            .filter(|line| *line > 0)
            .ok_or_else(|| DurableError::Git("anchor line is invalid".into()))?;
        let repo = self.store.open_repo(loc)?;
        let diff = repo
            .pr_diff(&rec.base_ref, &rec.head_oid, PR_DIFF_PER_FILE_LINE_CAP)?
            .ok_or_else(|| DurableError::Git("anchor diff is unavailable".into()))?;
        let resolved = diff.files.iter().any(|file| {
            let side_path = match side {
                AnchorSide::Old => file.old_path.as_deref().unwrap_or(file.path.as_str()),
                AnchorSide::New => file.path.as_str(),
            };
            let path_matches = side_path == anchor.path;
            path_matches
                && file.hunks.iter().any(|hunk| {
                    hunk.lines.iter().any(|candidate| match side {
                        AnchorSide::Old => candidate.old_no == Some(line),
                        AnchorSide::New => candidate.new_no == Some(line),
                    })
                })
        });
        if !resolved {
            return Err(DurableError::Git(
                "anchor path and line are not present in the current pull request diff".into(),
            ));
        }
        anchor.base_oid = Some(diff.base_oid);
        anchor.head_oid = Some(diff.head_oid);
        anchor.anchor_state = AnchorState::Live;
        Ok(anchor)
    }

    pub fn list_threads(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        principal: &Principal,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let doc = self.threads.load(&loc, &key)?;
        let viewer = Self::pseudonym(tenant, principal);
        Ok(viewed_threads_json(&doc.view_for(&viewer)))
    }

    pub fn create_thread(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        body: &Value,
        principal: &Principal,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let rec = self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let body_md = require_body_md(body)?;
        let anchor = parse_anchor(body)?
            .map(|anchor| self.resolve_thread_anchor(&loc, &rec, anchor))
            .transpose()?;
        let author = Self::thread_principal(tenant, principal);
        let thread = self
            .threads
            .create_thread(&loc, &key, anchor, author, body_md, now_unix())?;
        self.bump_pr_updated(&loc, number, principal);
        Ok(thread_json(&thread))
    }

    pub fn add_thread_comment(
        &self,
        target: PrActorContext<'_>,
        thread_id: &str,
        body: &Value,
    ) -> Result<Value, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let body_md = require_body_md(body)?;
        let author = Self::thread_principal(tenant, principal);
        let comment =
            self.threads
                .add_comment(&loc, &key, thread_id, author, body_md, now_unix())?;
        self.bump_pr_updated(&loc, number, principal);
        Ok(comment_json(&comment))
    }

    pub fn resolve_thread(
        &self,
        target: PrActorContext<'_>,
        thread_id: &str,
        body: &Value,
    ) -> Result<Value, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let resolved = body
            .get("resolved")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        self.threads
            .resolve_thread(&loc, &key, thread_id, resolved)?;
        Ok(json!({ "thread_id": thread_id, "resolved": resolved }))
    }

    pub fn start_review_batch(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        principal: &Principal,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let reviewer = Self::thread_principal(tenant, principal);
        let batch = self.threads.start_review(&loc, &key, reviewer)?;
        Ok(review_batch_json(&batch))
    }

    pub fn add_pending_comment(
        &self,
        target: PrActorContext<'_>,
        review_id: &str,
        body: &Value,
    ) -> Result<Value, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
        let loc = Self::loc(tenant, region, slug);
        let rec = self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let body_md = require_body_md(body)?;
        let anchor = parse_anchor(body)?
            .map(|anchor| self.resolve_thread_anchor(&loc, &rec, anchor))
            .transpose()?;
        let author = Self::thread_principal(tenant, principal);
        let request =
            PendingCommentRequest::new(loc, key, review_id, anchor, author, body_md, now_unix())?;
        let comment = self.threads.add_pending_comment(request)?;
        Ok(comment_json(&comment))
    }

    pub fn submit_review_batch(
        &self,
        target: PrActorContext<'_>,
        review_id: &str,
        body: &Value,
    ) -> Result<Value, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let verdict = match body.get("verdict").and_then(Value::as_str) {
            Some("approved") | Some("approve") => BatchVerdict::Approved,
            Some("changes_requested") | Some("request-changes") | Some("request_changes") => {
                BatchVerdict::ChangesRequested
            }
            Some("commented") | Some("comment") | None => BatchVerdict::Commented,
            Some(other) => {
                return Err(DurableError::Git(format!(
                    "unknown review verdict `{other}`"
                )))
            }
        };
        let summary_md = body
            .get("summary_md")
            .and_then(Value::as_str)
            .map(str::to_string);
        let actor = Self::thread_principal(tenant, principal);
        let request = SubmitReviewRequest::new(
            loc.clone(),
            key,
            review_id,
            actor,
            verdict,
            summary_md,
            now_unix(),
        )?;
        let submitted = self.threads.submit_review(request)?;
        if let Some(ref batch) = submitted {
            if !batch.review.advisory {
                let gate_verdict = match verdict {
                    BatchVerdict::Approved => Some("approve"),
                    BatchVerdict::ChangesRequested => Some("request-changes"),
                    _ => None,
                };
                if let Some(v) = gate_verdict {
                    let _ = self.submit_review(tenant, region, slug, number, v, principal);
                }
            }
        }
        self.bump_pr_updated(&loc, number, principal);
        Ok(json!({
            "emitted": submitted.is_some(),
            "review": submitted.as_ref().map(|b| review_batch_json(&b.review)),
            "comment_ids": submitted
                .as_ref()
                .map(|b| b.comment_ids.clone())
                .unwrap_or_default(),
        }))
    }

    pub fn discard_review_batch(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        review_id: &str,
        principal: &Principal,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number, principal)?;
        let key = Self::pr_object_key(slug, number);
        let actor = Self::thread_principal(tenant, principal);
        self.threads.discard_review(&loc, &key, review_id, &actor)?;
        Ok(json!({ "discarded": review_id }))
    }

    fn bump_pr_updated(&self, loc: &RepoLoc, number: u64, principal: &Principal) {
        if let Ok(operation_id) = self.fresh_operation_id() {
            let _ = self.pr_mutate(loc, number, PrMutation::Touch, &operation_id, principal);
        }
    }

    pub fn receive_pack_refs(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
    ) -> Result<Vec<(String, String)>, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        let mut refs: Vec<(String, String)> = repo
            .list_refs_bounded(WIRE_MAX_REFS)?
            .into_iter()
            .map(|(n, o)| (n, o.0))
            .collect();
        refs.sort();
        Ok(refs)
    }

    pub fn receive_pack(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        principal: &Principal,
        body: &[u8],
    ) -> Result<Vec<u8>, DurableError> {
        use crate::git_receive_pack::{
            all_ng, parse_cat_file_batch, parse_push_request, report_status,
        };
        use std::time::{SystemTime, UNIX_EPOCH};

        let loc = Self::loc(tenant, region, slug);
        let repo = Arc::new(self.store.open_repo(&loc)?);

        let (cmds, pack) = match parse_push_request(body) {
            Ok(v) => v,
            Err(e) => return Ok(report_status(&format!("parse-error: {e}"), &[])),
        };
        if cmds.is_empty() {
            return Ok(report_status("no-commands", &[]));
        }

        let objects: Vec<(String, String, Vec<u8>)> = if pack.is_empty() {
            Vec::new()
        } else {
            let exec = crate::git_wire_exec::GitWireExecutor::new(
                self.root.clone(),
                crate::git_wire_exec::GitWireExecutor::default_limits(),
                crate::git_wire_exec::GitWireExecutor::serving_hooks(),
                self.git_wire_credentials.bind(principal),
            )
            .with_shutdown_signal(self.git_shutdown.clone());
            match exec.ingest_pack(&loc, pack) {
                Ok(stream) => match parse_cat_file_batch(&stream) {
                    Ok(o) => o,
                    Err(e) => {
                        return Ok(report_status(
                            &format!("ingest-parse: {e}"),
                            &all_ng(&cmds, "object ingest failed"),
                        ))
                    }
                },
                Err(e) => {
                    return Ok(report_status(
                        &format!("index-pack-failed: {e}"),
                        &all_ng(&cmds, "object ingest rejected"),
                    ))
                }
            }
        };

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let qdir =
            std::env::temp_dir().join(format!("myelin-ct006d-q-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&qdir);
        let q = DurableGitRepo::init_quarantine(&qdir, &repo.path().join("objects"))?;
        let mut quarantine = Vec::new();
        for (oid, ty, bytes) in &objects {
            let written = q.write_raw_object(ty, bytes)?;
            if &written.0 != oid {
                let _ = std::fs::remove_dir_all(&qdir);
                return Ok(report_status(
                    &format!("oid-mismatch: claimed {oid}, computed {}", written.0),
                    &all_ng(&cmds, "object integrity"),
                ));
            }
            quarantine.push(QuarantineObject {
                oid: PushOid::new(oid.clone()),
                bytes: bytes.clone(),
            });
        }

        let mut updates = Vec::new();
        let mut per_ref_status: Vec<(String, Option<String>)> = Vec::new();
        let existing_tips: Vec<CoreOid> = repo
            .list_refs_bounded(WIRE_MAX_REFS)
            .unwrap_or_default()
            .into_iter()
            .map(|(_, oid)| oid)
            .collect();
        for c in &cmds {
            let new_zero = c.new.chars().all(|ch| ch == '0');
            let old_zero = c.old.chars().all(|ch| ch == '0');
            if !new_zero
                && !q
                    .history_connectivity_complete(&CoreOid::new(c.new.clone()), &existing_tips)
                    .unwrap_or(false)
            {
                let _ = std::fs::remove_dir_all(&qdir);
                return Ok(report_status(
                    "ok",
                    &all_ng(
                        &cmds,
                        "rejected: incomplete history (missing ancestor/tree/blob) for a ref",
                    ),
                ));
            }
            let forced = if !old_zero && !new_zero {
                !q.is_fast_forward(
                    Some(&CoreOid::new(c.old.clone())),
                    &CoreOid::new(c.new.clone()),
                )
                .unwrap_or(false)
            } else {
                false
            };
            updates.push(ProposedRefUpdate {
                ref_name: RefName::new(c.ref_name.clone()),
                expected_old: if old_zero {
                    PushOid::zero()
                } else {
                    PushOid::new(c.old.clone())
                },
                new_oid: if new_zero {
                    PushOid::zero()
                } else {
                    PushOid::new(c.new.clone())
                },
                forced,
                commit_oids: if new_zero {
                    vec![]
                } else {
                    vec![PushOid::new(c.new.clone())]
                },
            });
            per_ref_status.push((c.ref_name.clone(), None));
        }

        let protection = match self.prs.get_protection(&loc) {
            Ok(p) => p,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&qdir);
                return Ok(report_status(
                    "ok",
                    &all_ng(
                        &cmds,
                        &format!(
                            "rejected (branch-protection policy unreadable - fail-closed): {e}"
                        ),
                    ),
                ));
            }
        };
        let pusher_has_protected_push = self.repo_authz.authorize_repo_permission(
            principal,
            &loc,
            RepoPermission::ProtectedPush,
        );
        let mut protected_updates = Vec::new();
        for (index, u) in updates.iter().enumerate() {
            let ref_str = u.ref_name.0.as_str();
            let configured = protection
                .as_ref()
                .and_then(|c| c.resolve(ref_str))
                .is_some();
            let is_protected = configured || u.ref_name.is_protected();
            if !is_protected {
                continue;
            }
            let ruleset = effective_ruleset(protection.as_ref(), ref_str);
            protected_updates.push((index, ruleset.clone()));
            if self.checks.is_some() {
                continue;
            }
            let is_delete = u.new_oid.is_zero();
            let (green, fork_unendorsed, endorsed) =
                self.check_facts_for_head(&loc, u.new_oid.0.as_str(), principal);
            let head = GitOid(u.new_oid.0.clone());
            if let Err(reason) = evaluate_protected_ref_push(
                &u.ref_name,
                is_delete,
                u.forced,
                pusher_has_protected_push,
                &ruleset,
                &head,
                &green,
                &fork_unendorsed,
                &endorsed,
            ) {
                let _ = std::fs::remove_dir_all(&qdir);
                return Ok(report_status(
                    "ok",
                    &all_ng(&cmds, &format!("rejected (branch protection): {reason:?}")),
                ));
            }
        }

        let ref_store = self.open_durable_refstore(repo.clone(), slug, tenant, region, principal);
        let push = PushSession {
            updates,
            quarantine,
            pusher: Pusher {
                pseudonym: Self::pseudonym(tenant, principal),
                is_agent: false,
            },
        };
        let outcome = if self.checks.is_some() {
            match self.receive_with_check_admission(
                &loc,
                principal,
                check_projection::ProtectedPushMutation::new(
                    Arc::clone(&repo),
                    objects.clone(),
                    ref_store,
                    push,
                ),
                protected_updates,
                pusher_has_protected_push,
            ) {
                Ok(outcome) => outcome,
                Err(check_projection::ProtectedPushAdmissionError::Scope(error)) => {
                    let _ = std::fs::remove_dir_all(&qdir);
                    return Ok(report_status(
                        "ok",
                        &all_ng(
                            &cmds,
                            &format!(
                                "rejected (check admission scope unavailable - fail-closed): {error}"
                            ),
                        ),
                    ));
                }
                Err(check_projection::ProtectedPushAdmissionError::Policy(reason)) => {
                    let _ = std::fs::remove_dir_all(&qdir);
                    return Ok(report_status(
                        "ok",
                        &all_ng(&cmds, &format!("rejected (branch protection): {reason:?}")),
                    ));
                }
                Err(check_projection::ProtectedPushAdmissionError::Projection(error)) => {
                    let _ = std::fs::remove_dir_all(&qdir);
                    return Ok(report_status(
                        "ok",
                        &all_ng(
                            &cmds,
                            &format!(
                                "rejected (check projection unavailable - fail-closed): {error}"
                            ),
                        ),
                    ));
                }
            }
        } else {
            let migration = ObjectPromotion {
                repo: &repo,
                objects: &objects,
            };
            ref_store.receive(&push, &migration, CrashPoint::None)
        };
        let _ = std::fs::remove_dir_all(&qdir);

        match outcome.map_err(|e| DurableError::Git(format!("ref-CAS: {e:?}")))? {
            PushOutcome::Accepted { .. } => {
                let _ = repo.heal_head_symref();
                Ok(report_status("ok", &per_ref_status))
            }
            PushOutcome::Rejected(reason) => Ok(report_status(
                "ok",
                &all_ng(&cmds, &format!("rejected: {reason:?}")),
            )),
            PushOutcome::Crashed(_) => Err(DurableError::Git("receive-pack crashed".into())),
        }
    }

    fn pr_json(rec: &PrRecord) -> Value {
        json!({
            "number": rec.number,
            "title": if rec.title.is_empty() { Value::Null } else { json!(rec.title) },
            "body_md": rec.body_md,
            "pr_state": Self::pr_state_token(rec.state),
            "base_ref": rec.base_ref,
            "head_ref": rec.head_ref,
            "head_oid": rec.head_oid,
            "author": rec.author_pseudonym,
            "author_is_agent": rec.author_is_agent,
            "reviews": rec.reviews.len(),
            "updated_at": rec.updated_at,
            "created_at": rec.created_at,
            "durable": true,
        })
    }

    fn commits_in_pr_count(&self, loc: &RepoLoc, rec: &PrRecord) -> Option<(u64, bool)> {
        let repo = self.store.open_repo(loc).ok()?;
        let (rows, has_more) = repo.commits_in_pr(&rec.base_ref, &rec.head_oid, 500).ok()?;
        Some((rows.len() as u64, has_more))
    }
}

struct ObjectPromotion<'a> {
    repo: &'a DurableGitRepo,
    objects: &'a [(String, String, Vec<u8>)],
}

impl QuarantineMigration for ObjectPromotion<'_> {
    fn migrate(&self, _quarantine: &[QuarantineObject]) -> Result<(), String> {
        for (claimed_oid, ty, bytes) in self.objects {
            let written = self
                .repo
                .write_raw_object(ty, bytes)
                .map_err(|e| e.to_string())?;
            if &written.0 != claimed_oid {
                return Err(format!(
                    "refusing migration: object oid mismatch (claimed {claimed_oid}, git computed {})",
                    written.0
                ));
            }
        }
        Ok(())
    }
}

fn region_of<'a>(ctx: &'a HandlerCtx<'_>) -> &'a str {
    ctx.scope.region().0.as_str()
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn require_body_md(body: &Value) -> Result<String, DurableError> {
    body.get("body_md")
        .or_else(|| body.get("body"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| DurableError::Git("comment body missing a non-empty `body_md`".into()))
}

fn parse_anchor(body: &Value) -> Result<Option<ThreadAnchor>, DurableError> {
    let Some(value) = body.get("anchor") else {
        return Ok(None);
    };
    let anchor = value
        .as_object()
        .ok_or_else(|| DurableError::Git("anchor must be an object".into()))?;
    if anchor.len() != 3
        || !anchor.contains_key("path")
        || !anchor.contains_key("line")
        || !anchor.contains_key("side")
    {
        return Err(DurableError::Git(
            "anchor must contain exactly path, line, and side".into(),
        ));
    }
    let path = anchor
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| valid_anchor_path(path))
        .ok_or_else(|| DurableError::Git("anchor path is invalid".into()))?;
    let line = anchor
        .get("line")
        .and_then(Value::as_u64)
        .filter(|line| *line > 0 && *line <= u32::MAX as u64)
        .ok_or_else(|| DurableError::Git("anchor line is invalid".into()))?;
    let side = match anchor.get("side").and_then(Value::as_str) {
        Some("old") => AnchorSide::Old,
        Some("new") => AnchorSide::New,
        _ => return Err(DurableError::Git("anchor side is invalid".into())),
    };
    Ok(Some(ThreadAnchor {
        path: path.to_string(),
        line: Some(line),
        side: Some(side),
        base_oid: None,
        head_oid: None,
        anchor_state: AnchorState::Live,
    }))
}

fn valid_anchor_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4 * 1024
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn principal_role_token(role: PrincipalRole) -> &'static str {
    match role {
        PrincipalRole::Human => "human",
        PrincipalRole::Agent => "agent",
        PrincipalRole::Service => "service",
    }
}

fn principal_json(p: &ThreadPrincipal) -> Value {
    json!({
        "kind": principal_role_token(p.kind),
        "display": p.display,
        "on_behalf_of": p.on_behalf_of,
        "trigger": p.trigger,
    })
}

fn anchor_state_token(s: AnchorState) -> &'static str {
    match s {
        AnchorState::Live => "live",
        AnchorState::Moved => "moved",
        AnchorState::Outdated => "outdated",
    }
}

fn comment_json(c: &CommentRecord) -> Value {
    json!({
        "id": c.id,
        "author": principal_json(&c.author),
        "body_md": match c.state { CommentState::Removed => Value::Null, _ => json!(c.body_md) },
        "created_at": c.created_at,
        "edited_at": c.edited_at,
        "state": match c.state { CommentState::Removed => "removed", _ => "visible" },
        "review_id": c.review_id,
        "pending": c.pending,
    })
}

fn thread_json(t: &ThreadRecord) -> Value {
    json!({
        "id": t.id,
        "anchor": t.anchor.as_ref().map(|a| json!({
            "path": a.path,
            "line": a.line,
            "side": a.side.map(|side| match side { AnchorSide::Old => "old", AnchorSide::New => "new" }),
            "base_oid": a.base_oid,
            "head_oid": a.head_oid,
            "anchor_state": anchor_state_token(a.anchor_state),
        })),
        "resolved": t.resolved,
        "comments": t.comments.iter().map(comment_json).collect::<Vec<_>>(),
    })
}

fn review_batch_json(r: &ReviewBatch) -> Value {
    json!({
        "id": r.id,
        "reviewer": principal_json(&r.reviewer),
        "verdict": r.verdict.as_str(),
        "advisory": r.advisory,
        "submitted_at": r.submitted_at,
        "summary_md": r.summary_md,
    })
}

fn viewed_threads_json(v: &ViewedThreads) -> Value {
    let (anchored, discussion): (Vec<&ThreadRecord>, Vec<&ThreadRecord>) =
        v.threads.iter().partition(|t| t.anchor.is_some());
    json!({
        "discussion": discussion.iter().map(|t| thread_json(t)).collect::<Vec<_>>(),
        "anchored": anchored.iter().map(|t| thread_json(t)).collect::<Vec<_>>(),
        "threads": v.threads.iter().map(thread_json).collect::<Vec<_>>(),
        "reviews": v.reviews.iter().map(review_batch_json).collect::<Vec<_>>(),
        "durable": true,
    })
}

const PR_LIST_QUERY_MAX_BYTES: usize = 16 * 1024;

enum ParsedPrListCursor {
    Legacy(usize),
    Keyset(PrListCursor),
}

fn parse_pr_list_cursor(value: &str) -> Result<ParsedPrListCursor, EdgeError> {
    if let Ok(parsed) = value.parse::<usize>() {
        if value == parsed.to_string() && parsed <= PR_LIST_OFFSET_MAX {
            return Ok(ParsedPrListCursor::Legacy(parsed));
        }
    }
    if value.starts_with(PR_LIST_CURSOR_PREFIX) {
        return PrListCursor::parse(value)
            .map(ParsedPrListCursor::Keyset)
            .map_err(|_| EdgeError::BadRequest("invalid pull request cursor".into()));
    }
    Err(EdgeError::BadRequest("invalid pull request cursor".into()))
}

fn decode_pr_list_query_component(raw: &str) -> Result<String, EdgeError> {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(EdgeError::BadRequest(
                    "pull request list query contains malformed percent encoding".into(),
                ));
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    let form_value = raw.replace('+', " ");
    let decoded = percent_encoding::percent_decode_str(&form_value)
        .decode_utf8()
        .map_err(|_| EdgeError::BadRequest("pull request list query is not valid UTF-8".into()))?
        .into_owned();
    if decoded.chars().any(char::is_control) {
        return Err(EdgeError::BadRequest(
            "pull request list query contains a control character".into(),
        ));
    }
    Ok(decoded)
}

fn repo_pr_list_query(
    ctx: &HandlerCtx<'_>,
    viewer_pseudonym: String,
    repo_slug: &str,
) -> Result<PrListQuery, EdgeError> {
    if ctx.request.query.len() > PR_LIST_QUERY_MAX_BYTES {
        return Err(EdgeError::BadRequest(
            "pull request list query is too large".into(),
        ));
    }
    let mut state = None;
    let mut sort = None;
    let mut cursor = None;
    let mut limit = None;
    if !ctx.request.query.is_empty() {
        for pair in ctx.request.query.split('&') {
            let (raw_name, raw_value) = pair.split_once('=').ok_or_else(|| {
                EdgeError::BadRequest("pull request list query is malformed".into())
            })?;
            let name = decode_pr_list_query_component(raw_name)?;
            let value = decode_pr_list_query_component(raw_value)?;
            let duplicate = |field: &str| {
                EdgeError::BadRequest(format!(
                    "duplicate pull request list query parameter `{field}`"
                ))
            };
            match name.as_str() {
                "state" => {
                    if state.is_some() {
                        return Err(duplicate("state"));
                    }
                    state = Some(PrListState::parse(&value).ok_or_else(|| {
                        EdgeError::BadRequest("invalid pull request state filter".into())
                    })?);
                }
                "sort" => {
                    if sort.is_some() {
                        return Err(duplicate("sort"));
                    }
                    sort = Some(PrListSort::parse(&value).ok_or_else(|| {
                        EdgeError::BadRequest("invalid pull request sort".into())
                    })?);
                }
                "cursor" => {
                    if cursor.is_some() {
                        return Err(duplicate("cursor"));
                    }
                    cursor = Some(parse_pr_list_cursor(&value)?);
                }
                "limit" => {
                    if limit.is_some() {
                        return Err(duplicate("limit"));
                    }
                    let parsed = value.parse::<usize>().ok().filter(|parsed| {
                        value == parsed.to_string() && (1..=MAX_PAGE_LIMIT).contains(parsed)
                    });
                    limit = Some(parsed.ok_or_else(|| {
                        EdgeError::BadRequest(format!(
                            "pull request list limit must be canonical and within 1..={MAX_PAGE_LIMIT}"
                        ))
                    })?);
                }
                _ => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown pull request list query parameter `{name}`"
                    )))
                }
            }
        }
    }
    let cursor_fields = match &cursor {
        Some(ParsedPrListCursor::Keyset(cursor)) => {
            let PrListCursorEndpoint::Repository(cursor_state) = cursor.endpoint() else {
                return Err(EdgeError::BadRequest(
                    "pull request list cursor scope mismatch".into(),
                ));
            };
            Some((cursor_state, cursor.sort(), cursor.limit()))
        }
        _ => None,
    };
    let effective_state =
        cursor_fields.map_or(state.unwrap_or(PrListState::Open), |fields| fields.0);
    let effective_sort =
        cursor_fields.map_or(sort.unwrap_or(PrListSort::Updated), |fields| fields.1);
    let effective_limit =
        cursor_fields.map_or(limit.unwrap_or(DEFAULT_PAGE_LIMIT), |fields| fields.2);
    if state.is_some_and(|value| value != effective_state)
        || sort.is_some_and(|value| value != effective_sort)
        || limit.is_some_and(|value| value != effective_limit)
    {
        return Err(EdgeError::BadRequest(
            "pull request list cursor scope mismatch".into(),
        ));
    }
    let page = match cursor {
        None => PrListPage::Initial,
        Some(ParsedPrListCursor::Legacy(offset)) => PrListPage::LegacyOffset(offset),
        Some(ParsedPrListCursor::Keyset(cursor)) => {
            let expected = pr_list_static_scope(
                tenant_of(ctx),
                region_of(ctx),
                &viewer_pseudonym,
                PrListCursorEndpoint::Repository(effective_state),
                Some(repo_slug),
                effective_sort,
                effective_limit,
            );
            if cursor.static_scope() != expected {
                return Err(EdgeError::BadRequest(
                    "pull request list cursor scope mismatch".into(),
                ));
            }
            PrListPage::Keyset(cursor)
        }
    };
    PrListQuery::from_page(
        effective_state,
        effective_sort,
        page,
        effective_limit,
        viewer_pseudonym,
    )
    .map_err(|_| EdgeError::BadRequest("invalid pull request page".into()))
}

fn cross_pr_list_query(
    ctx: &HandlerCtx<'_>,
    viewer_pseudonym: String,
) -> Result<PrCrossListQuery, EdgeError> {
    if ctx.request.query.len() > PR_LIST_QUERY_MAX_BYTES {
        return Err(EdgeError::BadRequest(
            "pull request list query is too large".into(),
        ));
    }
    let mut bucket = None;
    let mut sort = None;
    let mut cursor = None;
    let mut limit = None;
    if !ctx.request.query.is_empty() {
        for pair in ctx.request.query.split('&') {
            let (raw_name, raw_value) = pair.split_once('=').ok_or_else(|| {
                EdgeError::BadRequest("pull request list query is malformed".into())
            })?;
            let name = decode_pr_list_query_component(raw_name)?;
            let value = decode_pr_list_query_component(raw_value)?;
            let duplicate = |field: &str| {
                EdgeError::BadRequest(format!(
                    "duplicate pull request list query parameter `{field}`"
                ))
            };
            match name.as_str() {
                "bucket" => {
                    if bucket.is_some() {
                        return Err(duplicate("bucket"));
                    }
                    bucket = Some(PrListBucket::parse(&value).ok_or_else(|| {
                        EdgeError::BadRequest("invalid pull request bucket".into())
                    })?);
                }
                "sort" => {
                    if sort.is_some() {
                        return Err(duplicate("sort"));
                    }
                    sort = Some(PrListSort::parse(&value).ok_or_else(|| {
                        EdgeError::BadRequest("invalid pull request sort".into())
                    })?);
                }
                "cursor" => {
                    if cursor.is_some() {
                        return Err(duplicate("cursor"));
                    }
                    cursor = Some(parse_pr_list_cursor(&value)?);
                }
                "limit" => {
                    if limit.is_some() {
                        return Err(duplicate("limit"));
                    }
                    let parsed = value.parse::<usize>().ok().filter(|parsed| {
                        value == parsed.to_string() && (1..=MAX_PAGE_LIMIT).contains(parsed)
                    });
                    limit = Some(parsed.ok_or_else(|| {
                        EdgeError::BadRequest(format!(
                            "pull request list limit must be canonical and within 1..={MAX_PAGE_LIMIT}"
                        ))
                    })?);
                }
                _ => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown pull request list query parameter `{name}`"
                    )))
                }
            }
        }
    }
    let cursor_fields = match &cursor {
        Some(ParsedPrListCursor::Keyset(cursor)) => {
            let PrListCursorEndpoint::CrossRepository(cursor_bucket) = cursor.endpoint() else {
                return Err(EdgeError::BadRequest(
                    "pull request list cursor scope mismatch".into(),
                ));
            };
            Some((cursor_bucket, cursor.sort(), cursor.limit()))
        }
        _ => None,
    };
    let effective_bucket = cursor_fields
        .map_or(bucket.unwrap_or(PrListBucket::NeedsReview), |fields| {
            fields.0
        });
    let effective_sort =
        cursor_fields.map_or(sort.unwrap_or(PrListSort::Updated), |fields| fields.1);
    let effective_limit =
        cursor_fields.map_or(limit.unwrap_or(DEFAULT_PAGE_LIMIT), |fields| fields.2);
    if bucket.is_some_and(|value| value != effective_bucket)
        || sort.is_some_and(|value| value != effective_sort)
        || limit.is_some_and(|value| value != effective_limit)
    {
        return Err(EdgeError::BadRequest(
            "pull request list cursor scope mismatch".into(),
        ));
    }
    let page = match cursor {
        None => PrListPage::Initial,
        Some(ParsedPrListCursor::Legacy(offset)) => PrListPage::LegacyOffset(offset),
        Some(ParsedPrListCursor::Keyset(cursor)) => {
            let expected = pr_list_static_scope(
                tenant_of(ctx),
                region_of(ctx),
                &viewer_pseudonym,
                PrListCursorEndpoint::CrossRepository(effective_bucket),
                None,
                effective_sort,
                effective_limit,
            );
            if cursor.static_scope() != expected {
                return Err(EdgeError::BadRequest(
                    "pull request list cursor scope mismatch".into(),
                ));
            }
            PrListPage::Keyset(cursor)
        }
    };
    PrCrossListQuery::from_page(
        effective_bucket,
        effective_sort,
        page,
        effective_limit,
        viewer_pseudonym,
    )
    .map_err(|_| EdgeError::BadRequest("invalid pull request page".into()))
}

fn qualify_ref(gitref: &str) -> String {
    if gitref.starts_with("refs/") {
        gitref.to_string()
    } else {
        format!("refs/heads/{gitref}")
    }
}

const LATEST_COMMIT_WALK_CAP: usize = 500;

const REPO_SCAN_MAX_CANDIDATES: usize = 10_000;

const PR_LIST_PER_REPO_MAX_RECORDS: usize = PR_LIST_OFFSET_MAX;
const PR_LIST_PER_REPO_MAX_BYTES: usize = 64 * 1024 * 1024;

const CROSS_PR_LIST_MAX_RECORDS: usize = 10_000;
const CROSS_PR_LIST_MAX_BYTES: usize = 64 * 1024 * 1024;

fn checked_cross_pr_list_total(
    current: usize,
    addition: usize,
    maximum: usize,
    dimension: &'static str,
) -> Result<usize, DurableError> {
    current
        .checked_add(addition)
        .filter(|total| *total <= maximum)
        .ok_or_else(|| DurableError::Git(format!("pull request list limit exceeded: {dimension}")))
}

fn serialized_pr_records_bytes(records: &[PrRecord]) -> Result<usize, DurableError> {
    records.iter().try_fold(0usize, |total, record| {
        let bytes = serde_json::to_vec(record)
            .map_err(|error| DurableError::Io(format!("serialize pull request record: {error}")))?;
        checked_cross_pr_list_total(
            total,
            bytes.len(),
            usize::MAX,
            "cross-repository serialized bytes",
        )
    })
}

const BOOT_RECOVERY_MAX_PENDING_MERGES: usize = 10_000;
const BOOT_RECOVERY_MAX_PENDING_MERGE_BYTES: usize = 64 * 1024 * 1024;
const BOOT_RECOVERY_MAX_RETAINED_OUTBOX_ROWS: usize = 100_000;
const BOOT_RECOVERY_MAX_RETAINED_OUTBOX_BYTES: usize = 256 * 1024 * 1024;

const BLOB_INLINE_CAP: usize = 512 * 1024;

const README_MAX_BYTES: usize = 512 * 1024;

const RAW_BLOB_MAX_BYTES: usize = 64 * 1024 * 1024;

const CODE_SEARCH_MAX_REPOS: usize = 100;
const CODE_SEARCH_MAX_ENTRIES: usize = 10_000;
const CODE_SEARCH_MAX_BLOB_BYTES: usize = 512 * 1024;
const CODE_SEARCH_MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const CODE_SEARCH_MAX_RESULTS: usize = 100;
const CODE_SEARCH_MAX_EXCERPT_CHARS: usize = 500;

struct CodeSearchBudget {
    entries: usize,
    bytes: usize,
    incomplete: bool,
    exhausted: bool,
}

impl CodeSearchBudget {
    fn new() -> Self {
        Self {
            entries: 0,
            bytes: 0,
            incomplete: false,
            exhausted: false,
        }
    }
}

fn first_root_tree_page(repo: &DurableGitRepo, branch_ref: &str) -> Result<TreePage, DurableError> {
    match repo.tree_page(branch_ref, "", TreePageRequest::default()) {
        Ok(TreePageLookup::Dir(page)) => Ok(page),
        Ok(TreePageLookup::IsFile | TreePageLookup::Missing) => Err(DurableError::NotFound(
            format!("default branch `{branch_ref}` did not resolve to a root tree"),
        )),
        Err(TreePageError::Durable(error)) => Err(error),
        Err(error) => Err(DurableError::Git(format!(
            "default root tree page failed: {error}"
        ))),
    }
}

fn read_text_blob_at_snapshot_bounded(
    repo: &DurableGitRepo,
    snapshot_oid: &CoreOid,
    path: &str,
    maximum_bytes: usize,
) -> Result<Option<String>, DurableError> {
    match repo.read_blob_at_commit_oid_bounded(snapshot_oid, path, maximum_bytes)? {
        BlobPathLookup::Found {
            bytes,
            is_binary: false,
            ..
        } => Ok(Some(String::from_utf8_lossy(&bytes).to_string())),
        BlobPathLookup::Found { .. }
        | BlobPathLookup::TooLarge { .. }
        | BlobPathLookup::IsDir
        | BlobPathLookup::Missing => Ok(None),
    }
}

fn search_repo_code(
    repo: &DurableGitRepo,
    slug: &str,
    branch_ref: &str,
    query: &str,
    hits: &mut Vec<Value>,
    budget: &mut CodeSearchBudget,
) -> Result<(), DurableError> {
    let root = match repo.tree_page(
        branch_ref,
        "",
        TreePageRequest {
            limit: TREE_PAGE_MAX_LIMIT,
            query: None,
            cursor: None,
        },
    ) {
        Ok(TreePageLookup::Dir(page)) => page,
        Ok(TreePageLookup::IsFile | TreePageLookup::Missing) => return Ok(()),
        Err(TreePageError::Durable(error)) => return Err(error),
        Err(error) => return Err(DurableError::Git(format!("code search tree read: {error}"))),
    };
    let snapshot = root.snapshot_oid;
    let snapshot_ref = snapshot.as_str().to_string();
    let mut directories = vec![String::new()];
    while let Some(directory) = directories.pop() {
        let mut cursor = None;
        loop {
            let page = match repo.tree_page(
                &snapshot_ref,
                &directory,
                TreePageRequest {
                    limit: TREE_PAGE_MAX_LIMIT,
                    query: None,
                    cursor,
                },
            ) {
                Ok(TreePageLookup::Dir(page)) => page,
                Ok(TreePageLookup::IsFile | TreePageLookup::Missing) => break,
                Err(TreePageError::Durable(error)) => return Err(error),
                Err(error) => {
                    return Err(DurableError::Git(format!("code search tree read: {error}")))
                }
            };
            for entry in &page.entries {
                budget.entries = budget.entries.saturating_add(1);
                if budget.entries > CODE_SEARCH_MAX_ENTRIES {
                    budget.incomplete = true;
                    budget.exhausted = true;
                    return Ok(());
                }
                let path = if directory.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{directory}/{}", entry.name)
                };
                if entry.is_dir {
                    directories.push(path);
                    continue;
                }
                let blob = match repo.read_blob_at_commit_oid_bounded(
                    &snapshot,
                    &path,
                    CODE_SEARCH_MAX_BLOB_BYTES,
                )? {
                    BlobPathLookup::Found {
                        bytes,
                        is_binary: false,
                        ..
                    } => bytes,
                    BlobPathLookup::TooLarge { .. } => {
                        budget.incomplete = true;
                        continue;
                    }
                    BlobPathLookup::Found { .. }
                    | BlobPathLookup::IsDir
                    | BlobPathLookup::Missing => continue,
                };
                budget.bytes = budget.bytes.saturating_add(blob.len());
                if budget.bytes > CODE_SEARCH_MAX_TOTAL_BYTES {
                    budget.incomplete = true;
                    budget.exhausted = true;
                    return Ok(());
                }
                let text = String::from_utf8_lossy(&blob);
                for (index, line) in text.lines().enumerate() {
                    if !line.contains(query) {
                        continue;
                    }
                    hits.push(json!({
                        "repo": slug,
                        "ref": branch_ref,
                        "snapshot_oid": snapshot.as_str(),
                        "path": path,
                        "line": index + 1,
                        "excerpt": line.chars().take(CODE_SEARCH_MAX_EXCERPT_CHARS).collect::<String>(),
                    }));
                    if hits.len() >= CODE_SEARCH_MAX_RESULTS {
                        budget.incomplete = true;
                        budget.exhausted = true;
                        return Ok(());
                    }
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
    }
    Ok(())
}

fn short_oid12(oid: &str) -> String {
    oid.chars().take(12).collect()
}

fn sanitize_fork_import_error(_error: DurableError) -> DurableError {
    DurableError::Git("fork commit import could not be completed".into())
}

fn commit_brief_json(m: &CommitMeta) -> Value {
    json!({
        "short_oid": short_oid12(&m.oid),
        "oid": m.oid,
        "summary": m.summary,
        "author": m.author_name,
        "committed_at": m.time,
    })
}

fn tree_entries_json(
    entries: &[myelin_git::durable::TreeEntryInfo],
    base: &str,
    per_entry: &std::collections::BTreeMap<String, CommitMeta>,
) -> Vec<Value> {
    entries
        .iter()
        .map(|e| {
            let full = if base.is_empty() {
                e.name.clone()
            } else {
                format!("{base}/{}", e.name)
            };
            let mut o = json!({ "name": e.name, "path": full, "is_dir": e.is_dir });
            if let Some(sz) = e.size {
                o["size"] = json!(sz);
            }
            if let Some(m) = per_entry.get(&e.name) {
                o["latest_commit"] = commit_brief_json(m);
            }
            o
        })
        .collect()
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\' && *c != '/')
        .collect();
    if cleaned.is_empty() {
        "download".to_string()
    } else {
        cleaned
    }
}

fn commit_row(m: CommitMeta) -> CommitRow {
    CommitRow {
        oid: m.oid,
        summary: m.summary,
        author: m.author_name,
        committed_at: m.time,
        parents: m.parents,
    }
}

fn commit_diff_vm(d: CommitDetail) -> CommitDiff {
    CommitDiff {
        commit: commit_row(d.meta),
        message: d.message,
        files: d
            .files
            .into_iter()
            .map(|f| DiffFile {
                path: f.path,
                old_path: f.old_path,
                status: f.status,
                lines: f
                    .lines
                    .into_iter()
                    .map(|(origin, content)| DiffLineView { origin, content })
                    .collect(),
            })
            .collect(),
    }
}

const PR_DIFF_PER_FILE_LINE_CAP: usize = 4000;

fn pr_diff_line(l: myelin_git::durable::DiffLineDelta) -> PrDiffLine {
    PrDiffLine {
        origin: l.origin,
        content: l.content,
        old_no: l.old_no,
        new_no: l.new_no,
    }
}

fn pr_diff_vm(number: u64, base_ref: &str, diff: PrDiff, offset: usize, limit: usize) -> PrDiffVM {
    let total_files = diff.total_files;
    let files: Vec<PrDiffFile> = diff
        .files
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|f| PrDiffFile {
            path: f.path,
            old_path: f.old_path,
            new_blob_oid: f.new_blob_oid,
            status: f.status,
            kind: f.kind.as_str().to_string(),
            additions: f.additions,
            deletions: f.deletions,
            size_bytes: f.size_bytes,
            hunks: f
                .hunks
                .into_iter()
                .map(|h| PrDiffHunk {
                    header: h.header,
                    old_start: h.old_start,
                    old_lines: h.old_lines,
                    new_start: h.new_start,
                    new_lines: h.new_lines,
                    lines: h.lines.into_iter().map(pr_diff_line).collect(),
                })
                .collect(),
            deleted_body_available: f.deleted_body_available,
            truncated: f.truncated,
        })
        .collect();
    let next_cursor = if offset.saturating_add(limit) < total_files {
        Some(offset.saturating_add(limit).to_string())
    } else {
        None
    };
    PrDiffVM {
        number,
        base_ref: base_ref.to_string(),
        base_oid: diff.base_oid,
        head_oid: diff.head_oid,
        three_dot: diff.three_dot,
        files,
        restricted_files: 0,
        total_files,
        total_additions: diff.total_additions,
        total_deletions: diff.total_deletions,
        next_cursor,
        limit,
    }
}

fn map_durable_err(e: DurableError) -> EdgeError {
    match e {
        DurableError::NotFound(m) => EdgeError::NotFound(m),
        DurableError::Git(m) if m == "pull request list cursor visible set changed" => {
            EdgeError::Conflict("pull request list cursor is stale; restart pagination".into())
        }
        DurableError::Git(m) if m.starts_with("browse response limit exceeded:") => {
            EdgeError::PayloadTooLarge(
                "repository view exceeds the interactive browse limit".into(),
            )
        }
        DurableError::Git(m) if m.starts_with("tree page limit exceeded:") => {
            EdgeError::PayloadTooLarge(
                "repository tree exceeds the interactive browse limit".into(),
            )
        }
        DurableError::Git(m) if m.starts_with("pr diff computation limit exceeded:") => {
            EdgeError::PayloadTooLarge(
                "pull request diff exceeds the interactive file limit".into(),
            )
        }
        DurableError::Git(m) if m.starts_with("commit diff computation limit exceeded:") => {
            EdgeError::PayloadTooLarge("commit diff exceeds the interactive content limit".into())
        }
        DurableError::Git(m) if m.starts_with("blame limit exceeded:") => {
            EdgeError::PayloadTooLarge("file exceeds the interactive blame limit".into())
        }
        DurableError::Git(m) if m.starts_with("blame unavailable:") => {
            EdgeError::BadRequest(m)
        }
        DurableError::Git(m) if m.starts_with("pull request list limit exceeded:") => {
            EdgeError::PayloadTooLarge(
                "pull request list exceeds the interactive record limit".into(),
            )
        }
        DurableError::Git(m) if m.starts_with("pull request record limit exceeded:") => {
            EdgeError::PayloadTooLarge("pull request record exceeds the storage limit".into())
        }
        DurableError::Git(m) if m.starts_with("branch protection limit exceeded:") => {
            EdgeError::PayloadTooLarge("branch protection policy exceeds the storage limit".into())
        }
        DurableError::Git(m) if m.starts_with("wire ref limit exceeded:") => {
            EdgeError::PayloadTooLarge("repository exceeds the smart-HTTP ref limit".into())
        }
        DurableError::Git(m)
            if m.contains("traversal")
                || m.contains("segment")
                || m.contains("slug")
                || m.contains("missing")
                || m.contains("exceeds")
                || m.contains("anchor") =>
        {
            EdgeError::BadRequest(m)
        }
        DurableError::CasMismatch { .. } => EdgeError::Conflict(e.to_string()),
        DurableError::Forbidden(m) => EdgeError::Forbidden(m),
        other => EdgeError::Internal(other.to_string()),
    }
}

fn map_repo_summary_durable_err(error: DurableError) -> EdgeError {
    match error {
        DurableError::Git(message) if message.starts_with("wire ref limit exceeded:") => {
            EdgeError::PayloadTooLarge(
                "repository catalogue exceeds the interactive list limit".into(),
            )
        }
        other => map_durable_err(other),
    }
}

const REPO_SUMMARY_QUERY_MAX_BYTES: usize = 16 * 1024;
const REPO_SUMMARY_RESPONSE_MAX_BYTES: usize = 1024 * 1024;

struct RepoSummaryQuery {
    limit: usize,
    cursor: Option<String>,
}

fn repo_summary_requested(query: &str) -> bool {
    query.split('&').any(|pair| {
        let raw_name = pair.split_once('=').map_or(pair, |(name, _)| name);
        let form_name = raw_name.replace('+', " ");
        percent_encoding::percent_decode_str(&form_name)
            .decode_utf8()
            .is_ok_and(|name| name == "view")
    })
}

fn decode_repo_summary_query_component(raw: &str) -> Result<String, EdgeError> {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(EdgeError::BadRequest(
                    "repository summary query contains malformed percent encoding".into(),
                ));
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    let form_value = raw.replace('+', " ");
    let decoded = percent_encoding::percent_decode_str(&form_value)
        .decode_utf8()
        .map_err(|_| EdgeError::BadRequest("repository summary query is not valid UTF-8".into()))?
        .into_owned();
    if decoded.chars().any(char::is_control) {
        return Err(EdgeError::BadRequest(
            "repository summary query contains a control character".into(),
        ));
    }
    Ok(decoded)
}

fn parse_repo_summary_query(query: &str) -> Result<RepoSummaryQuery, EdgeError> {
    if query.len() > REPO_SUMMARY_QUERY_MAX_BYTES {
        return Err(EdgeError::BadRequest(
            "repository summary query is too large".into(),
        ));
    }
    let mut view = None;
    let mut limit = None;
    let mut cursor = None;
    for pair in query.split('&') {
        let (raw_name, raw_value) = pair.split_once('=').ok_or_else(|| {
            EdgeError::BadRequest("malformed repository summary query parameter".into())
        })?;
        let name = decode_repo_summary_query_component(raw_name)?;
        let value = decode_repo_summary_query_component(raw_value)?;
        let duplicate = |field: &str| {
            EdgeError::BadRequest(format!(
                "duplicate repository summary query parameter `{field}`"
            ))
        };
        match name.as_str() {
            "view" => {
                if view.is_some() {
                    return Err(duplicate("view"));
                }
                if value != "summary" {
                    return Err(EdgeError::BadRequest(
                        "repository list view must be `summary`".into(),
                    ));
                }
                view = Some(());
            }
            "limit" => {
                if limit.is_some() {
                    return Err(duplicate("limit"));
                }
                let parsed = value.parse::<usize>().ok().filter(|parsed| {
                    value == parsed.to_string() && (1..=MAX_PAGE_LIMIT).contains(parsed)
                });
                limit = Some(parsed.ok_or_else(|| {
                    EdgeError::BadRequest(format!(
                        "repository summary limit must be canonical and within 1..={MAX_PAGE_LIMIT}"
                    ))
                })?);
            }
            "cursor" => {
                if cursor.is_some() {
                    return Err(duplicate("cursor"));
                }
                if !value.starts_with(REPO_LIST_CURSOR_PREFIX)
                    || value.len() > REPO_LIST_CURSOR_MAX_BYTES
                {
                    return Err(EdgeError::BadRequest(
                        "repository summary cursor is malformed".into(),
                    ));
                }
                cursor = Some(value);
            }
            "" => {
                return Err(EdgeError::BadRequest(
                    "empty repository summary query parameter name".into(),
                ))
            }
            _ => {
                return Err(EdgeError::BadRequest(format!(
                    "unknown repository summary query parameter `{name}`"
                )))
            }
        }
    }
    if view.is_none() {
        return Err(EdgeError::BadRequest(
            "repository summary query requires `view=summary`".into(),
        ));
    }
    Ok(RepoSummaryQuery {
        limit: limit.unwrap_or(DEFAULT_PAGE_LIMIT),
        cursor,
    })
}

fn repo_summary_cursor_scope(tenant: &str, region: &str) -> [u8; 32] {
    let mut hash = blake3::Hasher::new();
    hash.update(b"myelin.edge.durable-repository-catalogue.v1\0");
    for value in [tenant, region] {
        hash.update(&(value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    *hash.finalize().as_bytes()
}

fn parse_repo_summary_cursor(
    value: &str,
    tenant: &str,
    region: &str,
) -> Result<RepoListCursor, EdgeError> {
    let cursor = RepoListCursor::parse(value)
        .map_err(|_| EdgeError::BadRequest("repository summary cursor is malformed".into()))?;
    if cursor.scope() != repo_summary_cursor_scope(tenant, region) {
        return Err(EdgeError::BadRequest(
            "repository summary cursor scope mismatch".into(),
        ));
    }
    Ok(cursor)
}

fn repo_summary_response(value: &Value) -> Result<EdgeResponse, EdgeError> {
    let body = serde_json::to_vec(value)
        .map_err(|error| EdgeError::Internal(format!("serialize repository summary: {error}")))?;
    if body.len() > REPO_SUMMARY_RESPONSE_MAX_BYTES {
        return Err(EdgeError::PayloadTooLarge(
            "repository summary exceeds the response byte limit".into(),
        ));
    }
    Ok(EdgeResponse::Bytes {
        status: 200,
        content_type: "application/json".into(),
        headers: Vec::new(),
        body,
    })
}

mod check_projection;
mod blame;
mod http;
pub use check_projection::GitDatabaseProviders;
pub use http::register_git_durable;
use http::{cross_pr_before_key, mint_pr_list_cursors, BlobViewOptions, RawResponseOptions};
