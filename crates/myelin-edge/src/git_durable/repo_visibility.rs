use std::collections::BTreeSet;

use myelin_git::{coordinate::RepositorySlug, durable::DurableError};
use myelin_identity::Principal;

use super::DurableGitBackend;

const MAX_EXACT_REPOSITORIES: usize = 10_000;

impl DurableGitBackend {
    pub(crate) fn visible_existing_repositories(
        &self,
        principal: &Principal,
        candidates: &[String],
    ) -> Result<BTreeSet<String>, DurableError> {
        if candidates.len() > MAX_EXACT_REPOSITORIES {
            return Err(DurableError::Git(format!(
                "at most {MAX_EXACT_REPOSITORIES} Git repositories may be checked at once"
            )));
        }

        let mut requested = BTreeSet::new();
        for candidate in candidates {
            RepositorySlug::parse(candidate).map_err(|_| {
                DurableError::Git("Git reference repository slug is malformed".into())
            })?;
            requested.insert(candidate.clone());
        }

        let tenant = principal.tenant.as_str();
        let region = principal.region.as_str();
        let requested = requested.into_iter().collect::<Vec<_>>();
        let authorized = self
            .repo_authz
            .visible_repos(principal, tenant, region, &requested);
        Ok(authorized
            .into_iter()
            .filter(|repository| {
                self.store
                    .repo_exists(&Self::loc(tenant, region, repository))
            })
            .collect())
    }
}
