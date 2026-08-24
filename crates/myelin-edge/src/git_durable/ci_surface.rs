use crate::git_durable::DurableGitBackend;
use crate::repo_authz::RepoPermission;
use myelin_git::{core::RepoLoc, durable::DurableError};
use myelin_identity::Principal;

impl DurableGitBackend {
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

    pub(crate) fn may_view_ci_repo(&self, principal: &Principal, slug: &str) -> bool {
        let loc = RepoLoc::new(principal.tenant.as_str(), principal.region.as_str(), slug);
        self.repo_authorizer()
            .authorize_repo_permission(principal, &loc, RepoPermission::Pull)
    }

    pub(crate) fn visible_requested_repo_slugs_for_ci(
        &self,
        principal: &Principal,
        candidates: &[String],
    ) -> Result<Vec<String>, DurableError> {
        Ok(self
            .visible_existing_repositories(principal, candidates)?
            .into_iter()
            .collect())
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
        let authz = GrantBackedRepos::new().grant_read("viewer", TENANT, "alpha");
        let backend =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        backend.create_repo(TENANT, REGION, "alpha").unwrap();
        backend.create_repo(TENANT, REGION, "hidden").unwrap();
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
        assert_eq!(
            backend
                .visible_requested_repo_slugs_for_ci(
                    &viewer,
                    &["hidden".into(), "alpha".into(), "alpha".into()],
                )
                .unwrap(),
            ["alpha"]
        );
        assert!(backend
            .visible_requested_repo_slugs_for_ci(&stranger, &["alpha".into()])
            .unwrap()
            .is_empty());
        assert!(backend
            .visible_requested_repo_slugs_for_ci(&viewer, &["not a slug".into()])
            .is_err());

        std::fs::remove_dir_all(root).unwrap();
    }
}
