//! Git-owned authorization adapters for CI's parent-repository read boundary.

use crate::git_durable::DurableGitBackend;
use crate::repo_authz::RepoPermission;
use myelin_git::{core::RepoLoc, durable::DurableError};
use myelin_identity::Principal;

impl DurableGitBackend {
    /// The bounded, leak-free repository visibility set CI run listing inherits from Git's live
    /// `list_objects(viewer, pull, repo)` authority. A CI run is readable exactly through its parent
    /// repository; CT-005 never invents a second run ACL.
    pub(crate) fn visible_repo_slugs_for_ci(
        &self,
        principal: &Principal,
    ) -> Result<Vec<String>, DurableError> {
        self.visible_pr_repo_slugs(
            principal.tenant.as_str(),
            principal.region.as_str(),
            principal,
        )
    }

    /// Authorize one CI run's canonical parent repo through Git's exact Pull permission.
    pub(crate) fn may_view_ci_repo(&self, principal: &Principal, slug: &str) -> bool {
        let loc = RepoLoc::new(principal.tenant.as_str(), principal.region.as_str(), slug);
        self.repo_authorizer()
            .authorize_repo_permission(principal, &loc, RepoPermission::Pull)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_authz::GrantBackedRepos;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::Arc;

    const TENANT: &str = "ci-surface";
    const REGION: &str = "eu-north";

    fn principal(id: &str) -> Principal {
        Principal::new(
            TenantId(TENANT.into()),
            Region(REGION.into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    #[test]
    fn ci_inherits_gits_exact_pull_visibility_without_a_second_acl() {
        let root = std::env::temp_dir().join(format!(
            "myelin-ci-surface-authz-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo_dir = root.join(TENANT).join(REGION);
        std::fs::create_dir_all(repo_dir.join("alpha.git")).unwrap();
        std::fs::create_dir_all(repo_dir.join("hidden.git")).unwrap();
        let authz = GrantBackedRepos::new().grant_read("viewer", TENANT, "alpha");
        let backend =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        let viewer = principal("viewer");
        let stranger = principal("stranger");

        assert_eq!(
            backend.visible_repo_slugs_for_ci(&viewer).unwrap(),
            ["alpha"]
        );
        assert!(backend
            .visible_repo_slugs_for_ci(&stranger)
            .unwrap()
            .is_empty());
        assert!(backend.may_view_ci_repo(&viewer, "alpha"));
        assert!(!backend.may_view_ci_repo(&viewer, "hidden"));
        assert!(!backend.may_view_ci_repo(&stranger, "alpha"));

        std::fs::remove_dir_all(root).unwrap();
    }
}
