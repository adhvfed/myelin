//! # Git wired through the product edge — the DURABLE front door (GT-003 / E1.2)
//!
//! MR-015 wired git's routes through the edge but its write handlers returned `{ durable: false }`
//! (honest stubs over the in-memory [`crate::GitEdgeState`]). GT-003 replaces the in-memory write source
//! at the LIVE front door with the **real on-disk durable backend** (GT-001):
//!
//! - **create-repo** → [`myelin_git::durable::DurableGitStore::create_repo`] (a real bare repo persists);
//! - **web-edit commit / ref-update** → the durable per-ref CAS over
//!   [`myelin_git::receive_pack::RefStore::open_durable`] (the SAME one-transaction ref-CAS + outbox
//!   emit the push path uses — `durable: true`; a stale base is still the honest `409`);
//! - **open-PR / review / merge / endorse** → the durable [`myelin_git::pr_store::DurablePrStore`] +
//!   the reused [`myelin_git::lifecycle`] / [`myelin_git::merge_gate`] / [`myelin_git::fork_gate`]
//!   logic — a merge advances the target ref via the durable CAS ONLY after the merge-gate + fork-trust
//!   gate admit (never a bypass);
//! - **reads (repo list / blob view / PR overview / checks)** reflect the DURABLE on-disk state (the real
//!   repo + the durable PR record), not a seeded in-memory ViewModel.
//!
//! The gateway still owns auth / tenant-from-token / IDOR / error / pagination (unchanged); every write
//! is under `ctx.scope` (the verified token's tenant + region) and the validated, traversal-safe resolver
//! (a repo under tenant A is never reachable via tenant B's locator). The reconciler
//! ([`myelin_git::reconcile`]) heals the apply-after-outbox-commit window before this front door serves.
//!
//! `myelin-git` PG-home for PR/review rows (the MR-022 provider) is the named **GT-003b** follow-on; the
//! durable medium here is on-disk repo metadata (path-isolated via the same resolver — GT-003 §2 option).

use crate::catalogue::{page_envelope, Handler, HandlerCtx, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::git_edge::{map_method, num_param, param, reroot, tenant_of};
use crate::repo_authz::{DenyAllRepos, RepoAuthorizer, RepoPermission};
// `AllowAllRepos` is now constructed ONLY by the test-support `rooted_inmem_for_test` helper (R2.6:
// the prod default is fail-closed `DenyAllRepos`), so its import is test-support-gated too.
#[cfg(any(test, feature = "test-support"))]
use crate::repo_authz::AllowAllRepos;
use crate::repo_authz_live::{NoRepoBootstrap, RepoBootstrapGrants};
use crate::request::{EdgeRequest, EdgeResponse};
use myelin_events::{Actor, EmitContextBase, IdMinter, OutboxStore, Region, TenantId, Timestamp};
// Used only by the `test-support`-gated `rooted_inmem_for_test` helper (MR-009b W3b.6).
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

/// Verified request identity and repository route coordinates for a Git edge operation.
#[derive(Clone, Copy)]
pub struct RepoActorContext<'a> {
    tenant: &'a str,
    region: &'a str,
    slug: &'a str,
    principal: &'a Principal,
}

impl<'a> RepoActorContext<'a> {
    /// Bind route coordinates to the already-authenticated principal.
    pub fn new(tenant: &'a str, region: &'a str, slug: &'a str, principal: &'a Principal) -> Self {
        Self {
            tenant,
            region,
            slug,
            principal,
        }
    }

    /// Add the pull-request number required by PR-scoped operations.
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

/// Authenticated repository context plus a pull-request number.
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

/// **The durable on-disk git backend the edge writes/reads through (GT-003).** Holds the durable git
/// store + the durable PR store rooted at one on-disk root, plus the shared outbox + id minter the ref
/// CAS co-commits its `git.ref.updated` through (the reconciler replays this outbox). The `(tenant,
/// region)` is taken from `ctx.scope` per request — never from the URL/body (the GIT-D8 invariant).
pub struct DurableGitBackend {
    store: DurableGitStore,
    /// Filesystem authority retained for repository-owned branch-protection policy and explicit
    /// legacy/test PR fixtures only. Production PR lifecycle records never read or write here.
    prs: DurablePrStore,
    /// Production PR lifecycle authority. `None` exists only in the test-support constructor so the
    /// long-standing filesystem fixtures remain hermetic.
    pg_prs: Option<PgPrStore>,
    /// Git-owned durable projection of CI check facts. Production always supplies it; test-support
    /// constructors leave it absent and continue to use explicit PR-record fixtures.
    checks: Option<myelin_git::check_status_store::PgCheckStatusProjection>,
    /// **R3.3 / R3.2 — the durable PR review-thread / comment / review-batch store.** Keyed by the
    /// canonical `object_key` (`pr:<slug>:<n>`); rooted at the SAME on-disk root as `store`/`prs`.
    threads: DurablePrThreadStore,
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
    /// **F3 (R4.1 dogfood) — the PUBLIC base URL prefix for the HTTP git-wire clone URL.** The wire is
    /// HTTP smart-transport ONLY (there is NO SSH server), and its real path grammar is
    /// `/{tenant}/{region}/{repo}.git` — so the clone URL rendered into every repo-home ViewModel must
    /// be `{base}/{tenant}/{region}/{repo}.git`, never the old hardcoded `ssh://git@myelin/{tenant}/…`
    /// (wrong scheme, missing region, wrong slug). The edge does not inherently know its own external
    /// origin, so the composition root injects its validated `MYELIN_PUBLIC_BASE_URL` (e.g.
    /// `https://git.example.com`); an empty value yields an HONEST relative
    /// `/{tenant}/{region}/{repo}.git` (a real path on this host) rather than a fabricated hostname.
    clone_base: String,
    /// The on-disk root holding `<tenant>/<region>/<repo>.git` bare repos — retained so the wire-serving
    /// tier (CT-006b) composes its sandboxed `GitCore` over the SAME root the durable store reads/writes.
    root: PathBuf,
    /// Process shutdown propagated into every per-request gVisor executor so `runsc` is killed and
    /// reaped before the HTTP drain deadline instead of outliving an aborted async task.
    git_shutdown: Arc<AtomicBool>,
    /// Per-wire short-lived credential authority. Production defaults unavailable/fail-closed until
    /// the composition root injects the live Identity adapter; test support injects an explicit
    /// deterministic issuer.
    git_wire_credentials: Arc<dyn crate::git_wire_exec::GitWireCredentialIssuerFactory>,
    /// **R0.3 / DELTA N2 — the per-repo object-authorization seam for the git wire.** The wire handlers
    /// consult this AFTER `repo_loc` resolves the repo and BEFORE serving any bytes, so an in-tenant
    /// principal with no grant on a repo cannot clone/fetch/push it (the un-granted-repo-reach hole,
    /// closed). Defaults to the [`AllowAllRepos`] fixture (the happy-path wire proofs dispatch);
    /// production boot injects a grant-backed authorizer via [`DurableGitBackend::with_repo_authorizer`]
    /// — the R2 platform-wide object-authz seam backs this with the real tuple store / Identity `check`.
    repo_authz: Arc<dyn RepoAuthorizer>,
    /// **R2.1a — the repo-create bootstrap-grant seam.** Consulted by
    /// [`DurableGitBackend::create_repo_as`] BEFORE the bare repo lands on disk: production injects
    /// [`crate::repo_authz_live::TupleRepoBootstrap`] (the creator→admin tuple through
    /// `write_tuples`, 4.6) so a fresh repo is immediately reachable by its creator under the
    /// deny-by-default [`crate::repo_authz_live::CheckBackedRepoAuthorizer`]. Defaults to the no-op
    /// (paired with the `AllowAllRepos` fixture, where no grant is needed). A grant failure ABORTS
    /// the create (fail-closed — never a repo no one can reach).
    bootstrap: Arc<dyn RepoBootstrapGrants>,
}

/// Loud summary of one placement-derived tenant recovery pass performed before serving starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitBootRecoveryReport {
    pub repos_reconciled: usize,
    pub refs_reapplied: usize,
    pub merges_recovered: usize,
}

/// Cell-wide aggregate for the placement-derived boot pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitCellBootRecoveryReport {
    pub tenants_recovered: usize,
    pub repos_reconciled: usize,
    pub refs_reapplied: usize,
    pub merges_recovered: usize,
}

/// Derive recovery scopes only from the durable local-tenant directory plus its canonical placement
/// row, then recover each active tenant. This is shared by Edge and MCP boot so neither process mints
/// tenant authority from Git filesystem paths or an incoming request.
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

/// One PR enriched for a list row (R3.1): the durable record + the rolled-up checks summary (Q4 —
/// rolled up in ONE pass, no N+1) + whether the viewer is a requested reviewer + the repo slug
/// (cross-repo rows only). The `summary` FAILS STATIC (`Unavailable`) if the repo's branch-protection
/// config could not be read — the row still lists (ux-git #5: a checks hiccup never blanks the row).
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
    /// Root the durable backend at an on-disk directory holding `<tenant>/<region>/<repo>.git` repos —
    /// the same root the durable git store + read backend resolve against. The wire object-authz seam
    /// (R0.3) defaults to [`AllowAllRepos`]; production injects a grant-backed authorizer via
    /// [`DurableGitBackend::with_repo_authorizer`].
    ///
    /// **The outbox + id minter are INJECTED (MR-009b W3b.4 — the composition root owns
    /// durability):** the production `main.rs` passes `OutboxStore::durable(PgOutboxBacking)` over
    /// the MR-022 provider pool AND a UNIQUE id source (`myelin_events::UlidMinter` — NEVER the
    /// per-instance-resetting default `MonotonicMinter`, whose colliding `event_id`s the durable
    /// path's `ON CONFLICT (event_id) DO NOTHING` silently drops; the W3b.3 named condition). A
    /// test passes the in-memory `OutboxStore::new()` double + a seeded deterministic minter.
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
            // F3: only the composition root's already validated public URL reaches clone responses.
            clone_base: public_clone_base.into().trim_end_matches('/').to_string(),
            root,
            git_shutdown: Arc::new(AtomicBool::new(false)),
            git_wire_credentials:
                crate::git_wire_exec::unavailable_git_wire_credential_issuer_factory(),
            // R2.6: the prod constructor default is FAIL-CLOSED (`DenyAllRepos`) — a composition root
            // that forgets `with_repo_authorizer` denies every repo rather than serving all of them.
            // Production `main.rs` ALWAYS injects the real `CheckBackedRepoAuthorizer`; the permissive
            // `AllowAllRepos` fixture now lives ONLY in the test-support `rooted_inmem_for_test` below.
            repo_authz: Arc::new(DenyAllRepos),
            bootstrap: Arc::new(NoRepoBootstrap),
        })
    }

    /// The in-memory-floor constructor for tests/drills: `rooted` over the in-memory outbox double
    /// plus the seeded deterministic `MonotonicMinter` (the pre-W3b.4 shape). Production roots use
    /// [`DurableGitBackend::rooted`] with a durable store + `UlidMinter` — this helper exists so
    /// the many test call sites stay one-line and the production signature stays injection-first.
    /// NOT a production path: the memory store loses events on restart (SI-007). MR-009b W3b.6
    /// gated `OutboxStore::new` — so this helper is `test-support`-gated with it, exactly as the
    /// W3b.4 note promised.
    #[cfg(any(test, feature = "test-support"))]
    pub fn rooted_inmem_for_test(root: impl Into<PathBuf>) -> DurableGitBackend {
        // The test/drill floor defaults to the permissive `AllowAllRepos` fixture so the many
        // non-authz test call sites stay one-line (each authz test overrides via
        // `with_repo_authorizer`). This is the ONLY `AllowAllRepos` construction left, and it is
        // inside this `test-support`-gated helper — never a production path (R2.6: prod `rooted`
        // fails closed with `DenyAllRepos`).
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

    /// **Inject the R0.3 per-repo object authorizer** (the wire object-authz seam) — the analogue of
    /// injecting the Identity-M1 [`Authorizer`](myelin_substrate::Authorizer) into the gateway. Boot
    /// wires the real grant-backed authorizer here; tests default to the [`AllowAllRepos`] fixture.
    /// **TODO(R2):** back this with the platform-wide object-authz tuple store / Identity `check()`.
    pub fn with_repo_authorizer(
        mut self,
        repo_authz: Arc<dyn RepoAuthorizer>,
    ) -> DurableGitBackend {
        self.repo_authz = repo_authz;
        self
    }

    /// The injected R0.3 per-repo object authorizer (the wire handlers consult this before serving).
    pub fn repo_authorizer(&self) -> &Arc<dyn RepoAuthorizer> {
        &self.repo_authz
    }

    /// **Inject the R2.1a repo-create bootstrap-grant writer** (the pair of
    /// [`DurableGitBackend::with_repo_authorizer`]: deny-by-default is only livable if a fresh
    /// repo's creator is granted on create). Production wires
    /// [`crate::repo_authz_live::TupleRepoBootstrap`] over the SAME tuple store the injected
    /// authorizer's engine reads.
    pub fn with_repo_bootstrap(
        mut self,
        bootstrap: Arc<dyn RepoBootstrapGrants>,
    ) -> DurableGitBackend {
        self.bootstrap = bootstrap;
        self
    }

    /// Bind the production process shutdown flag shared by upload-pack and receive-pack executors.
    pub fn with_git_shutdown_signal(mut self, shutdown: Arc<AtomicBool>) -> DurableGitBackend {
        self.git_shutdown = shutdown;
        self
    }

    /// Bind the live Identity credential authority used immediately before each Git wire sandbox
    /// launch. Without this injection the production default refuses every wire invocation.
    pub fn with_git_wire_credential_issuer(
        mut self,
        issuer: Arc<dyn crate::git_wire_exec::GitWireCredentialIssuerFactory>,
    ) -> DurableGitBackend {
        self.git_wire_credentials = issuer;
        self
    }

    /// **The wire-serving `GitCore` over the SAME on-disk root (CT-006b / GT-006).** Composes the
    /// production sandboxed [`crate::git_wire_exec::GitWireExecutor`] (wire ops → canonical `git` in the
    /// hardened gVisor sandbox, no-host-exec) with the in-process [`myelin_git::gix_backend::GixCore`]
    /// read backend. `advertise_refs(repo, UploadPack)` / `serve(repo, UploadPack, request)` flow
    /// through here against the real on-disk bare repos. The HTTP smart-transport listener that drives
    /// this over the wire (+ the receive-pack/PUSH path) is CT-006c.
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

    /// The shared outbox (so the reconciler / a relay can read the committed `git.ref.updated` rows).
    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    /// **The GT-003 recovery boot-hook (required before the front door serves).** Replay the committed
    /// `git.ref.updated` rows against one repo's on-disk refs, healing the apply-after-outbox-commit
    /// window ([`myelin_git::reconcile`]) — idempotent on `update_seq`. The production composition root
    /// drives this for every repo in the placement registry on boot, over the durable outbox tier
    /// (the events crate's `outbox` table); the edge's in-memory [`OutboxStore`] is the model of that
    /// tier. A repo with no behind refs is a clean no-op.
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

    /// Recover every durable Git write known for a placement-validated tenant before serving. The
    /// repository discovery set is exclusively durable authority: scoped committed ref witnesses
    /// union scoped pending merge intents. Filesystem directory names never mint recovery scope.
    /// Every target repository is opened and its ref witnesses reconciled before any pending merge
    /// intent is drained; an absent/corrupt target fails boot loud.
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

    /// Mint the database scope only from the verified principal, then require the route-derived
    /// locator to agree. A forged tenant/region argument is indistinguishable from an absent
    /// partition and never becomes query authority.
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

    /// Materialized migration seam for
    /// `pull_request.review = reviewer ∪ parent_repo->push`. The PR relation tuples are not yet
    /// projected into Identity, so settle the same union from the durable requested-reviewer fact
    /// and the live repo permission. Any row lookup failure denies.
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

    /// The GIT-1 tenant pseudonym for a principal (`<principal>@<tenant>.noreply`) — never a raw identity.
    fn pseudonym(tenant: &str, principal: &Principal) -> String {
        format!("{}@{}.noreply", principal.principal_id.0, tenant)
    }

    /// Whether a principal is an AGENT (ADR-08 legibility — an agent author is stamped `is_agent`,
    /// never disguised as a human).
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

    // ── create-repo (durable) ──

    /// Create a bare repo on disk under the verified `(tenant, region)`. Returns `true` iff newly created
    /// (an existing repo is a conflict the handler surfaces as `409`). Traversal-safe via the resolver.
    ///
    /// **The DIRECT (no-bootstrap-grant) path** — test fixtures / pre-authz callers that stage repos
    /// out-of-band. The edge's create HANDLER goes through [`DurableGitBackend::create_repo_as`]
    /// (R2.1a) so the creator→admin bootstrap grant is written; under the production deny-by-default
    /// authorizer a repo created HERE is reachable by no one until a tuple is granted separately.
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

    /// **Create a bare repo AS a verified principal (R2.1a — the edge create path).** Same as
    /// [`DurableGitBackend::create_repo`] plus the creator→admin bootstrap grant through the
    /// injected [`RepoBootstrapGrants`] seam, written BEFORE the directory lands:
    ///
    /// - **grant-first ordering:** the tuple write and the on-disk `git init` are two stores with no
    ///   shared transaction. Grant-first (not create-first) avoids the residue we refuse most — a
    ///   repo that exists but is reachable by NO ONE.
    /// - **fail-closed:** a grant-write failure ABORTS the create loudly (`DurableError::Git` → the
    ///   handler's 500); the repo is NOT created without its owner grant.
    /// - **compensation on create-failure (R2.1a-followup, defect #7):** if the grant committed but
    ///   the on-disk `git init` then FAILS, the error arm issues a COMPENSATING
    ///   [`RepoBootstrapGrants::revoke_creator`] (an exact-tuple `Remove` through the 4.6 write path)
    ///   so NO orphan `repo:<slug>#admin@<creator>` grant survives on a repo that does not exist. The
    ///   orphan is the real hole: a DIFFERENT principal could later create the same slug and the
    ///   original (failed-create) principal would silently still hold admin on it (cross-user
    ///   access). If the compensation ITSELF fails we surface BOTH errors loudly — a known, logged
    ///   orphan grant beats a silent one; a reconciler is the durable sweep for that narrow window.
    /// - **residual window (documented, out of scope):** a process crash BETWEEN the grant-commit and
    ///   the compensating remove still orphans the grant (no shared transaction across PG + the
    ///   filesystem, and no in-process compensation can run through a crash). Healing that requires an
    ///   out-of-band reconciler (grant with no on-disk repo → revoke) — named, not built here. The
    ///   common failure path (an on-disk create error the process observes) IS cleaned up.
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
                "creator bootstrap grant refused (repo NOT created — fail-closed): {e}"
            ))
        })?;
        // The grant is now durably committed. If the on-disk create fails, we MUST NOT leave the
        // grant as an orphan (defect #7 — cross-user access via slug reuse), so compensate before
        // returning the error.
        match self.store.create_repo(&loc) {
            Ok(_repo) => Ok(true),
            Err(create_err) => match self.bootstrap.revoke_creator(creator, &loc) {
                // Compensation succeeded: no orphan grant survives — surface the original create error.
                Ok(()) => Err(create_err),
                // Compensation FAILED: fail loud with BOTH — a KNOWN orphan grant (logged here) beats
                // a silent one, and a reconciler is the durable sweep for it.
                Err(revoke_err) => Err(DurableError::Git(format!(
                    "repo create FAILED and the compensating bootstrap-grant removal ALSO failed — \
                     an admin grant on `{slug}` is ORPHANED (reachable by slug reuse; a reconciler \
                     must revoke it): create error: {create_err}; compensation error: {revoke_err}"
                ))),
            },
        }
    }

    // ── repo list (durable read) ──

    /// The verified tenant's on-disk repo SLUGS (sorted). The tenant/region dir holds `<repo>.git`
    /// bare repos; resolve via a representative locator's parent so the scan stays inside the
    /// validated tenant/region path (no traversal). This is the CANDIDATE set the R2.1 list
    /// prefilter intersects with the principal's `pull`-visible set — never served raw.
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

    /// **List the repos `principal` may `pull` (R2.1 — the leak-free list).** The on-disk listing is
    /// the CANDIDATE set; the injected [`RepoAuthorizer::visible_repos`] prefilter (backed by the
    /// Identity `list_objects` Ids-materialise in production, per-candidate checks otherwise)
    /// resolves the visible subset BEFORE any ViewModel is built — an un-granted repo's slug/readme/
    /// tree never reach the response (0-leak: pre-filter by construction, never a post-filter).
    /// ViewModels are built from the REAL on-disk state (Populated/Empty) — never a seed.
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

    /// Build only the selected repository-catalogue rows. Candidate discovery and the complete
    /// visibility intersection happen before the keyset continuation is applied; repository state
    /// is opened and classified only for the final output page.
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

    /// **F3 — the HTTP git-wire clone URL for a repo.** The wire path grammar is
    /// `/{tenant}/{region}/{repo}.git` (the ONLY transport is HTTP smart-protocol), prefixed by the
    /// configured public base (`MYELIN_PUBLIC_BASE_URL`; empty → an honest relative path). Never the
    /// old `ssh://git@myelin/…` (no SSH server exists; the region segment + real slug were missing).
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

    // ── commit log + commit diff (durable read; reuses the durable repo's libgit2 walk/diff) ──

    /// A page of the commit log for a ref (newest-first) as [`CommitRow`] ViewModels + the `has_more`
    /// cursor flag. A bare ref name is qualified to `refs/heads/<ref>` (a fully-qualified `refs/…` is
    /// used as-is). Tenant-scoped via the validated resolver.
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

    /// One commit's diff page as a [`CommitDiff`] ViewModel (`None` if the oid is malformed/absent).
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

    // ── R3.2 · G-7 — the PR three-dot diff (N1) + expand-context (N2) ──

    /// The PR three-dot diff page (`GET …/prs/{n}/diff`) as a [`PrDiffVM`]. Resolves the PR record
    /// (404 if absent — a thread/diff op on a missing PR is the overview's 0-leak 404), computes
    /// `merge-base(base_ref, head_oid) … head_oid` over the durable repo, and pages FILES (MR-014
    /// envelope) at `offset`/`limit`. `restricted_files` is COUNT-ONLY — under the current repo-level
    /// `Pull` guard a viewer who may see the PR may see every file, so it is 0 (the per-path ACL is a
    /// named follow-on; the field is wired non-leaking so a future ACL feeds a count, never paths).
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
            // A malformed/absent head oid — an honest empty diff (the UI renders the empty state, not
            // an error): no files, the base_ref labelled, three_dot false.
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

    /// Expand-context lines (`GET …/file-lines/{oid}?path=&start=&end=`) — the raw context of a blob at
    /// `oid`, `start..=end`. `path` is carried for the client's column mapping only; the authz is the
    /// SAME object check as the blob route (`Pull` at the edge). `None` if the oid is malformed/absent.
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

    // ── R3.4 repo-browsing completeness: refs · tree-at-path · nested blob · raw/download ──

    /// The RefsVM for the switcher (`GET /repos/{repo}/refs`) — the legacy branches/tags/default
    /// fields plus bounded current/default pins and stable pagination. The route's
    /// [`RepoObjectGuard`] performs `Pull` authorization before its strict query/cursor parser runs.
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

    /// The enriched RepoHomeVM (`GET /repos/{repo}`) — default_branch, full README, latest_commit,
    /// per-entry latest-commit (ONE bounded walk), branch/tag counts, name-carrying entries. Built from
    /// the durable on-disk state; `NotFound` (404) if the repo is absent under the verified tenant.
    fn repo_home_json(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?; // NotFound → 404 (0-leak)
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
            // Full README (rendered via the editor read-path / sanitized markdown renderer on the client;
            // never a raw-HTML dump). `readme_excerpt` retained for back-compat with the pre-R3.4 VM.
            "readme": readme,
            "readme_excerpt": readme.as_ref().map(|r| r.chars().take(400).collect::<String>()),
            "latest_commit": latest.as_ref().map(commit_brief_json),
            "counts": counts,
            "entries": entries_json,
            "entries_page": entries_page,
            "snapshot_oid": page.snapshot_oid.0,
        }))
    }

    /// The TreeVM (`GET /repos/{repo}/tree/{ref}/{...path}`, root = empty path). A file requested under
    /// `tree/` returns `{ redirect_to_blob: true }` (the gate's kind-mismatch → client redirect); an
    /// absent path is `NotFound` (404). Shares the entry projection with the repo home.
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

    /// The enriched BlobVM (`GET /repos/{repo}/blob/{ref}/{...path}`, nested). Small files include a
    /// server-classified inline preview; larger files stop at the ODB header and return an honest
    /// metadata-only fallback. A directory requested under `blob/` returns
    /// `{ redirect_to_tree: true }` (kind mismatch → client redirect); an absent path is `NotFound`.
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
                // Binary bytes never enter a text projection. Large objects never reach this branch:
                // the bounded reader returns metadata from the object header before inflation.
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

    /// Serve raw file BYTES (`GET /repos/{repo}/raw|download/{ref}/{...path}`) — gateway-proxied,
    /// in-region, never a public signed URL (the sovereignty rail, BINDING). `attachment` sets
    /// `Content-Disposition: attachment` (the Download affordance); otherwise the bytes stream inline
    /// with a conservative content-type (never `text/html` — a repo blob is never browser-executed).
    /// Object-guarded on `Pull` at the route.
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
        // A conservative content-type: text stays `text/plain; charset=utf-8` (never executed),
        // binary is `application/octet-stream`. The filename is the basename of the requested path.
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
        // Defense-in-depth: never let a browser sniff a repo blob into an executable type.
        headers.push(("x-content-type-options".to_string(), "nosniff".to_string()));
        Ok(EdgeResponse::Bytes {
            status: 200,
            content_type,
            headers,
            body: bytes,
        })
    }

    // ── web-edit commit (durable ref-CAS) ──

    /// Commit a single-file web edit DURABLY: GF-6 stale-base CAS on the blob, then write the new commit
    /// to the odb and advance the ref via the durable per-ref CAS ([`RefStore`]). A stale blob base OR a
    /// raced ref tip is the honest `409`; a clean base persists (`durable: true`).
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

        // GF-6: the current blob oid (or "" for a new file) is the CAS base. The oid comes directly
        // from the tree entry; never inflate a potentially huge existing file merely to compare it.
        let current_base = repo
            .blob_oid_at_path(&full, path)?
            .map(|oid| oid.0)
            .unwrap_or_default();

        // The pure GF-6 CAS (reused) — a stale base refuses honestly (no silent overwrite).
        let probe = WebEditOutcome::evaluate(expected_base, &current_base, "pending", true);
        if let WebEditOutcome::StaleBase { current_oid } = probe {
            return Ok(WebEditOutcome::StaleBase { current_oid });
        }
        if let WebEditOutcome::Denied = probe {
            return Ok(WebEditOutcome::Denied);
        }

        // Build the real commit (blob → tree → commit) authored to the tenant pseudonym (GIT-1).
        let psn = Self::pseudonym(tenant, principal);
        let (new_commit, _new_blob, parent) =
            repo.build_file_commit(&full, path, contents.as_bytes(), "web edit", &psn, &psn)?;

        // Advance the ref via the durable per-ref CAS (the SAME one-tx ref-CAS + outbox the push uses).
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
            // A raced ref tip (someone committed between our read and CAS) → honest stale (409).
            PushOutcome::Rejected(_) => Ok(WebEditOutcome::StaleBase {
                current_oid: current_base,
            }),
            PushOutcome::Crashed(_) => Err(DurableError::Git("web-edit ref-CAS crashed".into())),
        }
    }

    // ── PR lifecycle (durable) ──

    /// Read a durable PR record back (the fresh-read proof a write persisted). `None` if absent under
    /// the verified `(tenant, region)`. Tenant-scoped via the validated resolver.
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
        // Peer-review finding #3: allocate from the FILENAME-authoritative max, independently of
        // parsing the list. A corrupt highest PR record makes list fail loud but its occupied filename
        // still counts here, so allocation can never reuse and overwrite it.
        // The directory read is authoritative too: an I/O fault must abort allocation, never reset it
        // to #1. Exhausting the u64 namespace is likewise a loud refusal, not a wrap to zero.
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
        self.store.open_repo(&loc)?; // 404 if the target repo is absent
                                     // The PR-open body carries ONLY the proposal (base/head/head_oid/draft) — NEVER branch-protection
                                     // POLICY (required set / approval threshold) or check FACTS (greens). Policy is repo-owned (set
                                     // via the repo-admin branch-protection op); facts are set by authorized producers (the CI
                                     // check-report op, the review op, the endorse op). This is the GT-003 bypass fix: a PR author
                                     // cannot weaken the gate by supplying loose policy or self-claimed greens at open.
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
        // Source authority is derived from the verified principal's partition, never from the body.
        // Pull authorization is checked on the exact source repo before its ref/OID is resolved; a
        // denial is 0-leak and indistinguishable from an absent source.
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
        // F8 (R4.1 dogfood): if the caller omitted `head_oid`, RESOLVE it from `head_ref`'s CURRENT tip
        // in the durable repo. Previously an absent head_oid was stored EMPTY and the PR was silently
        // unmergeable — a merge later failed with the confusing "invalid merge head: head_oid  is not a
        // commit in the repo". Resolving at OPEN time both (a) makes the ref-only open path mergeable
        // (the stored head_oid is the branch tip) and (b) turns a non-existent head_ref into a CLEAR
        // 400 the moment the PR is proposed, not a mystifying failure at merge. The explicit-head_oid
        // path is UNCHANGED — an author who pins a specific oid keeps exactly that oid.
        let qualified_head_ref = if head_ref.starts_with("refs/") {
            head_ref.clone()
        } else {
            format!("refs/heads/{head_ref}")
        };
        let source_tip = head_repo.read_ref(&qualified_head_ref)?;
        let head_oid = if head_oid.is_empty() {
            // Qualify a bare branch name to `refs/heads/<name>`; a fully-qualified `refs/…` is used as-is.
            match source_tip {
                Some(tip) => tip.0,
                // 400 at OPEN (map_durable_err routes a "missing" `Git` error to BadRequest) — never a
                // stored empty head that wedges the merge dialog with a confusing error later.
                None => {
                    return Err(DurableError::Git(format!(
                    "open-PR head_ref `{head_ref}` does not exist in the repo — no branch tip to \
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
        // R3.1: the title is REQUIRED at create (a hollow list is the ux-git #3 defect). A missing or
        // blank title is a clean 400 — never a silent empty title on a NEW PR (a legacy record with no
        // title predates the store and honestly renders as `#number`; a fresh create must carry one).
        let title = body
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| DurableError::Git("open-PR body missing a non-empty `title`".into()))?
            .to_string();
        // Cap the title (verifier note, R3.1): it is echoed in every list row — an unbounded
        // title is a response-bloat vector. 512 bytes is generous for a real title.
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
        // PostgreSQL owns allocation atomically; production never consults stale/corrupt legacy
        // filesystem PR files. The legacy/test authority still allocates from its filenames.
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
        rec.created_at = Some(now); // R3.3 N1 — the header's "opened …" date, stamped once at open.
        rec.updated_at = Some(now);
        self.pr_open(&loc, rec, operation_id, principal)
    }

    // ── R3.1 — the leak-free PR LIST (per-repo + cross-repo front door) ──────────────────────────
    //
    // **The leak-free prefilter (the anti-oracle rule).** The frozen Git ReBAC fragment defines
    // `pull_request.view = parent_repo->pull` (repo_authz.rs / contract 4.9), so the PR-list
    // permission `myelin_git::list_filter::PR_LIST_PERMISSION` (`"view"`) REDUCES, for every PR in a
    // repo, to the viewer's `pull` on the PARENT repo. Two consequences the wiring rides:
    //   • **per-repo `/repos/{repo}/prs`** is guarded by the SAME [`RepoObjectGuard`] `Pull` check as
    //     every other repo read (registered in `register_git_durable`): a viewer who cannot `pull`
    //     gets the 0-leak 404 (the "no access" state), and once past it EVERY PR in the repo is
    //     `view`-able — so counts/tab-badges/cursors computed over the on-disk set never leak a
    //     hidden PR (there is none to hide);
    //   • **cross-repo `/prs`** prefilters the repo candidate set through
    //     [`RepoAuthorizer::visible_repos`] (the `list_objects(viewer, pull, repo)` seam) FIRST, then
    //     lists PRs only within the visible repos — a forbidden repo's PRs never enter any bucket,
    //     count, or cursor.
    // This realises `compose_pr_list_query` / `PR_LIST_PERMISSION`'s leak-free *semantics* over both
    // storage authorities. The SQL composer in `list_filter.rs` targets an abstract authorization
    // projection rather than this literal query; the repo `pull` prefilter is the equivalent gate.

    /// **The per-repo PR list (R3.1).** Every PR in the repo (the caller has already cleared the
    /// `Pull` object guard, so all are `view`-able), enriched with the checks rollup + the viewer's
    /// review status. Storage applies state/sort/cursor and computes exact tab/sidebar counts over
    /// the already leak-free repository relation before only the bounded page is enriched.
    fn list_prs_for_repo(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        principal: &Principal,
        query: &PrListQuery,
    ) -> Result<EnrichedPrSlice, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.store.open_repo(&loc)?; // 404 if the repo is absent (never a phantom empty list).
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

    /// **The cross-repo PR front door (R3.1, single-cell for R3 — Q5).** Prefilter the on-disk repo
    /// candidates through the `visible_repos` `list_objects` seam FIRST (a forbidden repo never
    /// contributes a PR), then enrich the PRs under each visible repo, tagging each row with its repo
    /// slug. The bucket predicate (`yours` = authored-by-viewer; `needs-review` = viewer is a
    /// requested reviewer) is applied by the handler over this leak-free set. Per-repository reads
    /// and the request-wide record/serialized-byte aggregate are independently capped; below those
    /// ceilings the returned bucket remains exact.
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

    /// The stable state token for a PR (matches [`Self::pr_json`]).
    fn pr_state_token(state: PrState) -> &'static str {
        match state {
            PrState::Draft => "draft",
            PrState::Open => "open",
            PrState::Merged => "merged",
            PrState::Closed => "closed",
        }
    }

    /// The list-row VM JSON for one enriched PR (`PrListRowVM`). An empty `title` (a legacy record)
    /// serialises as `null` so the frontend renders the honest `#number` fallback, never a blank
    /// title masquerading as real.
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

    /// **Repo-admin: set the branch-protection policy (GT-003).** The required set + thresholds the merge
    /// enforces live HERE, never in author input. The edge gates this behind the distinct
    /// `git.repo.branch_protection.set` authorize action (the production authorizer resolves
    /// `Id.check(repo_admin)`); the durable config is path-isolated via the validated resolver.
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

    /// **CI check-report — a CI-PRODUCER capability (GT-003 / R2-exit Defect 1).** The authorized
    /// producer stamps the green / fork-unendorsed check facts on a PR for its head; the facts the merge
    /// gate reads come from HERE, never from the PR-open body.
    ///
    /// **The producer floor (why a plain writer cannot self-certify its own PR).** Reporting CI check
    /// facts is NOT an ordinary writer capability — if it were, a PR author (a writer) could stamp its
    /// own required checks green and defeat the merge gate / the protected-push gate. So this is gated
    /// as a CI-producer capability two ways: (1) the edge routes `DReportChecks` through the R2.1
    /// [`RepoObjectGuard`] (`RepoPermission::Push` — the object-level repo grant stays required); and
    /// (2) HERE, fail-closed, the reporting principal MUST be a SERVICE principal — the kind a CI
    /// runner's self-hosted-runner run token mints as (`mint`, P-ID-18). A `Human` (or `Agent`)
    /// principal is REFUSED regardless of any writer grant. How CI legitimately reports today: a CI run
    /// executes under a service principal (the run token), and that service principal — never the human
    /// developer who opened the PR — posts the check facts. The full producer-RELATION
    /// (`repo.report_checks` / a `ci_producer` edge in the frozen fragment) is the R2+ follow-on; this
    /// SERVICE-kind floor is the deny-a-human-writer guarantee in the meantime.
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
        // The producer floor: only a SERVICE principal (a CI run token) may attest CI check facts. A
        // human writer / an agent is refused (fail-closed) — it cannot self-certify its own PR.
        if !matches!(principal.kind, PrincipalKind::Service) {
            return Err(DurableError::Forbidden(format!(
                "git.checks.report is a CI-producer capability: principal `{}` (kind {:?}) is not a CI \
                 service producer — a human/agent writer cannot attest CI check facts on a PR",
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
        // Endorse the named contexts (or all currently-un-endorsed fork contexts). The maintainer's
        // `approve_untrusted_ci` capability is the gateway's authz gate; the durable record records the
        // resolved endorsement ([`myelin_git::fork_gate`] is the live resolver in the CLI/agent path).
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
            // Re-enter an already-admitted operation from the durable target-side intent before
            // touching the source repository. A source grant can be revoked or the fork deleted
            // after the ref CAS; neither may strand finalization of an operation that already moved
            // the target ref. Request retries still have to match the original actor + operation.
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
            // A fork PR's locked commit may live only in the source repository. Install its complete
            // verified object closure in the target ODB before the durable merge protocol is allowed
            // to advance the target ref. The import copies no source refs; the PG protocol below still
            // revalidates that the authoritative source ref equals this locked OID.
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

    // ── R3.3 / R3.2 — the PR review-thread / comment / review-batch surface ───────────────────────
    //
    // The canonical model is THREADS (an optional content anchor; comments belong to threads); review
    // batching layers on via `review_id` + the `ReviewBatch` lifecycle. Read = PR view (the `Pull`
    // object guard at the route); write = `pull_request.review` (requested reviewer or parent-repo
    // Push) — never a permissive authorizer. Storage keys by the canonical `object_key`
    // (`pr:<slug>:<n>`) so issues/
    // docs mount the SAME store later. Every mutation verifies the PR exists first (a thread on a
    // non-existent PR is a 404, mirroring the overview).

    /// The canonical type-qualified object key for a PR (`pr:<slug>:<n>`). Bare `type:id` form is a
    /// fixed point under [`myelin_refs::object_key`] (the tuple key IS this string), so issues/docs can
    /// mount the same store by their own `issue:<id>` / `doc:<id>` keys with zero migration.
    fn pr_object_key(slug: &str, number: u64) -> String {
        format!("pr:{slug}:{number}")
    }

    /// The [`ThreadPrincipal`] snapshot for the acting principal (GIT-1 pseudonym as the display;
    /// the four-channel agent badge rides `kind`). Never a raw identity.
    fn thread_principal(tenant: &str, principal: &Principal) -> ThreadPrincipal {
        let kind = match principal.kind {
            PrincipalKind::Agent { .. } => PrincipalRole::Agent,
            PrincipalKind::Service => PrincipalRole::Service,
            _ => PrincipalRole::Human,
        };
        ThreadPrincipal::plain(kind, Self::pseudonym(tenant, principal))
    }

    /// Verify the PR exists (a thread op on an absent PR is a 404, exactly like the overview).
    fn require_pr(
        &self,
        loc: &RepoLoc,
        number: u64,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        self.pr_get(loc, number, principal)?
            .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))
    }

    /// Resolve a requested line against the current authoritative PR diff and capture the immutable
    /// revision pair used for the decision. A stale or fabricated path/side/line never becomes a
    /// misleading `live` anchor.
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

    /// **GET …/prs/{n}/threads — the viewer-scoped conversation.** Read = PR view (the `Pull` guard).
    /// Pending review-batch comments authored by OTHERS never enter the projection (non-leak by
    /// construction — `SubjectThreads::view_for`).
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

    /// POST …/prs/{n}/threads — open a new thread with its first comment (`anchor` null = a discussion
    /// thread; an anchor object = a diff-line thread). Write = `pull_request.review`.
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

    /// POST …/prs/{n}/threads/{tid}/comments — reply to a thread. Write = `Push`.
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

    /// POST …/prs/{n}/threads/{tid}/resolve — resolve/unresolve a thread. Write = `Push`.
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

    /// POST …/prs/{n}/reviews/start — start a review batch (draft; verdict `in_progress`). Write =
    /// `pull_request.review`. (`/start` distinguishes this from the existing single-shot `POST …/reviews` verdict op
    /// that feeds the merge gate — a named deviation from N5's literal path, preserving R2's gate path.)
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

    /// POST …/prs/{n}/reviews/{rid}/comments — add a PENDING comment to a draft batch (visible only to
    /// its author until submit). Write = `pull_request.review`.
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

    /// POST …/prs/{n}/reviews/{rid}/submit `{ verdict, summary_md }` — submit the batch. Emits ONE
    /// batch event (R-BATCH-1; idempotent on retry). A NON-advisory (human) `approved` /
    /// `changes_requested` verdict ALSO feeds the merge gate via the durable review record (an agent
    /// batch stays advisory — it never gates). Write = `pull_request.review`.
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
        // Feed the merge gate: a NON-advisory approved/changes_requested verdict pushes a durable
        // review record (the gate reads `counting_approvals` + `has_blocking_review`). An agent batch
        // is advisory and a comment-only verdict adds no gate signal. Only the FIRST submit (Some)
        // feeds the gate — a re-submit is idempotent (None), so no double-counted approval.
        if let Some(ref batch) = submitted {
            if !batch.review.advisory {
                let gate_verdict = match verdict {
                    BatchVerdict::Approved => Some("approve"),
                    BatchVerdict::ChangesRequested => Some("request-changes"),
                    _ => None,
                };
                if let Some(v) = gate_verdict {
                    // Reuse the existing gate-feeding review op (self-approval is excluded there).
                    let _ = self.submit_review(tenant, region, slug, number, v, principal);
                }
            }
        }
        self.bump_pr_updated(&loc, number, principal);
        Ok(json!({
            // The ONE batch event's payload (server-side R-BATCH-1). `emitted` is true exactly once
            // (the first submit); a re-submit is idempotent → `emitted: false`, no double event.
            "emitted": submitted.is_some(),
            "review": submitted.as_ref().map(|b| review_batch_json(&b.review)),
            "comment_ids": submitted
                .as_ref()
                .map(|b| b.comment_ids.clone())
                .unwrap_or_default(),
        }))
    }

    /// DELETE …/prs/{n}/reviews/{rid} — discard a DRAFT batch (its private comments leave no trace).
    /// A submitted batch is public record — `Forbidden`. Write = `Push`.
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

    /// Bump the PR's `updated_at` after an authored conversation mutation (best-effort — a failure to
    /// re-stamp the record never fails the comment that already persisted).
    fn bump_pr_updated(&self, loc: &RepoLoc, number: u64, principal: &Principal) {
        if let Ok(operation_id) = self.fresh_operation_id() {
            let _ = self.pr_mutate(loc, number, PrMutation::Touch, &operation_id, principal);
        }
    }

    // ── git smart-HTTP PUSH (receive-pack) over the wire — CT-006d ──

    /// The receive-pack ref advertisement source: every `(ref_name, oid)` on the durable repo, sorted.
    /// A pure read of OUR tenant-scoped repo (no sandbox needed); the wire handler frames it + the
    /// service header + the restricted capability list. `NotFound` (404) if the repo is absent.
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

    /// **The receive-pack PUSH write path (CT-006d).** Parses the ref-update commands + packfile, ingests
    /// the UNTRUSTED pack in the hardened sandbox (`index-pack` into a writable `/tmp` quarantine — the
    /// real repo stays RO), stages the fully-resolved objects in a HOST quarantine (connectivity + non-ff
    /// computed there, never touching the real repo), then runs the in-process policy + the ONE-tx
    /// ref-CAS + `git.ref.updated` outbox emit ([`RefStore::receive`]) — migration writes the accepted
    /// objects into the real repo BETWEEN policy-pass and the CAS (reject-before-ref-moves; abort discards
    /// the quarantine). Returns the `report-status` body the client renders. A push to a non-existent repo
    /// is `NotFound` (404); every per-push refusal (corrupt pack / policy / non-ff / connectivity) is a
    /// clean `report-status` with `ng` per ref (HTTP 200) so `git push` shows the honest rejection.
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
        let repo = Arc::new(self.store.open_repo(&loc)?); // NotFound → 404 (no cross-tenant leak)

        let (cmds, pack) = match parse_push_request(body) {
            Ok(v) => v,
            Err(e) => return Ok(report_status(&format!("parse-error: {e}"), &[])),
        };
        if cmds.is_empty() {
            return Ok(report_status("no-commands", &[]));
        }

        // 1. Ingest the untrusted pack in the SANDBOX → fully-resolved objects (empty for delete-only).
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
                // A corrupt/forged/incomplete pack fails `index-pack` in the sandbox → honest reject.
                Err(e) => {
                    return Ok(report_status(
                        &format!("index-pack-failed: {e}"),
                        &all_ng(&cmds, "object ingest rejected"),
                    ))
                }
            }
        };

        // 2. Stage the objects in a HOST quarantine repo (alternates → the real repo so existing history
        //    + thin bases resolve) so connectivity + non-ff are computed WITHOUT touching the real repo.
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

        // 3. Build the proposed updates: `forced` = an existing ref advancing to a NON-descendant; a
        //    non-delete tip whose object set is INCOMPLETE (missing tree/blob) rejects the whole push.
        let mut updates = Vec::new();
        let mut per_ref_status: Vec<(String, Option<String>)> = Vec::new();
        // R0.7-D / DELTA N4: the durable repo's CURRENT ref tips (pre-push state), computed once for the
        // full-history connectivity walk below. The walk hides these so a thin push only pays for the
        // newly-introduced commits; an unreadable ref list fails safe to `[]` (a wider walk = stricter).
        let existing_tips: Vec<CoreOid> = repo
            .list_refs_bounded(WIRE_MAX_REFS)
            .unwrap_or_default()
            .into_iter()
            .map(|(_, oid)| oid)
            .collect();
        for c in &cmds {
            let new_zero = c.new.chars().all(|ch| ch == '0');
            let old_zero = c.old.chars().all(|ch| ch == '0');
            // R0.7-D / DELTA N4: FULL-history connectivity, not just the tip tree. `index-pack --fix-thin`
            // resolves delta bases but NOT missing PARENT commits, and the old tip-only check
            // (`commit_tree_complete`) verified only the tip commit's own tree — so a push whose tip has a
            // missing ancestor was accepted and later wedged clone/fetch. Walk from the new tip hiding the
            // existing tips; reject if any reachable commit/tree/blob/parent is absent. Fail-closed:
            // `.unwrap_or(false)` rejects on any infra error too.
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

        // 3b. **R0.2 / DELTA N1 — the branch-protection gate on a DIRECT push to a PROTECTED ref.** A
        //     `git push` straight to a protected branch must clear the SAME gate a PR merge into that
        //     branch would: reject force-push (non-ff) + delete, and REQUIRE the repo's configured
        //     `required_contexts` to be green-and-current for the pushed head. Protected-ness comes from
        //     the repo-owned [`BranchProtectionConfig`] (never a hardcoded literal) — a configured
        //     ruleset (any ref pattern) protects its refs; the built-in `main`/`release/*` literal is at
        //     most a fallback for an unconfigured protected-looking ref (never MORE permissive than the
        //     pre-R0.2 force/delete floor). Reject-before-the-ref-moves: any protected violation aborts
        //     the whole atomic push with a loud per-ref `ng` (0 under-gated protected push). The check
        //     facts are read from Git's OWN durable records (acyclic — this never calls CI, EI-02 §3).
        // FAIL-CLOSED on a config-load error (verifier finding): a MISSING protection file is `Ok(None)`
        // (no protection configured — proceed), but a CORRUPT/unreadable `branch-protection.json` is an
        // `Err`. Swallowing that Err into `None` (`.ok().flatten()`) would SILENTLY DISABLE protection —
        // a non-standard protected ref (e.g. `refs/heads/prod`) would skip the gate entirely, and
        // `main`/`release/*` would lose required-CI enforcement. That is a self-disabling security gate.
        // If we cannot read the policy we cannot safely accept ANY push, so reject the whole push loudly
        // (matching the PR-merge path, which fail-closes via `?` on the same load error).
        let protection = match self.prs.get_protection(&loc) {
            Ok(p) => p,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&qdir); // discard the host quarantine (no ref moves).
                return Ok(report_status(
                    "ok",
                    &all_ng(
                        &cmds,
                        &format!(
                            "rejected (branch-protection policy unreadable — fail-closed): {e}"
                        ),
                    ),
                ));
            }
        };
        // R2-exit Defect 2 — the pusher's PROTECTED-PUSH (admin/bypass) standing, resolved ONCE through
        // the R2.1 per-repo object authorizer (`repo.protected_push = admin`, contract 4.9). A DIRECT
        // push to a protected ref is admitted only if the pusher holds this admin-only rung OR the push
        // clears the FULL branch-protection ruleset (Defect 3) — a plain writer holds neither, so it can
        // no longer land arbitrary code on `refs/heads/main` over the wire. Computed here (not per-ref) —
        // the standing is a repo-level grant, and a push whose refs include ANY protected one is held to
        // the stronger check.
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
                continue; // a non-protected ref keeps the existing PushPolicy checks (unchanged).
            }
            let ruleset = effective_ruleset(protection.as_ref(), ref_str);
            protected_updates.push((index, ruleset.clone()));
            if self.checks.is_some() {
                // Production evaluates under the projection consumer's shared admission lock below,
                // holding it through the ref mutation. Reading here would reopen a check→CAS race.
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
                let _ = std::fs::remove_dir_all(&qdir); // discard the host quarantine (no ref moves).
                return Ok(report_status(
                    "ok",
                    &all_ng(&cmds, &format!("rejected (branch protection): {reason:?}")),
                ));
            }
        }

        // 4. The ONE-transaction ref-CAS + outbox via the durable RefStore. policy (secret-scan / size /
        //    pseudonymity) runs INSIDE `receive` BEFORE the migration; `ObjectPromotion::migrate` writes
        //    the accepted objects into the REAL repo (re-hashing each — a forged oid is impossible)
        //    between policy-pass and the CAS; the CAS + `git.ref.updated` commit together (BUS-2).
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
                                "rejected (check admission scope unavailable — fail-closed): {error}"
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
                                "rejected (check projection unavailable — fail-closed): {error}"
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
        let _ = std::fs::remove_dir_all(&qdir); // the host quarantine is discarded either way

        match outcome.map_err(|e| DurableError::Git(format!("ref-CAS: {e:?}")))? {
            PushOutcome::Accepted { .. } => {
                // F9 (R4.1 dogfood): the first push that lands a branch heals a dangling `init_bare`
                // HEAD (→ the default branch) so a fresh `git clone` checks out with NO "nonexistent
                // ref, unable to checkout" warning. Best-effort: the push already committed durably and
                // the read-side paginated ref summary remains the fallback, so a heal hiccup must never
                // turn an ACCEPTED push into a failure.
                let _ = repo.heal_head_symref();
                Ok(report_status("ok", &per_ref_status))
            }
            // A policy/non-ff refusal moved NO ref and discarded the quarantine — LOUD `ng` per ref.
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
            // R3.1: the title/body store. An empty title (a legacy record) is `null` — the honest
            // `#number` fallback, never a fabricated title.
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
            // R3.3 N1 — the header's "opened …" date (Intl-formatted client-side); null on a legacy
            // record (the header omits it rather than fabricating a date).
            "created_at": rec.created_at,
            "durable": true,
        })
    }

    /// The commits IN a PR count (R3.3 N1/N2 — the tab badge) via the bounded reachability walk. A cap
    /// keeps it O(cap); the count is exact for dogfood-scale PRs (the named floor is `cap+`).
    fn commits_in_pr_count(&self, loc: &RepoLoc, rec: &PrRecord) -> Option<(u64, bool)> {
        let repo = self.store.open_repo(loc).ok()?;
        let (rows, has_more) = repo.commits_in_pr(&rec.base_ref, &rec.head_oid, 500).ok()?;
        Some((rows.len() as u64, has_more))
    }
}

// ---------------------------------------------------------------------------
// Handlers (durable; ViewModel/record-backed)
// ---------------------------------------------------------------------------

/// The [`QuarantineMigration`] that promotes a sandbox-validated, policy-passed push into the REAL repo
/// (CT-006d). `RefStore::receive` calls `migrate` ONLY after the in-process policy admits the push and
/// BEFORE the ref CAS — so a secret/oversized/non-pseudonymous object NEVER reaches the real odb, and a
/// crash/abort after migrate leaves only orphan (unreferenced, GC'able) objects, never a moved ref. Each
/// object is written via `write_raw_object`, which RE-HASHES the content (a forged oid is impossible).
struct ObjectPromotion<'a> {
    repo: &'a DurableGitRepo,
    /// (claimed-oid, type, raw-payload) for every object the sandbox returned.
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

/// Wall-clock unix seconds for the durable `updated_at` stamp (the list's "updated" column +
/// `sort=updated`). Best-effort — a clock before the epoch stamps 0 (the row simply omits the time).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── R3.3 / R3.2 thread-surface helpers (body parsing + VM serialization) ─────────────────────────

/// Extract a non-empty `body_md` (or `body`) from a comment/thread POST body — a clean 400 if absent
/// or blank (never a silent empty comment).
fn require_body_md(body: &Value) -> Result<String, DurableError> {
    body.get("body_md")
        .or_else(|| body.get("body"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| DurableError::Git("comment body missing a non-empty `body_md`".into()))
}

/// Parse an optional diff-line content anchor from a POST body. A line anchor must fully specify
/// `{ path, line, side }`; the caller then resolves it against the current authoritative PR diff.
/// `None` (absent anchor) means a PR-level discussion thread.
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
        // A removed comment withholds its body ("Comment removed", tree preserved) — never serialised.
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

/// The `GET …/threads` envelope: the viewer-scoped threads split into discussion (anchor null) vs
/// anchored, plus the visible review batches. The overview consumes `discussion` + `reviews`; the diff
/// pack consumes `anchored`.
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

/// Strictly parse the per-repository PR-list query. PostgreSQL relations may be larger than the
/// transitional numeric cursor coordinate ceiling: their first pages and exact badges remain
/// available, while this v1 cursor never discloses an unusable continuation beyond the cap.
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

/// Qualify a bare ref (`main`) to `refs/heads/main`; a fully-qualified `refs/…` passes through.
fn qualify_ref(gitref: &str) -> String {
    if gitref.starts_with("refs/") {
        gitref.to_string()
    } else {
        format!("refs/heads/{gitref}")
    }
}

/// The bounded per-entry latest-commit history walk cap (R3.4 gate decision): entries not resolved
/// within this many commits render name-only (graceful degrade — never an N-walk-per-entry blow-up).
const LATEST_COMMIT_WALK_CAP: usize = 500;

/// Candidate repositories must be materialized before the permission list-object intersection.
/// Bound that tenant-local directory scan so a pathological partition cannot grow one request
/// without limit; normal response pagination happens after this leak-free authorization prefilter.
const REPO_SCAN_MAX_CANDIDATES: usize = 10_000;

/// Each visible repository is bounded independently before cross-repository aggregation begins.
const PR_LIST_PER_REPO_MAX_RECORDS: usize = PR_LIST_OFFSET_MAX;
const PR_LIST_PER_REPO_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Request/cell-wide cross-repository aggregation ceilings. They bound the final exact bucket set
/// even when every individual visible repository remains below its independent limit.
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

/// Boot recovery fails loud above this finite pending-command envelope so tenant backlog cannot
/// become an unbounded startup allocation.
const BOOT_RECOVERY_MAX_PENDING_MERGES: usize = 10_000;
const BOOT_RECOVERY_MAX_PENDING_MERGE_BYTES: usize = 64 * 1024 * 1024;
const BOOT_RECOVERY_MAX_RETAINED_OUTBOX_ROWS: usize = 100_000;
const BOOT_RECOVERY_MAX_RETAINED_OUTBOX_BYTES: usize = 256 * 1024 * 1024;

/// The inline-text cap for a blob view (R3.4). The ODB header is checked first: a larger object gets
/// a metadata-only download fallback and is never inflated merely to build the interactive page.
const BLOB_INLINE_CAP: usize = 512 * 1024;

/// README markdown shares the JSON response budget with tree metadata and is checked at the ODB
/// header before inflation. Oversized or binary README files simply omit the optional preview.
const README_MAX_BYTES: usize = 512 * 1024;

/// Match the web gateway's raw-response ceiling so the Edge rejects from the ODB header before
/// inflating a file the next hop must reject anyway.
const RAW_BLOB_MAX_BYTES: usize = 64 * 1024 * 1024;

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

/// The short oid (first 12 chars) — the browse log/tree short form.
fn short_oid12(oid: &str) -> String {
    oid.chars().take(12).collect()
}

/// Fork import failures can contain repository paths or low-level libgit2 diagnostics. Those are
/// useful only inside the storage boundary and must never be reflected through HTTP/MCP denial
/// text. The public mutation boundary exposes one stable, non-oracular failure.
fn sanitize_fork_import_error(_error: DurableError) -> DurableError {
    DurableError::Git("fork commit import could not be completed".into())
}

/// A compact latest-commit projection for a tree row / the repo-home latest-commit bar (R3.4).
fn commit_brief_json(m: &CommitMeta) -> Value {
    json!({
        "short_oid": short_oid12(&m.oid),
        "oid": m.oid,
        "summary": m.summary,
        "author": m.author_name,
        "committed_at": m.time,
    })
}

/// Project tree entries to JSON (shared by the repo home + the tree route — the gate's "share the
/// projection"). Each entry carries its basename `name`, its full `path` (for the link), `is_dir`, an
/// optional blob `size`, and the bounded-walk `latest_commit` when resolved.
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

/// Strip a filename to a safe `Content-Disposition` token: keep the basename's safe chars, drop quotes
/// / control / path separators (defense against header injection + path leakage in the attachment name).
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

/// Map the durable raw [`CommitMeta`] to the [`CommitRow`] ViewModel (the author is the GIT-1 pseudonym).
fn commit_row(m: CommitMeta) -> CommitRow {
    CommitRow {
        oid: m.oid,
        summary: m.summary,
        author: m.author_name,
        committed_at: m.time,
        parents: m.parents,
    }
}

/// Map the durable raw [`CommitDetail`] to the [`CommitDiff`] ViewModel.
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

/// The per-file rendered-line cap for the PR diff (over-cap files carry `truncated == true` + an
/// "Expand all" refetch). Generous for dogfood-scale reviews; the wire never dumps an unbounded file.
const PR_DIFF_PER_FILE_LINE_CAP: usize = 4000;

/// Map one durable [`myelin_git::durable::DiffLineDelta`] to the [`PrDiffLine`] VM.
fn pr_diff_line(l: myelin_git::durable::DiffLineDelta) -> PrDiffLine {
    PrDiffLine {
        origin: l.origin,
        content: l.content,
        old_no: l.old_no,
        new_no: l.new_no,
    }
}

/// Map the raw [`PrDiff`] to the [`PrDiffVM`], paging FILES (MR-014) at `offset`/`limit`. The whole
/// diff is computed in one libgit2 pass; the RESPONSE is paged (dogfood scale — a genuinely lazy
/// per-file compute is a named follow-on). `restricted_files` is 0 under the repo-level `Pull` guard.
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
        // A traversal-rejected slug / malformed body (e.g. R3.1 open-PR with no `title`) surfaces as a
        // clean 400 (never a silent wrong path, never a 500 for a client input error).
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
        // A capability-scoped refusal (e.g. a non-CI-producer reporting checks) is a fail-closed 403.
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
mod http;
pub use check_projection::GitDatabaseProviders;
pub use http::register_git_durable;
use http::{cross_pr_before_key, mint_pr_list_cursors, BlobViewOptions, RawResponseOptions};
