use crate::catalogue::{page_envelope, Handler, HandlerCtx, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::git_edge::{map_method, param, pull_request_number_param, reroot, tenant_of};
#[cfg(any(test, feature = "test-support"))]
use crate::repo_authz::AllowAllRepos;
use crate::repo_authz::{DenyAllRepos, RepoAuthorizer, RepoPermission};
use crate::repo_authz_live::{NoRepoBootstrap, RepoBootstrapGrants};
use crate::request::{EdgeRequest, EdgeResponse};
#[cfg(any(test, feature = "test-support"))]
use myelin_events::MonotonicMinter;
use myelin_events::{Actor, EmitContextBase, IdMinter, OutboxStore, Region, TenantId, Timestamp};
use myelin_git::api::{
    http_catalogue, valid_code_search_query, valid_code_search_repo, Method as GitMethod,
};
use myelin_git::check_status::GitOid;
use myelin_git::core::{Oid as CoreOid, RepoLoc};
use myelin_git::durable::{
    BlobPathLookup, CommitDetail, CommitMeta, FileLinesLookup, PrCommitPageError, PrCommitSnapshot,
    PrDiff, TreePage, TreePageError, TreePageLookup, TreePageRequest, COMMIT_LOG_MAX_OFFSET,
    FILE_LINES_MAX_RANGE, REFS_PAGE_DEFAULT_LIMIT, REFS_PAGE_MAX_LIMIT, REFS_PAGE_MAX_QUERY_BYTES,
    TREE_OBJECT_MAX_BYTES, TREE_PAGE_DEFAULT_LIMIT, TREE_PAGE_MAX_LIMIT, TREE_PAGE_MAX_QUERY_BYTES,
    WIRE_MAX_REFS,
};
use myelin_git::durable::{
    CatalogueRepoState, DurableError, DurableGitRepo, DurableGitStore, RefKind, RefsPageError,
    RefsPageRequest, RepoCreationClaim,
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
    AnchorSide, AnchorState, BatchVerdict, CommentRecord, CommentState, CommentWrite,
    DurablePrThreadStore, PendingCommentRequest, PrincipalRole, ReviewBatch, ReviewDecision,
    SubmitReviewRequest, ThreadAnchor, ThreadPrincipal, ThreadRecord, ViewedThreads,
};
use myelin_git::receive_pack::{
    evaluate_protected_ref_push, CrashPoint, Oid as PushOid, ProposedRefUpdate, PushOutcome,
    PushProvenance, PushSession, Pusher, QuarantineMigration, QuarantineObject, RefName, RefStore,
    RejectReason,
};
use myelin_git::refs_pagination::WIRE_MAX_REF_NAME_BYTES;
use myelin_git::web::{
    CommitDiff, CommitRow, DiffFile, DiffLineView, PrCommitCursor, PrDiffFile, PrDiffHunk,
    PrDiffLine, PrDiffVM, RepoHome, RepoListCursor, RepoListRow, WebEditOutcome,
    PR_COMMIT_CURSOR_MAX_POSITION,
};
#[cfg(test)]
use myelin_git::web::{REPO_LIST_CURSOR_MAX_BYTES, REPO_LIST_CURSOR_PREFIX};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{DurablePlacementBacking, KmsEngine, SubstrateProvider, TenantScope};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use self::mutation_support::*;
use self::pr_queries::*;
use self::repo_summary::*;
pub(crate) use self::repository_views::map_durable_err;
use self::repository_views::*;

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

const MAX_CHECK_CONTEXTS: usize = 256;
const MAX_CHECK_CONTEXT_BYTES: usize = 255;
const MAX_BRANCH_PROTECTION_RULESETS: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BranchProtectionRequest {
    rulesets: Vec<BranchProtectionRuleRequest>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BranchProtectionRuleRequest {
    ref_pattern: String,
    #[serde(default)]
    required_contexts: Vec<String>,
    #[serde(default)]
    required_approvals: u32,
    #[serde(default)]
    require_codeowner_review: bool,
    #[serde(default)]
    require_conversation_resolution: bool,
    #[serde(default)]
    allow_force_push: bool,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckReportRequest {
    green_contexts: Option<Vec<String>>,
    fork_unendorsed_contexts: Option<Vec<String>>,
    codeowner_review_satisfied: Option<bool>,
    outstanding_conversations: Option<u32>,
}

impl CheckReportRequest {
    fn has_update(&self) -> bool {
        self.green_contexts.is_some()
            || self.fork_unendorsed_contexts.is_some()
            || self.codeowner_review_satisfied.is_some()
            || self.outstanding_conversations.is_some()
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EndorseForkCiRequest {
    contexts: Option<Vec<String>>,
}

fn invalid_request(label: &str, error: impl std::fmt::Display) -> DurableError {
    DurableError::InvalidInput(format!("{label} body is malformed: {error}"))
}

fn validate_check_contexts(label: &str, contexts: &[String]) -> Result<(), DurableError> {
    if contexts.len() > MAX_CHECK_CONTEXTS {
        return Err(DurableError::InvalidInput(format!(
            "{label} may contain at most {MAX_CHECK_CONTEXTS} contexts"
        )));
    }
    let mut seen = std::collections::BTreeSet::new();
    for context in contexts {
        let valid_provider = context.split_once('/').is_some_and(|(provider, name)| {
            matches!(provider, "ci" | "external") && !name.is_empty()
        });
        if !valid_provider
            || context.len() > MAX_CHECK_CONTEXT_BYTES
            || context.trim() != context
            || context.chars().any(char::is_control)
        {
            return Err(DurableError::InvalidInput(format!(
                "{label} entries must be 1-{MAX_CHECK_CONTEXT_BYTES} byte `ci/<name>` or \
                 `external/<name>` policy tokens without surrounding whitespace"
            )));
        }
        if !seen.insert(context) {
            return Err(DurableError::InvalidInput(format!(
                "{label} contains duplicate context `{context}`"
            )));
        }
    }
    Ok(())
}

fn validate_ref_pattern(pattern: &str) -> Result<(), DurableError> {
    if !pattern.starts_with("refs/heads/")
        || pattern.len() <= "refs/heads/".len()
        || pattern.len() > WIRE_MAX_REF_NAME_BYTES
        || pattern.chars().any(char::is_whitespace)
        || pattern.chars().any(char::is_control)
    {
        return Err(DurableError::InvalidInput(
            "branch-protection `ref_pattern` must be a bounded `refs/heads/<pattern>` token".into(),
        ));
    }
    Ok(())
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

pub(crate) struct AgentFileWrite<'a> {
    pub target: RepoActorContext<'a>,
    pub gitref: &'a str,
    pub path: &'a str,
    pub expected_base: &'a str,
    pub contents: &'a str,
    pub start_ref: Option<&'a str>,
    pub operation_id: &'a PrOperationId,
}

struct FileCommit<'a> {
    target: RepoActorContext<'a>,
    gitref: &'a str,
    path: &'a str,
    expected_base: &'a str,
    contents: &'a str,
    start_ref: Option<&'a str>,
    message: &'a str,
    actor_is_agent: bool,
}

struct RequestOperation {
    nonce: String,
    pr_id: PrOperationId,
}

const AGENT_FILE_WRITE_MAX_BYTES: usize = 1024 * 1024;

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
            mutation.apply_to(record);
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

    fn request_operation_id(
        &self,
        request: &EdgeRequest,
        principal: &Principal,
    ) -> Result<PrOperationId, EdgeError> {
        if self.pg_prs.is_none() {
            return self.fresh_operation_id().map_err(map_durable_err);
        }
        let nonce = request.stable_idempotency_nonce(&principal.principal_id.0)?;
        PrOperationId::parse(&nonce)
            .map_err(|_| EdgeError::BadRequest("invalid `Idempotency-Key` header".into()))
    }

    fn required_request_operation(
        &self,
        request: &EdgeRequest,
        principal: &Principal,
    ) -> Result<RequestOperation, EdgeError> {
        let nonce = request.stable_idempotency_nonce(&principal.principal_id.0)?;
        let pr_id = PrOperationId::parse(&nonce)
            .map_err(|_| EdgeError::BadRequest("invalid `Idempotency-Key` header".into()))?;
        Ok(RequestOperation { nonce, pr_id })
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

    pub(super) fn repo_policy(
        &self,
        loc: &RepoLoc,
    ) -> Result<(Option<BranchProtectionConfig>, RefName), DurableError> {
        let config = self.prs.get_protection(loc)?;
        let default_ref = RefName::new(self.store.open_repo(loc)?.default_branch_ref()?);
        Ok((config, default_ref))
    }

    pub(super) fn effective_ruleset_for(
        &self,
        loc: &RepoLoc,
        base_ref: &str,
    ) -> Result<BranchProtectionRuleset, DurableError> {
        let (config, default_ref) = self.repo_policy(loc)?;
        Ok(effective_ruleset(config.as_ref(), base_ref, &default_ref))
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
        if creator.tenant.0 != tenant || creator.region.0 != region {
            return Err(DurableError::Forbidden(
                "repository creation scope does not match the verified creator".into(),
            ));
        }
        let loc = Self::loc(tenant, region, slug);
        let owner = serde_json::to_string(&(
            creator.tenant.0.as_str(),
            creator.region.0.as_str(),
            creator.principal_id.0.as_str(),
        ))
        .map_err(|error| DurableError::Git(format!("encode repository creation owner: {error}")))?;
        let claim = match self.store.claim_repo_creation(&loc, &owner)? {
            RepoCreationClaim::Existing(_) => return Ok(false),
            RepoCreationClaim::Acquired(claim) => claim,
        };
        claim.initialize()?;
        self.bootstrap.grant_creator(creator, &loc).map_err(|e| {
            DurableError::Git(format!(
                "creator bootstrap grant refused; the owner-bound repository claim remains retryable: {e}"
            ))
        })?;
        claim.complete()?;
        Ok(true)
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

    pub fn list_repositories(
        &self,
        principal: &Principal,
        limit: u32,
        cursor: Option<String>,
    ) -> Result<Value, EdgeError> {
        let limit = usize::try_from(limit)
            .ok()
            .filter(|limit| (1..=MAX_PAGE_LIMIT).contains(limit))
            .ok_or_else(|| {
                EdgeError::BadRequest(format!(
                    "repository summary limit must be within 1..={MAX_PAGE_LIMIT}"
                ))
            })?;
        let tenant = &principal.tenant.0;
        let region = &principal.region.0;
        let cursor = cursor
            .as_deref()
            .map(|value| parse_repo_summary_cursor(value, tenant, region))
            .transpose()?;
        let (rows, next_slug) = self
            .list_repo_summaries_visible(
                tenant,
                region,
                principal,
                cursor.as_ref().map(RepoListCursor::last_slug),
                limit,
            )
            .map_err(map_repo_summary_durable_err)?;
        let items = rows.iter().map(RepoListRow::to_json).collect::<Vec<_>>();
        let next_cursor = next_slug
            .map(|last_slug| {
                RepoListCursor::new(repo_summary_cursor_scope(tenant, region), last_slug)
                    .map(|cursor| cursor.encode())
                    .map_err(|error| {
                        EdgeError::Internal(format!(
                            "mint repository summary cursor failed: {error}"
                        ))
                    })
            })
            .transpose()?;
        Ok(page_envelope(json!(items), next_cursor, limit))
    }

    fn clone_url(&self, tenant: &str, region: &str, slug: &str) -> String {
        let wire_slug = slug.replace('/', "%2F");
        format!("{}/{tenant}/{region}/{wire_slug}.git", self.clone_base)
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
        let repo_ref = format!("myelin://{tenant}/git/repo/{slug}");
        let clone_url = self.clone_url(tenant, region, slug);
        let refs = repo.refs_summary()?;
        let default_branch = refs.default_branch.clone();
        let counts = json!({ "branches": refs.branch_count, "tags": refs.tag_count });
        if refs.default_tip.is_none() {
            return Ok(json!({
                "state": "empty",
                "slug": full_slug,
                "ref": repo_ref,
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
            "ref": repo_ref,
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
            search_repo_code(&repo, &slug, &branch_ref, query, &mut hits, &mut budget)?;
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

    pub fn search_code(
        &self,
        principal: &Principal,
        query: &str,
        repo: Option<&str>,
    ) -> Result<Value, EdgeError> {
        if !valid_code_search_query(query) {
            return Err(EdgeError::BadRequest("code search query is invalid".into()));
        }
        if repo.is_some_and(|repo| !valid_code_search_repo(repo)) {
            return Err(EdgeError::BadRequest(
                "code search repository filter is invalid".into(),
            ));
        }
        self.code_search_json(
            &principal.tenant.0,
            &principal.region.0,
            principal,
            query,
            repo,
        )
        .map_err(map_durable_err)
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

    pub fn read_file(
        &self,
        principal: &Principal,
        repo: &str,
        gitref: &str,
        path: &str,
    ) -> Result<Value, EdgeError> {
        if !valid_code_search_repo(repo) {
            return Err(EdgeError::BadRequest("repository slug is invalid".into()));
        }
        if gitref.is_empty() || gitref.len() > 1024 || gitref.chars().any(char::is_control) {
            return Err(EdgeError::BadRequest("Git ref is invalid".into()));
        }
        if !valid_anchor_path(path) {
            return Err(EdgeError::BadRequest("file path is invalid".into()));
        }
        let location = Self::loc(&principal.tenant.0, &principal.region.0, repo);
        if !self
            .repo_authz
            .authorize_repo_permission(principal, &location, RepoPermission::Pull)
        {
            return Err(EdgeError::NotFound("repository not found".into()));
        }
        let mut file = self
            .blob_json(&principal.tenant.0, &principal.region.0, repo, gitref, path)
            .map_err(map_durable_err)?;
        if let Some(object) = file.as_object_mut() {
            let may_edit = self.repo_authz.authorize_repo_permission(
                principal,
                &location,
                RepoPermission::Push,
            );
            object.insert("viewer_may_edit".into(), Value::Bool(may_edit));
        }
        Ok(file)
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
        start_ref: Option<&str>,
    ) -> Result<WebEditOutcome, DurableError> {
        self.commit_file(FileCommit {
            target,
            gitref,
            path,
            expected_base,
            contents,
            start_ref,
            message: "web edit",
            actor_is_agent: false,
        })
    }

    pub(crate) fn write_file_with_operation(
        &self,
        request: AgentFileWrite<'_>,
    ) -> Result<String, DurableError> {
        let request_hash = file_write_request_hash(&request);
        let AgentFileWrite {
            target,
            gitref,
            path,
            expected_base,
            contents,
            start_ref,
            operation_id,
        } = request;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal: actor,
        } = target;
        if actor.tenant.0 != tenant || actor.region.0 != region {
            return Err(DurableError::Git(
                "file-write actor must share the requested tenant and region".into(),
            ));
        }
        if contents.len() > AGENT_FILE_WRITE_MAX_BYTES {
            return Err(DurableError::Git(format!(
                "file contents exceed the {AGENT_FILE_WRITE_MAX_BYTES}-byte agent write limit"
            )));
        }
        let full_ref = branch_ref(gitref);
        let operation_trailer = format!("Myelin-Operation: {}", operation_id.digest());
        let request_trailer = format!("Myelin-Request: {request_hash}");
        let loc = Self::loc(tenant, region, slug);
        let repo = Arc::new(self.store.open_repo(&loc)?);
        if let Some(previous) = repo.find_reflog_commit_by_trailer(&full_ref, &operation_trailer)? {
            return replayed_file_write(previous.oid.0, &previous.message, &request_trailer);
        }

        let message = format!("agent file edit\n\n{operation_trailer}\n{request_trailer}");
        let outcome = self.commit_file_in_repo(
            repo.clone(),
            FileCommit {
                target,
                gitref,
                path,
                expected_base,
                contents,
                start_ref,
                message: &message,
                actor_is_agent: true,
            },
        )?;
        match outcome {
            WebEditOutcome::Committed { new_oid } => Ok(new_oid),
            WebEditOutcome::StaleBase { .. } => {
                if let Some(previous) =
                    repo.find_reflog_commit_by_trailer(&full_ref, &operation_trailer)?
                {
                    replayed_file_write(previous.oid.0, &previous.message, &request_trailer)
                } else {
                    Err(DurableError::Git(
                        "the file changed since it was read; nothing was overwritten".into(),
                    ))
                }
            }
            WebEditOutcome::Denied => Err(DurableError::Forbidden(
                "no write permission for this ref".into(),
            )),
        }
    }

    fn commit_file(&self, request: FileCommit<'_>) -> Result<WebEditOutcome, DurableError> {
        let target = request.target;
        let loc = Self::loc(target.tenant, target.region, target.slug);
        let repo = Arc::new(self.store.open_repo(&loc)?);
        self.commit_file_in_repo(repo, request)
    }

    fn commit_file_in_repo(
        &self,
        repo: Arc<DurableGitRepo>,
        request: FileCommit<'_>,
    ) -> Result<WebEditOutcome, DurableError> {
        let FileCommit {
            target,
            gitref,
            path,
            expected_base,
            contents,
            start_ref,
            message,
            actor_is_agent,
        } = request;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = target;
        let full = branch_ref(gitref);
        let start_full = start_ref.map(|value| {
            if value.starts_with("refs/heads/") {
                value.to_string()
            } else {
                format!("refs/heads/{value}")
            }
        });
        let prior_target = repo.read_ref(&full)?;
        let parent = match (&prior_target, start_full.as_deref()) {
            (Some(target), _) => Some(target.clone()),
            (None, Some(source)) => Some(repo.read_ref(source)?.ok_or_else(|| {
                DurableError::NotFound(format!("branch start ref `{source}` does not exist"))
            })?),
            (None, None) => None,
        };
        let current_base = match &parent {
            Some(parent) => repo
                .blob_oid_at_path(parent.as_str(), path)?
                .map(|oid| oid.0)
                .unwrap_or_default(),
            None => String::new(),
        };

        let probe = WebEditOutcome::evaluate(expected_base, &current_base, "pending", true);
        if let WebEditOutcome::StaleBase { current_oid } = probe {
            return Ok(WebEditOutcome::StaleBase { current_oid });
        }
        if let WebEditOutcome::Denied = probe {
            return Ok(WebEditOutcome::Denied);
        }

        let psn = Self::pseudonym(tenant, principal);
        let quarantine = FileEditQuarantine::new(&repo)?;
        let prepared = quarantine.repo.prepare_file_commit(
            parent.as_ref(),
            path,
            contents.as_bytes(),
            message,
            &psn,
            &psn,
        )?;
        let mut objects = Vec::with_capacity(prepared.trees.len() + 2);
        objects.push((
            prepared.blob.0.clone(),
            "blob".to_string(),
            quarantine
                .repo
                .read_object_bounded(&prepared.blob, AGENT_FILE_WRITE_MAX_BYTES)?,
        ));
        for tree in &prepared.trees {
            objects.push((
                tree.0.clone(),
                "tree".to_string(),
                quarantine
                    .repo
                    .read_object_bounded(tree, TREE_OBJECT_MAX_BYTES)?,
            ));
        }
        objects.push((
            prepared.commit.0.clone(),
            "commit".to_string(),
            quarantine
                .repo
                .read_object_bounded(&prepared.commit, 64 * 1024)?,
        ));
        let quarantine_objects = objects
            .iter()
            .map(|(oid, _, bytes)| QuarantineObject {
                oid: PushOid::new(oid.clone()),
                bytes: bytes.clone(),
            })
            .collect();

        let loc = Self::loc(tenant, region, slug);
        let ref_name = RefName::new(full.clone());
        self.admit_file_ref_update(
            &repo,
            &loc,
            principal,
            actor_is_agent,
            &ref_name,
            &prepared.commit,
        )?;

        let ref_store = self.open_durable_refstore(repo.clone(), slug, tenant, region, principal);
        let expected_old = prior_target
            .map(|p| PushOid::new(p.0))
            .unwrap_or_else(PushOid::zero);
        let push = PushSession {
            updates: vec![ProposedRefUpdate {
                ref_name,
                expected_old,
                new_oid: PushOid::new(prepared.commit.0.clone()),
                forced: false,
                commit_oids: vec![PushOid::new(prepared.commit.0.clone())],
            }],
            quarantine: quarantine_objects,
            pusher: Pusher::direct(psn, actor_is_agent),
        };
        let migration = ObjectPromotion {
            repo: &repo,
            objects: &objects,
        };
        match ref_store
            .receive(&push, &migration, CrashPoint::None)
            .map_err(|e| DurableError::Git(format!("ref-CAS: {e:?}")))?
        {
            PushOutcome::Accepted { .. } => {
                let _ = repo.heal_head_symref();
                Ok(WebEditOutcome::Committed {
                    new_oid: prepared.commit.0,
                })
            }
            PushOutcome::Rejected(RejectReason::NonFastForward { .. }) => {
                Ok(WebEditOutcome::StaleBase {
                    current_oid: current_base,
                })
            }
            PushOutcome::Rejected(_) => Ok(WebEditOutcome::Denied),
            PushOutcome::Crashed(_) => Err(DurableError::Git("web-edit ref-CAS crashed".into())),
        }
    }

    fn admit_file_ref_update(
        &self,
        repo: &DurableGitRepo,
        loc: &RepoLoc,
        principal: &Principal,
        actor_is_agent: bool,
        ref_name: &RefName,
        new_commit: &CoreOid,
    ) -> Result<(), DurableError> {
        let protection = self.prs.get_protection(loc)?;
        let is_configured = protection
            .as_ref()
            .and_then(|config| config.resolve(&ref_name.0))
            .is_some();
        let default_ref = RefName::new(repo.default_branch_ref()?);
        if !is_configured && !ref_name.has_baseline_protection(&default_ref) {
            return Ok(());
        }

        // Agent file edits are proposals. The human delegator's repository administration grant
        // must not silently turn an ungated `git.write_file` call into a protected direct push.
        let pusher_has_protected_push = !actor_is_agent
            && self.repo_authz.authorize_repo_permission(
                principal,
                loc,
                RepoPermission::ProtectedPush,
            );
        let (green, fork_unendorsed, endorsed) =
            self.check_facts_for_head(loc, new_commit.as_str(), principal);
        evaluate_protected_ref_push(
            ref_name,
            false,
            false,
            pusher_has_protected_push,
            &effective_ruleset(protection.as_ref(), &ref_name.0, &default_ref),
            &GitOid(new_commit.0.clone()),
            &green,
            &fork_unendorsed,
            &endorsed,
        )
        .map_err(|reason| {
            DurableError::Conflict(format!(
                "branch protection refused a direct file write to `{}`: {reason:?}; write to an \
                 unprotected working branch and open a pull request",
                ref_name.0
            ))
        })
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
        self.open_pr_for_actor_with_operation(
            RepoActorContext::new(tenant, region, slug, principal),
            body,
            principal,
            operation_id,
        )
    }

    /// Opens a PR attributed to `actor`, using `authorization_basis` only to read its source.
    ///
    /// Agent effects deliberately keep these principals separate: delegation conveys authority,
    /// but never changes who performed the mutation.
    pub(crate) fn open_pr_for_actor_with_operation(
        &self,
        target: RepoActorContext<'_>,
        body: &Value,
        authorization_basis: &Principal,
        operation_id: &PrOperationId,
    ) -> Result<PrRecord, DurableError> {
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal: actor,
        } = target;
        if actor.tenant.0 != tenant
            || actor.region.0 != region
            || authorization_basis.tenant != actor.tenant
            || authorization_basis.region != actor.region
        {
            return Err(DurableError::Git(
                "open-PR actor and authorization basis must share the requested tenant and region"
                    .into(),
            ));
        }
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
        let head_loc = Self::loc(tenant, region, head_repo_slug);
        if !self.repo_authz.authorize_repo_permission(
            authorization_basis,
            &head_loc,
            RepoPermission::Pull,
        ) {
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
        if body_md
            .as_ref()
            .is_some_and(|body| body.len() > myelin_git::typed_edges::MAX_CLOSES_MESSAGE_BYTES)
        {
            return Err(DurableError::Git(format!(
                "open-PR `body_md` exceeds {} bytes",
                myelin_git::typed_edges::MAX_CLOSES_MESSAGE_BYTES
            )));
        }
        let number = if self.pg_prs.is_some() {
            0
        } else {
            self.next_pr_number(&loc)?
        };
        let pr = PullRequest::open(
            number,
            base_ref,
            head_ref,
            Self::pseudonym(tenant, actor),
            body.get("draft").and_then(Value::as_bool).unwrap_or(false),
        );
        let mut rec = PrRecord::open(&pr, head_oid);
        rec.head_repo_slug = head_repo_slug.to_string();
        rec.title = title;
        rec.body_md = body_md;
        rec.author_is_agent = Self::is_agent(actor);
        if let Some(reviewers) = body.get("reviewers") {
            let reviewers = reviewers
                .as_array()
                .ok_or_else(|| DurableError::Git("open-PR `reviewers` must be an array".into()))?;
            if reviewers.len() > 100 {
                return Err(DurableError::Git(
                    "open-PR `reviewers` exceeds 100 entries".into(),
                ));
            }
            let author = actor.principal_id.0.as_str();
            let mut requested = std::collections::BTreeSet::new();
            for reviewer in reviewers {
                let reviewer = reviewer.as_str().ok_or_else(|| {
                    DurableError::Git("open-PR reviewer ids must be strings".into())
                })?;
                if reviewer.trim() != reviewer
                    || reviewer.is_empty()
                    || reviewer.len() > 255
                    || reviewer.chars().any(char::is_control)
                {
                    return Err(DurableError::Git(
                        "open-PR reviewer ids must be 1-255 clean bytes without surrounding whitespace"
                            .into(),
                    ));
                }
                if reviewer != author {
                    requested.insert(format!("{reviewer}@{tenant}.noreply"));
                }
            }
            rec.reviews.extend(
                requested
                    .into_iter()
                    .map(|reviewer_pseudonym| ReviewRecord {
                        reviewer_pseudonym,
                        state: ReviewState::Requested,
                        is_agent: false,
                    }),
            );
        }
        let now = now_unix();
        rec.created_at = Some(now);
        rec.updated_at = Some(now);
        self.pr_open(&loc, rec, operation_id, actor)
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
        let request = if body.is_null() {
            BranchProtectionRequest {
                rulesets: Vec::new(),
            }
        } else {
            serde_json::from_value::<BranchProtectionRequest>(body.clone())
                .map_err(|error| invalid_request("branch-protection", error))?
        };
        if request.rulesets.len() > MAX_BRANCH_PROTECTION_RULESETS {
            return Err(DurableError::InvalidInput(format!(
                "branch-protection may contain at most {MAX_BRANCH_PROTECTION_RULESETS} rulesets"
            )));
        }
        let mut patterns = std::collections::BTreeSet::new();
        let mut rulesets = Vec::with_capacity(request.rulesets.len());
        for rule in request.rulesets {
            validate_ref_pattern(&rule.ref_pattern)?;
            validate_check_contexts(
                "branch-protection `required_contexts`",
                &rule.required_contexts,
            )?;
            if !patterns.insert(rule.ref_pattern.clone()) {
                return Err(DurableError::InvalidInput(format!(
                    "branch-protection contains duplicate ref pattern `{}`",
                    rule.ref_pattern
                )));
            }
            rulesets.push(BranchProtectionRuleset {
                ref_pattern: rule.ref_pattern,
                required_contexts: rule.required_contexts,
                required_approvals: rule.required_approvals,
                require_codeowner_review: rule.require_codeowner_review,
                require_conversation_resolution: rule.require_conversation_resolution,
                allow_force_push: rule.allow_force_push,
            });
        }
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
            RepoActorContext::new(tenant, region, slug, principal).for_pr(number),
            body,
            &operation_id,
        )
    }

    pub fn report_checks_with_operation(
        &self,
        target: PrActorContext<'_>,
        body: &Value,
        operation_id: &PrOperationId,
    ) -> Result<PrRecord, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
        if !matches!(principal.kind, PrincipalKind::Service) {
            return Err(DurableError::Forbidden(format!(
                "git.checks.report is a CI-producer capability: principal `{}` (kind {:?}) is not a CI \
                 service producer - a human/agent writer cannot attest CI check facts on a PR",
                principal.principal_id.0, principal.kind
            )));
        }
        let loc = Self::loc(tenant, region, slug);
        let request = serde_json::from_value::<CheckReportRequest>(body.clone())
            .map_err(|error| invalid_request("check report", error))?;
        if !request.has_update() {
            return Err(DurableError::InvalidInput(
                "check report body must contain at least one check projection field".into(),
            ));
        }
        if let Some(contexts) = request.green_contexts.as_deref() {
            validate_check_contexts("check report `green_contexts`", contexts)?;
        }
        if let Some(contexts) = request.fork_unendorsed_contexts.as_deref() {
            validate_check_contexts("check report `fork_unendorsed_contexts`", contexts)?;
        }
        if let (Some(green), Some(unendorsed)) = (
            request.green_contexts.as_deref(),
            request.fork_unendorsed_contexts.as_deref(),
        ) {
            if let Some(context) = green.iter().find(|context| unendorsed.contains(context)) {
                return Err(DurableError::InvalidInput(format!(
                    "check context `{context}` cannot be both green and unendorsed"
                )));
            }
        }
        self.pr_mutate(
            &loc,
            number,
            PrMutation::ReportChecks {
                green_contexts: request.green_contexts,
                fork_unendorsed_contexts: request.fork_unendorsed_contexts,
                codeowner_review_satisfied: request.codeowner_review_satisfied,
                outstanding_conversations: request.outstanding_conversations,
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
            RepoActorContext::new(tenant, region, slug, principal).for_pr(number),
            verdict,
            &operation_id,
        )
    }

    pub fn submit_review_with_operation(
        &self,
        target: PrActorContext<'_>,
        verdict: &str,
        operation_id: &PrOperationId,
    ) -> Result<PrRecord, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
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
            RepoActorContext::new(tenant, region, slug, principal).for_pr(number),
            body,
            &operation_id,
        )
    }

    pub fn endorse_fork_ci_with_operation(
        &self,
        target: PrActorContext<'_>,
        body: &Value,
        operation_id: &PrOperationId,
    ) -> Result<PrRecord, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal,
        } = repo;
        let loc = Self::loc(tenant, region, slug);
        let rec = self
            .pr_get(&loc, number, principal)?
            .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))?;
        let request = if body.is_null() {
            EndorseForkCiRequest::default()
        } else {
            serde_json::from_value::<EndorseForkCiRequest>(body.clone())
                .map_err(|error| invalid_request("fork-CI endorsement", error))?
        };
        let to_endorse = match request.contexts {
            Some(contexts) => {
                validate_check_contexts("fork-CI endorsement `contexts`", &contexts)?;
                contexts
            }
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
        self.merge_for_actor_with_operation(
            RepoActorContext::new(tenant, region, slug, principal).for_pr(number),
            principal,
            operation_id,
            PushProvenance::direct(Self::is_agent(principal)),
        )
    }

    pub(crate) fn merge_human_approved_agent_with_operation(
        &self,
        target: PrActorContext<'_>,
        repo_reader: &Principal,
        operation_id: &PrOperationId,
    ) -> Result<MergeAttempt, DurableError> {
        if !Self::is_agent(target.repo.principal) {
            return Err(DurableError::Git(
                "human-approved agent merge requires an agent actor".into(),
            ));
        }
        self.merge_for_actor_with_operation(
            target,
            repo_reader,
            operation_id,
            PushProvenance::HumanApprovedAgent,
        )
    }

    /// Merges as `actor` while resolving source-repository visibility through an independently
    /// validated authority principal. The provenance controls protected-ref admission without
    /// replacing the actor that Git attributes the mutation to.
    fn merge_for_actor_with_operation(
        &self,
        target: PrActorContext<'_>,
        repo_reader: &Principal,
        operation_id: &PrOperationId,
        ref_update_provenance: PushProvenance,
    ) -> Result<MergeAttempt, DurableError> {
        let PrActorContext { repo, number } = target;
        let RepoActorContext {
            tenant,
            region,
            slug,
            principal: actor,
        } = repo;
        if actor.tenant != repo_reader.tenant || actor.region != repo_reader.region {
            return Err(DurableError::Git(
                "merge actor and repository authority belong to different scopes".into(),
            ));
        }
        let loc = Self::loc(tenant, region, slug);
        let repo = Arc::new(self.store.open_repo(&loc)?);
        let ref_store = self.open_durable_refstore(repo.clone(), slug, tenant, region, actor);
        if let Some(store) = &self.pg_prs {
            let scope = Self::verified_pr_scope(actor, &loc)?;
            if let Some(intent) = store.pending_merge_intent(&scope, slug, number)? {
                if intent.operation_id != operation_id.digest()
                    || intent.actor_subject_id != actor.principal_id.0.trim()
                    || intent.ref_update_provenance != ref_update_provenance
                {
                    return Err(DurableError::Git(
                        "a different merge operation is already pending".into(),
                    ));
                }
                return store
                    .recover_pending_merge_target(
                        &scope, slug, number, actor, &loc, &repo, &ref_store,
                    )?
                    .ok_or_else(|| {
                        DurableError::Io("pending merge disappeared during recovery".into())
                    });
            }
            let rec = self
                .pr_get(&loc, number, actor)?
                .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))?;
            let source_loc = Self::loc(&actor.tenant.0, &actor.region.0, &rec.head_repo_slug);
            if !self.repo_authz.authorize_repo_permission(
                repo_reader,
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
                actor,
                &self.prs,
                &loc,
                &repo,
                &source_repo,
                &ref_store,
                &Self::pseudonym(tenant, actor),
                ref_update_provenance,
                self.checks.is_some(),
            );
        }
        merge_pr(
            &self.prs,
            &loc,
            number,
            &ref_store,
            &repo,
            &Self::pseudonym(tenant, actor),
            ref_update_provenance,
        )
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
        let default_ref = match repo.default_branch_ref() {
            Ok(default_ref) => RefName::new(default_ref),
            Err(error) => {
                let _ = std::fs::remove_dir_all(&qdir);
                return Ok(report_status(
                    "ok",
                    &all_ng(
                        &cmds,
                        &format!(
                            "rejected (default-branch policy unreadable - fail-closed): {error}"
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
            let is_protected = configured || u.ref_name.has_baseline_protection(&default_ref);
            if !is_protected {
                continue;
            }
            let ruleset = effective_ruleset(protection.as_ref(), ref_str, &default_ref);
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
            pusher: Pusher::direct(
                Self::pseudonym(tenant, principal),
                Self::is_agent(principal),
            ),
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

    fn pr_json(tenant: &str, repo: &str, rec: &PrRecord) -> Value {
        json!({
            "number": rec.number,
            "ref": format!("myelin://{tenant}/git/pr/{repo}:{}", rec.number),
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

#[cfg(test)]
mod agent_file_write_tests {
    use super::*;
    use myelin_identity::RuntimeRef;
    use myelin_tenancy::TenantId;
    use std::time::{SystemTime, UNIX_EPOCH};

    const TENANT: &str = "acme";
    const REGION: &str = "fr-par";
    const REPO: &str = "product";
    const PATH: &str = "src/release.ts";

    #[test]
    fn a_retry_finds_its_commit_after_restart_and_later_branch_work() {
        let root = temp_root();
        let backend = DurableGitBackend::rooted_inmem_for_test(&root);
        backend.create_repo(TENANT, REGION, REPO).unwrap();
        let repo = backend.store.open_repo(&repo_loc()).unwrap();
        let (main_commit, base_blob, _) = repo
            .build_file_commit(
                "refs/heads/main",
                PATH,
                b"export const ready = false;\n",
                "seed",
                "human:founder@acme.noreply",
                "human:founder@acme.noreply",
            )
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&main_commit),
            "seed main",
            "human:founder@acme.noreply",
        )
        .unwrap();

        let actor = agent();
        let protected_operation = PrOperationId::parse("write-main").unwrap();
        let protected = backend
            .write_file_with_operation(AgentFileWrite {
                target: RepoActorContext::new(TENANT, REGION, REPO, &actor),
                gitref: "main",
                path: PATH,
                expected_base: &base_blob.0,
                contents: "export const ready = true;\n",
                start_ref: None,
                operation_id: &protected_operation,
            })
            .unwrap_err();
        assert!(matches!(protected, DurableError::Conflict(_)));
        assert!(protected
            .to_string()
            .contains("write to an unprotected working branch and open a pull request"));
        assert_eq!(
            repo.read_ref("refs/heads/main").unwrap(),
            Some(main_commit),
            "an agent cannot bypass protected-branch review"
        );

        let secret_operation = PrOperationId::parse("write-secret").unwrap();
        let secret = ["AK", "IAIOSFODNN7EXAMPLE"].concat();
        let secret_blob_oid = FileEditQuarantine::new(&repo)
            .unwrap()
            .repo
            .write_blob(secret.as_bytes())
            .unwrap();
        let rejected_secret = backend
            .write_file_with_operation(AgentFileWrite {
                target: RepoActorContext::new(TENANT, REGION, REPO, &actor),
                gitref: "agent/unsafe-fix",
                path: PATH,
                expected_base: &base_blob.0,
                contents: &secret,
                start_ref: Some("main"),
                operation_id: &secret_operation,
            })
            .unwrap_err();
        assert!(matches!(rejected_secret, DurableError::Forbidden(_)));
        assert_eq!(
            repo.read_ref("refs/heads/agent/unsafe-fix").unwrap(),
            None,
            "the shared receive policy rejects secrets before exposing a branch"
        );
        assert!(
            !repo.has_object(&secret_blob_oid)
                && repo
                    .read_object_bounded(&secret_blob_oid, secret.len())
                    .is_err(),
            "rejected content never enters the durable object database or remains readable by OID"
        );

        backend
            .set_branch_protection(
                TENANT,
                REGION,
                REPO,
                &serde_json::json!({
                    "rulesets": [{
                        "ref_pattern": "refs/heads/develop",
                        "required_contexts": [],
                        "required_approvals": 1,
                        "require_codeowner_review": false,
                        "require_conversation_resolution": false,
                        "allow_force_push": false,
                    }],
                }),
            )
            .unwrap();
        let configured_protection = backend
            .write_file_with_operation(AgentFileWrite {
                target: RepoActorContext::new(TENANT, REGION, REPO, &actor),
                gitref: "develop",
                path: PATH,
                expected_base: &base_blob.0,
                contents: "export const ready = true;\n",
                start_ref: Some("main"),
                operation_id: &PrOperationId::parse("write-develop").unwrap(),
            })
            .unwrap_err();
        assert!(configured_protection
            .to_string()
            .contains("branch protection refused"));
        assert_eq!(
            repo.read_ref("refs/heads/develop").unwrap(),
            None,
            "configured protection is identical at the file-edit and Git-wire doors"
        );

        let first_operation = PrOperationId::parse("write-ready").unwrap();
        let first_commit = backend
            .write_file_with_operation(AgentFileWrite {
                target: RepoActorContext::new(TENANT, REGION, REPO, &actor),
                gitref: "agent/release-fix",
                path: PATH,
                expected_base: &base_blob.0,
                contents: "export const ready = true;\n",
                start_ref: Some("main"),
                operation_id: &first_operation,
            })
            .unwrap();
        let first_blob = repo
            .blob_oid_at_path("refs/heads/agent/release-fix", PATH)
            .unwrap()
            .unwrap();

        let later_operation = PrOperationId::parse("write-note").unwrap();
        let later_commit = backend
            .write_file_with_operation(AgentFileWrite {
                target: RepoActorContext::new(TENANT, REGION, REPO, &actor),
                gitref: "agent/release-fix",
                path: PATH,
                expected_base: &first_blob.0,
                contents: "export const ready = true; // verified\n",
                start_ref: None,
                operation_id: &later_operation,
            })
            .unwrap();
        assert_ne!(later_commit, first_commit);
        drop(backend);

        let restarted = DurableGitBackend::rooted_inmem_for_test(&root);
        let replayed = restarted
            .write_file_with_operation(AgentFileWrite {
                target: RepoActorContext::new(TENANT, REGION, REPO, &actor),
                gitref: "agent/release-fix",
                path: PATH,
                expected_base: &base_blob.0,
                contents: "export const ready = true;\n",
                start_ref: Some("main"),
                operation_id: &first_operation,
            })
            .unwrap();
        assert_eq!(replayed, first_commit);
        assert_eq!(
            repo.read_ref("refs/heads/agent/release-fix")
                .unwrap()
                .unwrap()
                .0,
            later_commit,
            "replaying an older write reports its commit without rewinding the branch"
        );

        let conflict = restarted
            .write_file_with_operation(AgentFileWrite {
                target: RepoActorContext::new(TENANT, REGION, REPO, &actor),
                gitref: "agent/release-fix",
                path: PATH,
                expected_base: &base_blob.0,
                contents: "export const ready = 'different';\n",
                start_ref: Some("main"),
                operation_id: &first_operation,
            })
            .unwrap_err();
        assert!(conflict
            .to_string()
            .contains("idempotency key is already bound to a different file write"));

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn only_the_human_approved_agent_merge_crosses_the_protected_ref_seam() {
        let root = temp_root();
        let backend = DurableGitBackend::rooted_inmem_for_test(&root);
        backend.create_repo(TENANT, REGION, REPO).unwrap();
        let repo = backend.store.open_repo(&repo_loc()).unwrap();
        let (main, _, _) = repo
            .build_file_commit(
                "refs/heads/main",
                "README.md",
                b"# Product\n",
                "seed",
                "human:founder@acme.noreply",
                "human:founder@acme.noreply",
            )
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&main),
            "seed main",
            "human:founder@acme.noreply",
        )
        .unwrap();
        let (head, _, _) = repo
            .build_file_commit(
                "refs/heads/main",
                "approved.txt",
                b"A human approved this exact effect.\n",
                "prepare change",
                "agent:release-helper@acme.noreply",
                "agent:release-helper@acme.noreply",
            )
            .unwrap();
        backend
            .set_branch_protection(
                TENANT,
                REGION,
                REPO,
                &serde_json::json!({
                    "rulesets": [{
                        "ref_pattern": "refs/heads/main",
                        "required_contexts": [],
                        "required_approvals": 0,
                        "require_codeowner_review": false,
                        "require_conversation_resolution": false,
                        "allow_force_push": false
                    }]
                }),
            )
            .unwrap();
        let actor = agent();
        let opened = backend
            .open_pr_with_operation(
                TENANT,
                REGION,
                REPO,
                &serde_json::json!({
                    "title": "Ship the approved change",
                    "base_ref": "refs/heads/main",
                    "head_ref": "refs/heads/agent/release-helper",
                    "head_oid": head.0.clone()
                }),
                &actor,
                &PrOperationId::parse("open-agent-pr").unwrap(),
            )
            .unwrap();

        assert!(matches!(
            backend
                .merge_with_operation(
                    TENANT,
                    REGION,
                    REPO,
                    opened.number,
                    &actor,
                    &PrOperationId::parse("direct-agent-merge").unwrap(),
                )
                .unwrap(),
            MergeAttempt::RefRefused(RejectReason::AgentNeedsHuman { .. })
        ));
        assert_eq!(repo.read_ref("refs/heads/main").unwrap(), Some(main));

        let founder = Principal::new(
            TenantId(TENANT.into()),
            Region(REGION.into()),
            PrincipalId("human:founder".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        assert!(matches!(
            backend
                .merge_human_approved_agent_with_operation(
                    RepoActorContext::new(TENANT, REGION, REPO, &actor).for_pr(opened.number),
                    &founder,
                    &PrOperationId::parse("approved-agent-merge").unwrap(),
                )
                .unwrap(),
            MergeAttempt::Merged { .. }
        ));
        assert_eq!(repo.read_ref("refs/heads/main").unwrap(), Some(head));
        std::fs::remove_dir_all(root).ok();
    }

    fn agent() -> Principal {
        Principal::new(
            TenantId(TENANT.into()),
            Region(REGION.into()),
            PrincipalId("agent:release-helper".into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("runtime:local".into()),
                on_behalf_of: Some(PrincipalId("human:founder".into())),
            },
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn repo_loc() -> RepoLoc {
        RepoLoc::new(TENANT, REGION, REPO)
    }

    fn temp_root() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("myelin-agent-file-write-{nonce}"))
    }
}

#[cfg(test)]
mod durable_error_mapping_tests {
    use super::*;

    #[test]
    fn invalid_file_edit_paths_are_public_bad_requests() {
        let error = map_durable_err(DurableError::InvalidInput(
            "file edit path contains a reserved Git administrative component".into(),
        ));

        assert_eq!(error.status(), 400);
        assert_eq!(error.code(), "bad_request");
        assert_eq!(
            error.client_message(),
            "file edit path contains a reserved Git administrative component"
        );
    }

    #[test]
    fn an_owner_bound_creation_claim_is_a_public_conflict() {
        let error = map_durable_err(DurableError::Conflict(
            "repository `core` is already being created by another principal".into(),
        ));

        assert_eq!(error.status(), 409);
        assert_eq!(error.code(), "conflict");
        assert_eq!(
            error.client_message(),
            "repository `core` is already being created by another principal"
        );
    }

    #[test]
    fn reused_pr_operation_id_maps_to_a_public_conflict() {
        let error = map_durable_err(DurableError::Git(
            "PR operation id conflicts with durable state".into(),
        ));

        assert_eq!(error.status(), 409);
        assert_eq!(error.code(), "conflict");
        assert_eq!(
            error.client_message(),
            "idempotency key is already bound to a different pull request operation"
        );
    }
}

mod blame;
mod check_projection;
mod http;
mod mutation_support;
mod pr_queries;
mod recovery;
mod repo_summary;
mod repository_views;
mod review_threads;
pub use check_projection::GitDatabaseProviders;
pub use http::register_git_durable;
use http::{cross_pr_before_key, mint_pr_list_cursors, BlobViewOptions, RawResponseOptions};
pub use recovery::{recover_placed_git_at_boot, GitBootRecoveryReport, GitCellBootRecoveryReport};
