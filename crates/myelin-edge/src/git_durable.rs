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
use crate::request::EdgeResponse;
use myelin_events::{Actor, EmitContextBase, IdMinter, OutboxStore, Region, TenantId, Timestamp};
// Used only by the `test-support`-gated `rooted_inmem_for_test` helper (MR-009b W3b.6).
#[cfg(any(test, feature = "test-support"))]
use myelin_events::MonotonicMinter;
use myelin_git::api::{http_catalogue, Method as GitMethod};
use myelin_git::check_status::GitOid;
use myelin_git::core::{Oid as CoreOid, RepoLoc};
use myelin_git::durable::{BlobPathLookup, CommitDetail, CommitMeta, PrDiff, TreePathLookup};
use myelin_git::durable::{DurableError, DurableGitRepo, DurableGitStore};
use myelin_git::lifecycle::{
    BranchProtectionRuleset, PrState, PullRequest, ReviewState, ReviewVerdict,
};
use myelin_git::pr_store::{
    effective_ruleset, evaluate_merge, merge_pr, BranchProtectionConfig, ChecksSummary,
    DurablePrStore, MergeAttempt, PrRecord, ReviewRecord,
};
use myelin_git::pr_threads::{
    AnchorState, BatchVerdict, CommentRecord, CommentState, DurablePrThreadStore, PrincipalRole,
    ReviewBatch, ThreadAnchor, ThreadPrincipal, ThreadRecord, ViewedThreads,
};
use myelin_git::receive_pack::{
    evaluate_protected_ref_push, CrashPoint, InMemoryObjectDb, Oid as PushOid, ProposedRefUpdate,
    PushOutcome, PushSession, Pusher, QuarantineMigration, QuarantineObject, RefName, RefStore,
};
use myelin_git::web::{
    CommitDiff, CommitRow, DiffFile, DiffLineView, PrDiffFile, PrDiffHunk, PrDiffLine, PrDiffVM,
    RepoHome, WebEditOutcome,
};
use myelin_identity::{Principal, PrincipalKind};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// **The durable on-disk git backend the edge writes/reads through (GT-003).** Holds the durable git
/// store + the durable PR store rooted at one on-disk root, plus the shared outbox + id minter the ref
/// CAS co-commits its `git.ref.updated` through (the reconciler replays this outbox). The `(tenant,
/// region)` is taken from `ctx.scope` per request — never from the URL/body (the GIT-D8 invariant).
pub struct DurableGitBackend {
    store: DurableGitStore,
    prs: DurablePrStore,
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
    /// origin, so the base is read from `MYELIN_PUBLIC_BASE_URL` at construction (e.g.
    /// `https://git.example.com`); UNSET → the empty string, which yields an HONEST relative
    /// `/{tenant}/{region}/{repo}.git` (a real path on this host) rather than a fabricated hostname.
    clone_base: String,
    /// The on-disk root holding `<tenant>/<region>/<repo>.git` bare repos — retained so the wire-serving
    /// tier (CT-006b) composes its sandboxed `GitCore` over the SAME root the durable store reads/writes.
    root: PathBuf,
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
        outbox: OutboxStore,
        minter: Arc<dyn IdMinter>,
    ) -> DurableGitBackend {
        let root = root.into();
        DurableGitBackend {
            store: DurableGitStore::rooted(root.clone()),
            prs: DurablePrStore::rooted(root.clone()),
            threads: DurablePrThreadStore::rooted(root.clone()),
            outbox,
            minter,
            // F3: the public HTTP base for clone URLs — env-driven, honest empty default (relative path).
            clone_base: public_clone_base(),
            root,
            // R2.6: the prod constructor default is FAIL-CLOSED (`DenyAllRepos`) — a composition root
            // that forgets `with_repo_authorizer` denies every repo rather than serving all of them.
            // Production `main.rs` ALWAYS injects the real `CheckBackedRepoAuthorizer`; the permissive
            // `AllowAllRepos` fixture now lives ONLY in the test-support `rooted_inmem_for_test` below.
            repo_authz: Arc::new(DenyAllRepos),
            bootstrap: Arc::new(NoRepoBootstrap),
        }
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
        DurableGitBackend::rooted(root, OutboxStore::new(), Arc::new(MonotonicMinter::new()))
            .with_repo_authorizer(Arc::new(AllowAllRepos))
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

    /// **The wire-serving `GitCore` over the SAME on-disk root (CT-006b / GT-006).** Composes the
    /// production sandboxed [`crate::git_wire_exec::GitWireExecutor`] (wire ops → canonical `git` in the
    /// hardened gVisor sandbox, no-host-exec) with the in-process [`myelin_git::gix_backend::GixCore`]
    /// read backend. `advertise_refs(repo, UploadPack)` / `serve(repo, UploadPack, request)` flow
    /// through here against the real on-disk bare repos. The HTTP smart-transport listener that drives
    /// this over the wire (+ the receive-pack/PUSH path) is CT-006c.
    pub fn wire_serving(
        &self,
    ) -> myelin_git::core::RoutedGitCore<
        crate::git_wire_exec::GitWireExecutor,
        myelin_git::gix_backend::GixCore<myelin_git::gix_backend::RootedResolver>,
    > {
        crate::git_wire_exec::production_git_core_default(self.root.clone())
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
        let records = myelin_git::reconcile::refs_from_outbox(&self.outbox, Some(slug));
        myelin_git::reconcile::reconcile_refs(&repo, &records)
    }

    fn loc(tenant: &str, region: &str, slug: &str) -> RepoLoc {
        RepoLoc::new(tenant, region, slug)
    }

    fn emit_ctx(tenant: &str, region: &str, principal: &Principal) -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId(tenant.into()),
            region: Region(region.into()),
            actor: Actor(principal.clone()),
            schema_ver: 1,
            // The substrate clock injection is the production composition-root's job; a fixed RFC-3339
            // stamp is sufficient here (the edge does not drain the outbox — the relay does).
            occurred_at: Timestamp("2026-06-29T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-29T00:00:01Z".into()),
            caused_by: None,
        }
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
    fn scan_repo_slugs(&self, tenant: &str, region: &str) -> Vec<String> {
        let mut slugs: Vec<String> = Vec::new();
        let probe = Self::loc(tenant, region, "_probe");
        let Ok(probe_path) = self.store.repo_path(&probe) else {
            return slugs;
        };
        let Some(dir) = probe_path.parent() else {
            return slugs;
        };
        let Ok(rd) = std::fs::read_dir(dir) else {
            return slugs;
        };
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(slug) = name.strip_suffix(".git") {
                slugs.push(slug.to_string());
            }
        }
        slugs.sort();
        slugs
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
    ) -> Vec<RepoHome> {
        let candidates = self.scan_repo_slugs(tenant, region);
        let visible = self
            .repo_authz
            .visible_repos(principal, tenant, region, &candidates);
        let mut out = Vec::new();
        for slug in visible {
            let loc = Self::loc(tenant, region, &slug);
            let Ok(repo) = self.store.open_repo(&loc) else {
                continue;
            };
            out.push(self.repo_home(tenant, region, &slug, &repo));
        }
        out
    }

    /// **F3 — the HTTP git-wire clone URL for a repo.** The wire path grammar is
    /// `/{tenant}/{region}/{repo}.git` (the ONLY transport is HTTP smart-protocol), prefixed by the
    /// configured public base (`MYELIN_PUBLIC_BASE_URL`; empty → an honest relative path). Never the
    /// old `ssh://git@myelin/…` (no SSH server exists; the region segment + real slug were missing).
    fn clone_url(&self, tenant: &str, region: &str, slug: &str) -> String {
        format!("{}/{tenant}/{region}/{slug}.git", self.clone_base)
    }

    fn repo_home(&self, tenant: &str, region: &str, slug: &str, repo: &DurableGitRepo) -> RepoHome {
        let full_slug = format!("{tenant}/{slug}");
        let clone_url = self.clone_url(tenant, region, slug);
        let entries = repo
            .tree_entries_at_ref("refs/heads/main")
            .unwrap_or_default();
        if entries.is_empty() {
            RepoHome::Empty {
                slug: full_slug,
                clone_url,
            }
        } else {
            let readme = repo
                .read_file_at_ref("refs/heads/main", "README.md")
                .ok()
                .flatten()
                .map(|(b, _)| String::from_utf8_lossy(&b).chars().take(400).collect())
                .unwrap_or_default();
            RepoHome::Populated {
                slug: full_slug,
                readme_excerpt: readme,
                entries,
                clone_url,
            }
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
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        offset: usize,
        limit: usize,
    ) -> Result<Option<PrDiffVM>, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let Some(rec) = self.prs.get(&loc, number)? else {
            return Ok(None);
        };
        let repo = self.store.open_repo(&loc)?;
        let Some(diff) = repo.pr_diff(&rec.base_ref, &rec.head_oid, PR_DIFF_PER_FILE_LINE_CAP)? else {
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
    ) -> Result<Option<Vec<PrDiffLine>>, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        Ok(repo
            .file_lines(oid, start, end)?
            .map(|lines| lines.into_iter().map(pr_diff_line).collect()))
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
    fn repo_home_json(&self, tenant: &str, region: &str, slug: &str) -> Result<Value, DurableError> {
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
        let readme = repo
            .read_file_at_ref(&branch_ref, "README.md")
            .ok()
            .flatten()
            .map(|(b, _)| String::from_utf8_lossy(&b).to_string());
        let latest = repo.commit_log(&branch_ref, 0, 1)?.0.into_iter().next();
        let per_entry = repo
            .latest_commits_in_dir(&branch_ref, "", LATEST_COMMIT_WALK_CAP)
            .unwrap_or_default();
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
                    .latest_commits_in_dir(gitref, base, LATEST_COMMIT_WALK_CAP)
                    .unwrap_or_default();
                let entries_json = tree_entries_json(&entries, base, &per_entry);
                // A subtree README renders too (same read-path); binary/absent → no readme.
                let readme_path = if base.is_empty() {
                    "README.md".to_string()
                } else {
                    format!("{base}/README.md")
                };
                let readme = match repo.read_blob_at_path(gitref, &readme_path)? {
                    BlobPathLookup::Found {
                        bytes,
                        is_binary: false,
                        ..
                    } => Some(String::from_utf8_lossy(&bytes).to_string()),
                    _ => None,
                };
                Ok(json!({
                    "ref": gitref,
                    "path": base,
                    "entries": entries_json,
                    "readme": readme,
                }))
            }
        }
    }

    /// The enriched BlobVM (`GET /repos/{repo}/blob/{ref}/{...path}`, nested). Adds server-side binary
    /// detection, byte size, truncation head, and the gateway-proxied raw/download URLs. A directory
    /// requested under `blob/` returns `{ redirect_to_tree: true }` (kind mismatch → client redirect);
    /// an absent path is `NotFound` (404).
    fn blob_json(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        gitref: &str,
        path: &str,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        match repo.read_blob_at_path(gitref, path)? {
            BlobPathLookup::IsDir => {
                Ok(json!({ "redirect_to_tree": true, "ref": gitref, "path": path }))
            }
            BlobPathLookup::Missing => Err(DurableError::NotFound(format!(
                "no such file `{path}` at `{gitref}`"
            ))),
            BlobPathLookup::Found {
                bytes,
                oid,
                is_binary,
                size,
            } => {
                let is_truncated = !is_binary && size as usize > BLOB_INLINE_CAP;
                // The inline text: empty for binary (the download fallback renders instead), a head for a
                // large file, the whole content otherwise. NEVER a `split('\n')` of binary bytes.
                let contents = if is_binary {
                    String::new()
                } else if is_truncated {
                    let head: Vec<u8> = bytes.iter().take(BLOB_INLINE_CAP).copied().collect();
                    String::from_utf8_lossy(&head).to_string()
                } else {
                    String::from_utf8_lossy(&bytes).to_string()
                };
                let raw = format!(
                    "/{}/git/repos/{slug}/raw/{gitref}/{path}",
                    crate::catalogue::API_VERSION
                );
                let download = format!(
                    "/{}/git/repos/{slug}/download/{gitref}/{path}",
                    crate::catalogue::API_VERSION
                );
                Ok(json!({
                    "path": path,
                    "contents": contents,
                    "base_oid": oid.0,
                    "viewer_may_edit": true,
                    "is_binary": is_binary,
                    "size_bytes": size,
                    "is_truncated": is_truncated,
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
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc).map_err(map_durable_err)?;
        let (bytes, is_binary) = match repo.read_blob_at_path(gitref, path).map_err(map_durable_err)? {
            BlobPathLookup::Found { bytes, is_binary, .. } => (bytes, is_binary),
            BlobPathLookup::IsDir => {
                return Err(EdgeError::BadRequest("path is a directory, not a file".into()))
            }
            BlobPathLookup::Missing => {
                return Err(EdgeError::NotFound("no such file at that ref".into()))
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
            if attachment {
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
        tenant: &str,
        region: &str,
        slug: &str,
        gitref: &str,
        path: &str,
        expected_base: &str,
        contents: &str,
        principal: &Principal,
    ) -> Result<WebEditOutcome, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = Arc::new(self.store.open_repo(&loc)?);
        let full = format!("refs/heads/{gitref}");

        // GF-6: the current blob oid (or "" for a new file) is the CAS base.
        let current_base = repo
            .read_file_at_ref(&full, path)?
            .map(|(_, oid)| oid.0)
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
    ) -> Result<Option<PrRecord>, DurableError> {
        self.prs.get(&Self::loc(tenant, region, slug), number)
    }

    fn next_pr_number(&self, loc: &RepoLoc) -> u64 {
        // Peer-review finding #3: allocate from the FILENAME-authoritative max (not `list()`, which is a
        // tolerant view that skips a corrupt record — deriving the next number from it would REUSE a
        // corrupt highest PR's number and overwrite its file). A corrupt record still counts here.
        self.prs
            .max_pr_number(loc)
            .map(|m| m.unwrap_or(0) + 1)
            .unwrap_or(1)
    }

    pub fn open_pr(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        body: &Value,
        principal: &Principal,
    ) -> Result<PrRecord, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?; // 404 if the repo is absent
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
        let head_oid = if head_oid.is_empty() {
            // Qualify a bare branch name to `refs/heads/<name>`; a fully-qualified `refs/…` is used as-is.
            let qualified = if head_ref.starts_with("refs/") {
                head_ref.clone()
            } else {
                format!("refs/heads/{head_ref}")
            };
            match repo.read_ref(&qualified)? {
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
        let number = self.next_pr_number(&loc);
        let pr = PullRequest::open(
            number,
            base_ref,
            head_ref,
            Self::pseudonym(tenant, principal),
            body.get("draft").and_then(Value::as_bool).unwrap_or(false),
        );
        let mut rec = PrRecord::open(&pr, head_oid);
        rec.title = title;
        rec.body_md = body_md;
        rec.author_is_agent = Self::is_agent(principal);
        let now = now_unix();
        rec.created_at = Some(now); // R3.3 N1 — the header's "opened …" date, stamped once at open.
        rec.updated_at = Some(now);
        self.prs.open_pr(&loc, &rec)?;
        Ok(rec)
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
        viewer_pseudonym: &str,
        repo_slug: Option<&str>,
    ) -> Result<Vec<EnrichedPr>, DurableError> {
        let records = self.prs.list(loc)?;
        // ONE config read for the whole repo (fail static on error — see above).
        let config = match self.prs.get_protection(loc) {
            Ok(cfg) => Some(cfg),
            Err(_) => None, // read failed → every row's summary degrades to Unavailable below.
        };
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
        self.enrich_prs(&loc, &Self::pseudonym(tenant, principal), None)
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
        let candidates = self.scan_repo_slugs(tenant, region);
        let visible = self
            .repo_authz
            .visible_repos(principal, tenant, region, &candidates);
        let viewer = Self::pseudonym(tenant, principal);
        let mut out = Vec::new();
        for slug in visible {
            let loc = Self::loc(tenant, region, &slug);
            if self.store.open_repo(&loc).is_err() {
                continue; // a slug that lost its repo dir — skip, never error the whole front door.
            }
            out.extend(self.enrich_prs(&loc, &viewer, Some(&slug))?);
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
        let mut rec = self
            .prs
            .get(&loc, number)?
            .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))?;
        if let Some(g) = body.get("green_contexts").and_then(Value::as_array) {
            rec.green_contexts = g
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        if let Some(g) = body
            .get("fork_unendorsed_contexts")
            .and_then(Value::as_array)
        {
            rec.fork_unendorsed_contexts = g
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        if let Some(b) = body
            .get("codeowner_review_satisfied")
            .and_then(Value::as_bool)
        {
            rec.codeowner_review_satisfied = b;
        }
        if let Some(n) = body
            .get("outstanding_conversations")
            .and_then(Value::as_u64)
        {
            rec.outstanding_conversations = n as u32;
        }
        rec.updated_at = Some(now_unix());
        self.prs.put(&loc, &rec)?;
        Ok(rec)
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
        let loc = Self::loc(tenant, region, slug);
        let mut rec = self
            .prs
            .get(&loc, number)?
            .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))?;
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
        rec.reviews.push(ReviewRecord {
            reviewer_pseudonym: Self::pseudonym(tenant, principal),
            state: ReviewState::Submitted(v),
            is_agent: Self::is_agent(principal),
        });
        rec.updated_at = Some(now_unix());
        self.prs.put(&loc, &rec)?;
        Ok(rec)
    }

    pub fn endorse_fork_ci(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        body: &Value,
    ) -> Result<PrRecord, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let mut rec = self
            .prs
            .get(&loc, number)?
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
        for c in to_endorse {
            if !rec.endorsed_contexts.contains(&c) {
                rec.endorsed_contexts.push(c);
            }
        }
        rec.updated_at = Some(now_unix());
        self.prs.put(&loc, &rec)?;
        Ok(rec)
    }

    pub fn merge(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        principal: &Principal,
    ) -> Result<MergeAttempt, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = Arc::new(self.store.open_repo(&loc)?);
        let ref_store = self.open_durable_refstore(repo.clone(), slug, tenant, region, principal);
        // merge_pr sources the required set + thresholds from the REPO-OWNED ruleset (never author
        // input), validates head_oid against the on-disk repo, and advances the ref via the durable CAS
        // only on a fully-admitted gate.
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
    // object guard at the route); write = a real write grant (the `Push` object guard) — never a
    // permissive authorizer. Storage keys by the canonical `object_key` (`pr:<slug>:<n>`) so issues/
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
    fn require_pr(&self, loc: &RepoLoc, number: u64) -> Result<PrRecord, DurableError> {
        self.prs
            .get(loc, number)?
            .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))
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
        self.require_pr(&loc, number)?;
        let key = Self::pr_object_key(slug, number);
        let doc = self.threads.load(&loc, &key)?;
        let viewer = Self::pseudonym(tenant, principal);
        Ok(viewed_threads_json(&doc.view_for(&viewer)))
    }

    /// POST …/prs/{n}/threads — open a new thread with its first comment (`anchor` null = a discussion
    /// thread; an anchor object = a diff-line thread). Write = `Push`.
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
        self.require_pr(&loc, number)?;
        let key = Self::pr_object_key(slug, number);
        let body_md = require_body_md(body)?;
        let anchor = parse_anchor(body);
        let author = Self::thread_principal(tenant, principal);
        let thread = self
            .threads
            .create_thread(&loc, &key, anchor, author, body_md, now_unix())?;
        self.bump_pr_updated(&loc, number);
        Ok(thread_json(&thread))
    }

    /// POST …/prs/{n}/threads/{tid}/comments — reply to a thread. Write = `Push`.
    pub fn add_thread_comment(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        thread_id: &str,
        body: &Value,
        principal: &Principal,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number)?;
        let key = Self::pr_object_key(slug, number);
        let body_md = require_body_md(body)?;
        let author = Self::thread_principal(tenant, principal);
        let comment = self
            .threads
            .add_comment(&loc, &key, thread_id, author, body_md, now_unix())?;
        self.bump_pr_updated(&loc, number);
        Ok(comment_json(&comment))
    }

    /// POST …/prs/{n}/threads/{tid}/resolve — resolve/unresolve a thread. Write = `Push`.
    pub fn resolve_thread(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        thread_id: &str,
        body: &Value,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number)?;
        let key = Self::pr_object_key(slug, number);
        let resolved = body.get("resolved").and_then(Value::as_bool).unwrap_or(true);
        self.threads.resolve_thread(&loc, &key, thread_id, resolved)?;
        Ok(json!({ "thread_id": thread_id, "resolved": resolved }))
    }

    /// POST …/prs/{n}/reviews/start — start a review batch (draft; verdict `in_progress`). Write =
    /// `Push`. (`/start` distinguishes this from the existing single-shot `POST …/reviews` verdict op
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
        self.require_pr(&loc, number)?;
        let key = Self::pr_object_key(slug, number);
        let reviewer = Self::thread_principal(tenant, principal);
        let batch = self.threads.start_review(&loc, &key, reviewer)?;
        Ok(review_batch_json(&batch))
    }

    /// POST …/prs/{n}/reviews/{rid}/comments — add a PENDING comment to a draft batch (visible only to
    /// its author until submit). Write = `Push`.
    pub fn add_pending_comment(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        review_id: &str,
        body: &Value,
        principal: &Principal,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number)?;
        let key = Self::pr_object_key(slug, number);
        let body_md = require_body_md(body)?;
        let anchor = parse_anchor(body);
        let author = Self::thread_principal(tenant, principal);
        let comment = self.threads.add_pending_comment(
            &loc,
            &key,
            review_id,
            anchor,
            author,
            body_md,
            now_unix(),
        )?;
        Ok(comment_json(&comment))
    }

    /// POST …/prs/{n}/reviews/{rid}/submit `{ verdict, summary_md }` — submit the batch. Emits ONE
    /// batch event (R-BATCH-1; idempotent on retry). A NON-advisory (human) `approved` /
    /// `changes_requested` verdict ALSO feeds the merge gate via the durable review record (an agent
    /// batch stays advisory — it never gates). Write = `Push`.
    pub fn submit_review_batch(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        number: u64,
        review_id: &str,
        body: &Value,
        principal: &Principal,
    ) -> Result<Value, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        self.require_pr(&loc, number)?;
        let key = Self::pr_object_key(slug, number);
        let verdict = match body.get("verdict").and_then(Value::as_str) {
            Some("approved") | Some("approve") => BatchVerdict::Approved,
            Some("changes_requested") | Some("request-changes") | Some("request_changes") => {
                BatchVerdict::ChangesRequested
            }
            Some("commented") | Some("comment") | None => BatchVerdict::Commented,
            Some(other) => {
                return Err(DurableError::Git(format!("unknown review verdict `{other}`")))
            }
        };
        let summary_md = body
            .get("summary_md")
            .and_then(Value::as_str)
            .map(str::to_string);
        let actor = Self::thread_principal(tenant, principal);
        let submitted = self.threads.submit_review(
            &loc,
            &key,
            review_id,
            &actor,
            verdict,
            summary_md,
            now_unix(),
        )?;
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
        self.bump_pr_updated(&loc, number);
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
        self.require_pr(&loc, number)?;
        let key = Self::pr_object_key(slug, number);
        let actor = Self::thread_principal(tenant, principal);
        self.threads.discard_review(&loc, &key, review_id, &actor)?;
        Ok(json!({ "discarded": review_id }))
    }

    /// Bump the PR's `updated_at` after an authored conversation mutation (best-effort — a failure to
    /// re-stamp the record never fails the comment that already persisted).
    fn bump_pr_updated(&self, loc: &RepoLoc, number: u64) {
        if let Ok(Some(mut rec)) = self.prs.get(loc, number) {
            rec.updated_at = Some(now_unix());
            let _ = self.prs.put(loc, &rec);
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
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let mut green = Vec::new();
        let mut fork_unendorsed = Vec::new();
        let mut endorsed = Vec::new();
        if let Ok(prs) = self.prs.list(loc) {
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
            let exec = crate::git_wire_exec::GitWireExecutor::serving_default(self.root.clone());
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
                self.check_facts_for_head(&loc, u.new_oid.0.as_str());
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
        let eval = evaluate_merge(&ruleset, &rec).map_err(|e| DurableError::Git(e.to_string()))?;
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
        let (rows, has_more) = repo
            .commits_in_pr(&rec.base_ref, &rec.head_oid, 500)
            .ok()?;
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

/// **F3 (R4.1 dogfood) — the public base URL prefix for HTTP git-wire clone URLs.** Read once from
/// `MYELIN_PUBLIC_BASE_URL` at backend construction. UNSET (or empty) → the empty string, which makes
/// [`DurableGitBackend::clone_url`] emit an HONEST relative `/{tenant}/{region}/{repo}.git` (a real
/// path on whatever origin the edge is served from) rather than a fabricated hostname. A configured
/// value has any trailing `/` trimmed so the `{base}/{tenant}/…` join never doubles the slash.
fn public_clone_base() -> String {
    std::env::var("MYELIN_PUBLIC_BASE_URL")
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string()
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

/// Parse an optional diff-line content anchor from a POST body (`{ anchor: { path, line, side? } }`).
/// A fresh anchor is authored `live`; the store's re-resolution (moved/outdated) is the diff pack's
/// concern. `None` (absent anchor) = a PR-level discussion thread.
fn parse_anchor(body: &Value) -> Option<ThreadAnchor> {
    let a = body.get("anchor")?;
    let path = a.get("path").and_then(Value::as_str)?.to_string();
    let line = a.get("line").and_then(Value::as_u64);
    Some(ThreadAnchor {
        path,
        line,
        anchor_state: AnchorState::Live,
    })
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

/// The inline-text cap for a blob view (R3.4). A text blob larger than this renders a head + a
/// "download full file" affordance; binary blobs never render inline at all (download fallback).
const BLOB_INLINE_CAP: usize = 512 * 1024;

/// The short oid (first 12 chars) — the browse log/tree short form.
fn short_oid12(oid: &str) -> String {
    oid.chars().take(12).collect()
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
        // A traversal-rejected slug / malformed body (e.g. R3.1 open-PR with no `title`) surfaces as a
        // clean 400 (never a silent wrong path, never a 500 for a client input error).
        DurableError::Git(m)
            if m.contains("traversal")
                || m.contains("segment")
                || m.contains("slug")
                || m.contains("missing") =>
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
        let all = self
            .be
            .list_repos_visible(tenant_of(ctx), region_of(ctx), ctx.principal);
        let offset = ctx
            .page
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = ctx.page.limit;
        let items: Vec<Value> = all
            .iter()
            .skip(offset)
            .take(limit)
            .map(|r| r.to_json())
            .collect();
        let next = if offset.saturating_add(limit) < all.len() {
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
        Ok(EdgeResponse::json(200, &json!({ "items": items, "page": page })))
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
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                param(ctx, "ref")?,
                param(ctx, "path")?,
                expected_base,
                contents,
                ctx.principal,
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
        let rec = self
            .be
            .open_pr(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                &body,
                ctx.principal,
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
            .prs
            .get(&loc, num_param(ctx, "n")?)
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
            .prs
            .get(&loc, num_param(ctx, "n")?)
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such pull request".into()))?;
        let repo = self.be.store.open_repo(&loc).map_err(map_durable_err)?;
        let (metas, has_more) = repo
            .commits_in_pr(&rec.base_ref, &rec.head_oid, ctx.page.limit.min(500))
            .map_err(map_durable_err)?;
        let items: Vec<Value> = metas
            .into_iter()
            .map(|m| commit_row(m).to_json())
            .collect();
        let next = if has_more { Some("more".to_string()) } else { None };
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
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
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
/// oid); an absent/malformed oid → an empty `lines` (never a 500 for a stale expand request).
struct DFileLines {
    be: Arc<DurableGitBackend>,
}
impl Handler for DFileLines {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let start = ctx
            .request
            .query_param("start")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1);
        let end = ctx
            .request
            .query_param("end")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0); // 0 = to end-of-file
        let lines = self
            .be
            .file_lines(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                param(ctx, "oid")?,
                start,
                end,
            )
            .map_err(map_durable_err)?
            .unwrap_or_default();
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
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                param(ctx, "tid")?,
                &body,
                ctx.principal,
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
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
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

/// POST …/prs/{n}/reviews/start — start a review batch (draft). `Push`-guarded.
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

/// POST …/prs/{n}/reviews/{rid}/comments — add a pending comment to a draft batch. `Push`-guarded.
struct DPrReviewComment {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrReviewComment {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = ctx.request.json_body()?;
        let vm = self
            .be
            .add_pending_comment(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                param(ctx, "rid")?,
                &body,
                ctx.principal,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.pr.review.comment", "comment": vm }, "durable": true }),
        ))
    }
}

/// POST …/prs/{n}/reviews/{rid}/submit — submit the batch (ONE event, R-BATCH-1). `Push`-guarded.
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
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                param(ctx, "rid")?,
                &body,
                ctx.principal,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.pr.review.submit", "result": vm }, "durable": true }),
        ))
    }
}

/// DELETE …/prs/{n}/reviews/{rid} — discard a draft batch. `Push`-guarded.
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
            .prs
            .get(&loc, num_param(ctx, "n")?)
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such pull request".into()))?;
        let vm = self.be.pr_checks_json(&loc, &rec).map_err(map_durable_err)?;
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
        Some("created") => enriched.sort_by(|a, b| b.rec.number.cmp(&a.rec.number)),
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
            .list_prs_for_repo(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?, ctx.principal)
            .map_err(map_durable_err)?;
        let viewer =
            DurableGitBackend::pseudonym(tenant_of(ctx), ctx.principal);
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
        let rec = self
            .be
            .submit_review(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                verdict,
                ctx.principal,
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
        let rec = self
            .be
            .endorse_fork_ci(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                &body,
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
        let attempt = self
            .be
            .merge(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                ctx.principal,
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
                    .prs
                    .get(&loc, num_param(ctx, "n")?)
                    .ok()
                    .flatten()
                    .and_then(|rec| self.be.pr_checks_json(&loc, &rec).ok());
                Ok(EdgeResponse::json(
                    409,
                    &json!({
                        "error": {
                            "code": "merge_blocked",
                            "message": "merge blocked by branch protection",
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
        let rec = self
            .be
            .report_checks(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                ctx.principal,
                &body,
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
        if !self
            .be
            .repo_authorizer()
            .authorize_repo_permission(ctx.principal, &loc, self.permission)
        {
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
/// | web-edit commit / open-PR / PR review / CI check-report | `Push` | 403 |
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
                guarded(&be, Push, Arc::new(DPrReview { be: be.clone() })),
                "git.pr.review",
            ),
            // The X-1 endorsement: the DISTINCT approve_untrusted_ci relation (never collapsed to
            // write — endorsing an untrusted fork run is its own trust decision).
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/endorse-fork-ci") => (
                guarded(&be, ApproveUntrustedCi, Arc::new(DEndorse { be: be.clone() })),
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
    // ── R3.3 / R3.2 — the thread + review-batch surface. READ = `Pull` (thread read = PR view);
    //    WRITE = `Push` (comment/review write is a real write grant, never a permissive authorizer). ──
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
        guarded(&be, Push, Arc::new(DPrThreadCreate { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/threads/{tid}/comments"),
        "git.pr.comment.create",
        guarded(&be, Push, Arc::new(DPrThreadComment { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/threads/{tid}/resolve"),
        "git.pr.thread.resolve",
        guarded(&be, Push, Arc::new(DPrThreadResolve { be: be.clone() })),
    );
    // The review-batch lifecycle (G-8). `/reviews/start` (not `POST /reviews`) preserves the existing
    // single-shot `POST /reviews` verdict op that feeds the merge gate — a named deviation from N5's
    // literal path. Discard is `POST …/discard` (the gateway git grammar is Get/Post only, no DELETE).
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/reviews/start"),
        "git.pr.review.start",
        guarded(&be, Push, Arc::new(DPrReviewStart { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/reviews/{rid}/comments"),
        "git.pr.review.comment",
        guarded(&be, Push, Arc::new(DPrReviewComment { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/reviews/{rid}/submit"),
        "git.pr.review.submit",
        guarded(&be, Push, Arc::new(DPrReviewSubmit { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/reviews/{rid}/discard"),
        "git.pr.review.discard",
        guarded(&be, Push, Arc::new(DPrReviewDiscard { be: be.clone() })),
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
        std::env::temp_dir().join(format!("myelin-compensation-{tag}-{}-{nanos}", std::process::id()))
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
        let be = DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_bootstrap(boot.clone());
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
        let be = DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_bootstrap(boot.clone());
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
        let be = DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_bootstrap(boot.clone());
        let creator = principal("svc:creator", "acme");

        let err = be
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .expect_err("create fails");
        assert_eq!(boot.revokes.lock().unwrap().len(), 1, "compensation was attempted");
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
        let be = DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_bootstrap(boot.clone());
        let creator = principal("svc:creator", "acme");
        assert!(be
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .unwrap());
        // A second create of the same slug is a conflict (Ok(false)) — no second grant, no revoke.
        assert!(!be
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .unwrap());
        assert_eq!(boot.grants.lock().unwrap().len(), 1, "granted only on the first create");
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
        std::env::temp_dir().join(format!("myelin-prlist-{tag}-{}-{nanos}", std::process::id()))
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
    fn serve(
        handler: &dyn Handler,
        viewer: &Principal,
        repo: Option<&str>,
        query: &str,
    ) -> Value {
        let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
        let mut params = BTreeMap::new();
        if let Some(r) = repo {
            params.insert("repo".to_string(), r.to_string());
        }
        let req = EdgeRequest::new("GET", "/v1/git/prs", query, vec![], vec![]);
        let page = Page::from_request(&req);
        let ctx = HandlerCtx {
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
        let be = DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_authorizer(Arc::new(authz));
        let viewer = human("u:viewer");
        open_pr(&be, "core", "Only PR", &viewer);
        let handler = DRepoPrList { be: Arc::new(be) };
        let body = serve(
            &handler,
            &viewer,
            Some("core"),
            &format!("state=all&cursor={}", usize::MAX),
        );
        assert_eq!(body["items"].as_array().unwrap().len(), 0, "past-the-end page is empty");
        assert!(body["page"]["next_cursor"].is_null(), "no next past the end");
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
        assert!(be.open_pr(TENANT, REGION, "core", &ok_body, &author).is_ok());
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
        let be = DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_authorizer(Arc::new(authz));
        let viewer = human("u:viewer");
        // The viewer AUTHORS a PR in BOTH repos (so the bucket predicate `yours` WOULD match beta if
        // the prefilter leaked).
        open_pr(&be, "alpha", "Alpha change", &viewer);
        open_pr(&be, "beta", "Beta change (forbidden repo)", &viewer);

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
        let be = DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_authorizer(Arc::new(authz));
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
        assert_eq!(merged["counts"]["open"], 2, "the Open badge still reads 2 on the Merged tab");
        std::fs::remove_dir_all(&root).ok();
    }

    /// **Cursor stability: paging the sorted set with `limit` visits every row exactly once, in a
    /// stable order, with a correct bidirectional cursor** (`prev_cursor` None at the head; `next`
    /// None at the tail).
    #[test]
    fn per_repo_list_cursor_is_stable_and_bidirectional() {
        let root = temp_root("cursor");
        let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "core");
        let be = DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_authorizer(Arc::new(authz));
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
        let p2 = serve(&handler, &viewer, Some("core"), "state=all&limit=2&cursor=2");
        assert_eq!(p2["page"]["prev_cursor"], "0");
        assert_eq!(p2["page"]["next_cursor"], "4");

        // Page 3 (tail): 1 row, next None.
        let p3 = serve(&handler, &viewer, Some("core"), "state=all&limit=2&cursor=4");
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
        assert!(row["title"].is_null(), "empty title → null (the #number fallback is honest)");
        assert_eq!(row["number"], 9);
        assert_eq!(row["checks_summary"]["verdict"], "unavailable", "fails static, still lists");
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
        assert!(!url.contains("ssh://"), "no ssh scheme (there is no SSH server): {url}");
        assert!(!url.contains("git@myelin"), "no fabricated ssh host: {url}");

        // End-to-end through the repo-home projection (an empty repo still advertises the URL).
        let author = human("u:author");
        be.create_repo_as(TENANT, REGION, "widgets", &author).unwrap();
        let loc = DurableGitBackend::loc(TENANT, REGION, "widgets");
        let repo = be.store.open_repo(&loc).expect("open repo");
        let advertised = match be.repo_home(TENANT, REGION, "widgets", &repo) {
            RepoHome::Empty { clone_url, .. } | RepoHome::Populated { clone_url, .. } => clone_url,
            other => panic!("a fresh repo projects an Empty/Populated home, got {other:?}"),
        };
        assert!(advertised.ends_with("/acme/eu-west/widgets.git"), "got {advertised}");
        assert!(!advertised.contains("ssh://"), "no ssh in the projection: {advertised}");
        std::fs::remove_dir_all(&root).ok();
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
        repo.update_ref_cas("refs/heads/feature", None, Some(&tip), "create", "psn@acme.noreply")
            .expect("create feature ref");

        // Open with head_ref but NO head_oid → the resolver fills in the branch tip.
        let body = json!({
            "title": "resolve my head",
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
            // head_oid deliberately OMITTED
        });
        let rec = be.open_pr(TENANT, REGION, "core", &body, &author).expect("open PR");
        assert_eq!(
            rec.head_oid, tip.0,
            "F8: an omitted head_oid is resolved from head_ref's current tip"
        );

        // A bare (unqualified) head_ref resolves too (qualified to refs/heads/<name>).
        let body_bare = json!({ "title": "bare head_ref", "head_ref": "feature" });
        let rec2 = be.open_pr(TENANT, REGION, "core", &body_bare, &author).expect("open PR");
        assert_eq!(rec2.head_oid, tip.0, "F8: a bare branch name also resolves to the tip");

        // A non-existent head_ref → a clean 400 at OPEN (mapped from the durable error), NOT an empty
        // head_oid that wedges the merge dialog with "invalid merge head" later.
        let bad = json!({ "title": "ghost branch", "head_ref": "refs/heads/does-not-exist" });
        let err = be.open_pr(TENANT, REGION, "core", &bad, &author).expect_err("must refuse");
        assert_eq!(
            map_durable_err(err).status(),
            400,
            "F8: a non-existent head_ref is a 400 at open, not a merge-time surprise"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod pr_thread_tests {
    //! **R3.3 / R3.2 — the PR thread / comment / review-batch surface at the edge.** Drives the
    //! handlers to prove: (a) thread READ = PR view (the `Pull` guard) but WRITE = a real write grant
    //! (the `Push` guard — a read-only viewer is 403 on a comment, NOT AllowAll); (b) a pending review
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
        std::env::temp_dir().join(format!("myelin-prthread-{tag}-{}-{nanos}", std::process::id()))
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
        let ctx = HandlerCtx {
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
        let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        be.create_repo_as(TENANT, REGION, SLUG, &writer).unwrap();
        let body = json!({
            "title": "R3.3 flagship", "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature", "head_oid": head_oid, "draft": false,
        });
        be.open_pr(TENANT, REGION, SLUG, &body, &writer).unwrap();
        (Arc::new(be), writer, reader)
    }

    /// **Write = a REAL permission (Push), not AllowAll.** A read-only viewer may LIST threads (read =
    /// PR view) but is 403 on creating a comment (the `Push` object guard denies).
    #[test]
    fn thread_read_is_pr_view_but_write_needs_a_real_write_grant() {
        let (be, _writer, reader) = setup("authz", &"0".repeat(40));
        // The reader may LIST (Pull-guarded read).
        let list = guarded(&be, RepoPermission::Pull, Arc::new(DPrThreads { be: be.clone() }));
        let v = serve(&*list, "GET", &reader, &[("repo", SLUG), ("n", "1")], Value::Null)
            .expect("reader may read threads");
        assert!(v["threads"].is_array());
        // The reader may NOT create a comment (Push-guarded write) — 403, never a silent allow.
        let create =
            guarded(&be, RepoPermission::Push, Arc::new(DPrThreadCreate { be: be.clone() }));
        let err = serve(
            &*create,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            json!({ "body_md": "hi" }),
        )
        .expect_err("a read-only viewer must be forbidden from commenting");
        assert!(matches!(err, EdgeError::Forbidden(_)), "got {err:?}");
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
        let batch = serve(&*start, "POST", &reader, &[("repo", SLUG), ("n", "1")], Value::Null).unwrap();
        let rid = batch["applied"]["review"]["id"].as_str().unwrap().to_string();
        serve(
            &*pending,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
            json!({ "body_md": "draft note" }),
        )
        .unwrap();

        // The WRITER (a different viewer) sees NO thread and NO draft batch.
        let seen = serve(&*threads, "GET", &writer, &[("repo", SLUG), ("n", "1")], Value::Null).unwrap();
        assert_eq!(seen["threads"].as_array().unwrap().len(), 0, "pending comment is private");
        assert_eq!(seen["reviews"].as_array().unwrap().len(), 0, "draft batch is hidden");

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
        assert_eq!(again["applied"]["result"]["emitted"], false, "no double event");

        // Now the writer sees the (submitted) thread.
        let seen = serve(&*threads, "GET", &writer, &[("repo", SLUG), ("n", "1")], Value::Null).unwrap();
        assert_eq!(seen["threads"].as_array().unwrap().len(), 1, "submit makes it public");
    }

    /// A submitted human `changes_requested` batch flips the merge gate → the checks projection reads
    /// `changes_requested: true` and `gate_admitted: false` (the VERIFIED R2 gate input).
    #[test]
    fn a_changes_requested_batch_blocks_the_gate() {
        let (be, _writer, reader) = setup("blockgate", &"0".repeat(40));
        let start = Arc::new(DPrReviewStart { be: be.clone() });
        let submit = Arc::new(DPrReviewSubmit { be: be.clone() });
        let checks = Arc::new(DPrChecks { be: be.clone() });

        let batch = serve(&*start, "POST", &reader, &[("repo", SLUG), ("n", "1")], Value::Null).unwrap();
        let rid = batch["applied"]["review"]["id"].as_str().unwrap().to_string();
        serve(
            &*submit,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
            json!({ "verdict": "changes_requested" }),
        )
        .unwrap();
        let ck = serve(&*checks, "GET", &reader, &[("repo", SLUG), ("n", "1")], Value::Null).unwrap();
        assert_eq!(ck["changes_requested"], true, "the gate ingests changes_requested");
        assert_eq!(ck["gate_admitted"], false, "a live request-changes blocks the merge");
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
        let merge = guarded(&be, RepoPermission::ProtectedPush, Arc::new(DMerge { be: be.clone() }));
        let resp = serve(&*merge, "POST", &writer, &[("repo", SLUG), ("n", "1")], Value::Null)
            .expect("merge handler returns a body (409 is an Ok EdgeResponse, not an Err)");
        assert_eq!(resp["error"]["code"], "merge_blocked");
        assert_eq!(resp["checks"]["gate_admitted"], false, "the 409 carries the fresh gate state");
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
        let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        be.create_repo_as(TENANT, REGION, SLUG, &writer).unwrap();
        let loc = DurableGitBackend::loc(TENANT, REGION, SLUG);
        let repo = be.store.open_repo(&loc).unwrap();
        let b0 = repo.write_blob(b"a\nb\nc\n").unwrap();
        let t0 = repo.write_tree(&[("file.txt", &b0)]).unwrap();
        let base = repo
            .write_commit(&t0, &[], "base", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        repo.update_ref_cas("refs/heads/main", None, Some(&base), "c", "psn@acme.noreply")
            .unwrap();
        let bh = repo.write_blob(b"a\nB\nc\nd\n").unwrap();
        let th = repo.write_tree(&[("file.txt", &bh)]).unwrap();
        let head = repo
            .write_commit(&th, &[&base], "head", "psn@acme.noreply", "psn@acme.noreply")
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

    /// **R3.2 · G-7 N1 — the PR diff is `Pull`-guarded, 0-leak, three-dot, count-only restricted.** A
    /// reader (PR view) gets the three-dot diff; a stranger with NO grant gets a 0-leak 404 (never a
    /// distinguishable Forbidden, never a leaked path). `restricted_files` is count-only (0 here).
    #[test]
    fn pr_diff_is_pull_guarded_zero_leak_and_three_dot() {
        let (be, reader, _head) = setup_diff("diffauthz");
        let guard = guarded(&be, RepoPermission::Pull, Arc::new(DPrDiff { be: be.clone() }));
        // The reader sees the three-dot diff of file.txt (the PR's own change).
        let v = serve(&*guard, "GET", &reader, &[("repo", SLUG), ("n", "1")], Value::Null)
            .expect("a reader may view the PR diff");
        assert_eq!(v["number"], 1);
        assert_eq!(v["three_dot"], true, "durable repos are libgit2-backed → merge-base");
        assert_eq!(v["total_files"], 1);
        assert_eq!(v["files"][0]["path"], "file.txt");
        assert_eq!(v["files"][0]["status"], "M");
        assert_eq!(v["files"][0]["kind"], "text");
        // Line numbers cross the wire additively.
        let lines = v["files"][0]["hunks"][0]["lines"].as_array().unwrap();
        assert!(lines.iter().any(|l| l["origin"] == "+" && l["content"] == "d" && l["new_no"] == 4));
        // Restricted disclosure is COUNT-ONLY — the field is a number, never a path list.
        assert_eq!(v["restricted_files"], 0, "count-only; 0 under the repo-level Pull guard");
        assert!(v["restricted_files"].is_number());

        // A STRANGER with no grant → a 0-leak 404 (NotFound), never a distinguishable Forbidden.
        let stranger = human("u:stranger");
        let err = serve(&*guard, "GET", &stranger, &[("repo", SLUG), ("n", "1")], Value::Null)
            .expect_err("a stranger must not view the diff");
        assert!(matches!(err, EdgeError::NotFound(_)), "0-leak 404, got {err:?}");
    }

    /// A diff request for an ABSENT PR is a clean 404 (exactly like the overview — never a 500).
    #[test]
    fn pr_diff_absent_pr_is_not_found() {
        let (be, reader, _head) = setup_diff("diffabsent");
        let guard = guarded(&be, RepoPermission::Pull, Arc::new(DPrDiff { be: be.clone() }));
        let err = serve(&*guard, "GET", &reader, &[("repo", SLUG), ("n", "999")], Value::Null)
            .expect_err("absent PR");
        assert!(matches!(err, EdgeError::NotFound(_)), "got {err:?}");
    }
}
