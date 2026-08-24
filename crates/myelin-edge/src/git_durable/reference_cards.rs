use std::collections::{BTreeMap, BTreeSet};

use myelin_git::coordinate::RepositorySlug;
use myelin_git::core::{is_canonical_object_id, Oid};
use myelin_git::durable::DurableError;
use myelin_identity::Principal;
use myelin_storage::TenantScope;

use super::DurableGitBackend;

const MAX_COORDINATES: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitReferenceCardBatch {
    pub repositories: Vec<String>,
    pub pull_requests: Vec<GitPullRequestCard>,
    pub commits: Vec<GitCommitCard>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitPullRequestCard {
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitCommitCard {
    pub repository: String,
    pub oid: String,
    pub title: String,
}

impl DurableGitBackend {
    pub(crate) fn visible_reference_cards(
        &self,
        principal: &Principal,
        repositories: &[String],
        pull_requests: &[(String, u64)],
        commits: &[(String, String)],
    ) -> Result<GitReferenceCardBatch, DurableError> {
        if repositories
            .len()
            .saturating_add(pull_requests.len())
            .saturating_add(commits.len())
            > MAX_COORDINATES
        {
            return Err(DurableError::Git(format!(
                "at most {MAX_COORDINATES} Git references may be projected at once"
            )));
        }

        let requested_repositories = requested_repositories(repositories, pull_requests, commits)?;
        let requested_pull_requests = requested_pull_requests(pull_requests)?;
        let requested_commits = requested_commits(commits)?;
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
        let mut commits = Vec::new();
        let mut visible_commits = BTreeMap::<String, Vec<Oid>>::new();
        for (repository, oid) in requested_commits {
            if existing.contains(&repository) {
                visible_commits
                    .entry(repository)
                    .or_default()
                    .push(Oid::new(oid));
            }
        }
        for (repository, oids) in visible_commits {
            let repo = self
                .store
                .open_repo(&Self::loc(tenant, region, &repository))?;
            for commit in repo.commit_meta_at_oids(&oids)? {
                commits.push(GitCommitCard {
                    title: commit_card_title(&repository, &commit.oid, &commit.summary),
                    repository: repository.clone(),
                    oid: commit.oid,
                });
            }
        }
        Ok(GitReferenceCardBatch {
            repositories,
            pull_requests,
            commits,
        })
    }
}

fn requested_repositories(
    repositories: &[String],
    pull_requests: &[(String, u64)],
    commits: &[(String, String)],
) -> Result<Vec<String>, DurableError> {
    let mut requested = BTreeSet::new();
    for repository in repositories
        .iter()
        .chain(pull_requests.iter().map(|(repository, _)| repository))
        .chain(commits.iter().map(|(repository, _)| repository))
    {
        RepositorySlug::parse(repository)
            .map_err(|_| DurableError::Git("Git reference repository slug is malformed".into()))?;
        requested.insert(repository.clone());
    }
    Ok(requested.into_iter().collect())
}

fn requested_commits(
    commits: &[(String, String)],
) -> Result<BTreeSet<(String, String)>, DurableError> {
    commits
        .iter()
        .map(|(repository, oid)| {
            if !is_canonical_object_id(oid) {
                return Err(DurableError::Git(
                    "Git reference commit object id is malformed".into(),
                ));
            }
            Ok((repository.clone(), oid.clone()))
        })
        .collect()
}

fn commit_card_title(repository: &str, oid: &str, summary: &str) -> String {
    const MAX_TITLE_BYTES: usize = 512;
    let summary = summary.trim();
    if summary.is_empty() || summary.chars().any(char::is_control) {
        return format!("{repository} {}", oid.chars().take(12).collect::<String>());
    }
    if summary.len() <= MAX_TITLE_BYTES {
        return summary.to_string();
    }
    let mut end = MAX_TITLE_BYTES;
    while !summary.is_char_boundary(end) {
        end -= 1;
    }
    summary[..end].to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_coordinates_and_titles_are_canonical_and_bounded() {
        let oid = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            requested_commits(&[
                ("team/api".into(), oid.into()),
                ("team/api".into(), oid.into())
            ])
            .unwrap(),
            BTreeSet::from([("team/api".into(), oid.into())])
        );
        assert!(requested_commits(&[("team/api".into(), oid.to_uppercase())]).is_err());
        assert_eq!(
            commit_card_title("team/api", oid, "  Ship safely  "),
            "Ship safely"
        );
        assert_eq!(
            commit_card_title("team/api", oid, "bad\0summary"),
            "team/api 0123456789ab"
        );
        let multibyte = "ø".repeat(300);
        let title = commit_card_title("team/api", oid, &multibyte);
        assert_eq!(title.len(), 512);
        assert_eq!(title.chars().count(), 256);
    }
}
