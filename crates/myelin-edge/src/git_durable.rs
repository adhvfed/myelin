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

use crate::catalogue::{page_envelope, Handler, HandlerCtx};
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
use myelin_git::api::{http_catalogue, Method as GitMethod};
use myelin_git::check_status::GitOid;
use myelin_git::core::{Oid as CoreOid, RepoLoc};
use myelin_git::durable::{
    BlobPathLookup, CommitDetail, CommitMeta, FileLinesLookup, PrDiff, TreePathLookup,
    FILE_LINES_MAX_RANGE,
};
use myelin_git::durable::{DurableError, DurableGitRepo, DurableGitStore};
use myelin_git::events::pseudonymized_event_principal;
use myelin_git::lifecycle::{
    BranchProtectionRuleset, PrState, PullRequest, ReviewState, ReviewVerdict,
};
use myelin_git::pg_pr_store::{PgPrStore, PrMutation, PrOperationId};
use myelin_git::pr_store::{
    effective_ruleset, evaluate_merge, merge_pr, BranchProtectionConfig, ChecksSummary,
    DurablePrStore, MergeAttempt, PrRecord, ReviewRecord,
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
use myelin_git::web::{
    CommitDiff, CommitRow, DiffFile, DiffLineView, PrDiffFile, PrDiffHunk, PrDiffLine, PrDiffVM,
    RepoHome, WebEditOutcome,
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
        provider: SubstrateProvider,
        kms: Arc<KmsEngine>,
        runtime: tokio::runtime::Handle,
        outbox: OutboxStore,
        minter: Arc<dyn IdMinter>,
    ) -> Result<DurableGitBackend, DurableError> {
        let root = root.into();
        Ok(DurableGitBackend {
            store: DurableGitStore::rooted(root.clone()),
            prs: DurablePrStore::rooted(root.clone()),
            pg_prs: Some(PgPrStore::new(provider, kms, runtime)?),
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
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        let records =
            myelin_git::reconcile::refs_from_outbox_scoped(&self.outbox, tenant, region, slug)?;
        myelin_git::reconcile::reconcile_refs(&repo, &records)
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
            Some(store) => store.list_pending_merges(scope)?,
            None => Vec::new(),
        };
        let mut repos =
            myelin_git::reconcile::repo_slugs_from_outbox_scoped(&self.outbox, tenant, region)?;
        repos.extend(pending.iter().map(|item| item.repo_slug.clone()));

        let mut report = GitBootRecoveryReport::default();
        for slug in &repos {
            let reconciled = self.reconcile_repo(tenant, region, slug)?;
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
                PR_LIST_MAX_RECORDS,
            ),
            None => self.prs.list_bounded(loc, PR_LIST_MAX_RECORDS),
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
        let entries = repo.tree_entries_at_ref("refs/heads/main")?;
        if entries.is_empty() {
            Ok(RepoHome::Empty {
                slug: full_slug,
                clone_url,
            })
        } else {
            let readme = match repo.read_blob_at_path_bounded(
                "refs/heads/main",
                "README.md",
                64 * 1024,
            )? {
                BlobPathLookup::Found { bytes, .. } => {
                    String::from_utf8_lossy(&bytes).chars().take(400).collect()
                }
                BlobPathLookup::TooLarge { .. }
                | BlobPathLookup::IsDir
                | BlobPathLookup::Missing => String::new(),
            };
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

    /// The RefsVM for the switcher (`GET /repos/{repo}/refs`) — branches + tags + default_branch, all
    /// permission-checked by the [`RepoObjectGuard`] (`Pull`) at the route. Reads the on-disk refdb.
    fn refs_json(&self, tenant: &str, region: &str, slug: &str) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        let view = repo.refs_view()?;
        let default_branch = view.default_branch.clone();
        let branches: Vec<Value> = view
            .branches
            .iter()
            .map(|(name, oid)| {
                json!({ "name": name, "oid": oid.0, "is_default": *name == default_branch })
            })
            .collect();
        let tags: Vec<Value> = view
            .tags
            .iter()
            .map(|(name, oid)| json!({ "name": name, "oid": oid.0 }))
            .collect();
        Ok(json!({ "branches": branches, "tags": tags, "default_branch": default_branch }))
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
        let refs = repo.refs_view()?;
        let default_branch = refs.default_branch.clone();
        let counts = json!({ "branches": refs.branches.len(), "tags": refs.tags.len() });
        let branch_ref = format!("refs/heads/{default_branch}");
        let entries = match repo.tree_at_path(&branch_ref, "")? {
            TreePathLookup::Dir(e) => e,
            _ => Vec::new(),
        };
        if entries.is_empty() {
            return Ok(json!({
                "state": "empty",
                "slug": full_slug,
                "clone_url": clone_url,
                "default_branch": default_branch,
                "counts": counts,
            }));
        }
        let readme = read_text_blob_bounded(&repo, &branch_ref, "README.md", README_MAX_BYTES)?;
        let latest = repo.commit_log(&branch_ref, 0, 1)?.0.into_iter().next();
        let per_entry = repo
            .latest_commits_in_dir(&branch_ref, "", LATEST_COMMIT_WALK_CAP)?;
        let entries_json = tree_entries_json(&entries, "", &per_entry);
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
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        match repo.tree_at_path(gitref, path)? {
            TreePathLookup::IsFile => {
                Ok(json!({ "redirect_to_blob": true, "ref": gitref, "path": path }))
            }
            TreePathLookup::Missing => Err(DurableError::NotFound(format!(
                "no such path `{path}` at `{gitref}`"
            ))),
            TreePathLookup::Dir(entries) => {
                let base = path.trim_matches('/');
                let per_entry = repo
                    .latest_commits_in_dir(gitref, base, LATEST_COMMIT_WALK_CAP)?;
                let entries_json = tree_entries_json(&entries, base, &per_entry);
                // A subtree README renders too (same read-path); binary/absent → no readme.
                let readme_path = if base.is_empty() {
                    "README.md".to_string()
                } else {
                    format!("{base}/README.md")
                };
                let readme =
                    read_text_blob_bounded(&repo, gitref, &readme_path, README_MAX_BYTES)?;
                Ok(json!({
                    "ref": gitref,
                    "path": base,
                    "entries": entries_json,
                    "readme": readme,
                }))
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
    // This realises `compose_pr_list_query` / `PR_LIST_PERMISSION`'s leak-free *semantics* over the
    // on-disk JSON PR store. The SQL composer in `list_filter.rs` targets a `pr` table (the PG home,
    // GT-003b) that this store does not yet materialise, so it is not the literal execution path here
    // — the reduction to the repo `pull` prefilter is (named floor; identical leak-free guarantee).

    /// Enrich every PR under one repo for the list: read the repo-owned branch-protection config
    /// ONCE (no N+1 — the effective ruleset per PR is a pure function of that config + the PR's
    /// base_ref), roll up each PR's checks summary against its effective required set, and mark the
    /// viewer's requested-reviewer status. On a config-READ error the whole repo's rows fail static
    /// (`Unavailable`) rather than dropping PRs (a checks-projection hiccup must not hide PRs).
    fn enrich_prs(
        &self,
        loc: &RepoLoc,
        principal: &Principal,
        viewer_pseudonym: &str,
        repo_slug: Option<&str>,
    ) -> Result<Vec<EnrichedPr>, DurableError> {
        let records = self.pr_list(loc, principal)?;
        // ONE config read for the whole repo (fail static on error — see above).
        // A read failure degrades every row's summary to Unavailable below.
        let config = self.prs.get_protection(loc).ok();
        let config_readable = config.is_some();
        let config = config.flatten();
        Ok(records
            .into_iter()
            .map(|rec| {
                let summary = if config_readable {
                    let ruleset = effective_ruleset(config.as_ref(), &rec.base_ref);
                    rec.checks_summary(&ruleset)
                } else {
                    ChecksSummary::unavailable()
                };
                let you_requested = rec.is_review_requested_of(viewer_pseudonym);
                EnrichedPr {
                    rec,
                    summary,
                    you_requested,
                    repo_slug: repo_slug.map(str::to_string),
                }
            })
            .collect())
    }

    /// **The per-repo PR list (R3.1).** Every PR in the repo (the caller has already cleared the
    /// `Pull` object guard, so all are `view`-able), enriched with the checks rollup + the viewer's
    /// review status. The handler applies the `state`/`sort`/cursor + computes tab/sidebar counts
    /// over THIS (already leak-free) set.
    fn list_prs_for_repo(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        principal: &Principal,
    ) -> Result<Vec<EnrichedPr>, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.store.open_repo(&loc)?; // 404 if the repo is absent (never a phantom empty list).
        self.enrich_prs(&loc, principal, &Self::pseudonym(tenant, principal), None)
    }

    /// **The cross-repo PR front door (R3.1, single-cell for R3 — Q5).** Prefilter the on-disk repo
    /// candidates through the `visible_repos` `list_objects` seam FIRST (a forbidden repo never
    /// contributes a PR), then enrich the PRs under each visible repo, tagging each row with its repo
    /// slug. The bucket predicate (`yours` = authored-by-viewer; `needs-review` = viewer is a
    /// requested reviewer) is applied by the handler over this leak-free set.
    fn list_prs_cross(
        &self,
        tenant: &str,
        region: &str,
        principal: &Principal,
    ) -> Result<Vec<EnrichedPr>, DurableError> {
        let candidates = self.scan_repo_slugs(tenant, region)?;
        let visible = self
            .repo_authz
            .visible_repos(principal, tenant, region, &candidates);
        let viewer = Self::pseudonym(tenant, principal);
        let mut out = Vec::new();
        for slug in visible {
            let loc = Self::loc(tenant, region, &slug);
            self.store.open_repo(&loc)?;
            out.extend(self.enrich_prs(&loc, principal, &viewer, Some(&slug))?);
        }
        Ok(out)
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

    /// **R0.2 / DELTA N1 — Git's OWN recorded check facts for a pushed head commit.** Scans the repo's
    /// durable PR records for any whose `head_oid` equals the pushed oid and gathers the recorded green
    /// / fork-unendorsed / endorsed context names (the facts authorized producers stamped — the CI
    /// check-report op, the maintainer endorsement op). Returns `(green, fork_unendorsed, endorsed)` for
    /// the merge gate. ACYCLIC: it reads facts Git already holds; the wire push NEVER synchronously
    /// calls CI (EI-02 §3). The store-backed per-commit `check_status` projection is the GIT-P20
    /// follow-on; until then a commit's recorded facts live on its PR record(s).
    fn check_facts_for_head(
        &self,
        loc: &RepoLoc,
        head_oid: &str,
        principal: &Principal,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let mut green = Vec::new();
        let mut fork_unendorsed = Vec::new();
        let mut endorsed = Vec::new();
        if let Ok(prs) = self.pr_list(loc, principal) {
            for rec in prs.into_iter().filter(|r| r.head_oid == head_oid) {
                green.extend(rec.green_contexts);
                fork_unendorsed.extend(rec.fork_unendorsed_contexts);
                endorsed.extend(rec.endorsed_contexts);
            }
        }
        (green, fork_unendorsed, endorsed)
    }

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
            .list_refs()?
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
            .list_refs()
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
        for u in &updates {
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
        let migration = ObjectPromotion {
            repo: &repo,
            objects: &objects,
        };
        let outcome = ref_store.receive(&push, &migration, CrashPoint::None);
        let _ = std::fs::remove_dir_all(&qdir); // the host quarantine is discarded either way

        match outcome.map_err(|e| DurableError::Git(format!("ref-CAS: {e:?}")))? {
            PushOutcome::Accepted { .. } => {
                // F9 (R4.1 dogfood): the first push that lands a branch heals a dangling `init_bare`
                // HEAD (→ the default branch) so a fresh `git clone` checks out with NO "nonexistent
                // ref, unable to checkout" warning. Best-effort: the push already committed durably and
                // the read-side resolver (`refs_view`) remains the fallback, so a heal hiccup must never
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

    /// **The checks + merge-gate projection JSON (`PrChecksVM`).** `gate_admitted` is AUTHORITATIVE —
    /// the UI reflects it, never recomputes. Reused by the checks endpoint AND the merge-409 re-render
    /// (so a gate that flipped mid-dialog returns the fresh blocked card, never a stale one). `blocked`
    /// carries the honest, VERIFIED gate inputs: `changes_requested` (a live request-changes blocks
    /// unconditionally) and `approvals` (a real threshold; self-approval excluded) — both are what the
    /// R2 ruleset actually ingests, so the copy never implies a gate input that isn't real.
    fn pr_checks_json(&self, loc: &RepoLoc, rec: &PrRecord) -> Result<Value, DurableError> {
        let ruleset = self.prs.effective_ruleset_for(loc, &rec.base_ref)?;
        let eval = evaluate_merge(&ruleset, rec).map_err(|e| DurableError::Git(e.to_string()))?;
        let has_blocking_review = rec.reviews.iter().any(|r| {
            matches!(
                r.state,
                ReviewState::Submitted(ReviewVerdict::RequestChanges)
            )
        });
        let counting_approvals = rec
            .reviews
            .iter()
            .filter(|r| {
                matches!(r.state, ReviewState::Submitted(ReviewVerdict::Approve))
                    && r.reviewer_pseudonym != rec.author_pseudonym
            })
            .count() as u32;
        Ok(json!({
            "required_contexts": ruleset.required_contexts,
            "required_approvals": ruleset.required_approvals,
            "green_contexts": rec.green_contexts,
            "endorsed_contexts": rec.endorsed_contexts,
            // The X-1 fork-trust surface: contexts that passed on an UNTRUSTED FORK run and are
            // recorded-but-neutral until a maintainer endorses them (the badge the UI renders).
            "fork_unendorsed_contexts": rec.fork_unendorsed_contexts,
            "gate_admitted": eval.admitted(),
            // The VERIFIED review-gate inputs (R2 `evaluate_ruleset`): the UI renders these as
            // blocked-reason copy WITHOUT inventing a gate input that isn't real.
            "changes_requested": has_blocking_review,
            "current_approvals": counting_approvals,
            "durable": true,
        }))
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

/// Parse the `?state=` tab filter to the set of PR-state tokens it selects. `open` (the default)
/// covers both `open` and `draft` (a draft is an open-in-progress PR — the sketch lists it under
/// Open); `merged`/`closed` are exact; `all` selects everything. An unknown value falls back to
/// `open` (never an empty list on a typo).
fn state_filter(state: Option<&str>) -> &'static [&'static str] {
    match state.unwrap_or("open") {
        "merged" => &["merged"],
        "closed" => &["closed"],
        "all" => &["draft", "open", "merged", "closed"],
        _ => &["draft", "open"],
    }
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

/// Counts and bucket badges require the full already-authorized PR set. Cap that set at the storage
/// query itself so exact list semantics cannot turn into an unbounded filesystem/SQL materialization.
const PR_LIST_MAX_RECORDS: usize = 10_000;

/// The inline-text cap for a blob view (R3.4). The ODB header is checked first: a larger object gets
/// a metadata-only download fallback and is never inflated merely to build the interactive page.
const BLOB_INLINE_CAP: usize = 512 * 1024;

/// README markdown shares the JSON response budget with tree metadata and is checked at the ODB
/// header before inflation. Oversized or binary README files simply omit the optional preview.
const README_MAX_BYTES: usize = 512 * 1024;

/// Match the web gateway's raw-response ceiling so the Edge rejects from the ODB header before
/// inflating a file the next hop must reject anyway.
const RAW_BLOB_MAX_BYTES: usize = 64 * 1024 * 1024;

fn read_text_blob_bounded(
    repo: &DurableGitRepo,
    gitref: &str,
    path: &str,
    maximum_bytes: usize,
) -> Result<Option<String>, DurableError> {
    match repo.read_blob_at_path_bounded(gitref, path, maximum_bytes)? {
        BlobPathLookup::Found { bytes, is_binary: false, .. } => {
            Ok(Some(String::from_utf8_lossy(&bytes).to_string()))
        }
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
        DurableError::Git(m) if m.starts_with("browse response limit exceeded:") => {
            EdgeError::PayloadTooLarge("repository view exceeds the interactive browse limit".into())
        }
        DurableError::Git(m) if m.starts_with("pr diff computation limit exceeded:") => {
            EdgeError::PayloadTooLarge("pull request diff exceeds the interactive file limit".into())
        }
        DurableError::Git(m) if m.starts_with("commit diff computation limit exceeded:") => {
            EdgeError::PayloadTooLarge("commit diff exceeds the interactive content limit".into())
        }
        DurableError::Git(m) if m.starts_with("pull request list limit exceeded:") => {
            EdgeError::PayloadTooLarge("pull request list exceeds the interactive record limit".into())
        }
        DurableError::Git(m) if m.starts_with("pull request record limit exceeded:") => {
            EdgeError::PayloadTooLarge("pull request record exceeds the storage limit".into())
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

struct DRepoList {
    be: Arc<DurableGitBackend>,
}
impl Handler for DRepoList {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        // R2.1: the LIST endpoint is prefiltered to the principal's `pull`-visible set (the
        // `list_objects` seam) BEFORE pagination — an un-granted repo's existence is never leaked.
        let offset = ctx
            .page
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = ctx.page.limit;
        let (page, has_more) = self
            .be
            .list_repos_visible(
                tenant_of(ctx),
                region_of(ctx),
                ctx.principal,
                offset,
                limit,
            )
            .map_err(map_durable_err)?;
        let items: Vec<Value> = page.iter().map(|repo| repo.to_json()).collect();
        let next = if has_more {
            Some(offset.saturating_add(limit).to_string())
        } else {
            None
        };
        Ok(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next, limit),
        ))
    }
}

struct DRepoCreate {
    be: Arc<DurableGitBackend>,
}
impl Handler for DRepoCreate {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
        let slug = body
            .get("slug")
            .or_else(|| body.get("name"))
            .and_then(Value::as_str)
            .ok_or_else(|| EdgeError::BadRequest("create-repo body missing `slug`".into()))?;
        // R2.1a: the CREATE-through-the-edge path writes the creator→admin bootstrap grant (via the
        // injected RepoBootstrapGrants seam) before the bare repo lands — under the deny-by-default
        // CheckBackedRepoAuthorizer the creator can immediately clone/push its fresh repo.
        let created = self
            .be
            .create_repo_as(tenant_of(ctx), region_of(ctx), slug, ctx.principal)
            .map_err(map_durable_err)?;
        if !created {
            return Err(EdgeError::Conflict(format!("repo `{slug}` already exists")));
        }
        Ok(EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.repo.create", "slug": slug }, "durable": true }),
        ))
    }
}

struct DRepoHome {
    be: Arc<DurableGitBackend>,
}
impl Handler for DRepoHome {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        // R3.4: the enriched repo-home VM (default_branch, full README, latest_commit, per-entry
        // bounded-walk activity, branch/tag counts, name-carrying entries).
        let home = self
            .be
            .repo_home_json(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?)
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &home))
    }
}

struct DCommitLog {
    be: Arc<DurableGitBackend>,
}
impl Handler for DCommitLog {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let offset = ctx
            .page
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = ctx.page.limit;
        let (rows, has_more) = self
            .be
            .commit_log(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                param(ctx, "ref")?,
                offset,
                limit,
            )
            .map_err(map_durable_err)?;
        let items: Vec<Value> = rows.iter().map(CommitRow::to_json).collect();
        let next = if has_more {
            Some(offset.saturating_add(limit).to_string())
        } else {
            None
        };
        // R3.4: bidirectional paging + honest position (range + page, NO fabricated total — the gate
        // decision). `prev_cursor` is the "Newer" link (null on the first page); `range`/`offset` drive
        // the "Commits 31–60 · Seite 2" readout without a costly total commit count.
        let prev = if offset > 0 {
            Some(offset.saturating_sub(limit).to_string())
        } else {
            None
        };
        let range_from = if items.is_empty() { 0 } else { offset + 1 };
        let range_to = offset + items.len();
        let page = json!({
            "next_cursor": next,
            "prev_cursor": prev,
            "limit": limit,
            "offset": offset,
            "range": { "from": range_from, "to": range_to },
        });
        Ok(EdgeResponse::json(
            200,
            &json!({ "items": items, "page": page }),
        ))
    }
}

struct DCommitDiff {
    be: Arc<DurableGitBackend>,
}
impl Handler for DCommitDiff {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let diff = self
            .be
            .commit_diff(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                param(ctx, "oid")?,
            )
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such commit".into()))?;
        Ok(EdgeResponse::json(200, &diff.to_json()))
    }
}

struct DBlobView {
    be: Arc<DurableGitBackend>,
}
impl Handler for DBlobView {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        // R3.4: nested `{...path}` + binary/size/truncation + gateway-proxied raw/download URLs.
        let vm = self
            .be
            .blob_json(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                param(ctx, "ref")?,
                param(ctx, "path")?,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &vm))
    }
}

struct DRefs {
    be: Arc<DurableGitBackend>,
}
impl Handler for DRefs {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let vm = self
            .be
            .refs_json(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?)
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &vm))
    }
}

struct DTree {
    be: Arc<DurableGitBackend>,
}
impl Handler for DTree {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        // The catch-all `{...path}` binds the whole nested path (empty at the tree root).
        let path = ctx.params.get("path").map(String::as_str).unwrap_or("");
        let vm = self
            .be
            .tree_json(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                param(ctx, "ref")?,
                path,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &vm))
    }
}

/// Raw/download byte-serving (R3.4). `attachment` is fixed at registration (raw = inline, download =
/// attachment) so the disposition is not client-controlled.
struct DRawFile {
    be: Arc<DurableGitBackend>,
    attachment: bool,
}

#[derive(Clone, Copy)]
struct RawResponseOptions {
    attachment: bool,
    maximum_bytes: usize,
}

#[derive(Clone, Copy)]
struct BlobViewOptions {
    maximum_preview_bytes: usize,
    maximum_transfer_bytes: usize,
}
impl Handler for DRawFile {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let path = ctx.params.get("path").map(String::as_str).unwrap_or("");
        self.be.raw_response(
            tenant_of(ctx),
            region_of(ctx),
            param(ctx, "repo")?,
            param(ctx, "ref")?,
            path,
            self.attachment,
        )
    }
}

struct DWebEditCommit {
    be: Arc<DurableGitBackend>,
}
impl Handler for DWebEditCommit {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = ctx.request.json_body()?;
        let expected_base = body
            .get("base_oid")
            .and_then(Value::as_str)
            .ok_or_else(|| EdgeError::BadRequest("commit body missing `base_oid`".into()))?;
        let contents = body
            .get("contents")
            .and_then(Value::as_str)
            .ok_or_else(|| EdgeError::BadRequest("commit body missing `contents`".into()))?;
        let outcome = self
            .be
            .web_edit_commit(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                ),
                param(ctx, "ref")?,
                param(ctx, "path")?,
                expected_base,
                contents,
            )
            .map_err(map_durable_err)?;
        match outcome {
            WebEditOutcome::Denied => Err(EdgeError::Forbidden("no write permission for this ref".into())),
            WebEditOutcome::StaleBase { .. } => Err(EdgeError::Conflict(
                "the file changed since you opened it — refused so nothing is silently overwritten \
                 (GF-6: no 3-way editor in v1)"
                    .into(),
            )),
            committed @ WebEditOutcome::Committed { .. } => Ok(EdgeResponse::json(
                200,
                &json!({ "applied": committed.to_json(), "durable": true }),
            )),
        }
    }
}

struct DOpenPr {
    be: Arc<DurableGitBackend>,
}
impl Handler for DOpenPr {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
        let operation_id = self.be.request_operation_id(ctx.request)?;
        let rec = self
            .be
            .open_pr_with_operation(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                &body,
                ctx.principal,
                &operation_id,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.pr.open", "pr": DurableGitBackend::pr_json(&rec) }, "durable": true }),
        ))
    }
}

struct DPrOverview {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrOverview {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let loc = DurableGitBackend::loc(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?);
        let rec = self
            .be
            .pr_get(&loc, num_param(ctx, "n")?, ctx.principal)
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such pull request".into()))?;
        let mut vm = DurableGitBackend::pr_json(&rec);
        // R3.3 N1 — the commits-in-PR count (the tab badge) enriches the base record. A read failure
        // degrades to `null` (the badge simply omits the count — never a fabricated number).
        if let Some(obj) = vm.as_object_mut() {
            match self.be.commits_in_pr_count(&loc, &rec) {
                Some((count, has_more)) => {
                    obj.insert("commits_count".into(), json!(count));
                    obj.insert("commits_count_capped".into(), json!(has_more));
                }
                None => {
                    obj.insert("commits_count".into(), Value::Null);
                }
            }
        }
        Ok(EdgeResponse::json(200, &vm))
    }
}

/// **GET …/prs/{n}/commits — the commits IN a PR (R3.3 N2).** Reachable from the head but not the base
/// (three-dot semantics via the reachability walk), newest-first, MR-014 envelope. `Pull`-guarded.
struct DPrCommits {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrCommits {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let loc = DurableGitBackend::loc(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?);
        let rec = self
            .be
            .pr_get(&loc, num_param(ctx, "n")?, ctx.principal)
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such pull request".into()))?;
        let repo = self.be.store.open_repo(&loc).map_err(map_durable_err)?;
        let (metas, has_more) = repo
            .commits_in_pr(&rec.base_ref, &rec.head_oid, ctx.page.limit.min(500))
            .map_err(map_durable_err)?;
        let items: Vec<Value> = metas.into_iter().map(|m| commit_row(m).to_json()).collect();
        let next = if has_more {
            Some("more".to_string())
        } else {
            None
        };
        Ok(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next, ctx.page.limit),
        ))
    }
}

/// **GET …/prs/{n}/diff?cursor=&view= — the PR three-dot diff (R3.2 · G-7 N1).** `Pull`-guarded
/// (read = PR view; 0-leak 404 on an absent PR). `?view=` is the layout hint the client owns (the
/// server is layout-agnostic); `cursor` pages FILES (MR-014). `restricted_files` is count-only.
struct DPrDiff {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrDiff {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let offset = ctx
            .page
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = ctx.page.limit;
        let vm = self
            .be
            .pr_diff(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(num_param(ctx, "n")?),
                offset,
                limit,
            )
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such pull request".into()))?;
        Ok(EdgeResponse::json(200, &vm.to_json()))
    }
}

/// **GET …/file-lines/{oid}?path=&start=&end= — expand-context (R3.2 · G-7 N2).** `Pull`-guarded
/// (the SAME object check as the blob route). Returns `{ lines: [...] }` (context lines at a blob
/// oid); an absent canonical oid → empty `lines`, while malformed or unbounded input fails with 400.
struct DFileLines {
    be: Arc<DurableGitBackend>,
}

struct FileLinesQuery {
    path: String,
    start: usize,
    end: usize,
}

const FILE_LINES_MAX_QUERY_BYTES: usize = 16 * 1024;

fn decode_form_query_value(raw: &str) -> Result<String, EdgeError> {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(EdgeError::BadRequest(
                    "file-lines query contains malformed percent encoding".into(),
                ));
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    let form_value = raw.replace('+', " ");
    percent_encoding::percent_decode_str(&form_value)
        .decode_utf8()
        .map(|value| value.into_owned())
        .map_err(|_| EdgeError::BadRequest("file-lines query is not valid UTF-8".into()))
}

fn parse_file_lines_query(query: &str) -> Result<FileLinesQuery, EdgeError> {
    if query.len() > FILE_LINES_MAX_QUERY_BYTES {
        return Err(EdgeError::BadRequest("file-lines query is too large".into()));
    }
    let mut path = None;
    let mut start = None;
    let mut end = None;
    for pair in query.split('&') {
        let (name, value) = pair
            .split_once('=')
            .ok_or_else(|| EdgeError::BadRequest("malformed file-lines query parameter".into()))?;
        let duplicate = |field: &str| {
            EdgeError::BadRequest(format!("duplicate file-lines query parameter `{field}`"))
        };
        match name {
            "path" => {
                if path.is_some() {
                    return Err(duplicate("path"));
                }
                let decoded = decode_form_query_value(value)?;
                if !valid_anchor_path(&decoded) {
                    return Err(EdgeError::BadRequest("file-lines path is invalid".into()));
                }
                path = Some(decoded);
            }
            "start" | "end" => {
                let slot = if name == "start" { &mut start } else { &mut end };
                if slot.is_some() {
                    return Err(duplicate(name));
                }
                let number = value.parse::<u64>().map_err(|_| {
                    EdgeError::BadRequest(format!(
                        "file-lines `{name}` must be a positive line number"
                    ))
                })?;
                if number == 0 || number > u32::MAX as u64 {
                    return Err(EdgeError::BadRequest(format!(
                        "file-lines `{name}` must be a positive line number"
                    )));
                }
                *slot = Some(number as usize);
            }
            "" => return Err(EdgeError::BadRequest("empty file-lines query parameter".into())),
            other => {
                return Err(EdgeError::BadRequest(format!(
                    "unknown file-lines query parameter `{other}`"
                )))
            }
        }
    }
    let path = path.ok_or_else(|| EdgeError::BadRequest("file-lines path is required".into()))?;
    let start = start.ok_or_else(|| EdgeError::BadRequest("file-lines start is required".into()))?;
    let end = end.ok_or_else(|| EdgeError::BadRequest("file-lines end is required".into()))?;
    if end < start || end - start + 1 > FILE_LINES_MAX_RANGE {
        return Err(EdgeError::BadRequest(format!(
            "file-lines range must be ordered and no larger than {FILE_LINES_MAX_RANGE} lines"
        )));
    }
    Ok(FileLinesQuery { path, start, end })
}

fn canonical_blob_oid(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Handler for DFileLines {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let FileLinesQuery { path, start, end } = parse_file_lines_query(&ctx.request.query)?;
        debug_assert!(!path.is_empty());
        let oid = param(ctx, "oid")?;
        if !canonical_blob_oid(oid) {
            return Err(EdgeError::BadRequest(
                "file-lines oid must be a canonical lowercase object id".into(),
            ));
        }
        let lookup = self
            .be
            .file_lines(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                oid,
                start,
                end,
            )
            .map_err(map_durable_err)?;
        let lines: Vec<PrDiffLine> = match lookup {
            FileLinesLookup::Found(lines) => lines.into_iter().map(pr_diff_line).collect(),
            FileLinesLookup::Binary | FileLinesLookup::Missing => Vec::new(),
            FileLinesLookup::TooLarge { maximum, .. } => {
                return Err(EdgeError::PayloadTooLarge(format!(
                    "file is too large for context expansion (maximum {maximum} bytes)"
                )))
            }
        };
        let items: Vec<Value> = lines.iter().map(PrDiffLine::to_json).collect();
        Ok(EdgeResponse::json(200, &json!({ "lines": items })))
    }
}

// ── R3.3 / R3.2 thread + review-batch handlers ──────────────────────────────────────────────────

/// GET …/prs/{n}/threads — the viewer-scoped conversation. `Pull`-guarded (read = PR view).
struct DPrThreads {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrThreads {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let vm = self
            .be
            .list_threads(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                ctx.principal,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &vm))
    }
}

/// POST …/prs/{n}/threads — open a new thread + first comment. `Push`-guarded.
struct DPrThreadCreate {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrThreadCreate {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = ctx.request.json_body()?;
        let vm = self
            .be
            .create_thread(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                &body,
                ctx.principal,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.pr.thread.create", "thread": vm }, "durable": true }),
        ))
    }
}

/// POST …/prs/{n}/threads/{tid}/comments — reply to a thread. `Push`-guarded.
struct DPrThreadComment {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrThreadComment {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = ctx.request.json_body()?;
        let vm = self
            .be
            .add_thread_comment(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(num_param(ctx, "n")?),
                param(ctx, "tid")?,
                &body,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.pr.comment.create", "comment": vm }, "durable": true }),
        ))
    }
}

/// POST …/prs/{n}/threads/{tid}/resolve — resolve/unresolve a thread. `Push`-guarded.
struct DPrThreadResolve {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrThreadResolve {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
        let vm = self
            .be
            .resolve_thread(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(num_param(ctx, "n")?),
                param(ctx, "tid")?,
                &body,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.pr.thread.resolve", "result": vm }, "durable": true }),
        ))
    }
}

/// POST …/prs/{n}/reviews/start — start a review batch (draft). `pull_request.review`-guarded.
struct DPrReviewStart {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrReviewStart {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let vm = self
            .be
            .start_review_batch(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                ctx.principal,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.pr.review.start", "review": vm }, "durable": true }),
        ))
    }
}

/// POST …/prs/{n}/reviews/{rid}/comments — add a pending comment to a draft batch.
/// `pull_request.review`-guarded.
struct DPrReviewComment {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrReviewComment {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = ctx.request.json_body()?;
        let vm = self
            .be
            .add_pending_comment(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(num_param(ctx, "n")?),
                param(ctx, "rid")?,
                &body,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.pr.review.comment", "comment": vm }, "durable": true }),
        ))
    }
}

/// POST …/prs/{n}/reviews/{rid}/submit — submit the batch (ONE event, R-BATCH-1).
/// `pull_request.review`-guarded.
struct DPrReviewSubmit {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrReviewSubmit {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
        let vm = self
            .be
            .submit_review_batch(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(num_param(ctx, "n")?),
                param(ctx, "rid")?,
                &body,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.pr.review.submit", "result": vm }, "durable": true }),
        ))
    }
}

/// DELETE …/prs/{n}/reviews/{rid} — discard a draft batch. `pull_request.review`-guarded.
struct DPrReviewDiscard {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrReviewDiscard {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let vm = self
            .be
            .discard_review_batch(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                param(ctx, "rid")?,
                ctx.principal,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.pr.review.discard", "result": vm }, "durable": true }),
        ))
    }
}

struct DPrChecks {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrChecks {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let loc = DurableGitBackend::loc(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?);
        let rec = self
            .be
            .pr_get(&loc, num_param(ctx, "n")?, ctx.principal)
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such pull request".into()))?;
        let vm = self
            .be
            .pr_checks_json(&loc, &rec)
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &vm))
    }
}

/// Build the R3.1 PR-list envelope: sort newest-first, paginate over an offset cursor (with BOTH
/// `next_cursor` and `prev_cursor` — the bidirectional pager, fixes ux-git #12), and attach `counts`
/// (computed over the ALREADY-leak-free `enriched` set — a forbidden PR never reached it, the anti-
/// oracle rule). `enriched` is the prefiltered+bucketed set; `counts` is the caller-supplied tally.
fn pr_list_envelope(mut enriched: Vec<EnrichedPr>, ctx: &HandlerCtx<'_>, counts: Value) -> Value {
    let sort = ctx.request.query_param("sort");
    // Newest-first. `sort=created` orders by the monotonic PR number (a create-order proxy — the
    // record has no created_at); the default `sort=updated` orders by the durable updated stamp,
    // tie-broken by number so the order is total + stable (cursor stability).
    match sort.as_deref() {
        Some("created") => enriched.sort_by_key(|item| std::cmp::Reverse(item.rec.number)),
        _ => enriched.sort_by(|a, b| {
            b.rec
                .updated_at
                .cmp(&a.rec.updated_at)
                .then(b.rec.number.cmp(&a.rec.number))
        }),
    }
    let total = enriched.len();
    let offset = ctx
        .page
        .cursor
        .as_deref()
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = ctx.page.limit;
    let items: Vec<Value> = enriched
        .iter()
        .skip(offset)
        .take(limit)
        .map(DurableGitBackend::pr_list_row_json)
        .collect();
    // saturating: `cursor` is attacker-supplied; usize::MAX must yield an empty page, never
    // an add-overflow panic (verifier finding, R3.1).
    let next_cursor = if offset.saturating_add(limit) < total {
        Some(offset.saturating_add(limit).to_string())
    } else {
        None
    };
    // `prev_cursor` is `None` at the head (the "Newer" control is aria-disabled, not removed).
    let prev_cursor = if offset > 0 {
        Some(offset.saturating_sub(limit).to_string())
    } else {
        None
    };
    json!({
        "items": items,
        "page": {
            "next_cursor": next_cursor,
            "prev_cursor": prev_cursor,
            "limit": limit,
            "offset": offset,
            "total": total,
        },
        "counts": counts,
    })
}

/// **`GET /v1/git/repos/{repo}/prs` — the per-repo PR list (R3.1).** Registered through the
/// [`RepoObjectGuard`] `Pull` check (the leak-free prefilter: `pull_request.view = parent_repo->pull`
/// — a viewer who cannot pull gets the 0-leak 404 "no access" state; once past it every PR is
/// view-able). The `?state=` tab filters the returned rows; `counts` (open/merged/closed/all + the
/// sidebar's yours/needs-review) are computed over the FULL leak-free set so a tab badge never leaks.
struct DRepoPrList {
    be: Arc<DurableGitBackend>,
}
impl Handler for DRepoPrList {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let all = self
            .be
            .list_prs_for_repo(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                ctx.principal,
            )
            .map_err(map_durable_err)?;
        let viewer = DurableGitBackend::pseudonym(tenant_of(ctx), ctx.principal);
        // Counts over the FULL (already leak-free) set — never a post-filtered subset.
        let count = |pred: &dyn Fn(&EnrichedPr) -> bool| all.iter().filter(|e| pred(e)).count();
        let counts = json!({
            "open": count(&|e| matches!(e.rec.state, PrState::Open | PrState::Draft)),
            "merged": count(&|e| matches!(e.rec.state, PrState::Merged)),
            "closed": count(&|e| matches!(e.rec.state, PrState::Closed)),
            "all": all.len(),
            "yours": count(&|e| e.rec.author_pseudonym == viewer),
            "needs_review": count(&|e| e.you_requested),
        });
        // The `?state=` tab filter (leak-free: it narrows an already-authorised set).
        let wanted = state_filter(ctx.request.query_param("state").as_deref());
        let filtered: Vec<EnrichedPr> = all
            .into_iter()
            .filter(|e| wanted.contains(&DurableGitBackend::pr_state_token(e.rec.state)))
            .collect();
        Ok(EdgeResponse::json(
            200,
            &pr_list_envelope(filtered, ctx, counts),
        ))
    }
}

/// **`GET /v1/git/prs?bucket=needs-review|yours` — the cross-repo front door (R3.1, single-cell).**
/// NOT object-guarded (no `{repo}` segment): the prefilter is [`DurableGitBackend::list_prs_cross`]'s
/// `visible_repos` seam (a forbidden repo's PRs never enter the set). The `bucket` predicate then
/// selects `yours` (authored-by-viewer) or `needs-review` (viewer is a requested reviewer) — the
/// default is `needs-review` (the attention job). Each row carries its `repo` slug (the repo chip).
struct DMyPrs {
    be: Arc<DurableGitBackend>,
}
impl Handler for DMyPrs {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let all = self
            .be
            .list_prs_cross(tenant_of(ctx), region_of(ctx), ctx.principal)
            .map_err(map_durable_err)?;
        let viewer = DurableGitBackend::pseudonym(tenant_of(ctx), ctx.principal);
        let bucket = ctx.request.query_param("bucket");
        let in_bucket = |e: &EnrichedPr| match bucket.as_deref().unwrap_or("needs-review") {
            "yours" => e.rec.author_pseudonym == viewer,
            // "needs-review" (default): the viewer is a requested reviewer AND not the author (you
            // do not review your own PR). Closed/merged PRs never need review.
            _ => {
                e.you_requested
                    && e.rec.author_pseudonym != viewer
                    && matches!(e.rec.state, PrState::Open | PrState::Draft)
            }
        };
        let bucketed: Vec<EnrichedPr> = all.into_iter().filter(|e| in_bucket(e)).collect();
        let counts = json!({ "bucket": bucketed.len() });
        Ok(EdgeResponse::json(
            200,
            &pr_list_envelope(bucketed, ctx, counts),
        ))
    }
}

struct DPrReview {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrReview {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = ctx.request.json_body()?;
        let verdict = body
            .get("verdict")
            .and_then(Value::as_str)
            .ok_or_else(|| EdgeError::BadRequest("review body missing `verdict`".into()))?;
        let operation_id = self.be.request_operation_id(ctx.request)?;
        let rec = self
            .be
            .submit_review_with_operation(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                verdict,
                ctx.principal,
                &operation_id,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.pr.review", "verdict": verdict, "reviews": rec.reviews.len() }, "durable": true }),
        ))
    }
}

struct DEndorse {
    be: Arc<DurableGitBackend>,
}
impl Handler for DEndorse {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
        let operation_id = self.be.request_operation_id(ctx.request)?;
        let rec = self
            .be
            .endorse_fork_ci_with_operation(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                &body,
                ctx.principal,
                &operation_id,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.pr.endorse_fork_ci", "endorsed": rec.endorsed_contexts }, "durable": true }),
        ))
    }
}

struct DMerge {
    be: Arc<DurableGitBackend>,
}
impl Handler for DMerge {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let operation_id = self.be.request_operation_id(ctx.request)?;
        let attempt = self
            .be
            .merge_with_operation(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                ctx.principal,
                &operation_id,
            )
            .map_err(map_durable_err)?;
        match attempt {
            MergeAttempt::Merged {
                base_ref,
                new_oid,
                update_seq,
            } => Ok(EdgeResponse::json(
                200,
                &json!({
                    "applied": { "action": "git.pr.merge", "merged": true, "base_ref": base_ref,
                                 "new_oid": new_oid, "update_seq": update_seq },
                    "durable": true,
                }),
            )),
            // The merge gate BLOCKED — a loud, honest refusal (no ref advanced). N6: a 409 carrying the
            // FRESH re-rendered `checks` (the gate may have flipped mid-dialog) so the UI re-renders the
            // blocked card WITHOUT a second round-trip and NEVER merges on stale state. `gate_admitted`
            // stays authoritative — the UI reflects it, never recomputes.
            MergeAttempt::Blocked(_eval) => {
                let loc =
                    DurableGitBackend::loc(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?);
                let checks = self
                    .be
                    .pr_get(&loc, num_param(ctx, "n")?, ctx.principal)
                    .ok()
                    .flatten()
                    .and_then(|rec| self.be.pr_checks_json(&loc, &rec).ok());
                Ok(EdgeResponse::json(
                    409,
                    &json!({
                        "error": {
                            "code": "merge_blocked",
                            "message": "merge blocked by policy: branch protection requirements are unmet",
                        },
                        "checks": checks,
                        "durable": true,
                    }),
                ))
            }
            MergeAttempt::RefRefused(reason) => Err(EdgeError::Conflict(format!(
                "merge ref advance refused: {reason:?}"
            ))),
            // An arbitrary / non-existent / non-descendant head — refused, no ref advance (never advance
            // a protected ref to an arbitrary oid). 422: the merge target is unprocessable.
            MergeAttempt::InvalidHead(why) => {
                Err(EdgeError::BadRequest(format!("invalid merge head: {why}")))
            }
        }
    }
}

struct DSetBranchProtection {
    be: Arc<DurableGitBackend>,
}
impl Handler for DSetBranchProtection {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
        let n = self
            .be
            .set_branch_protection(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?, &body)
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.repo.branch_protection.set", "rulesets": n }, "durable": true }),
        ))
    }
}

struct DReportChecks {
    be: Arc<DurableGitBackend>,
}
impl Handler for DReportChecks {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = ctx.request.json_body()?;
        let operation_id = self.be.request_operation_id(ctx.request)?;
        let rec = self
            .be
            .report_checks_with_operation(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                ctx.principal,
                &body,
                &operation_id,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.checks.report", "green_contexts": rec.green_contexts }, "durable": true }),
        ))
    }
}

struct DCodeSearch;
impl Handler for DCodeSearch {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        // The ranked, ACL-pre-filtered code-search INDEX is the Search track; the durable front door
        // serves an empty, tenant-scoped page here (honest — never a faked hit).
        Ok(EdgeResponse::json(
            200,
            &page_envelope(json!([]), None, ctx.page.limit),
        ))
    }
}

/// **The R2.1 OBJECT-AUTHZ chokepoint for every object-addressed git JSON route — the registration-
/// declared object check (the platform forward pattern).** The gateway's action gate authorizes the
/// ACTION only; this guard is the OBJECT leg: it wraps the route's handler at REGISTRATION time with
/// the ONE frozen-fragment [`RepoPermission`] the route needs, and consults the injected
/// [`RepoAuthorizer`] on the `(principal, repo:<slug>, permission)` triple BEFORE the inner handler
/// parses a byte of the request body. Deny postures mirror the wire oracle convention exactly:
///
/// - a **`Pull`** denial is the 0-leak **404** (`repository not found` — repo existence is never
///   leaked to an un-granted in-tenant principal; identical to the absent-repo response);
/// - every other denial is a fail-closed **403** naming the missing grant class.
///
/// The repo is resolved from the validated `{repo}` path segment + the VERIFIED `ctx.scope`
/// (tenant-from-token, GIT-D8) — never a body/header field. A route with no `{repo}` param cannot
/// pass this guard (fail-closed 400), so a mis-registered object route is loud, never silently
/// action-only.
///
/// **Why this lives at the subsystem's registration and not a `GatewayBuilder::route_scoped`:** the
/// gateway holds no object-authorizer handle (main.rs injects the object seam into the git BACKEND),
/// the per-route permission varies across FOUR rungs (pull/push/protected_push/approve_untrusted_ci
/// — not a binary object spec), and the deny posture varies with it (0-leak 404 vs 403). Mirroring
/// R2.2's `sse_route`/`sse_route_scoped` registration-time contract, the enforced-by-construction
/// property lands HERE: [`register_git_durable`] wraps every object-addressed route through
/// [`guarded`], so a new git route either declares its object permission or visibly ships without
/// one at the single registration site. The next subsystem mounting object-addressed routes copies
/// this guard shape over its own object grammar (or generalises it into the gateway once two
/// subsystems share an object-authorizer seam there).
struct RepoObjectGuard {
    be: Arc<DurableGitBackend>,
    permission: RepoPermission,
    inner: Arc<dyn Handler>,
}

impl Handler for RepoObjectGuard {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let slug = param(ctx, "repo")?;
        let loc = RepoLoc::new(tenant_of(ctx), region_of(ctx), slug);
        if !self.be.repo_authorizer().authorize_repo_permission(
            ctx.principal,
            &loc,
            self.permission,
        ) {
            return Err(match self.permission {
                // The 0-leak read posture (the wire's `map_wire_err` convention): an un-granted
                // reader learns nothing an absent repo would not also say.
                RepoPermission::Pull => EdgeError::NotFound("repository not found".into()),
                RepoPermission::Push => {
                    EdgeError::Forbidden("no write grant for this repository".into())
                }
                RepoPermission::ProtectedPush => EdgeError::Forbidden(
                    "no admin (protected_push) grant for this repository".into(),
                ),
                RepoPermission::ApproveUntrustedCi => EdgeError::Forbidden(
                    "no fork-CI endorsement grant (approve_untrusted_ci) for this repository"
                        .into(),
                ),
            });
        }
        self.inner.handle(ctx)
    }
}

/// Wrap `inner` in the [`RepoObjectGuard`] for `permission` — the one-line registration-time
/// declaration every object-addressed git route goes through.
fn guarded(
    be: &Arc<DurableGitBackend>,
    permission: RepoPermission,
    inner: Arc<dyn Handler>,
) -> Arc<dyn Handler> {
    Arc::new(RepoObjectGuard {
        be: be.clone(),
        permission,
        inner,
    })
}

/// Temporary materialized form of the frozen
/// `pull_request.review = reviewer ∪ parent_repo->push` permission. Production PR rows already
/// carry requested-reviewer facts, but the matching Identity `pull_request` tuples are not yet
/// projected. Until they are, evaluate the same union from the two authoritative facts available
/// here: the live repo `Push` check or a requested-reviewer record on this exact PR. A failed PR
/// lookup denies without distinguishing absence from an authorization failure.
struct PrReviewGuard {
    be: Arc<DurableGitBackend>,
    inner: Arc<dyn Handler>,
}

impl Handler for PrReviewGuard {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let slug = param(ctx, "repo")?;
        let number = num_param(ctx, "n")?;
        if !self
            .be
            .authorize_pr_review(tenant_of(ctx), region_of(ctx), slug, number, ctx.principal)
        {
            return Err(EdgeError::Forbidden(
                "no review grant for this pull request".into(),
            ));
        }
        self.inner.handle(ctx)
    }
}

fn pr_review_guarded(be: &Arc<DurableGitBackend>, inner: Arc<dyn Handler>) -> Arc<dyn Handler> {
    Arc::new(PrReviewGuard {
        be: be.clone(),
        inner,
    })
}

/// **Register Git through the product edge over the DURABLE backend (GT-003).** Iterates Git's OWN
/// catalogue (anti-duplication — the route set is Git's, re-rooted under `/v1/git/...`) and binds the
/// durable handlers. The gateway owns auth/scope/IDOR/error/pagination; every write persists on the real
/// on-disk backend under `ctx.scope` (the verified tenant + region), the merge passes the gate, and the
/// resolver is traversal-safe.
///
/// **R2.1 — every object-addressed route is registered through the [`RepoObjectGuard`]** with the
/// frozen-fragment permission it needs (the object leg the action-only gateway gate cannot do). The
/// per-handler mapping (each reduces to a `repo:<slug>` check; PR routes reduce via the fragment's
/// ttu arms, the parent repo being the validated `{repo}` segment):
///
/// | route | permission | deny |
/// |---|---|---|
/// | repo home / commit log / commit diff / blob view / PR overview / PR checks | `Pull` | 0-leak 404 |
/// | web-edit commit / open-PR / CI check-report | `Push` | 403 |
/// | PR review / discussion writes | `pull_request.review` | 403 |
/// | endorse fork CI | `ApproveUntrustedCi` | 403 |
/// | merge (`pull_request.merge = parent_repo->protected_push`) | `ProtectedPush` | 403 |
/// | set branch protection | `ProtectedPush` | 403 |
///
/// NOT object-guarded (deliberate): `GET /repos` (the LIST — prefiltered leak-free inside
/// [`DRepoList`] via the `list_objects` seam, which is stronger than a single object check);
/// `POST /repos` (create — there is no repo OBJECT yet; the gateway's `git.repo.create` ACTION gate
/// authorizes it and the R2.1a creator→admin bootstrap grant makes the fresh repo owned — a
/// tenant-level "may create repos" object check needs a tenant/project object the frozen fragment
/// does not define, named for the fragment's next revision); `GET /search/code` (serves the empty
/// envelope; the ACL-prefiltered index is the Search track).
pub fn register_git_durable(mut b: GatewayBuilder, be: Arc<DurableGitBackend>) -> GatewayBuilder {
    use RepoPermission::{ApproveUntrustedCi, ProtectedPush, Pull, Push};
    for ep in http_catalogue() {
        // R3.4: the blob READ route is widened to a nested `{...path}` catch-all (the core routing
        // change vs the single-segment catalogue entry). The POST web-edit stays single-segment
        // (nested web-edit is the GT-004b composer follow-on, not this surface).
        let pattern = match (ep.method, ep.path) {
            (GitMethod::Get, "/api/git/repos/{repo}/blob/{ref}/{path}") => {
                reroot("/api/git/repos/{repo}/blob/{ref}/{...path}")
            }
            _ => reroot(ep.path),
        };
        let method = map_method(ep.method);
        let (handler, action): (Arc<dyn Handler>, &'static str) = match (ep.method, ep.path) {
            (GitMethod::Get, "/api/git/repos") => {
                // The LIST: prefiltered inside the handler (list_objects seam) — see above.
                (Arc::new(DRepoList { be: be.clone() }), "git.repos.list")
            }
            (GitMethod::Post, "/api/git/repos") => {
                // Create: action-gated + bootstrap-granted — no repo object exists yet (see above).
                (Arc::new(DRepoCreate { be: be.clone() }), "git.repo.create")
            }
            (GitMethod::Get, "/api/git/repos/{repo}/prs/{n}") => (
                guarded(&be, Pull, Arc::new(DPrOverview { be: be.clone() })),
                "git.pr.view",
            ),
            (GitMethod::Get, "/api/git/repos/{repo}/prs/{n}/checks") => (
                guarded(&be, Pull, Arc::new(DPrChecks { be: be.clone() })),
                "git.pr.checks",
            ),
            (GitMethod::Get, "/api/git/repos/{repo}/blob/{ref}/{path}") => (
                guarded(&be, Pull, Arc::new(DBlobView { be: be.clone() })),
                "git.blob.view",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/blob/{ref}/{path}") => (
                guarded(&be, Push, Arc::new(DWebEditCommit { be: be.clone() })),
                "git.blob.commit",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/prs") => (
                guarded(&be, Push, Arc::new(DOpenPr { be: be.clone() })),
                "git.pr.open",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/reviews") => (
                pr_review_guarded(&be, Arc::new(DPrReview { be: be.clone() })),
                "git.pr.review",
            ),
            // The X-1 endorsement: the DISTINCT approve_untrusted_ci relation (never collapsed to
            // write — endorsing an untrusted fork run is its own trust decision).
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/endorse-fork-ci") => (
                guarded(
                    &be,
                    ApproveUntrustedCi,
                    Arc::new(DEndorse { be: be.clone() }),
                ),
                "git.pr.endorse_fork_ci",
            ),
            // Merge: `pull_request.merge = parent_repo->protected_push` (§5-frozen) — the guard
            // checks the reduction on the parent repo (the validated `{repo}` segment). A push-only
            // writer does NOT clear this.
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/merge") => (
                guarded(&be, ProtectedPush, Arc::new(DMerge { be: be.clone() })),
                "git.pr.merge",
            ),
            // Repo-admin: set branch-protection policy — a DISTINCT authorize action AND the
            // admin-only protected_push OBJECT check (a non-admin writer is rejected here even if
            // action-granted).
            (GitMethod::Post, "/api/git/repos/{repo}/branch-protection") => (
                guarded(
                    &be,
                    ProtectedPush,
                    Arc::new(DSetBranchProtection { be: be.clone() }),
                ),
                "git.repo.branch_protection.set",
            ),
            // CI check-report — a DISTINCT authorize action (the producer is CI/M4; a PR author is not
            // granted it) + the per-repo write grant (an in-tenant CI producer stamps greens only on
            // repos it is granted on).
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/checks") => (
                guarded(&be, Push, Arc::new(DReportChecks { be: be.clone() })),
                "git.checks.report",
            ),
            (GitMethod::Get, "/api/git/search/code") => (Arc::new(DCodeSearch), "git.search.code"),
            (_, other) => (
                Arc::new(DCodeSearch),
                Box::leak(format!("git.unmapped:{other}").into_boxed_str()),
            ),
        };
        b = b.route(method, &pattern, action, handler);
    }
    // The GT-004 browse READ endpoints Git's catalogue doesn't expose yet — added here (reusing the
    // durable repo's libgit2 reads, never a git reimplementation), tenant-scoped exactly like the
    // catalogue routes (the gateway owns auth/scope/IDOR/error/pagination per route). All GET (reads),
    // all object-guarded on `Pull` (R2.1).
    let get = map_method(GitMethod::Get);
    let post = map_method(GitMethod::Post);
    // R3.1 — the per-repo PR LIST. Object-guarded on `Pull`: the leak-free prefilter is the frozen
    // `pull_request.view = parent_repo->pull` reduction (a viewer who cannot pull gets the 0-leak 404
    // "no access" state; once past it every PR in the repo is view-able, so counts never leak).
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/prs"),
        "git.prs.list",
        guarded(&be, Pull, Arc::new(DRepoPrList { be: be.clone() })),
    );
    // ── R3.3 N2 — commits IN a PR (the tab badge's full list). `Pull` (read = PR view). ──
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/prs/{n}/commits"),
        "git.pr.commits",
        guarded(&be, Pull, Arc::new(DPrCommits { be: be.clone() })),
    );
    // ── R3.2 · G-7 — the PR three-dot diff (N1) + expand-context (N2). Both `Pull` (read = PR view /
    //    the same object-check as the blob route). 0-leak 404 on deny; restricted files count-only. ──
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/prs/{n}/diff"),
        "git.pr.diff",
        guarded(&be, Pull, Arc::new(DPrDiff { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/file-lines/{oid}"),
        "git.file.lines",
        guarded(&be, Pull, Arc::new(DFileLines { be: be.clone() })),
    );
    // ── R3.3 / R3.2 — the thread + review-batch surface. READ = `Pull` (thread read = PR
    //    view); WRITE = `pull_request.review` (requested reviewer OR parent-repo Push). ──
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/prs/{n}/threads"),
        "git.pr.threads.list",
        guarded(&be, Pull, Arc::new(DPrThreads { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/threads"),
        "git.pr.thread.create",
        pr_review_guarded(&be, Arc::new(DPrThreadCreate { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/threads/{tid}/comments"),
        "git.pr.comment.create",
        pr_review_guarded(&be, Arc::new(DPrThreadComment { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/threads/{tid}/resolve"),
        "git.pr.thread.resolve",
        pr_review_guarded(&be, Arc::new(DPrThreadResolve { be: be.clone() })),
    );
    // The review-batch lifecycle (G-8). `/reviews/start` (not `POST /reviews`) preserves the existing
    // single-shot `POST /reviews` verdict op that feeds the merge gate — a named deviation from N5's
    // literal path. Discard is `POST …/discard` (the gateway git grammar is Get/Post only, no DELETE).
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/reviews/start"),
        "git.pr.review.start",
        pr_review_guarded(&be, Arc::new(DPrReviewStart { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/reviews/{rid}/comments"),
        "git.pr.review.comment",
        pr_review_guarded(&be, Arc::new(DPrReviewComment { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/reviews/{rid}/submit"),
        "git.pr.review.submit",
        pr_review_guarded(&be, Arc::new(DPrReviewSubmit { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/reviews/{rid}/discard"),
        "git.pr.review.discard",
        pr_review_guarded(&be, Arc::new(DPrReviewDiscard { be: be.clone() })),
    );
    // R3.1 — the CROSS-REPO PR front door (`/prs`). NOT object-guarded (no `{repo}`): the prefilter is
    // the `visible_repos` `list_objects` seam inside the handler (stronger than a single object check
    // — a forbidden repo's PRs never enter the set), so it registers action-gated only, like `/repos`.
    b = b.route(
        get,
        &reroot("/api/git/prs"),
        "git.prs.mine",
        Arc::new(DMyPrs { be: be.clone() }),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}"),
        "git.repo.view",
        guarded(&be, Pull, Arc::new(DRepoHome { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/commits/{ref}"),
        "git.commits.log",
        guarded(&be, Pull, Arc::new(DCommitLog { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/commit/{oid}"),
        "git.commit.diff",
        guarded(&be, Pull, Arc::new(DCommitDiff { be: be.clone() })),
    );
    // R3.4 repo-browsing completeness — the ref switcher + nested tree + raw/download read endpoints.
    // All GET reads, all object-guarded on `Pull` (0-leak 404 on deny — the un-granted-repo posture),
    // tenant-scoped via `ctx.scope` exactly like the catalogue routes.
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/refs"),
        "git.refs.list",
        guarded(&be, Pull, Arc::new(DRefs { be: be.clone() })),
    );
    // One catch-all handles BOTH the tree root (`/tree/{ref}`, empty `{...path}`) and nested paths.
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/tree/{ref}/{...path}"),
        "git.tree.view",
        guarded(&be, Pull, Arc::new(DTree { be: be.clone() })),
    );
    // Raw/download byte-serving — gateway-proxied, in-region, `Content-Disposition` set server-side
    // (BINDING: no public signed URLs). `raw` = inline, `download` = attachment.
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/raw/{ref}/{...path}"),
        "git.blob.raw",
        guarded(
            &be,
            Pull,
            Arc::new(DRawFile {
                be: be.clone(),
                attachment: false,
            }),
        ),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/download/{ref}/{...path}"),
        "git.blob.download",
        guarded(
            &be,
            Pull,
            Arc::new(DRawFile {
                be: be.clone(),
                attachment: true,
            }),
        ),
    );
    b
}

#[cfg(test)]
mod event_privacy_tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalStatus, RuntimeRef};
    use myelin_tenancy::Region as IdRegion;

    #[test]
    fn production_refstore_context_scrubs_all_raw_agent_identifiers() {
        let principal = Principal::new(
            myelin_tenancy::TenantId("acme".into()),
            IdRegion("fr-par".into()),
            PrincipalId("agent:raw@example.test".into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("runtime://raw-machine/session".into()),
                on_behalf_of: Some(PrincipalId("person@example.test".into())),
            },
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let first = DurableGitBackend::emit_ctx("acme", "fr-par", &principal);
        let second = DurableGitBackend::emit_ctx("acme", "fr-par", &principal);
        assert_eq!(first.actor, second.actor, "the tenant pseudonym is stable");
        assert_ne!(first.actor.0.principal_id, principal.principal_id);
        let serialized = serde_json::to_string(&first.actor).unwrap();
        for raw in [
            "agent:raw@example.test",
            "runtime://raw-machine/session",
            "person@example.test",
        ] {
            assert!(
                !serialized.contains(raw),
                "raw Agent identifier leaked: {raw}"
            );
        }

        let request = RepoActorContext::new("acme", "fr-par", "core", &principal).for_pr(42);
        let debug = format!("{request:?}");
        assert!(!debug.contains("agent:raw@example.test"));
        assert!(debug.contains("principal: \"<redacted>\""));
    }

    #[test]
    fn fork_import_diagnostics_are_sanitized_before_public_boundaries() {
        let raw = DurableError::Git(
            "failed to index /srv/tenants/acme/private-fork.git: object secretdeadbeef".into(),
        );
        let public = sanitize_fork_import_error(raw).to_string();
        assert_eq!(
            public,
            "durable git op failed: fork commit import could not be completed"
        );
        assert!(!public.contains("/srv/tenants"));
        assert!(!public.contains("secretdeadbeef"));
    }
}

#[cfg(test)]
mod create_compensation_tests {
    //! **R2.1a-followup, defect #7 — the create-fail compensation path.** `create_repo_as` writes
    //! the creator→admin grant BEFORE the on-disk `git init`; if the on-disk create then fails, the
    //! error arm MUST issue a compensating [`RepoBootstrapGrants::revoke_creator`] so no orphan admin
    //! grant survives on a repo that does not exist (the cross-user hole: orphan grant + slug reuse
    //! by another principal). These tests pin the ordering and the compensation with a recording
    //! bootstrap double, forcing the on-disk create to fail deterministically.

    use super::*;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_tenancy::{Region as IdRegion, TenantId};
    use std::sync::Mutex;

    fn principal(id: &str, tenant: &str) -> Principal {
        Principal::new(
            TenantId(tenant.into()),
            IdRegion("fr-par".into()),
            PrincipalId(id.into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "myelin-compensation-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    /// A recording bootstrap double: counts grant/revoke calls and records the (creator, slug) each
    /// was invoked with, so a test asserts the compensating remove fired with the EXACT tuple key.
    #[derive(Default)]
    struct RecordingBootstrap {
        grants: Mutex<Vec<(String, String)>>,
        revokes: Mutex<Vec<(String, String)>>,
        /// When set, `revoke_creator` returns Err (the compensation-ALSO-fails path).
        revoke_fails: bool,
    }

    impl RepoBootstrapGrants for RecordingBootstrap {
        fn grant_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String> {
            self.grants
                .lock()
                .unwrap()
                .push((creator.principal_id.0.clone(), repo.repo.clone()));
            Ok(())
        }
        fn revoke_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String> {
            self.revokes
                .lock()
                .unwrap()
                .push((creator.principal_id.0.clone(), repo.repo.clone()));
            if self.revoke_fails {
                Err("simulated compensation transport failure".into())
            } else {
                Ok(())
            }
        }
    }

    /// Make the on-disk create fail deterministically: plant a regular FILE where the
    /// `<tenant>/<region>` directory would go, so `create_dir_all(parent)` inside
    /// `DurableGitStore::create_repo` errors — the grant has already committed at that point.
    fn block_on_disk_create(root: &std::path::Path, tenant: &str, region: &str) {
        let tenant_dir = root.join(tenant);
        std::fs::create_dir_all(&tenant_dir).unwrap();
        std::fs::write(tenant_dir.join(region), b"not-a-directory").unwrap();
    }

    /// **The happy path still grants and never compensates** (a successful create leaves the grant).
    #[test]
    fn successful_create_grants_and_does_not_revoke() {
        let root = temp_root("ok");
        let boot = Arc::new(RecordingBootstrap::default());
        let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_bootstrap(boot.clone());
        let creator = principal("svc:creator", "acme");

        let created = be
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .expect("create succeeds");
        assert!(created);
        assert_eq!(boot.grants.lock().unwrap().len(), 1, "granted once");
        assert!(
            boot.revokes.lock().unwrap().is_empty(),
            "no compensation on the happy path"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// **Defect #7 — an on-disk create failure AFTER the grant committed issues the compensating
    /// remove** with the EXACT (creator, slug) the grant used, and surfaces the create error.
    #[test]
    fn create_failure_after_grant_compensates_the_orphan() {
        let root = temp_root("fail");
        block_on_disk_create(&root, "acme", "fr-par");
        let boot = Arc::new(RecordingBootstrap::default());
        let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_bootstrap(boot.clone());
        let creator = principal("svc:creator", "acme");

        let err = be
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .expect_err("the on-disk create must fail");
        // The grant fired, then the compensating remove fired with the SAME tuple key.
        assert_eq!(
            *boot.grants.lock().unwrap(),
            vec![("svc:creator".to_string(), "widgets".to_string())]
        );
        assert_eq!(
            *boot.revokes.lock().unwrap(),
            vec![("svc:creator".to_string(), "widgets".to_string())],
            "the compensating remove fired with the exact grant tuple"
        );
        // The surfaced error is the underlying create error (compensation succeeded → not the
        // orphan-known message).
        let msg = err.to_string();
        assert!(
            !msg.contains("ORPHANED"),
            "compensation succeeded, so no orphan-known error: {msg}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// **When the compensation ITSELF fails, the error names the ORPHANED grant loudly** (a known,
    /// logged orphan beats a silent one — the reconciler's cue).
    #[test]
    fn compensation_failure_surfaces_the_known_orphan_loudly() {
        let root = temp_root("double-fail");
        block_on_disk_create(&root, "acme", "fr-par");
        let boot = Arc::new(RecordingBootstrap {
            revoke_fails: true,
            ..Default::default()
        });
        let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_bootstrap(boot.clone());
        let creator = principal("svc:creator", "acme");

        let err = be
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .expect_err("create fails");
        assert_eq!(
            boot.revokes.lock().unwrap().len(),
            1,
            "compensation was attempted"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("ORPHANED") && msg.contains("compensation error"),
            "the doubly-failed path names the orphan loudly: {msg}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// **An existing repo short-circuits: neither grant nor revoke fires** (the conflict path, not a
    /// create — no bootstrap tuple churn on a `409`).
    #[test]
    fn existing_repo_neither_grants_nor_revokes() {
        let root = temp_root("exists");
        let boot = Arc::new(RecordingBootstrap::default());
        let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_bootstrap(boot.clone());
        let creator = principal("svc:creator", "acme");
        assert!(be
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .unwrap());
        // A second create of the same slug is a conflict (Ok(false)) — no second grant, no revoke.
        assert!(!be
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .unwrap());
        assert_eq!(
            boot.grants.lock().unwrap().len(),
            1,
            "granted only on the first create"
        );
        assert!(boot.revokes.lock().unwrap().is_empty(), "no compensation");
        std::fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod pr_list_tests {
    //! **R3.1 — the PR-list endpoints: leak-free prefilter, no-oracle counts, cursor stability.** The
    //! per-repo list rides the SAME `Pull` [`RepoObjectGuard`] as every repo read (tested elsewhere);
    //! these drive the list HANDLERS directly to prove: (a) the cross-repo front door never surfaces a
    //! PR from a repo the viewer cannot `pull` (even one the viewer AUTHORED) — the prefilter runs
    //! before the bucket predicate; (b) tab/bucket counts are computed over the leak-free set; (c) the
    //! bidirectional cursor pages the sorted set with no gaps/dups; (d) a legacy record with no title
    //! rows as `#number` and a config-read failure fails static ("checks unavailable"), never blanks.

    use super::*;
    use crate::catalogue::Page;
    use crate::repo_authz::GrantBackedRepos;
    use crate::request::EdgeRequest;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_storage::TenantScope;
    use myelin_tenancy::{Region as IdRegion, TenantId};
    use std::collections::BTreeMap;

    const TENANT: &str = "acme";
    const REGION: &str = "eu-west";

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "myelin-prlist-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn human(id: &str) -> Principal {
        Principal::new(
            TenantId(TENANT.into()),
            IdRegion(REGION.into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn open_pr(be: &DurableGitBackend, slug: &str, title: &str, opener: &Principal) {
        be.create_repo_as(TENANT, REGION, slug, opener).ok(); // idempotent-ish (409 → already there)
        let body = json!({
            "title": title,
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
            "head_oid": "0".repeat(40),
            "draft": false,
        });
        be.open_pr(TENANT, REGION, slug, &body, opener)
            .unwrap_or_else(|e| panic!("open PR in {slug}: {e:?}"));
    }

    /// Drive a list handler with a viewer + query string, returning the parsed JSON body.
    fn serve(handler: &dyn Handler, viewer: &Principal, repo: Option<&str>, query: &str) -> Value {
        let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
        let mut params = BTreeMap::new();
        if let Some(r) = repo {
            params.insert("repo".to_string(), r.to_string());
        }
        let req = EdgeRequest::new("GET", "/v1/git/prs", query, vec![], vec![]);
        let page = Page::from_request(&req);
        let identity = crate::catalogue::test_request_identity(viewer, &scope);
        let ctx = HandlerCtx {
            identity: &identity,
            principal: viewer,
            scope: &scope,
            params: &params,
            page: &page,
            request: &req,
        };
        match handler.handle(&ctx) {
            Ok(resp) => resp.json_body().expect("json body"),
            Err(e) => panic!("handler errored: {e:?}"),
        }
    }

    /// **A forged cursor never panics (verifier finding, R3.1): `?cursor=usize::MAX` must yield a
    /// clean empty page** — the offset+limit arithmetic saturates instead of overflowing (which
    /// panicked under overflow-checks, i.e. every debug/CI profile → a 500 on a crafted query).
    #[test]
    fn forged_max_cursor_yields_empty_page_never_panics() {
        let root = temp_root("forged-cursor");
        let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "core");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        let viewer = human("u:viewer");
        open_pr(&be, "core", "Only PR", &viewer);
        let handler = DRepoPrList { be: Arc::new(be) };
        let body = serve(
            &handler,
            &viewer,
            Some("core"),
            &format!("state=all&cursor={}", usize::MAX),
        );
        assert_eq!(
            body["items"].as_array().unwrap().len(),
            0,
            "past-the-end page is empty"
        );
        assert!(
            body["page"]["next_cursor"].is_null(),
            "no next past the end"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn repository_list_pages_visible_slugs_before_building_view_models() {
        let root = temp_root("repo-page");
        let authz = GrantBackedRepos::new()
            .grant_read("u:viewer", TENANT, "alpha")
            .grant_read("u:viewer", TENANT, "beta")
            .grant_read("u:viewer", TENANT, "gamma");
        let be = DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_authorizer(Arc::new(authz));
        let viewer = human("u:viewer");
        for slug in ["alpha", "beta", "gamma"] {
            be.create_repo_as(TENANT, REGION, slug, &viewer).unwrap();
        }

        let handler = DRepoList { be: Arc::new(be) };
        let body = serve(&handler, &viewer, None, "limit=1&cursor=1");
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["slug"], "acme/beta");
        assert_eq!(body["page"]["next_cursor"], "2");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn repository_candidate_scan_stops_before_unbounded_materialization() {
        let root = temp_root("repo-scan-bound");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let viewer = human("u:viewer");
        for slug in ["alpha", "beta"] {
            be.create_repo_as(TENANT, REGION, slug, &viewer).unwrap();
        }

        let error = be
            .scan_repo_slugs_bounded(TENANT, REGION, 1)
            .expect_err("the second repository must trip the candidate ceiling");
        assert!(matches!(
            error,
            DurableError::Git(message)
                if message == "browse response limit exceeded: repository candidate count"
        ));
        assert_eq!(
            be.scan_repo_slugs_bounded(TENANT, REGION, 2).unwrap(),
            vec!["alpha".to_string(), "beta".to_string()]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// **The PR title is capped at create (verifier note, R3.1)** — an unbounded title is echoed
    /// into every list row (response-bloat vector). >512 bytes is a clean error, never stored.
    #[test]
    fn oversized_title_is_rejected_at_create() {
        let root = temp_root("title-cap");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let author = human("u:author");
        be.create_repo_as(TENANT, REGION, "core", &author).unwrap();
        let body = json!({
            "title": "x".repeat(513),
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
            "head_oid": "0".repeat(40),
        });
        let err = be.open_pr(TENANT, REGION, "core", &body, &author);
        assert!(err.is_err(), "513-byte title must be rejected");
        // At the cap is fine.
        let ok_body = json!({
            "title": "x".repeat(512),
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
            "head_oid": "0".repeat(40),
        });
        assert!(be
            .open_pr(TENANT, REGION, "core", &ok_body, &author)
            .is_ok());
        std::fs::remove_dir_all(&root).ok();
    }

    /// **The cross-repo front door is leak-free: a PR in a repo the viewer cannot `pull` NEVER
    /// appears — even one the viewer AUTHORED.** The `visible_repos` prefilter runs BEFORE the bucket
    /// predicate, so a forbidden repo contributes nothing to the items OR the count (the anti-oracle
    /// rule). This is the PR-list analogue of the `visible_repos_filters_to_the_granted_set` proof.
    #[test]
    fn cross_repo_bucket_never_leaks_a_forbidden_repos_pr() {
        let root = temp_root("cross-leak");
        // Viewer is granted `read` on `alpha` only; `beta` is invisible to them.
        let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "alpha");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let viewer = human("u:viewer");
        // The viewer AUTHORS a PR in BOTH repos (so the bucket predicate `yours` WOULD match beta if
        // the prefilter leaked).
        open_pr(&be, "alpha", "Alpha change", &viewer);
        open_pr(&be, "beta", "Beta change (forbidden repo)", &viewer);
        // Install the visibility boundary after seeding: production open checks Pull on the exact
        // source repo, while this test is specifically about the subsequent cross-repo read filter.
        let be = be.with_repo_authorizer(Arc::new(authz));

        let handler = DMyPrs { be: Arc::new(be) };
        let body = serve(&handler, &viewer, None, "bucket=yours");
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "only the visible repo's PR is listed");
        assert_eq!(items[0]["repo"], "alpha");
        assert_eq!(items[0]["title"], "Alpha change");
        // The count is over the leak-free set — beta's PR never contributes.
        assert_eq!(body["counts"]["bucket"], 1);
        assert_eq!(body["page"]["total"], 1);
        std::fs::remove_dir_all(&root).ok();
    }

    /// **The per-repo list: rows carry the title, tab/sidebar counts are over the full authorised set,
    /// and the `?state=` filter narrows the returned rows without changing the counts** (a badge never
    /// under-counts because a tab is active).
    #[test]
    fn per_repo_list_rows_titles_and_counts() {
        let root = temp_root("per-repo");
        let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "core");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        let viewer = human("u:viewer");
        open_pr(&be, "core", "First PR", &viewer);
        open_pr(&be, "core", "Second PR", &viewer);

        let handler = DRepoPrList { be: Arc::new(be) };
        // Default (state=open) — both open PRs listed, titles present.
        let body = serve(&handler, &viewer, Some("core"), "");
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        let titles: Vec<&str> = items.iter().map(|i| i["title"].as_str().unwrap()).collect();
        assert!(titles.contains(&"First PR") && titles.contains(&"Second PR"));
        // Counts over the full set.
        assert_eq!(body["counts"]["open"], 2);
        assert_eq!(body["counts"]["all"], 2);
        assert_eq!(body["counts"]["merged"], 0);
        assert_eq!(body["counts"]["yours"], 2, "the viewer authored both");
        // A tab with no matches returns zero rows but the counts are unchanged (no under-count).
        let merged = serve(&handler, &viewer, Some("core"), "state=merged");
        assert_eq!(merged["items"].as_array().unwrap().len(), 0);
        assert_eq!(
            merged["counts"]["open"], 2,
            "the Open badge still reads 2 on the Merged tab"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// **Cursor stability: paging the sorted set with `limit` visits every row exactly once, in a
    /// stable order, with a correct bidirectional cursor** (`prev_cursor` None at the head; `next`
    /// None at the tail).
    #[test]
    fn per_repo_list_cursor_is_stable_and_bidirectional() {
        let root = temp_root("cursor");
        let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "core");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        let viewer = human("u:viewer");
        for i in 1..=5 {
            open_pr(&be, "core", &format!("PR {i}"), &viewer);
        }
        let handler = DRepoPrList { be: Arc::new(be) };

        // Page 1 (limit 2): head → prev None, next = "2".
        let p1 = serve(&handler, &viewer, Some("core"), "state=all&limit=2");
        assert_eq!(p1["items"].as_array().unwrap().len(), 2);
        assert_eq!(p1["page"]["total"], 5);
        assert!(p1["page"]["prev_cursor"].is_null(), "head has no Newer");
        assert_eq!(p1["page"]["next_cursor"], "2");

        // Page 2: prev = "0", next = "4".
        let p2 = serve(
            &handler,
            &viewer,
            Some("core"),
            "state=all&limit=2&cursor=2",
        );
        assert_eq!(p2["page"]["prev_cursor"], "0");
        assert_eq!(p2["page"]["next_cursor"], "4");

        // Page 3 (tail): 1 row, next None.
        let p3 = serve(
            &handler,
            &viewer,
            Some("core"),
            "state=all&limit=2&cursor=4",
        );
        assert_eq!(p3["items"].as_array().unwrap().len(), 1);
        assert!(p3["page"]["next_cursor"].is_null(), "tail has no Older");

        // The three pages cover all 5 numbers exactly once (no gaps/dups — stable order).
        let mut seen: Vec<u64> = Vec::new();
        for pg in [&p1, &p2, &p3] {
            for it in pg["items"].as_array().unwrap() {
                seen.push(it["number"].as_u64().unwrap());
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, vec![1, 2, 3, 4, 5]);
        std::fs::remove_dir_all(&root).ok();
    }

    /// **A legacy record (empty title) rows as `null` title (→ `#number` fallback) and a degraded
    /// checks projection fails static** — the row VM is honest, never fabricates, never blanks.
    #[test]
    fn row_vm_title_null_and_checks_unavailable_are_honest() {
        // A legacy record: empty title, no updated stamp.
        let pr = myelin_git::lifecycle::PullRequest::open(
            9,
            "refs/heads/main",
            "refs/heads/feature",
            "psn:old@acme",
            false,
        );
        let rec = PrRecord::open(&pr, "abc");
        assert_eq!(rec.title, "");
        let enriched = EnrichedPr {
            rec,
            summary: ChecksSummary::unavailable(),
            you_requested: false,
            repo_slug: Some("core".into()),
        };
        let row = DurableGitBackend::pr_list_row_json(&enriched);
        assert!(
            row["title"].is_null(),
            "empty title → null (the #number fallback is honest)"
        );
        assert_eq!(row["number"], 9);
        assert_eq!(
            row["checks_summary"]["verdict"], "unavailable",
            "fails static, still lists"
        );
        assert_eq!(row["updated_at"], Value::Null);
    }

    /// **F3 (R4.1 dogfood) — the repo-home clone URL is the HONEST HTTP git-wire shape, never
    /// `ssh://git@myelin/…`.** The wire is HTTP smart-transport ONLY (no SSH server) and its path
    /// grammar is `/{tenant}/{region}/{repo}.git` — so the advertised clone URL must end in the real
    /// `(tenant, region, repo)` path and carry no `ssh://` scheme / fabricated host.
    #[test]
    fn f3_clone_url_is_http_wire_shape_never_ssh() {
        let root = temp_root("f3-clone-url");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        // The builder: `{base}/{tenant}/{region}/{repo}.git` (base empty by default → a relative path).
        let url = be.clone_url(TENANT, REGION, "widgets");
        assert!(
            url.ends_with("/acme/eu-west/widgets.git"),
            "the wire path grammar is /{{tenant}}/{{region}}/{{repo}}.git — got {url}"
        );
        assert!(
            !url.contains("ssh://"),
            "no ssh scheme (there is no SSH server): {url}"
        );
        assert!(!url.contains("git@myelin"), "no fabricated ssh host: {url}");

        // End-to-end through the repo-home projection (an empty repo still advertises the URL).
        let author = human("u:author");
        be.create_repo_as(TENANT, REGION, "widgets", &author)
            .unwrap();
        let loc = DurableGitBackend::loc(TENANT, REGION, "widgets");
        let repo = be.store.open_repo(&loc).expect("open repo");
        let advertised = match be
            .repo_home(TENANT, REGION, "widgets", &repo)
            .expect("repo home reads")
        {
            RepoHome::Empty { clone_url, .. } | RepoHome::Populated { clone_url, .. } => clone_url,
            other => panic!("a fresh repo projects an Empty/Populated home, got {other:?}"),
        };
        assert!(
            advertised.ends_with("/acme/eu-west/widgets.git"),
            "got {advertised}"
        );
        assert!(
            !advertised.contains("ssh://"),
            "no ssh in the projection: {advertised}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pr_number_allocation_never_resets_or_wraps() {
        assert_eq!(DurableGitBackend::next_pr_number_after(None).unwrap(), 1);
        assert_eq!(DurableGitBackend::next_pr_number_after(Some(41)).unwrap(), 42);
        let err = DurableGitBackend::next_pr_number_after(Some(u64::MAX))
            .expect_err("an exhausted namespace must fail instead of wrapping");
        assert!(err.to_string().contains("number space exhausted"));
    }

    /// **F8 (R4.1 dogfood) — open a PR with a `head_ref` but NO `head_oid` → the stored PR carries the
    /// ref's CURRENT tip (so it is mergeable), and a non-existent `head_ref` is refused with a clear
    /// 400 at OPEN (never the confusing "invalid merge head" at merge time).**
    #[test]
    fn f8_open_pr_resolves_head_oid_from_head_ref_tip() {
        let root = temp_root("f8-resolve-head");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let author = human("u:author");
        be.create_repo_as(TENANT, REGION, "core", &author).unwrap();

        // Seed a real commit as the tip of `refs/heads/feature` (the proposed head branch).
        let loc = DurableGitBackend::loc(TENANT, REGION, "core");
        let repo = be.store.open_repo(&loc).expect("open repo");
        let blob = repo.write_blob(b"hello\n").expect("blob");
        let tree = repo.write_tree(&[("f.txt", &blob)]).expect("tree");
        let tip = repo
            .write_commit(&tree, &[], "seed", "psn@acme.noreply", "psn@acme.noreply")
            .expect("commit");
        repo.update_ref_cas(
            "refs/heads/feature",
            None,
            Some(&tip),
            "create",
            "psn@acme.noreply",
        )
        .expect("create feature ref");

        // Open with head_ref but NO head_oid → the resolver fills in the branch tip.
        let body = json!({
            "title": "resolve my head",
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
            // head_oid deliberately OMITTED
        });
        let rec = be
            .open_pr(TENANT, REGION, "core", &body, &author)
            .expect("open PR");
        assert_eq!(
            rec.head_oid, tip.0,
            "F8: an omitted head_oid is resolved from head_ref's current tip"
        );

        // A bare (unqualified) head_ref resolves too (qualified to refs/heads/<name>).
        let body_bare = json!({ "title": "bare head_ref", "head_ref": "feature" });
        let rec2 = be
            .open_pr(TENANT, REGION, "core", &body_bare, &author)
            .expect("open PR");
        assert_eq!(
            rec2.head_oid, tip.0,
            "F8: a bare branch name also resolves to the tip"
        );

        // A non-existent head_ref → a clean 400 at OPEN (mapped from the durable error), NOT an empty
        // head_oid that wedges the merge dialog with "invalid merge head" later.
        let bad = json!({ "title": "ghost branch", "head_ref": "refs/heads/does-not-exist" });
        let err = be
            .open_pr(TENANT, REGION, "core", &bad, &author)
            .expect_err("must refuse");
        assert_eq!(
            map_durable_err(err).status(),
            400,
            "F8: a non-existent head_ref is a 400 at open, not a merge-time surprise"
        );
        let oversized = be.raw_response_bounded(
                TENANT,
                REGION,
                "core",
                "refs/heads/feature",
                "f.txt",
                RawResponseOptions { attachment: true, maximum_bytes: 1 },
            );
        assert!(matches!(oversized, Err(error) if error.status() == 413));
        assert_eq!(
            read_text_blob_bounded(&repo, "refs/heads/feature", "f.txt", 1).unwrap(),
            None,
            "an oversized README-style preview must stop at the object header",
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn blob_view_stops_at_the_object_header_for_oversized_previews() {
        let root = temp_root("blob-preview-bound");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let author = human("u:author");
        be.create_repo_as(TENANT, REGION, "core", &author).unwrap();
        let loc = DurableGitBackend::loc(TENANT, REGION, "core");
        let repo = be.store.open_repo(&loc).expect("open repo");
        let blob = repo.write_blob(b"hello\n").expect("blob");
        let tree = repo.write_tree(&[("large.txt", &blob)]).expect("tree");
        let tip = repo
            .write_commit(&tree, &[], "seed", "psn@acme.noreply", "psn@acme.noreply")
            .expect("commit");
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&tip),
            "create",
            "psn@acme.noreply",
        )
        .expect("create main");

        let metadata = be
            .blob_json_bounded(
                TENANT,
                REGION,
                "core",
                "main",
                "large.txt",
                BlobViewOptions {
                    maximum_preview_bytes: 1,
                    maximum_transfer_bytes: 4,
                },
            )
            .expect("metadata-only blob view");
        assert_eq!(metadata["contents"], "");
        assert_eq!(metadata["base_oid"], blob.as_str());
        assert_eq!(metadata["size_bytes"], 6);
        assert_eq!(metadata["preview_unavailable"], true);
        assert_eq!(metadata["download_available"], false);
        assert_eq!(metadata["viewer_may_edit"], false);

        let inline = be
            .blob_json_bounded(
                TENANT,
                REGION,
                "core",
                "main",
                "large.txt",
                BlobViewOptions {
                    maximum_preview_bytes: 6,
                    maximum_transfer_bytes: 6,
                },
            )
            .expect("inline blob view");
        assert_eq!(inline["contents"], "hello\n");
        assert_eq!(inline["preview_unavailable"], false);
        assert_eq!(inline["download_available"], true);
        assert_eq!(inline["viewer_may_edit"], false);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn oversized_interactive_reads_map_to_bounded_public_responses() {
        for (private, public) in [
            (
                "browse response limit exceeded: private repository detail",
                "repository view exceeds the interactive browse limit",
            ),
            (
                "pr diff computation limit exceeded: private repository detail",
                "pull request diff exceeds the interactive file limit",
            ),
            (
                "commit diff computation limit exceeded: private repository detail",
                "commit diff exceeds the interactive content limit",
            ),
            (
                "pull request list limit exceeded: private repository detail",
                "pull request list exceeds the interactive record limit",
            ),
            (
                "pull request record limit exceeded: private repository detail",
                "pull request record exceeds the storage limit",
            ),
        ] {
            let mapped = map_durable_err(DurableError::Git(private.into()));
            assert_eq!(mapped.status(), 413);
            assert_eq!(mapped.to_string(), format!("413 (payload_too_large): {public}"));
        }
    }
}

#[cfg(test)]
mod file_lines_boundary_tests {
    use super::*;

    #[test]
    fn file_lines_query_is_exact_decoded_and_bounded() {
        let parsed = parse_file_lines_query("path=src%2Fmain+file.rs&start=2&end=4")
            .expect("canonical bounded query");
        assert_eq!(parsed.path, "src/main file.rs");
        assert_eq!((parsed.start, parsed.end), (2, 4));

        for query in [
            "",
            "path=x&start=1",
            "path=x&start=1&end=1&extra=x",
            "path=x&path=y&start=1&end=1",
            "path=..%2Fsecret&start=1&end=1",
            "path=x&start=0&end=1",
            "path=x&start=2&end=1",
            "path=x&start=1&end=1001",
            "path=x%ZZ&start=1&end=1",
        ] {
            assert!(
                matches!(parse_file_lines_query(query), Err(EdgeError::BadRequest(_))),
                "query must fail closed: {query}"
            );
        }
        assert!(matches!(
            parse_file_lines_query(&"x".repeat(FILE_LINES_MAX_QUERY_BYTES + 1)),
            Err(EdgeError::BadRequest(_))
        ));
    }

    #[test]
    fn file_lines_oid_requires_the_full_lowercase_content_address() {
        assert!(canonical_blob_oid(
            "0123456789abcdef0123456789abcdef01234567"
        ));
        assert!(!canonical_blob_oid("01234567"));
        assert!(!canonical_blob_oid(
            "0123456789ABCDEF0123456789ABCDEF01234567"
        ));
        assert!(!canonical_blob_oid(
            "g123456789abcdef0123456789abcdef01234567"
        ));
    }
}

#[cfg(test)]
mod pr_thread_tests {
    //! **R3.3 / R3.2 — the PR thread / comment / review-batch surface at the edge.** Drives the
    //! handlers to prove: (a) thread READ = PR view (the `Pull` guard) while WRITE follows the exact
    //! review union (requested reviewer or repo pusher; an unrelated reader is denied); (b) a pending review
    //! comment is invisible to a second viewer until submit; (c) submit emits exactly ONE batch event
    //! (idempotent on retry); (d) a submitted human `changes_requested` verdict flips the merge gate to
    //! blocked; (e) a blocked merge returns a 409 carrying the FRESH re-rendered checks (N6).

    use super::*;
    use crate::catalogue::Page;
    use crate::repo_authz::GrantBackedRepos;
    use crate::request::EdgeRequest;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_storage::TenantScope;
    use myelin_tenancy::{Region as IdRegion, TenantId};
    use std::collections::BTreeMap;

    const TENANT: &str = "acme";
    const REGION: &str = "eu-west";
    const SLUG: &str = "core";

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "myelin-prthread-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn human(id: &str) -> Principal {
        Principal::new(
            TenantId(TENANT.into()),
            IdRegion(REGION.into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    /// Drive a handler (JSON body + path params), returning the `Result` so authz denials are testable.
    fn serve(
        handler: &dyn Handler,
        method: &str,
        viewer: &Principal,
        params: &[(&str, &str)],
        body: Value,
    ) -> Result<Value, EdgeError> {
        let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
        let pmap: BTreeMap<String, String> = params
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let bytes = if body.is_null() {
            vec![]
        } else {
            serde_json::to_vec(&body).unwrap()
        };
        let req = EdgeRequest::new(method, "/v1/git/x", "", vec![], bytes);
        let page = Page::from_request(&req);
        let identity = crate::catalogue::test_request_identity(viewer, &scope);
        let ctx = HandlerCtx {
            identity: &identity,
            principal: viewer,
            scope: &scope,
            params: &pmap,
            page: &page,
            request: &req,
        };
        handler.handle(&ctx).map(|r| r.json_body().expect("json"))
    }

    /// A backend with a writer + a read-only viewer, a repo, and one open PR at head `head_oid`.
    fn setup(tag: &str, head_oid: &str) -> (Arc<DurableGitBackend>, Principal, Principal) {
        let root = temp_root(tag);
        let authz = GrantBackedRepos::new()
            .grant_write("u:writer", TENANT, SLUG)
            .grant_read("u:reader", TENANT, SLUG);
        let writer = human("u:writer");
        let reader = human("u:reader");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        be.create_repo_as(TENANT, REGION, SLUG, &writer).unwrap();
        let body = json!({
            "title": "R3.3 flagship", "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature", "head_oid": head_oid, "draft": false,
        });
        be.open_pr(TENANT, REGION, SLUG, &body, &writer).unwrap();
        (Arc::new(be), writer, reader)
    }

    /// **Write = `pull_request.review`, not `Push` alone.** A requested reviewer may comment without
    /// repo write, a repo writer remains admitted by the union's inheritance arm, and an unrelated
    /// read-only viewer is denied.
    #[test]
    fn thread_write_admits_requested_reviewer_or_repo_pusher_only() {
        let (be, writer, reader) = setup("authz", &"0".repeat(40));
        // The reader may LIST (Pull-guarded read).
        let list = guarded(
            &be,
            RepoPermission::Pull,
            Arc::new(DPrThreads { be: be.clone() }),
        );
        let v = serve(
            &*list,
            "GET",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .expect("reader may read threads");
        assert!(v["threads"].is_array());
        let create = pr_review_guarded(&be, Arc::new(DPrThreadCreate { be: be.clone() }));
        let err = serve(
            &*create,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            json!({ "body_md": "hi" }),
        )
        .expect_err("an unrelated read-only viewer must be forbidden from commenting");
        assert!(matches!(err, EdgeError::Forbidden(_)), "got {err:?}");

        // The repo writer is the `parent_repo->push` union arm.
        serve(
            &*create,
            "POST",
            &writer,
            &[("repo", SLUG), ("n", "1")],
            json!({ "body_md": "writer comment" }),
        )
        .expect("a repo pusher may review");

        // Materialize the direct `reviewer` arm on the current PR. The reader still has no Push.
        let loc = DurableGitBackend::loc(TENANT, REGION, SLUG);
        be.prs
            .update(&loc, 1, |record| {
                record.reviews.push(ReviewRecord {
                    reviewer_pseudonym: DurableGitBackend::pseudonym(TENANT, &reader),
                    state: ReviewState::Requested,
                    is_agent: false,
                });
                Ok(())
            })
            .expect("request review");
        serve(
            &*create,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            json!({ "body_md": "requested-reviewer comment" }),
        )
        .expect("a directly requested reviewer may review without repo Push");
    }

    #[test]
    fn oversized_comment_is_rejected_before_conversation_storage() {
        let (be, writer, _reader) = setup("body-limit", &"0".repeat(40));
        let create = pr_review_guarded(&be, Arc::new(DPrThreadCreate { be: be.clone() }));

        let error = serve(
            &*create,
            "POST",
            &writer,
            &[("repo", SLUG), ("n", "1")],
            json!({
                "body_md": "x".repeat(myelin_git::pr_threads::MAX_COMMENT_BODY_BYTES + 1),
            }),
        )
        .expect_err("oversized comment must fail before persistence");

        assert!(matches!(error, EdgeError::BadRequest(_)), "got {error:?}");
        assert!(be
            .threads
            .load(&DurableGitBackend::loc(TENANT, REGION, SLUG), "pr:core:1")
            .unwrap()
            .threads
            .is_empty());
    }

    /// A pending review comment is invisible to a second viewer until submit; submit emits ONE event.
    #[test]
    fn pending_comment_is_private_and_submit_emits_one_event() {
        let (be, writer, reader) = setup("pending", &"0".repeat(40));
        let threads = Arc::new(DPrThreads { be: be.clone() });
        let start = Arc::new(DPrReviewStart { be: be.clone() });
        let pending = Arc::new(DPrReviewComment { be: be.clone() });
        let submit = Arc::new(DPrReviewSubmit { be: be.clone() });

        // The reader (a real reviewer with read) starts a batch + drafts a pending comment.
        let batch = serve(
            &*start,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .unwrap();
        let rid = batch["applied"]["review"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        serve(
            &*pending,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
            json!({ "body_md": "draft note" }),
        )
        .unwrap();

        // The WRITER (a different viewer) sees NO thread and NO draft batch.
        let seen = serve(
            &*threads,
            "GET",
            &writer,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .unwrap();
        assert_eq!(
            seen["threads"].as_array().unwrap().len(),
            0,
            "pending comment is private"
        );
        assert_eq!(
            seen["reviews"].as_array().unwrap().len(),
            0,
            "draft batch is hidden"
        );

        // Submit → ONE event (emitted true); a re-submit is idempotent (emitted false).
        let first = serve(
            &*submit,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
            json!({ "verdict": "commented" }),
        )
        .unwrap();
        assert_eq!(first["applied"]["result"]["emitted"], true);
        let again = serve(
            &*submit,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
            json!({ "verdict": "commented" }),
        )
        .unwrap();
        assert_eq!(
            again["applied"]["result"]["emitted"], false,
            "no double event"
        );

        // Now the writer sees the (submitted) thread.
        let seen = serve(
            &*threads,
            "GET",
            &writer,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .unwrap();
        assert_eq!(
            seen["threads"].as_array().unwrap().len(),
            1,
            "submit makes it public"
        );
    }

    /// A submitted human `changes_requested` batch flips the merge gate → the checks projection reads
    /// `changes_requested: true` and `gate_admitted: false` (the VERIFIED R2 gate input).
    #[test]
    fn a_changes_requested_batch_blocks_the_gate() {
        let (be, _writer, reader) = setup("blockgate", &"0".repeat(40));
        let start = Arc::new(DPrReviewStart { be: be.clone() });
        let submit = Arc::new(DPrReviewSubmit { be: be.clone() });
        let checks = Arc::new(DPrChecks { be: be.clone() });

        let batch = serve(
            &*start,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .unwrap();
        let rid = batch["applied"]["review"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        serve(
            &*submit,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
            json!({ "verdict": "changes_requested" }),
        )
        .unwrap();
        let ck = serve(
            &*checks,
            "GET",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .unwrap();
        assert_eq!(
            ck["changes_requested"], true,
            "the gate ingests changes_requested"
        );
        assert_eq!(
            ck["gate_admitted"], false,
            "a live request-changes blocks the merge"
        );
    }

    /// A blocked merge returns a 409 carrying the FRESH re-rendered checks (N6 — the UI re-renders the
    /// blocked card without a second round-trip, never merges on stale state).
    #[test]
    fn a_blocked_merge_returns_409_with_rerendered_checks() {
        // A protected base (`refs/heads/main`) defaults CLOSED (requires a non-author approval) → the
        // merge is blocked by POLICY. Build the backend with an ADMIN grant for the writer so the
        // OBJECT guard (ProtectedPush) admits and the POLICY gate is what blocks (the N6 case).
        let root = temp_root("merge409");
        let authz = GrantBackedRepos::new().grant_admin("u:writer", TENANT, SLUG);
        let writer = human("u:writer");
        let be = Arc::new(
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz)),
        );
        be.create_repo_as(TENANT, REGION, SLUG, &writer).unwrap();
        be.open_pr(
            TENANT,
            REGION,
            SLUG,
            &json!({ "title": "N6", "base_ref": "refs/heads/main", "head_ref": "refs/heads/feature",
                     "head_oid": "0".repeat(40), "draft": false }),
            &writer,
        )
        .unwrap();
        let merge = guarded(
            &be,
            RepoPermission::ProtectedPush,
            Arc::new(DMerge { be: be.clone() }),
        );
        let resp = serve(
            &*merge,
            "POST",
            &writer,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .expect("merge handler returns a body (409 is an Ok EdgeResponse, not an Err)");
        assert_eq!(resp["error"]["code"], "merge_blocked");
        assert_eq!(
            resp["checks"]["gate_admitted"], false,
            "the 409 carries the fresh gate state"
        );
    }

    /// Seed a repo with a real base commit on `main` + a head commit branched off it (modifying
    /// file.txt), open a PR at the real head oid, and return the backend + a reader + the head oid.
    fn setup_diff(tag: &str) -> (Arc<DurableGitBackend>, Principal, String) {
        let root = temp_root(tag);
        let authz = GrantBackedRepos::new()
            .grant_write("u:writer", TENANT, SLUG)
            .grant_read("u:reader", TENANT, SLUG);
        let writer = human("u:writer");
        let reader = human("u:reader");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        be.create_repo_as(TENANT, REGION, SLUG, &writer).unwrap();
        let loc = DurableGitBackend::loc(TENANT, REGION, SLUG);
        let repo = be.store.open_repo(&loc).unwrap();
        let b0 = repo.write_blob(b"a\nb\nc\n").unwrap();
        let t0 = repo.write_tree(&[("file.txt", &b0)]).unwrap();
        let base = repo
            .write_commit(&t0, &[], "base", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&base),
            "c",
            "psn@acme.noreply",
        )
        .unwrap();
        let bh = repo.write_blob(b"a\nB\nc\nd\n").unwrap();
        let th = repo.write_tree(&[("file.txt", &bh)]).unwrap();
        let head = repo
            .write_commit(
                &th,
                &[&base],
                "head",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        be.open_pr(
            TENANT,
            REGION,
            SLUG,
            &json!({ "title": "diff pr", "base_ref": "refs/heads/main",
                     "head_ref": "refs/heads/feature", "head_oid": head.0, "draft": false }),
            &writer,
        )
        .unwrap();
        (Arc::new(be), reader, head.0)
    }

    #[test]
    fn line_anchors_are_strictly_validated_and_revision_bound() {
        let (be, reviewer, head) = setup_diff("anchors");
        let new_side = be
            .create_thread(
                TENANT,
                REGION,
                SLUG,
                1,
                &json!({
                    "body_md": "new-side note",
                    "anchor": { "path": "file.txt", "line": 4, "side": "new" },
                }),
                &reviewer,
            )
            .expect("a displayed new-side line resolves");
        assert_eq!(new_side["anchor"]["side"], "new");
        assert_eq!(new_side["anchor"]["head_oid"], head);
        assert_eq!(new_side["anchor"]["base_oid"].as_str().unwrap().len(), 40);

        let old_side = be
            .create_thread(
                TENANT,
                REGION,
                SLUG,
                1,
                &json!({
                    "body_md": "old-side note",
                    "anchor": { "path": "file.txt", "line": 2, "side": "old" },
                }),
                &reviewer,
            )
            .expect("a displayed old-side line resolves");
        assert_eq!(old_side["anchor"]["side"], "old");

        for invalid in [
            json!({ "body_md": "missing side", "anchor": { "path": "file.txt", "line": 2 } }),
            json!({ "body_md": "stale line", "anchor": { "path": "file.txt", "line": 99, "side": "new" } }),
            json!({ "body_md": "unsafe path", "anchor": { "path": "../secret", "line": 1, "side": "new" } }),
        ] {
            let error = be
                .create_thread(TENANT, REGION, SLUG, 1, &invalid, &reviewer)
                .expect_err("malformed or stale anchor must be rejected");
            assert!(error.to_string().contains("anchor"), "got {error:?}");
        }

        let stored = be
            .threads
            .load(&DurableGitBackend::loc(TENANT, REGION, SLUG), "pr:core:1")
            .unwrap();
        assert_eq!(stored.threads.len(), 2, "invalid anchors persisted nothing");
    }

    /// **R3.2 · G-7 N1 — the PR diff is `Pull`-guarded, 0-leak, three-dot, count-only restricted.** A
    /// reader (PR view) gets the three-dot diff; a stranger with NO grant gets a 0-leak 404 (never a
    /// distinguishable Forbidden, never a leaked path). `restricted_files` is count-only (0 here).
    #[test]
    fn pr_diff_is_pull_guarded_zero_leak_and_three_dot() {
        let (be, reader, _head) = setup_diff("diffauthz");
        let guard = guarded(
            &be,
            RepoPermission::Pull,
            Arc::new(DPrDiff { be: be.clone() }),
        );
        // The reader sees the three-dot diff of file.txt (the PR's own change).
        let v = serve(
            &*guard,
            "GET",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .expect("a reader may view the PR diff");
        assert_eq!(v["number"], 1);
        assert_eq!(
            v["three_dot"], true,
            "durable repos are libgit2-backed → merge-base"
        );
        assert_eq!(v["total_files"], 1);
        assert_eq!(v["files"][0]["path"], "file.txt");
        assert_eq!(v["files"][0]["status"], "M");
        assert_eq!(v["files"][0]["kind"], "text");
        // Line numbers cross the wire additively.
        let lines = v["files"][0]["hunks"][0]["lines"].as_array().unwrap();
        assert!(lines
            .iter()
            .any(|l| l["origin"] == "+" && l["content"] == "d" && l["new_no"] == 4));
        // Restricted disclosure is COUNT-ONLY — the field is a number, never a path list.
        assert_eq!(
            v["restricted_files"], 0,
            "count-only; 0 under the repo-level Pull guard"
        );
        assert!(v["restricted_files"].is_number());

        // A STRANGER with no grant → a 0-leak 404 (NotFound), never a distinguishable Forbidden.
        let stranger = human("u:stranger");
        let err = serve(
            &*guard,
            "GET",
            &stranger,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .expect_err("a stranger must not view the diff");
        assert!(
            matches!(err, EdgeError::NotFound(_)),
            "0-leak 404, got {err:?}"
        );
    }

    /// A diff request for an ABSENT PR is a clean 404 (exactly like the overview — never a 500).
    #[test]
    fn pr_diff_absent_pr_is_not_found() {
        let (be, reader, _head) = setup_diff("diffabsent");
        let guard = guarded(
            &be,
            RepoPermission::Pull,
            Arc::new(DPrDiff { be: be.clone() }),
        );
        let err = serve(
            &*guard,
            "GET",
            &reader,
            &[("repo", SLUG), ("n", "999")],
            Value::Null,
        )
        .expect_err("absent PR");
        assert!(matches!(err, EdgeError::NotFound(_)), "got {err:?}");
    }
}
