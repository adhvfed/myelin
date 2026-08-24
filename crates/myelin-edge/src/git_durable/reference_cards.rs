use std::collections::BTreeSet;

use myelin_git::coordinate::RepositorySlug;
use myelin_git::durable::DurableError;
use myelin_identity::Principal;
use myelin_storage::TenantScope;

use super::DurableGitBackend;

const MAX_COORDINATES: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitReferenceCardBatch {
    pub repositories: Vec<String>,
    pub pull_requests: Vec<GitPullRequestCard>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitPullRequestCard {
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub state: String,
}

impl DurableGitBackend {
    pub(crate) fn visible_reference_cards(
        &self,
        principal: &Principal,
        repositories: &[String],
        pull_requests: &[(String, u64)],
    ) -> Result<GitReferenceCardBatch, DurableError> {
        if repositories.len().saturating_add(pull_requests.len()) > MAX_COORDINATES {
            return Err(DurableError::Git(format!(
                "at most {MAX_COORDINATES} Git references may be projected at once"
            )));
        }

        let requested_repositories = requested_repositories(repositories, pull_requests)?;
        let requested_pull_requests = requested_pull_requests(pull_requests)?;
        let tenant = principal.tenant.as_str();
        let region = principal.region.as_str();
        let existing = self.visible_existing_repositories(principal, &requested_repositories)?;

        let repositories = repositories
            .iter()
            .filter(|repository| existing.contains(*repository))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let visible_coordinates = requested_pull_requests
            .into_iter()
            .filter(|(repository, _)| existing.contains(repository))
            .collect::<Vec<_>>();
        let records = match &self.pg_prs {
            Some(store) => {
                let scope = TenantScope::from_verified_token(principal, principal.region.clone());
                store.get_many(&scope, &visible_coordinates)?
            }
            None => visible_coordinates
                .into_iter()
                .filter_map(|(repository, number)| {
                    self.prs
                        .get(&Self::loc(tenant, region, &repository), number)
                        .transpose()
                        .map(|result| result.map(|record| (repository, record)))
                })
                .collect::<Result<Vec<_>, _>>()?,
        };
        let pull_requests = records
            .into_iter()
            .map(|(repository, record)| {
                let title = if record.title.is_empty() {
                    format!("{repository} #{}", record.number)
                } else {
                    record.title
                };
                GitPullRequestCard {
                    repository,
                    number: record.number,
                    title,
                    state: Self::pr_state_token(record.state).into(),
                }
            })
            .collect();
        Ok(GitReferenceCardBatch {
            repositories,
            pull_requests,
        })
    }
}

fn requested_repositories(
    repositories: &[String],
    pull_requests: &[(String, u64)],
) -> Result<Vec<String>, DurableError> {
    let mut requested = BTreeSet::new();
    for repository in repositories
        .iter()
        .chain(pull_requests.iter().map(|(repository, _)| repository))
    {
        RepositorySlug::parse(repository)
            .map_err(|_| DurableError::Git("Git reference repository slug is malformed".into()))?;
        requested.insert(repository.clone());
    }
    Ok(requested.into_iter().collect())
}

fn requested_pull_requests(
    pull_requests: &[(String, u64)],
) -> Result<BTreeSet<(String, u64)>, DurableError> {
    pull_requests
        .iter()
        .map(|(repository, number)| {
            if *number == 0 || i64::try_from(*number).is_err() {
                return Err(DurableError::Git(
                    "Git reference pull-request number is malformed".into(),
                ));
            }
            Ok((repository.clone(), *number))
        })
        .collect()
}
