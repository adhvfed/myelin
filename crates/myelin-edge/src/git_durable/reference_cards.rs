use std::collections::{BTreeMap, BTreeSet};

use myelin_git::coordinate::RepositorySlug;
use myelin_git::core::{is_canonical_object_id, Oid};
use myelin_git::durable::{DurableError, RefKind};
use myelin_git::receive_pack::RefName;
use myelin_identity::Principal;
use myelin_storage::TenantScope;

use super::DurableGitBackend;

const MAX_COORDINATES: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitReferenceCardBatch {
    pub repositories: Vec<String>,
    pub pull_requests: Vec<GitPullRequestCard>,
    pub commits: Vec<GitCommitCard>,
    pub refs: Vec<GitRefCard>,
}

pub(crate) struct GitReferenceCardRequest<'a> {
    pub repositories: &'a [String],
    pub pull_requests: &'a [(String, u64)],
    pub commits: &'a [(String, String)],
    pub refs: &'a [(String, String)],
}

impl GitReferenceCardRequest<'_> {
    fn coordinate_count(&self) -> usize {
        self.repositories
            .len()
            .saturating_add(self.pull_requests.len())
            .saturating_add(self.commits.len())
            .saturating_add(self.refs.len())
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitRefCard {
    pub repository: String,
    pub qualified_name: String,
    pub title: String,
    pub kind: RefKind,
}

impl DurableGitBackend {
    pub(crate) fn visible_reference_cards(
        &self,
        principal: &Principal,
        request: GitReferenceCardRequest<'_>,
    ) -> Result<GitReferenceCardBatch, DurableError> {
        if request.coordinate_count() > MAX_COORDINATES {
            return Err(DurableError::Git(format!(
                "at most {MAX_COORDINATES} Git references may be projected at once"
            )));
        }

        let requested_repositories = requested_repositories(&request)?;
        let requested_pull_requests = requested_pull_requests(request.pull_requests)?;
        let requested_commits = requested_commits(request.commits)?;
        let requested_refs = requested_refs(request.refs)?;
        let tenant = principal.tenant.as_str();
        let region = principal.region.as_str();
        let existing = self.visible_existing_repositories(principal, &requested_repositories)?;

        let repositories = request
            .repositories
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
        let mut refs = Vec::new();
        let mut visible_refs = BTreeMap::<String, Vec<RefName>>::new();
        for (repository, ref_name) in requested_refs {
            if existing.contains(&repository) {
                visible_refs.entry(repository).or_default().push(ref_name);
            }
        }
        for (repository, ref_names) in visible_refs {
            let repo = self
                .store
                .open_repo(&Self::loc(tenant, region, &repository))?;
            for (qualified_name, _) in repo.read_refs_at_names(&ref_names)? {
                let Some((kind, short_name)) = RefKind::from_qualified_name(&qualified_name.0)
                else {
                    continue;
                };
                refs.push(GitRefCard {
                    title: ref_card_title(&repository, short_name),
                    repository: repository.clone(),
                    qualified_name: qualified_name.0,
                    kind,
                });
            }
        }
        Ok(GitReferenceCardBatch {
            repositories,
            pull_requests,
            commits,
            refs,
        })
    }
}

fn requested_repositories(
    request: &GitReferenceCardRequest<'_>,
) -> Result<Vec<String>, DurableError> {
    let mut requested = BTreeSet::new();
    for repository in request
        .repositories
        .iter()
        .chain(
            request
                .pull_requests
                .iter()
                .map(|(repository, _)| repository),
        )
        .chain(request.commits.iter().map(|(repository, _)| repository))
        .chain(request.refs.iter().map(|(repository, _)| repository))
    {
        RepositorySlug::parse(repository)
            .map_err(|_| DurableError::Git("Git reference repository slug is malformed".into()))?;
        requested.insert(repository.clone());
    }
    Ok(requested.into_iter().collect())
}

fn requested_refs(refs: &[(String, String)]) -> Result<BTreeSet<(String, RefName)>, DurableError> {
    refs.iter()
        .map(|(repository, name)| {
            let name = RefName::new(name);
            name.validate()
                .map_err(|_| DurableError::Git("Git reference name is malformed".into()))?;
            if RefKind::from_qualified_name(&name.0).is_none() {
                return Err(DurableError::Git(
                    "Git reference must name a branch or tag".into(),
                ));
            }
            Ok((repository.clone(), name))
        })
        .collect()
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
    let summary = summary.trim();
    if summary.is_empty() || summary.chars().any(char::is_control) {
        return format!("{repository} {}", oid.chars().take(12).collect::<String>());
    }
    bounded_card_title(summary)
}

fn ref_card_title(repository: &str, short_name: &str) -> String {
    bounded_card_title(&format!("{repository} · {short_name}"))
}

fn bounded_card_title(value: &str) -> String {
    const MAX_TITLE_BYTES: usize = 512;
    if value.len() <= MAX_TITLE_BYTES {
        return value.to_string();
    }
    let mut end = MAX_TITLE_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
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

    #[test]
    fn branch_and_tag_coordinates_are_typed_deduplicated_and_bounded() {
        assert_eq!(
            requested_refs(&[
                ("team/api".into(), "refs/heads/release/one".into()),
                ("team/api".into(), "refs/heads/release/one".into()),
                ("team/api".into(), "refs/tags/v1".into()),
            ])
            .unwrap(),
            BTreeSet::from([
                ("team/api".into(), RefName::new("refs/heads/release/one")),
                ("team/api".into(), RefName::new("refs/tags/v1")),
            ])
        );
        assert!(requested_refs(&[("team/api".into(), "refs/notes/build".into())]).is_err());
        assert!(requested_refs(&[("team/api".into(), "refs/heads/../hidden".into())]).is_err());

        let title = ref_card_title("team/api", &"ø".repeat(300));
        assert_eq!(title.len(), 512);
        assert!(title.starts_with("team/api · "));
        assert_eq!(title.chars().count(), 261);
    }
}
