use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use myelin_identity::Principal;

use super::{claimed_tombstones, root_references, ReferenceCard, ReferenceCardProjector};
use crate::DurableGitBackend;

pub(super) struct GitReferenceCardProjector {
    git: Arc<DurableGitBackend>,
}

impl GitReferenceCardProjector {
    pub(super) fn new(git: Arc<DurableGitBackend>) -> Self {
        Self { git }
    }
}

impl ReferenceCardProjector for GitReferenceCardProjector {
    fn project(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard> {
        let repository_references = root_references(viewer, references, "git", "repo");
        let pull_request_roots = root_references(viewer, references, "git", "pr");
        let pull_request_comment_roots = pull_request_comment_references(viewer, references);
        let commit_roots = root_references(viewer, references, "git", "commit");
        let ref_roots = root_references(viewer, references, "git", "ref");
        let blob_roots = blob_root_references(viewer, references);
        if repository_references.is_empty()
            && pull_request_roots.is_empty()
            && pull_request_comment_roots.is_empty()
            && commit_roots.is_empty()
            && ref_roots.is_empty()
            && blob_roots.is_empty()
        {
            return HashMap::new();
        }
        let mut cards = claimed_tombstones(&repository_references);
        cards.extend(claimed_tombstones(&pull_request_roots));
        cards.extend(claimed_pr_comment_tombstones(&pull_request_comment_roots));
        cards.extend(claimed_tombstones(&commit_roots));
        cards.extend(claimed_tombstones(&ref_roots));
        cards.extend(claimed_blob_tombstones(&blob_roots));

        let repositories = repository_references
            .keys()
            .filter(|slug| myelin_git::coordinate::RepositorySlug::parse(slug).is_ok())
            .cloned()
            .collect::<Vec<_>>();
        let pull_request_references = canonical_pull_request_references(&pull_request_roots);
        let pull_requests = pull_request_references.keys().cloned().collect::<Vec<_>>();
        let pull_request_comment_references =
            canonical_pull_request_comment_references(&pull_request_comment_roots);
        let comments = pull_request_comment_references
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let commit_references = canonical_commit_references(&commit_roots);
        let commits = commit_references.keys().cloned().collect::<Vec<_>>();
        let ref_references = canonical_git_ref_references(&ref_roots);
        let refs = ref_references.keys().cloned().collect::<Vec<_>>();
        let blob_references = canonical_blob_references(&blob_roots);
        let blobs = blob_references.keys().cloned().collect::<Vec<_>>();
        let request = crate::git_durable::GitReferenceCardRequest {
            repositories: &repositories,
            pull_requests: &pull_requests,
            commits: &commits,
            refs: &refs,
            blobs: &blobs,
            comments: &comments,
        };
        let Ok(visible) = self.git.visible_reference_cards(viewer, request) else {
            return cards;
        };

        for repository in visible.repositories {
            let Some(references) = repository_references.get(&repository) else {
                continue;
            };
            let card = ReferenceCard::projection(repository, "active", "git", "git_repository");
            insert_card(&mut cards, references, card);
        }
        for pull_request in visible.pull_requests {
            let coordinate = (pull_request.repository, pull_request.number);
            let Some(references) = pull_request_references.get(&coordinate) else {
                continue;
            };
            let card = ReferenceCard::projection(
                pull_request.title,
                pull_request.state,
                "pull_request",
                "git_pull_request",
            );
            insert_card(&mut cards, references, card);
        }
        for commit in visible.commits {
            let coordinate = (commit.repository, commit.oid);
            let Some(references) = commit_references.get(&coordinate) else {
                continue;
            };
            let card = ReferenceCard::projection(commit.title, "committed", "commit", "git_commit");
            insert_card(&mut cards, references, card);
        }
        for git_ref in visible.refs {
            let coordinate = (git_ref.repository, git_ref.qualified_name);
            let Some(references) = ref_references.get(&coordinate) else {
                continue;
            };
            let (state, icon) = match git_ref.kind {
                myelin_git::durable::RefKind::Branch => ("branch", "branch"),
                myelin_git::durable::RefKind::Tag => ("tag", "tag"),
            };
            let card = ReferenceCard::projection(git_ref.title, state, icon, "git_ref");
            insert_card(&mut cards, references, card);
        }
        for blob in visible.blobs {
            let coordinate = (blob.repository, blob.location);
            let Some(references) = blob_references.get(&coordinate) else {
                continue;
            };
            for reference in references {
                cards.insert(
                    reference.original.clone(),
                    ReferenceCard::projection_at(
                        blob.title.clone(),
                        "file",
                        "file",
                        "git_blob",
                        reference.sub_anchor.clone(),
                    ),
                );
            }
        }
        for comment in visible.comments {
            let Some(references) = pull_request_comment_references.get(&comment.location) else {
                continue;
            };
            let (render_hint, flag) = comment.presentation();
            for reference in references {
                cards.insert(
                    reference.original.clone(),
                    ReferenceCard::projection_at_with_flag(
                        comment.title.clone(),
                        comment.state.clone(),
                        "comment",
                        render_hint,
                        Some(reference.sub_anchor.clone()),
                        flag.map(str::to_owned),
                    ),
                );
            }
        }
        cards
    }
}

fn insert_card(
    cards: &mut HashMap<String, ReferenceCard>,
    references: &[String],
    card: ReferenceCard,
) {
    for reference in references {
        cards.insert(reference.clone(), card.clone());
    }
}

fn canonical_pull_request_references(
    roots: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<(String, u64), Vec<String>> {
    roots
        .iter()
        .filter_map(|(id, references)| {
            parse_pull_request_coordinate(id).map(|coordinate| (coordinate, references.clone()))
        })
        .collect()
}

fn parse_pull_request_coordinate(id: &str) -> Option<(String, u64)> {
    let (repository, number) = id.rsplit_once(':')?;
    myelin_git::coordinate::RepositorySlug::parse(repository).ok()?;
    let number = myelin_git::coordinate::parse_positive_decimal(number)?;
    i64::try_from(number).ok()?;
    Some((repository.to_owned(), number))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OwnedPrCommentReference {
    original: String,
    sub_anchor: String,
}

fn pull_request_comment_references(
    viewer: &Principal,
    references: &[String],
) -> BTreeMap<String, Vec<OwnedPrCommentReference>> {
    let mut owned = BTreeMap::<String, Vec<OwnedPrCommentReference>>::new();
    for reference in references {
        let Ok(parsed) = myelin_refs::parse_scoped(reference) else {
            continue;
        };
        if parsed.tenant != viewer.tenant || parsed.subsystem != "git" || parsed.type_ != "pr" {
            continue;
        }
        let Some(myelin_refs::Sub::Comment(comment_id)) = parsed.sub else {
            continue;
        };
        owned
            .entry(parsed.id)
            .or_default()
            .push(OwnedPrCommentReference {
                original: reference.clone(),
                sub_anchor: format!("comment-{comment_id}"),
            });
    }
    owned
}

fn canonical_pull_request_comment_references(
    roots: &BTreeMap<String, Vec<OwnedPrCommentReference>>,
) -> BTreeMap<crate::git_durable::GitPrCommentLocation, Vec<OwnedPrCommentReference>> {
    let mut canonical = BTreeMap::new();
    for (id, references) in roots {
        let Some((repository, number)) = parse_pull_request_coordinate(id) else {
            continue;
        };
        for reference in references {
            let Some(comment_id) = reference.sub_anchor.strip_prefix("comment-") else {
                continue;
            };
            let Some(location) = crate::git_durable::GitPrCommentLocation::new(
                repository.clone(),
                number,
                comment_id.to_owned(),
            ) else {
                continue;
            };
            canonical
                .entry(location)
                .or_insert_with(Vec::new)
                .push(reference.clone());
        }
    }
    canonical
}

fn canonical_commit_references(
    roots: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<(String, String), Vec<String>> {
    roots
        .iter()
        .filter_map(|(id, references)| {
            let (repository, oid) = id.rsplit_once(':')?;
            myelin_git::coordinate::RepositorySlug::parse(repository).ok()?;
            myelin_git::core::is_canonical_object_id(oid)
                .then(|| ((repository.to_owned(), oid.to_owned()), references.clone()))
        })
        .collect()
}

fn canonical_git_ref_references(
    roots: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<(String, String), Vec<String>> {
    roots
        .iter()
        .filter_map(|(id, references)| {
            let (repository, ref_name) =
                myelin_git::receive_pack::GitRefEventKey::parse_id(id).ok()?;
            myelin_git::coordinate::RepositorySlug::parse(&repository).ok()?;
            myelin_git::durable::RefKind::from_qualified_name(&ref_name.0)?;
            Some(((repository, ref_name.0), references.clone()))
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OwnedBlobReference {
    original: String,
    sub_anchor: Option<String>,
}

fn blob_root_references(
    viewer: &Principal,
    references: &[String],
) -> BTreeMap<String, Vec<OwnedBlobReference>> {
    let mut owned = BTreeMap::<String, Vec<OwnedBlobReference>>::new();
    for reference in references {
        let Ok(parsed) = myelin_refs::parse_scoped(reference) else {
            continue;
        };
        if parsed.tenant != viewer.tenant || parsed.subsystem != "git" || parsed.type_ != "blob" {
            continue;
        }
        let sub_anchor = match parsed.sub {
            None => None,
            Some(myelin_refs::Sub::LineRange { start, end }) => Some(format!("L{start}-L{end}")),
            Some(_) => continue,
        };
        owned
            .entry(parsed.id)
            .or_default()
            .push(OwnedBlobReference {
                original: reference.clone(),
                sub_anchor,
            });
    }
    owned
}

fn canonical_blob_references(
    roots: &BTreeMap<String, Vec<OwnedBlobReference>>,
) -> BTreeMap<(String, myelin_git::blob_coordinate::GitBlobLocation), Vec<OwnedBlobReference>> {
    roots
        .iter()
        .filter_map(|(id, references)| {
            myelin_git::blob_coordinate::GitBlobEventKey::parse_id(id)
                .ok()
                .map(|coordinate| (coordinate, references.clone()))
        })
        .collect()
}

fn claimed_blob_tombstones(
    owned_references: &BTreeMap<String, Vec<OwnedBlobReference>>,
) -> HashMap<String, ReferenceCard> {
    owned_references
        .values()
        .flatten()
        .map(|reference| (reference.original.clone(), ReferenceCard::Tombstone))
        .collect()
}

fn claimed_pr_comment_tombstones(
    owned_references: &BTreeMap<String, Vec<OwnedPrCommentReference>>,
) -> HashMap<String, ReferenceCard> {
    owned_references
        .values()
        .flatten()
        .map(|reference| (reference.original.clone(), ReferenceCard::Tombstone))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn viewer() -> Principal {
        Principal::stub(
            PrincipalId("reader".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    #[test]
    fn pull_request_roots_have_one_canonical_coordinate_shape() {
        let roots = BTreeMap::from([
            (
                "team/api:41".into(),
                vec!["myelin://acme/git/pr/team/api:41".into()],
            ),
            (
                "team/api:041".into(),
                vec!["myelin://acme/git/pr/team/api:041".into()],
            ),
            (
                "team api:42".into(),
                vec!["myelin://acme/git/pr/team api:42".into()],
            ),
        ]);

        assert_eq!(
            canonical_pull_request_references(&roots),
            BTreeMap::from([(
                ("team/api".into(), 41),
                vec!["myelin://acme/git/pr/team/api:41".into()]
            )])
        );
    }

    #[test]
    fn commit_roots_require_a_repository_and_full_lowercase_object_id() {
        let oid = "0123456789abcdef0123456789abcdef01234567";
        let roots = BTreeMap::from([
            (
                format!("team/api:{oid}"),
                vec![format!("myelin://acme/git/commit/team/api:{oid}")],
            ),
            (
                format!("team/api:{}", oid.to_uppercase()),
                vec![format!(
                    "myelin://acme/git/commit/team/api:{}",
                    oid.to_uppercase()
                )],
            ),
            (
                "team/api:deadbeef".into(),
                vec!["myelin://acme/git/commit/team/api:deadbeef".into()],
            ),
        ]);

        assert_eq!(
            canonical_commit_references(&roots),
            BTreeMap::from([(
                ("team/api".into(), oid.into()),
                vec![format!("myelin://acme/git/commit/team/api:{oid}")]
            )])
        );
    }

    #[test]
    fn git_ref_roots_require_canonical_event_components_and_a_browsable_namespace() {
        let roots = BTreeMap::from([
            (
                "team%2Fapi:refs%2Fheads%2Frelease%2Fone".into(),
                vec!["myelin://acme/git/ref/team%2Fapi:refs%2Fheads%2Frelease%2Fone".into()],
            ),
            (
                "team%2fapi:refs%2Fheads%2Fmain".into(),
                vec!["myelin://acme/git/ref/team%2fapi:refs%2Fheads%2Fmain".into()],
            ),
            (
                "team%2Fapi:refs%2Fnotes%2Fbuild".into(),
                vec!["myelin://acme/git/ref/team%2Fapi:refs%2Fnotes%2Fbuild".into()],
            ),
        ]);

        assert_eq!(
            canonical_git_ref_references(&roots),
            BTreeMap::from([(
                ("team/api".into(), "refs/heads/release/one".into()),
                vec!["myelin://acme/git/ref/team%2Fapi:refs%2Fheads%2Frelease%2Fone".into()]
            )])
        );
    }

    #[test]
    fn blob_roots_share_one_coordinate_and_preserve_only_line_range_subanchors() {
        let root = "myelin://acme/git/blob/team%2Fapi:refs%2Fheads%2Fmain:src%2Fmain%2Ers";
        let references = [
            root.into(),
            format!("{root}#L7-L9"),
            format!("{root}#comment-not-a-blob-anchor"),
            "myelin://acme/git/blob/team/api:main:src%2Fmain.rs".into(),
            "myelin://other/git/blob/team%2Fapi:refs%2Fheads%2Fmain:src%2Fmain%2Ers".into(),
        ];

        let roots = blob_root_references(&viewer(), &references);
        assert_eq!(
            roots.len(),
            2,
            "the malformed legacy id is claimed but not canonical"
        );
        let canonical = canonical_blob_references(&roots);
        assert_eq!(canonical.len(), 1);
        let ((repository, location), references) = canonical.into_iter().next().unwrap();
        assert_eq!(repository, "team/api");
        assert_eq!(location.ref_name(), "refs/heads/main");
        assert_eq!(location.path(), "src/main.rs");
        assert_eq!(
            references
                .into_iter()
                .map(|reference| reference.sub_anchor)
                .collect::<Vec<_>>(),
            vec![None, Some("L7-L9".into())]
        );
    }

    #[test]
    fn pull_request_comments_are_claimed_as_exact_canonical_coordinates() {
        let root = "myelin://acme/git/pr/team/api:41";
        let references = [
            format!("{root}#comment-c-7"),
            format!("{root}#comment-c-07"),
            format!("{root}#thread-t-8"),
            "myelin://other/git/pr/team/api:41#comment-c-7".into(),
        ];

        let roots = pull_request_comment_references(&viewer(), &references);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots.values().next().unwrap().len(), 2);
        assert_eq!(
            canonical_pull_request_comment_references(&roots),
            BTreeMap::from([(
                crate::git_durable::GitPrCommentLocation::new("team/api".into(), 41, "c-7".into(),)
                    .unwrap(),
                vec![OwnedPrCommentReference {
                    original: format!("{root}#comment-c-7"),
                    sub_anchor: "comment-c-7".into(),
                }],
            )])
        );
    }
}
