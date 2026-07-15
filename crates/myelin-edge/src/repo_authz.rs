//! # The per-repo object-authorization seam at the git wire (R0.3 / DELTA N2, HIGH)
//!
//! **The security invariant this seam closes: no un-granted repo reach.** The gateway's action-only
//! [`Authorizer`](myelin_substrate::Authorizer) gates a wire call on the ACTION
//! (`git.wire.upload_pack` / `git.wire.receive_pack`) — but NOT on the OBJECT (the specific repo). So
//! before R0.3 ANY in-tenant principal who held the action could clone/fetch/push ANY repo in the
//! tenant: an in-tenant principal with no grant on repo X could still read/write X. That is a missing
//! object-level authorization leg.
//!
//! This module is the **WIRE object-authz seam** — a per-repo `(principal, repo, access)` decision the
//! three wire handlers consult AFTER `repo_loc` resolves the repo and BEFORE any bytes/refs are
//! served. A denial is FAIL-CLOSED. It is DELIBERATELY narrow: it authorizes REPO objects on the git
//! wire only. It does NOT touch the platform-wide [`Authorizer`] trait.
//!
//! ## The R2 seed (named follow-on)
//! **R0.3 is the SEED of the R2 platform-wide object-authz seam.** R2 generalises object authorization
//! across every subsystem (backed by the real Zanzibar tuple store / Identity `check(subject, verb,
//! object)`). Here we build the git-wire slice R2 extends: the [`RepoAuthorizer`] trait + the
//! [`RepoAccess`] read/write split are shaped so R2 can back them with the real tuple store WITHOUT
//! changing the call sites in [`crate::git_wire_http`]. Production boot injects a grant-backed
//! implementation; the fixtures below prove the seam is load-bearing (the deny path is provable), not
//! vacuous — exactly as [`crate::authz::AllowAll`] does for the action-only gateway seam.
//!
//! **TODO(R2):** back [`RepoAuthorizer`] with the real per-repo grant store / Identity `check()` in the
//! production composition root (the analogue of injecting the Identity-M1 authorizer into the gateway).

use myelin_git::core::RepoLoc;
use myelin_identity::Principal;
use std::collections::BTreeSet;

/// The access a wire route needs on a repo object: a READ (clone/fetch/upload-pack) or a WRITE
/// (push/receive-pack). The read/write split is the R2-extensible shape — R2 maps these onto the
/// platform verb grammar (`git.repo.read` / `git.repo.write`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepoAccess {
    /// A read of the repo (the upload-pack advert + serve — clone/fetch).
    Read,
    /// A write to the repo (the receive-pack advert + push).
    Write,
}

/// **The frozen-fragment repo permission an object-addressed route needs (R2.1 — the coarse
/// [`RepoAccess`] split generalised onto the 4.9 Git fragment's permission grammar).** The wire's
/// Read/Write pair could not express the TIGHTER permissions (`protected_push` gates merge +
/// branch-protection; `approve_untrusted_ci` gates the fork-CI endorsement), so the git JSON product
/// API's handlers could only be action-gated — the exact live bypass R2.1 closes. Each variant names
/// the compiled permission the production authorizer checks on `repo:<slug>`:
///
/// | variant | fragment permission | admits (frozen 4.9) |
/// |---|---|---|
/// | `Pull` | `repo.pull` | reader ∪ writer ∪ admin ∪ parent_project->view |
/// | `Push` | `repo.push` | writer ∪ admin ∪ parent_project->view |
/// | `ProtectedPush` | `repo.protected_push` | **admin only** (the merge / branch-protection gate) |
/// | `ApproveUntrustedCi` | `repo.approve_untrusted_ci` | the endorsement relation (NOT implied by admin) |
///
/// PR/blob/ref reads REDUCE to `Pull` via the fragment's tuple-to-userset arms
/// (`pull_request.view = parent_repo->pull`), and `pull_request.merge = parent_repo->protected_push`
/// reduces to `ProtectedPush` on the parent repo — so a repo-scoped check with the RIGHT variant is
/// exactly the object decision for every object-addressed git route (the parent repo is always the
/// validated `{repo}` path segment).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepoPermission {
    /// `repo.pull` — every read (repo home / commit log / commit diff / blob view / PR view / PR
    /// checks / clone / fetch). A denial is the 0-leak 404 (repo existence is never leaked).
    Pull,
    /// `repo.push` — the ordinary writes (web-edit commit / open-PR / PR review / CI check-report /
    /// wire push). A denial is a fail-closed 403.
    Push,
    /// `repo.protected_push` — the admin-only transitions: PR **merge**
    /// (`pull_request.merge = parent_repo->protected_push`, §5-frozen) and **set
    /// branch-protection** (repo-admin policy). A plain `Push` grant does NOT admit these.
    ProtectedPush,
    /// `repo.approve_untrusted_ci` — the X-1 fork-CI endorsement relation. Deliberately its OWN
    /// grant (the fragment does not fold it into `admin`): endorsing an untrusted fork run is a
    /// distinct trust decision.
    ApproveUntrustedCi,
}

/// **The per-repo object-authorization seam (R0.3 / DELTA N2 → R2.1, the platform object-authz
/// shape).** Given the VERIFIED principal, the resolved `(tenant, region, repo)` locator, and the
/// [`RepoPermission`] the route needs, decide whether the principal may reach THIS repo. Consulted
/// by every git wire handler (R0.3) AND — since R2.1 — by every object-addressed git JSON product
/// handler, after the route resolves the repo and before any bytes/view-models are served; a
/// `false` is fail-closed (a `Pull` denial is a 0-leak 404, every other denial a 403). The
/// cross-tenant IDOR reject still runs FIRST in the gateway — this seam is the IN-TENANT per-repo
/// grant check the action-only authorizer cannot do.
pub trait RepoAuthorizer: Send + Sync {
    /// May `principal` exercise the frozen-fragment `permission` on `repo`? Fail-closed on any
    /// denial. THE one object decision every git route reduces to (R2.1) — implementations map each
    /// variant onto the real compiled permission (never collapse `ProtectedPush`/
    /// `ApproveUntrustedCi` down to a Read/Write pair; that collapse is the bypass this seam closes).
    fn authorize_repo_permission(
        &self,
        principal: &Principal,
        repo: &RepoLoc,
        permission: RepoPermission,
    ) -> bool;

    /// May `principal` perform `access` on `repo`? The R0.3 wire entry point, kept so the wire call
    /// sites are unchanged — provided as the exact mapping onto the permission-aware seam
    /// (`Read → Pull`, `Write → Push`; the wire has no tighter routes).
    fn authorize_repo(&self, principal: &Principal, repo: &RepoLoc, access: RepoAccess) -> bool {
        let permission = match access {
            RepoAccess::Read => RepoPermission::Pull,
            RepoAccess::Write => RepoPermission::Push,
        };
        self.authorize_repo_permission(principal, repo, permission)
    }

    /// **The leak-free LIST prefilter (R2.1 — the `list_objects` seam for `GET /v1/git/repos`).**
    /// Which of `candidates` (repo slugs under the VERIFIED `(tenant, region)` — the on-disk
    /// listing) may `principal` `pull`? The returned subset preserves the input order. This is the
    /// ADR-07 conjoin analogue at the edge: resolve the visible repo set, INTERSECT with the on-disk
    /// listing — never serve the full set and post-filter in the client.
    ///
    /// The provided default settles each candidate through
    /// [`RepoAuthorizer::authorize_repo_permission`] (`Pull`) — correct and leak-free for the
    /// fixtures. The production authorizer overrides this with the Identity `list_objects`
    /// `Ids`-materialise fast path (one reverse-index read instead of N checks) and falls back to
    /// the per-candidate check where the index cannot answer (see
    /// [`crate::repo_authz_live::CheckBackedRepoAuthorizer`]).
    fn visible_repos(
        &self,
        principal: &Principal,
        tenant: &str,
        region: &str,
        candidates: &[String],
    ) -> Vec<String> {
        candidates
            .iter()
            .filter(|slug| {
                self.authorize_repo_permission(
                    principal,
                    &RepoLoc::new(tenant, region, slug.as_str()),
                    RepoPermission::Pull,
                )
            })
            .cloned()
            .collect()
    }
}

/// The M0 seam fixture that admits every `(principal, repo, access)` — the analogue of
/// [`crate::authz::AllowAll`] for the wire object-authz seam. It lets the happy-path wire proofs (real
/// `git clone`/`git push`) dispatch while the real per-repo grant store is the R2 follow-on. Production
/// injects a grant-backed [`RepoAuthorizer`]; [`DenyAllRepos`] / [`GrantBackedRepos`] prove the deny
/// path is a real 404/403, so the seam is load-bearing, not vacuous.
pub struct AllowAllRepos;

impl RepoAuthorizer for AllowAllRepos {
    fn authorize_repo_permission(
        &self,
        _principal: &Principal,
        _repo: &RepoLoc,
        _permission: RepoPermission,
    ) -> bool {
        true
    }
}

/// A seam fixture that DENIES every `(principal, repo, permission)` — the analogue of the substrate's
/// `DenyAll`. Proves the wire + product handlers consult the seam (a denial is a 0-leak 404 on read /
/// a 403 on write) — the seam is not vacuous.
pub struct DenyAllRepos;

impl RepoAuthorizer for DenyAllRepos {
    fn authorize_repo_permission(
        &self,
        _principal: &Principal,
        _repo: &RepoLoc,
        _permission: RepoPermission,
    ) -> bool {
        false
    }
}

/// The relation a [`GrantBackedRepos`] grant models — the smallest mirror of the frozen fragment's
/// repo relations (`reader` / `writer` / `admin` / `approve_untrusted_ci`), so the fixture expresses
/// the SAME permission lattice the production tuple store realises (a writer is not an admin; an
/// endorser is its own relation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GrantKind {
    /// `reader` — admits `Pull` only.
    Read,
    /// `writer` — admits `Push` (and `Pull`; write implies read). Does NOT admit `ProtectedPush`.
    Write,
    /// `admin` — admits `Pull` / `Push` / `ProtectedPush` (the fragment: `protected_push = admin`).
    /// Deliberately does NOT admit `ApproveUntrustedCi` (the fragment keeps it a distinct relation).
    Admin,
    /// `approve_untrusted_ci` — admits `ApproveUntrustedCi` only.
    EndorseForkCi,
}

/// A grant-backed [`RepoAuthorizer`] fixture — the smallest real model of the R2 tuple store. Holds an
/// explicit allow-set of `(principal_id, tenant, repo, relation)` grants mirroring the frozen
/// fragment's lattice: `admin ⊇ {pull, push, protected_push}`, `writer ⊇ {pull, push}`,
/// `reader ⊇ {pull}`, `approve_untrusted_ci` its own relation. A principal with NO grant on a repo is
/// DENIED (the un-granted-repo-reach hole, closed). This is the deny-by-default model the R2 grant
/// seam realises with durable tuples.
#[derive(Default)]
pub struct GrantBackedRepos {
    /// `(principal_id, tenant, repo, relation)` — the explicit allow-set (deny-by-default otherwise).
    grants: BTreeSet<(String, String, String, GrantKind)>,
}

impl GrantBackedRepos {
    /// A fresh, empty grant store (denies everything until granted).
    pub fn new() -> Self {
        Self::default()
    }

    fn grant(mut self, principal_id: &str, tenant: &str, repo: &str, kind: GrantKind) -> Self {
        self.grants.insert((
            principal_id.to_string(),
            tenant.to_string(),
            repo.to_string(),
            kind,
        ));
        self
    }

    /// Grant `principal_id` READ (`reader`) on `(tenant, repo)`.
    pub fn grant_read(self, principal_id: &str, tenant: &str, repo: &str) -> Self {
        self.grant(principal_id, tenant, repo, GrantKind::Read)
    }

    /// Grant `principal_id` WRITE (`writer`) on `(tenant, repo)` (write implies read; a writer is
    /// NOT an admin — `ProtectedPush` stays denied).
    pub fn grant_write(self, principal_id: &str, tenant: &str, repo: &str) -> Self {
        self.grant(principal_id, tenant, repo, GrantKind::Write)
    }

    /// Grant `principal_id` ADMIN on `(tenant, repo)` — admits `Pull`/`Push`/`ProtectedPush` (the
    /// fragment's `protected_push = admin`), but NOT `ApproveUntrustedCi` (its own relation).
    pub fn grant_admin(self, principal_id: &str, tenant: &str, repo: &str) -> Self {
        self.grant(principal_id, tenant, repo, GrantKind::Admin)
    }

    /// Grant `principal_id` the fork-CI ENDORSEMENT relation (`approve_untrusted_ci`) on
    /// `(tenant, repo)` — admits `ApproveUntrustedCi` only.
    pub fn grant_endorse_fork_ci(self, principal_id: &str, tenant: &str, repo: &str) -> Self {
        self.grant(principal_id, tenant, repo, GrantKind::EndorseForkCi)
    }
}

impl RepoAuthorizer for GrantBackedRepos {
    fn authorize_repo_permission(
        &self,
        principal: &Principal,
        repo: &RepoLoc,
        permission: RepoPermission,
    ) -> bool {
        let has = |k: GrantKind| {
            self.grants.contains(&(
                principal.principal_id.0.clone(),
                repo.tenant.to_string(),
                repo.repo.to_string(),
                k,
            ))
        };
        // The frozen lattice, deny-by-default on the stricter permission: admin ⊇ push ⊇ pull;
        // approve_untrusted_ci is its own relation (not implied by admin — the fragment keeps the
        // endorsement a distinct trust decision).
        match permission {
            RepoPermission::Pull => {
                has(GrantKind::Read) || has(GrantKind::Write) || has(GrantKind::Admin)
            }
            RepoPermission::Push => has(GrantKind::Write) || has(GrantKind::Admin),
            RepoPermission::ProtectedPush => has(GrantKind::Admin),
            RepoPermission::ApproveUntrustedCi => has(GrantKind::EndorseForkCi),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn principal(id: &str, tenant: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Service,
            TenantId(tenant.into()),
        )
    }

    #[test]
    fn allow_all_admits_and_deny_all_refuses_both_accesses() {
        let p = principal("p", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        for access in [RepoAccess::Read, RepoAccess::Write] {
            assert!(AllowAllRepos.authorize_repo(&p, &repo, access));
            assert!(!DenyAllRepos.authorize_repo(&p, &repo, access));
        }
    }

    #[test]
    fn grant_backed_denies_without_a_grant() {
        // An in-tenant principal with NO grant on the repo is denied BOTH read and write (the
        // un-granted-repo-reach hole, closed).
        let authz = GrantBackedRepos::new();
        let p = principal("mallory", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        assert!(!authz.authorize_repo(&p, &repo, RepoAccess::Read));
        assert!(!authz.authorize_repo(&p, &repo, RepoAccess::Write));
    }

    #[test]
    fn grant_backed_read_grant_admits_read_only() {
        let authz = GrantBackedRepos::new().grant_read("reader", "acme", "widgets");
        let p = principal("reader", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        assert!(authz.authorize_repo(&p, &repo, RepoAccess::Read));
        // A read grant does NOT confer write.
        assert!(!authz.authorize_repo(&p, &repo, RepoAccess::Write));
        // A different repo is not reachable off this grant.
        let other = RepoLoc::new("acme", "eu-west", "secrets");
        assert!(!authz.authorize_repo(&p, &other, RepoAccess::Read));
    }

    #[test]
    fn grant_backed_write_grant_implies_read() {
        let authz = GrantBackedRepos::new().grant_write("dev", "acme", "widgets");
        let p = principal("dev", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        assert!(authz.authorize_repo(&p, &repo, RepoAccess::Write));
        assert!(
            authz.authorize_repo(&p, &repo, RepoAccess::Read),
            "a write grant implies read"
        );
        // A different principal with the same id spelled differently is not granted.
        let q = principal("dev2", "acme");
        assert!(!authz.authorize_repo(&q, &repo, RepoAccess::Read));
    }

    /// **R2.1 — the permission lattice is the fragment's, not a Read/Write collapse:** a WRITE grant
    /// admits `Push` but NOT `ProtectedPush` (merge / branch-protection) and NOT
    /// `ApproveUntrustedCi` (the endorsement relation). The exact under-collapse the JSON-API bypass
    /// rode on is unrepresentable.
    #[test]
    fn write_grant_does_not_admit_protected_push_or_endorse() {
        let authz = GrantBackedRepos::new().grant_write("dev", "acme", "widgets");
        let p = principal("dev", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        assert!(authz.authorize_repo_permission(&p, &repo, RepoPermission::Push));
        assert!(
            !authz.authorize_repo_permission(&p, &repo, RepoPermission::ProtectedPush),
            "a writer must NOT merge / set branch protection (protected_push = admin)"
        );
        assert!(
            !authz.authorize_repo_permission(&p, &repo, RepoPermission::ApproveUntrustedCi),
            "a writer must NOT endorse untrusted fork CI"
        );
    }

    /// **R2.1 — `admin` admits pull/push/protected_push (the fragment: `protected_push = admin`)
    /// but NOT the endorsement relation** (`approve_untrusted_ci` is a distinct trust decision).
    #[test]
    fn admin_grant_admits_protected_push_but_not_endorse() {
        let authz = GrantBackedRepos::new().grant_admin("boss", "acme", "widgets");
        let p = principal("boss", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        assert!(authz.authorize_repo_permission(&p, &repo, RepoPermission::Pull));
        assert!(authz.authorize_repo_permission(&p, &repo, RepoPermission::Push));
        assert!(authz.authorize_repo_permission(&p, &repo, RepoPermission::ProtectedPush));
        assert!(
            !authz.authorize_repo_permission(&p, &repo, RepoPermission::ApproveUntrustedCi),
            "admin does not imply the endorsement relation"
        );
    }

    /// **R2.1 — the endorsement grant is ITS OWN relation:** it admits `ApproveUntrustedCi` and
    /// nothing else (an endorser cannot read/write/merge off that grant alone).
    #[test]
    fn endorse_grant_admits_only_the_endorsement() {
        let authz = GrantBackedRepos::new().grant_endorse_fork_ci("bot", "acme", "widgets");
        let p = principal("bot", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        assert!(authz.authorize_repo_permission(&p, &repo, RepoPermission::ApproveUntrustedCi));
        assert!(!authz.authorize_repo_permission(&p, &repo, RepoPermission::Pull));
        assert!(!authz.authorize_repo_permission(&p, &repo, RepoPermission::Push));
        assert!(!authz.authorize_repo_permission(&p, &repo, RepoPermission::ProtectedPush));
    }

    /// **R2.1 — the default `visible_repos` prefilter is leak-free:** only `Pull`-granted candidates
    /// survive, input order is preserved, and an un-granted repo's slug never appears (the list-leak
    /// hole, closed at the seam's default too).
    #[test]
    fn visible_repos_default_filters_to_pull_granted() {
        let authz = GrantBackedRepos::new()
            .grant_read("p", "acme", "alpha")
            .grant_write("p", "acme", "gamma");
        let p = principal("p", "acme");
        let candidates = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let visible = authz.visible_repos(&p, "acme", "eu-west", &candidates);
        assert_eq!(visible, vec!["alpha".to_string(), "gamma".to_string()]);
        // AllowAll passes everything through; DenyAll returns the empty set.
        assert_eq!(
            AllowAllRepos.visible_repos(&p, "acme", "eu-west", &candidates),
            candidates
        );
        assert!(DenyAllRepos
            .visible_repos(&p, "acme", "eu-west", &candidates)
            .is_empty());
    }
}
