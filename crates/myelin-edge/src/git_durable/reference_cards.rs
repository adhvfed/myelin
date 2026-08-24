use std::collections::{BTreeMap, BTreeSet};

use myelin_git::blob_coordinate::GitBlobLocation;
use myelin_git::coordinate::RepositorySlug;
use myelin_git::core::{is_canonical_object_id, Oid};
use myelin_git::durable::{DurableError, RefKind};
use myelin_git::pr_store::PrRecord;
use myelin_git::pr_threads::{AnchorState, CommentState};
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
    pub blobs: Vec<GitBlobCard>,
    pub comments: Vec<GitPrCommentCard>,
}

pub(crate) struct GitReferenceCardRequest<'a> {
    pub repositories: &'a [String],
    pub pull_requests: &'a [(String, u64)],
    pub commits: &'a [(String, String)],
    pub refs: &'a [(String, String)],
    pub blobs: &'a [(String, GitBlobLocation)],
    pub comments: &'a [GitPrCommentLocation],
}

impl GitReferenceCardRequest<'_> {
    fn coordinate_count(&self) -> usize {
        self.repositories
            .len()
            .saturating_add(self.pull_requests.len())
            .saturating_add(self.commits.len())
            .saturating_add(self.refs.len())
            .saturating_add(self.blobs.len())
            .saturating_add(self.comments.len())
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitBlobCard {
    pub repository: String,
    pub location: GitBlobLocation,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct GitPrCommentLocation {
    pub repository: String,
    pub number: u64,
    pub comment_id: String,
}

impl GitPrCommentLocation {
    pub(crate) fn new(repository: String, number: u64, comment_id: String) -> Option<Self> {
        (RepositorySlug::parse(&repository).is_ok()
            && number > 0
            && i64::try_from(number).is_ok()
            && canonical_conversation_id(&comment_id, "c-"))
        .then_some(Self {
            repository,
            number,
            comment_id,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitPrCommentCard {
    pub location: GitPrCommentLocation,
    pub title: String,
    pub state: String,
    context: GitPrCommentContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GitPrCommentContext {
    Discussion,
    Diff(AnchorState),
}

impl GitPrCommentCard {
    pub(crate) fn presentation(&self) -> (&'static str, Option<&'static str>) {
        match self.context {
            GitPrCommentContext::Discussion => ("git_pr_comment", None),
            GitPrCommentContext::Diff(AnchorState::Live) => ("git_pr_inline_comment", None),
            GitPrCommentContext::Diff(AnchorState::Moved) => {
                ("git_pr_inline_comment", Some("moved"))
            }
            GitPrCommentContext::Diff(AnchorState::Outdated) => {
                ("git_pr_inline_comment", Some("outdated"))
            }
        }
    }
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
        let requested_blobs = request.blobs.iter().cloned().collect::<BTreeSet<_>>();
        let requested_comments = requested_comments(request.comments)?;
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
        let pull_request_coordinates = requested_pull_requests
            .iter()
            .cloned()
            .chain(
                requested_comments
                    .iter()
                    .map(|comment| (comment.repository.clone(), comment.number)),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let viewer = Self::pseudonym(tenant, principal);
        let visible_pull_requests = self
            .reference_card_pr_records(principal, &pull_request_coordinates)?
            .into_iter()
            .filter(|((repository, _), record)| {
                existing.contains(repository) || record.has_review_relationship_with(&viewer)
            })
            .collect::<BTreeMap<_, _>>();
        let pull_requests = requested_pull_requests
            .iter()
            .filter_map(|coordinate @ (repository, _)| {
                visible_pull_requests.get(coordinate).map(|record| {
                    let title = if record.title.is_empty() {
                        format!("{repository} #{}", record.number)
                    } else {
                        record.title.clone()
                    };
                    GitPullRequestCard {
                        repository: repository.clone(),
                        number: record.number,
                        title,
                        state: Self::pr_state_token(record.state).into(),
                    }
                })
            })
            .collect();
        let mut requested_comment_ids = BTreeMap::<(String, u64), BTreeSet<String>>::new();
        for comment in &requested_comments {
            let coordinate = (comment.repository.clone(), comment.number);
            if visible_pull_requests.contains_key(&coordinate) {
                requested_comment_ids
                    .entry(coordinate)
                    .or_default()
                    .insert(comment.comment_id.clone());
            }
        }
        let mut comments = Vec::new();
        for ((repository, number), comment_ids) in requested_comment_ids {
            let loc = Self::loc(tenant, region, &repository);
            let document = self
                .threads
                .load(&loc, &format!("pr:{repository}:{number}"))?;
            for viewed in document.comments_for(&viewer, &comment_ids) {
                let (title, state) = comment_card_title(
                    &repository,
                    number,
                    viewed.comment.state,
                    &viewed.comment.body_md,
                    viewed.resolved,
                );
                comments.push(GitPrCommentCard {
                    location: GitPrCommentLocation {
                        repository: repository.clone(),
                        number,
                        comment_id: viewed.comment.id,
                    },
                    title,
                    state,
                    context: viewed
                        .anchor
                        .map_or(GitPrCommentContext::Discussion, |anchor| {
                            GitPrCommentContext::Diff(anchor.anchor_state)
                        }),
                });
            }
        }
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
                    title: named_card_title(&repository, short_name),
                    repository: repository.clone(),
                    qualified_name: qualified_name.0,
                    kind,
                });
            }
        }
        let mut blobs = Vec::new();
        let mut visible_blobs = BTreeMap::<String, Vec<GitBlobLocation>>::new();
        for (repository, location) in requested_blobs {
            if existing.contains(&repository) {
                visible_blobs.entry(repository).or_default().push(location);
            }
        }
        for (repository, locations) in visible_blobs {
            let repo = self
                .store
                .open_repo(&Self::loc(tenant, region, &repository))?;
            for (location, _) in repo.blob_oids_at_locations(&locations)? {
                blobs.push(GitBlobCard {
                    title: named_card_title(&repository, location.path()),
                    repository: repository.clone(),
                    location,
                });
            }
        }
        Ok(GitReferenceCardBatch {
            repositories,
            pull_requests,
            commits,
            refs,
            blobs,
            comments,
        })
    }

    fn reference_card_pr_records(
        &self,
        principal: &Principal,
        coordinates: &[(String, u64)],
    ) -> Result<BTreeMap<(String, u64), PrRecord>, DurableError> {
        let records = match &self.pg_prs {
            Some(store) => {
                let scope = TenantScope::from_verified_token(principal, principal.region.clone());
                store.get_many(&scope, coordinates)?
            }
            None => coordinates
                .iter()
                .filter_map(|(repository, number)| {
                    self.prs
                        .get(
                            &Self::loc(
                                principal.tenant.as_str(),
                                principal.region.as_str(),
                                repository,
                            ),
                            *number,
                        )
                        .transpose()
                        .map(|result| result.map(|record| (repository.clone(), record)))
                })
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(records
            .into_iter()
            .map(|(repository, record)| ((repository, record.number), record))
            .collect())
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
        .chain(request.blobs.iter().map(|(repository, _)| repository))
        .chain(request.comments.iter().map(|comment| &comment.repository))
    {
        RepositorySlug::parse(repository)
            .map_err(|_| DurableError::Git("Git reference repository slug is malformed".into()))?;
        requested.insert(repository.clone());
    }
    Ok(requested.into_iter().collect())
}

fn requested_comments(
    comments: &[GitPrCommentLocation],
) -> Result<BTreeSet<GitPrCommentLocation>, DurableError> {
    comments
        .iter()
        .map(|comment| {
            if comment.number == 0
                || i64::try_from(comment.number).is_err()
                || !canonical_conversation_id(&comment.comment_id, "c-")
            {
                return Err(DurableError::Git(
                    "Git pull-request comment coordinate is malformed".into(),
                ));
            }
            Ok(comment.clone())
        })
        .collect()
}

fn canonical_conversation_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|sequence| {
        !sequence.is_empty()
            && !sequence.starts_with('0')
            && sequence.bytes().all(|byte| byte.is_ascii_digit())
            && sequence.parse::<u64>().is_ok()
    })
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

fn named_card_title(repository: &str, short_name: &str) -> String {
    bounded_card_title(&format!("{repository} · {short_name}"))
}

fn comment_card_title(
    repository: &str,
    number: u64,
    comment_state: CommentState,
    body: &str,
    resolved: bool,
) -> (String, String) {
    if comment_state == CommentState::Removed {
        return (
            bounded_card_title(&format!("{repository} #{number} · comment removed")),
            "removed".into(),
        );
    }
    let title = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.chars().any(char::is_control))
        .map(bounded_card_title)
        .unwrap_or_else(|| bounded_card_title(&format!("{repository} #{number} · comment")));
    (title, if resolved { "resolved" } else { "open" }.into())
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

        let title = named_card_title("team/api", &"ø".repeat(300));
        assert_eq!(title.len(), 512);
        assert!(title.starts_with("team/api · "));
        assert_eq!(title.chars().count(), 261);
    }

    #[test]
    fn comment_coordinates_and_titles_keep_removed_content_private() {
        let comment = GitPrCommentLocation::new("team/api".into(), 41, "c-7".into()).unwrap();
        assert_eq!(
            requested_comments(&[comment.clone(), comment.clone()]).unwrap(),
            BTreeSet::from([comment])
        );
        assert!(GitPrCommentLocation::new("team/api".into(), 41, "c-07".into()).is_none());
        assert!(GitPrCommentLocation::new("team/api".into(), 0, "c-7".into()).is_none());

        assert_eq!(
            comment_card_title(
                "team/api",
                41,
                CommentState::Visible,
                "\n  One exact observation.  \nMore detail.",
                false,
            ),
            ("One exact observation.".into(), "open".into())
        );
        assert_eq!(
            comment_card_title(
                "team/api",
                41,
                CommentState::Removed,
                "content that must not surface",
                true,
            ),
            ("team/api #41 · comment removed".into(), "removed".into())
        );

        let discussion = GitPrCommentCard {
            location: GitPrCommentLocation::new("team/api".into(), 41, "c-7".into()).unwrap(),
            title: "One exact observation.".into(),
            state: "open".into(),
            context: GitPrCommentContext::Discussion,
        };
        assert_eq!(discussion.presentation(), ("git_pr_comment", None));

        let outdated = GitPrCommentCard {
            context: GitPrCommentContext::Diff(AnchorState::Outdated),
            ..discussion
        };
        assert_eq!(
            outdated.presentation(),
            ("git_pr_inline_comment", Some("outdated"))
        );
    }
}
