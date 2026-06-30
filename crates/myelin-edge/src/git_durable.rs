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
use crate::request::EdgeResponse;
use myelin_events::{
    Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId, Timestamp,
};
use myelin_git::api::{http_catalogue, Method as GitMethod};
use myelin_git::core::{Oid as CoreOid, RepoLoc};
use myelin_git::durable::{DurableError, DurableGitRepo, DurableGitStore};
use myelin_git::lifecycle::{
    BranchProtectionRuleset, PrState, PullRequest, ReviewState, ReviewVerdict,
};
use myelin_git::pr_store::{
    evaluate_merge, merge_pr, BranchProtectionConfig, DurablePrStore, MergeAttempt, PrRecord,
    ReviewRecord,
};
use myelin_git::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid as PushOid, ProposedRefUpdate, PushOutcome, PushSession,
    Pusher, QuarantineMigration, QuarantineObject, RefName, RefStore,
};
use myelin_git::durable::{CommitDetail, CommitMeta};
use myelin_git::web::{CommitDiff, CommitRow, DiffFile, DiffLineView, RepoHome, WebEditForm, WebEditOutcome};
use myelin_identity::Principal;
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
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
    /// The `ssh://` clone host rendered into the repo-home ViewModel.
    clone_host: String,
    /// The on-disk root holding `<tenant>/<region>/<repo>.git` bare repos — retained so the wire-serving
    /// tier (CT-006b) composes its sandboxed `GitCore` over the SAME root the durable store reads/writes.
    root: PathBuf,
}

impl DurableGitBackend {
    /// Root the durable backend at an on-disk directory holding `<tenant>/<region>/<repo>.git` repos —
    /// the same root the durable git store + read backend resolve against.
    pub fn rooted(root: impl Into<PathBuf>) -> DurableGitBackend {
        let root = root.into();
        DurableGitBackend {
            store: DurableGitStore::rooted(root.clone()),
            prs: DurablePrStore::rooted(root.clone()),
            outbox: OutboxStore::new(),
            minter: Arc::new(MonotonicMinter::new()),
            clone_host: "ssh://git@myelin".into(),
            root,
        }
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
    pub fn create_repo(&self, tenant: &str, region: &str, slug: &str) -> Result<bool, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        if self.store.repo_exists(&loc) {
            return Ok(false);
        }
        self.store.create_repo(&loc)?;
        Ok(true)
    }

    // ── repo list (durable read) ──

    /// List the verified tenant's repos from disk, building the [`RepoHome`] ViewModel from the REAL
    /// on-disk state (Populated with the default-branch tree, or Empty if no commits) — never a seed.
    fn list_repos(&self, tenant: &str, region: &str) -> Vec<RepoHome> {
        let mut out = Vec::new();
        // The tenant/region dir holds `<repo>.git` bare repos. Resolve via a representative locator's
        // parent so the scan stays inside the validated tenant/region path (no traversal).
        let probe = Self::loc(tenant, region, "_probe");
        let Ok(probe_path) = self.store.repo_path(&probe) else {
            return out;
        };
        let Some(dir) = probe_path.parent() else {
            return out;
        };
        let Ok(rd) = std::fs::read_dir(dir) else {
            return out;
        };
        let mut slugs: Vec<String> = Vec::new();
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(slug) = name.strip_suffix(".git") {
                slugs.push(slug.to_string());
            }
        }
        slugs.sort();
        for slug in slugs {
            let loc = Self::loc(tenant, region, &slug);
            let Ok(repo) = self.store.open_repo(&loc) else {
                continue;
            };
            out.push(self.repo_home(tenant, &slug, &repo));
        }
        out
    }

    fn repo_home(&self, tenant: &str, slug: &str, repo: &DurableGitRepo) -> RepoHome {
        let full_slug = format!("{tenant}/{slug}");
        let clone_url = format!("{}/{tenant}/{slug}.git", self.clone_host);
        let entries = repo.tree_entries_at_ref("refs/heads/main").unwrap_or_default();
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

    /// One repo's home ViewModel (`GET /v1/git/repos/{repo}`) — the durable per-repo home the browse
    /// UI lands on. `NotFound` (404) if the repo is absent under the verified tenant (the 0-leak posture:
    /// a cross-tenant repo simply is not found under this tenant's path).
    fn repo_home_one(&self, tenant: &str, region: &str, slug: &str) -> Result<RepoHome, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        Ok(self.repo_home(tenant, slug, &repo))
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

    // ── blob view (durable read) ──

    /// The single-file view ViewModel built from the durable on-disk blob (`None` if the repo/file is
    /// absent). `base_oid` is the REAL blob content-address (the GF-6 CAS base the next edit keys on).
    fn blob_form(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        gitref: &str,
        path: &str,
    ) -> Result<Option<WebEditForm>, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = match self.store.open_repo(&loc) {
            Ok(r) => r,
            Err(DurableError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let full = format!("refs/heads/{gitref}");
        match repo.read_file_at_ref(&full, path)? {
            Some((bytes, oid)) => Ok(Some(WebEditForm {
                path: path.to_string(),
                contents: String::from_utf8_lossy(&bytes).to_string(),
                base_oid: oid.0,
                viewer_may_edit: true,
            })),
            None => Ok(None),
        }
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
        let (new_commit, _new_blob, parent) = repo.build_file_commit(
            &full,
            path,
            contents.as_bytes(),
            "web edit",
            &psn,
            &psn,
        )?;

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
        self.prs
            .list(loc)
            .map(|v| v.iter().map(|r| r.number).max().unwrap_or(0) + 1)
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
        self.store.open_repo(&loc)?; // 404 if the repo is absent
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
        let number = self.next_pr_number(&loc);
        let pr = PullRequest::open(
            number,
            base_ref,
            head_ref,
            Self::pseudonym(tenant, principal),
            body.get("draft").and_then(Value::as_bool).unwrap_or(false),
        );
        let rec = PrRecord::open(&pr, head_oid);
        self.prs.open_pr(&loc, &rec)?;
        Ok(rec)
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
                            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
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
        self.prs.put_protection(&loc, &BranchProtectionConfig { rulesets })?;
        Ok(n)
    }

    /// **CI check-report (GT-003).** The authorized producer stamps the green / fork-unendorsed check
    /// facts on a PR for its head (the real CI producer is M4; the PR AUTHOR cannot call this — the edge
    /// gates the distinct `git.checks.report` action). The facts the merge gate reads come from HERE,
    /// never from the PR-open body.
    pub fn report_checks(
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
        if let Some(g) = body.get("green_contexts").and_then(Value::as_array) {
            rec.green_contexts = g.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }
        if let Some(g) = body.get("fork_unendorsed_contexts").and_then(Value::as_array) {
            rec.fork_unendorsed_contexts =
                g.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }
        if let Some(b) = body.get("codeowner_review_satisfied").and_then(Value::as_bool) {
            rec.codeowner_review_satisfied = b;
        }
        if let Some(n) = body.get("outstanding_conversations").and_then(Value::as_u64) {
            rec.outstanding_conversations = n as u32;
        }
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
            other => return Err(DurableError::Git(format!("unknown review verdict `{other}`"))),
        };
        rec.reviews.push(ReviewRecord {
            reviewer_pseudonym: Self::pseudonym(tenant, principal),
            state: ReviewState::Submitted(v),
            is_agent: false,
        });
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
            Some(a) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
            None => rec.fork_unendorsed_contexts.clone(),
        };
        for c in to_endorse {
            if !rec.endorsed_contexts.contains(&c) {
                rec.endorsed_contexts.push(c);
            }
        }
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
        let mut refs: Vec<(String, String)> =
            repo.list_refs()?.into_iter().map(|(n, o)| (n, o.0)).collect();
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
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let qdir = std::env::temp_dir().join(format!("myelin-ct006d-q-{}-{nanos}", std::process::id()));
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
        for c in &cmds {
            let new_zero = c.new.chars().all(|ch| ch == '0');
            let old_zero = c.old.chars().all(|ch| ch == '0');
            if !new_zero && !q.commit_tree_complete(&CoreOid::new(c.new.clone())).unwrap_or(false) {
                let _ = std::fs::remove_dir_all(&qdir);
                return Ok(report_status(
                    "ok",
                    &all_ng(&cmds, "rejected: incomplete object set (missing tree/blob) for a ref"),
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
                expected_old: if old_zero { PushOid::zero() } else { PushOid::new(c.old.clone()) },
                new_oid: if new_zero { PushOid::zero() } else { PushOid::new(c.new.clone()) },
                forced,
                commit_oids: if new_zero { vec![] } else { vec![PushOid::new(c.new.clone())] },
            });
            per_ref_status.push((c.ref_name.clone(), None));
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
        let migration = ObjectPromotion { repo: &repo, objects: &objects };
        let outcome = ref_store.receive(&push, &migration, CrashPoint::None);
        let _ = std::fs::remove_dir_all(&qdir); // the host quarantine is discarded either way

        match outcome.map_err(|e| DurableError::Git(format!("ref-CAS: {e:?}")))? {
            PushOutcome::Accepted { .. } => Ok(report_status("ok", &per_ref_status)),
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
            "pr_state": match rec.state {
                PrState::Draft => "draft",
                PrState::Open => "open",
                PrState::Merged => "merged",
                PrState::Closed => "closed",
            },
            "base_ref": rec.base_ref,
            "head_ref": rec.head_ref,
            "head_oid": rec.head_oid,
            "author": rec.author_pseudonym,
            "reviews": rec.reviews.len(),
            "durable": true,
        })
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
            let written = self.repo.write_raw_object(ty, bytes).map_err(|e| e.to_string())?;
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

/// Qualify a bare ref (`main`) to `refs/heads/main`; a fully-qualified `refs/…` passes through.
fn qualify_ref(gitref: &str) -> String {
    if gitref.starts_with("refs/") {
        gitref.to_string()
    } else {
        format!("refs/heads/{gitref}")
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

fn map_durable_err(e: DurableError) -> EdgeError {
    match e {
        DurableError::NotFound(m) => EdgeError::NotFound(m),
        // A traversal-rejected slug / bad input surfaces as a clean 400 (never a silent wrong path).
        DurableError::Git(m) if m.contains("traversal") || m.contains("segment") || m.contains("slug") => {
            EdgeError::BadRequest(m)
        }
        DurableError::CasMismatch { .. } => EdgeError::Conflict(e.to_string()),
        other => EdgeError::Internal(other.to_string()),
    }
}

struct DRepoList {
    be: Arc<DurableGitBackend>,
}
impl Handler for DRepoList {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let all = self.be.list_repos(tenant_of(ctx), region_of(ctx));
        let offset = ctx
            .page
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = ctx.page.limit;
        let items: Vec<Value> = all.iter().skip(offset).take(limit).map(|r| r.to_json()).collect();
        let next = if offset + limit < all.len() {
            Some((offset + limit).to_string())
        } else {
            None
        };
        Ok(EdgeResponse::json(200, &page_envelope(json!(items), next, limit)))
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
        let created = self
            .be
            .create_repo(tenant_of(ctx), region_of(ctx), slug)
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
        let home = self
            .be
            .repo_home_one(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?)
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &home.to_json()))
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
            Some((offset + limit).to_string())
        } else {
            None
        };
        Ok(EdgeResponse::json(200, &page_envelope(json!(items), next, limit)))
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
        let form = self
            .be
            .blob_form(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                param(ctx, "ref")?,
                param(ctx, "path")?,
            )
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such file at that ref".into()))?;
        Ok(EdgeResponse::json(200, &form.to_json()))
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
            .open_pr(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?, &body, ctx.principal)
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
        Ok(EdgeResponse::json(200, &DurableGitBackend::pr_json(&rec)))
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
        // The required set comes from the REPO-OWNED ruleset for the base ref (never author input).
        let ruleset = self
            .be
            .prs
            .effective_ruleset_for(&loc, &rec.base_ref)
            .map_err(map_durable_err)?;
        let eval = evaluate_merge(&ruleset, &rec).map_err(|e| EdgeError::Internal(e.to_string()))?;
        Ok(EdgeResponse::json(
            200,
            &json!({
                "required_contexts": ruleset.required_contexts,
                "required_approvals": ruleset.required_approvals,
                "green_contexts": rec.green_contexts,
                "endorsed_contexts": rec.endorsed_contexts,
                // The X-1 fork-trust surface: contexts that passed on an UNTRUSTED FORK run and are
                // recorded-but-neutral until a maintainer endorses them (the badge the UI renders).
                "fork_unendorsed_contexts": rec.fork_unendorsed_contexts,
                "gate_admitted": eval.admitted(),
                "durable": true,
            }),
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
            MergeAttempt::Merged { base_ref, new_oid, update_seq } => Ok(EdgeResponse::json(
                200,
                &json!({
                    "applied": { "action": "git.pr.merge", "merged": true, "base_ref": base_ref,
                                 "new_oid": new_oid, "update_seq": update_seq },
                    "durable": true,
                }),
            )),
            // The merge gate BLOCKED — a loud, honest refusal (no ref advanced). 409: the PR is not in a
            // mergeable state. The body names the gate outcome so the UI can humanise it.
            MergeAttempt::Blocked(eval) => Err(EdgeError::Conflict(format!(
                "merge blocked by policy (required-set admitted: {}, ruleset satisfied: {})",
                eval.gate.is_admitted(),
                eval.ruleset.is_satisfied()
            ))),
            MergeAttempt::RefRefused(reason) => Err(EdgeError::Conflict(format!(
                "merge ref advance refused: {reason:?}"
            ))),
            // An arbitrary / non-existent / non-descendant head — refused, no ref advance (never advance
            // a protected ref to an arbitrary oid). 422: the merge target is unprocessable.
            MergeAttempt::InvalidHead(why) => Err(EdgeError::BadRequest(format!(
                "invalid merge head: {why}"
            ))),
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

/// **Register Git through the product edge over the DURABLE backend (GT-003).** Iterates Git's OWN
/// catalogue (anti-duplication — the route set is Git's, re-rooted under `/v1/git/...`) and binds the
/// durable handlers. The gateway owns auth/scope/IDOR/error/pagination; every write persists on the real
/// on-disk backend under `ctx.scope` (the verified tenant + region), the merge passes the gate, and the
/// resolver is traversal-safe.
pub fn register_git_durable(mut b: GatewayBuilder, be: Arc<DurableGitBackend>) -> GatewayBuilder {
    for ep in http_catalogue() {
        let pattern = reroot(ep.path);
        let method = map_method(ep.method);
        let (handler, action): (Arc<dyn Handler>, &'static str) = match (ep.method, ep.path) {
            (GitMethod::Get, "/api/git/repos") => {
                (Arc::new(DRepoList { be: be.clone() }), "git.repos.list")
            }
            (GitMethod::Post, "/api/git/repos") => {
                (Arc::new(DRepoCreate { be: be.clone() }), "git.repo.create")
            }
            (GitMethod::Get, "/api/git/repos/{repo}/prs/{n}") => {
                (Arc::new(DPrOverview { be: be.clone() }), "git.pr.view")
            }
            (GitMethod::Get, "/api/git/repos/{repo}/prs/{n}/checks") => {
                (Arc::new(DPrChecks { be: be.clone() }), "git.pr.checks")
            }
            (GitMethod::Get, "/api/git/repos/{repo}/blob/{ref}/{path}") => {
                (Arc::new(DBlobView { be: be.clone() }), "git.blob.view")
            }
            (GitMethod::Post, "/api/git/repos/{repo}/blob/{ref}/{path}") => {
                (Arc::new(DWebEditCommit { be: be.clone() }), "git.blob.commit")
            }
            (GitMethod::Post, "/api/git/repos/{repo}/prs") => {
                (Arc::new(DOpenPr { be: be.clone() }), "git.pr.open")
            }
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/reviews") => {
                (Arc::new(DPrReview { be: be.clone() }), "git.pr.review")
            }
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/endorse-fork-ci") => {
                (Arc::new(DEndorse { be: be.clone() }), "git.pr.endorse_fork_ci")
            }
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/merge") => {
                (Arc::new(DMerge { be: be.clone() }), "git.pr.merge")
            }
            // Repo-admin: set branch-protection policy — a DISTINCT authorize action (the production
            // authorizer resolves `Id.check(repo_admin)`; a non-admin is rejected by the gateway).
            (GitMethod::Post, "/api/git/repos/{repo}/branch-protection") => (
                Arc::new(DSetBranchProtection { be: be.clone() }),
                "git.repo.branch_protection.set",
            ),
            // CI check-report — a DISTINCT authorize action (the producer is CI/M4; a PR author is not
            // granted it). The PR author cannot stamp greens.
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/checks") => {
                (Arc::new(DReportChecks { be: be.clone() }), "git.checks.report")
            }
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
    // catalogue routes (the gateway owns auth/scope/IDOR/error/pagination per route). All GET (reads).
    let get = map_method(GitMethod::Get);
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}"),
        "git.repo.view",
        Arc::new(DRepoHome { be: be.clone() }),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/commits/{ref}"),
        "git.commits.log",
        Arc::new(DCommitLog { be: be.clone() }),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/commit/{oid}"),
        "git.commit.diff",
        Arc::new(DCommitDiff { be: be.clone() }),
    );
    b
}
