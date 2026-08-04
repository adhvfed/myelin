use myelin_git::core::RepoLoc;
use myelin_identity::Principal;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepoAccess {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepoPermission {
    Pull,
    Push,
    ProtectedPush,
    ApproveUntrustedCi,
}

pub trait RepoAuthorizer: Send + Sync {
    fn authorize_repo_permission(
        &self,
        principal: &Principal,
        repo: &RepoLoc,
        permission: RepoPermission,
    ) -> bool;

    fn authorize_repo(&self, principal: &Principal, repo: &RepoLoc, access: RepoAccess) -> bool {
        let permission = match access {
            RepoAccess::Read => RepoPermission::Pull,
            RepoAccess::Write => RepoPermission::Push,
        };
        self.authorize_repo_permission(principal, repo, permission)
    }

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GrantKind {
    Read,
    Write,
    Admin,
    EndorseForkCi,
}

#[derive(Default)]
pub struct GrantBackedRepos {
    grants: BTreeSet<(String, String, String, GrantKind)>,
}

impl GrantBackedRepos {
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

    pub fn grant_read(self, principal_id: &str, tenant: &str, repo: &str) -> Self {
        self.grant(principal_id, tenant, repo, GrantKind::Read)
    }

    pub fn grant_write(self, principal_id: &str, tenant: &str, repo: &str) -> Self {
        self.grant(principal_id, tenant, repo, GrantKind::Write)
    }

    pub fn grant_admin(self, principal_id: &str, tenant: &str, repo: &str) -> Self {
        self.grant(principal_id, tenant, repo, GrantKind::Admin)
    }

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
        assert!(!authz.authorize_repo(&p, &repo, RepoAccess::Write));
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
        let q = principal("dev2", "acme");
        assert!(!authz.authorize_repo(&q, &repo, RepoAccess::Read));
    }

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

    #[test]
    fn visible_repos_default_filters_to_pull_granted() {
        let authz = GrantBackedRepos::new()
            .grant_read("p", "acme", "alpha")
            .grant_write("p", "acme", "gamma");
        let p = principal("p", "acme");
        let candidates = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let visible = authz.visible_repos(&p, "acme", "eu-west", &candidates);
        assert_eq!(visible, vec!["alpha".to_string(), "gamma".to_string()]);
        assert_eq!(
            AllowAllRepos.visible_repos(&p, "acme", "eu-west", &candidates),
            candidates
        );
        assert!(DenyAllRepos
            .visible_repos(&p, "acme", "eu-west", &candidates)
            .is_empty());
    }
}
