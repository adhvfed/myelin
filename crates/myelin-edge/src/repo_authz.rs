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

/// **The per-repo object-authorization seam (R0.3 / DELTA N2 — the R2 seed).** Given the VERIFIED
/// principal, the resolved `(tenant, region, repo)` locator, and the [`RepoAccess`] the route needs,
/// decide whether the principal may reach THIS repo. Consulted by every git wire handler after
/// `repo_loc` and before any bytes are served; a `false` is fail-closed (the handler returns a 0-leak
/// 404 for a READ denial, a 403 for a WRITE denial). The cross-tenant IDOR reject still runs FIRST in
/// the gateway — this seam is the IN-TENANT per-repo grant check the action-only authorizer cannot do.
pub trait RepoAuthorizer: Send + Sync {
    /// May `principal` perform `access` on `repo`? Fail-closed on any denial.
    fn authorize_repo(&self, principal: &Principal, repo: &RepoLoc, access: RepoAccess) -> bool;
}

/// The M0 seam fixture that admits every `(principal, repo, access)` — the analogue of
/// [`crate::authz::AllowAll`] for the wire object-authz seam. It lets the happy-path wire proofs (real
/// `git clone`/`git push`) dispatch while the real per-repo grant store is the R2 follow-on. Production
/// injects a grant-backed [`RepoAuthorizer`]; [`DenyAllRepos`] / [`GrantBackedRepos`] prove the deny
/// path is a real 404/403, so the seam is load-bearing, not vacuous.
pub struct AllowAllRepos;

impl RepoAuthorizer for AllowAllRepos {
    fn authorize_repo(&self, _principal: &Principal, _repo: &RepoLoc, _access: RepoAccess) -> bool {
        true
    }
}

/// A seam fixture that DENIES every `(principal, repo, access)` — the analogue of the substrate's
/// `DenyAll`. Proves the wire handlers consult the seam (a denial is a 0-leak 404 on read / a 403 on
/// write) — the seam is not vacuous.
pub struct DenyAllRepos;

impl RepoAuthorizer for DenyAllRepos {
    fn authorize_repo(&self, _principal: &Principal, _repo: &RepoLoc, _access: RepoAccess) -> bool {
        false
    }
}

/// A grant-backed [`RepoAuthorizer`] fixture — the smallest real model of the R2 tuple store. Holds an
/// explicit allow-set of `(principal_id, tenant, repo, access)` grants; a request is admitted IFF a
/// matching grant is present, with **write implying read** (a Write grant also satisfies a Read). A
/// principal with NO grant on a repo is DENIED (the un-granted-repo-reach hole, closed). This is the
/// deny-by-default model the R2 grant seam realises with durable tuples.
#[derive(Default)]
pub struct GrantBackedRepos {
    /// `(principal_id, tenant, repo, access)` — the explicit allow-set (deny-by-default otherwise).
    grants: BTreeSet<(String, String, String, RepoAccess)>,
}

impl GrantBackedRepos {
    /// A fresh, empty grant store (denies everything until granted).
    pub fn new() -> Self {
        Self::default()
    }

    /// Grant `principal_id` READ on `(tenant, repo)`.
    pub fn grant_read(mut self, principal_id: &str, tenant: &str, repo: &str) -> Self {
        self.grants.insert((
            principal_id.to_string(),
            tenant.to_string(),
            repo.to_string(),
            RepoAccess::Read,
        ));
        self
    }

    /// Grant `principal_id` WRITE on `(tenant, repo)` (write implies read).
    pub fn grant_write(mut self, principal_id: &str, tenant: &str, repo: &str) -> Self {
        self.grants.insert((
            principal_id.to_string(),
            tenant.to_string(),
            repo.to_string(),
            RepoAccess::Write,
        ));
        self
    }
}

impl RepoAuthorizer for GrantBackedRepos {
    fn authorize_repo(&self, principal: &Principal, repo: &RepoLoc, access: RepoAccess) -> bool {
        let key = |a: RepoAccess| {
            (
                principal.principal_id.0.clone(),
                repo.tenant.to_string(),
                repo.repo.to_string(),
                a,
            )
        };
        // A Write grant satisfies a Read request (write implies read); a Read grant does NOT satisfy a
        // Write request (deny-by-default on the stricter access).
        match access {
            RepoAccess::Read => {
                self.grants.contains(&key(RepoAccess::Read))
                    || self.grants.contains(&key(RepoAccess::Write))
            }
            RepoAccess::Write => self.grants.contains(&key(RepoAccess::Write)),
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
}
