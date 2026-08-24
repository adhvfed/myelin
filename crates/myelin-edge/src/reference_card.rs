use std::collections::{BTreeMap, HashMap};

use myelin_identity::Principal;
use myelin_issues::StoredIssue;
use serde::Serialize;
use sqlx::types::Uuid;
use std::sync::Arc;

use crate::ci_http::{canonical_uuid, repo_slug_from_ref, DurableCiReadApi};
use crate::{
    DurableAgentThreadReferenceApi, DurableChatReferenceApi, DurableGitBackend,
    DurableIssueReadApi, DurableKnowledgeReadApi,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReferenceCard {
    Projection {
        title: String,
        state: String,
        icon: String,
        render_hint: String,
        sub_anchor: Option<String>,
        flag: Option<String>,
    },
    Reference,
    Tombstone,
}

impl ReferenceCard {
    fn projection(
        title: String,
        state: impl Into<String>,
        icon: impl Into<String>,
        render_hint: impl Into<String>,
    ) -> Self {
        Self::Projection {
            title,
            state: state.into(),
            icon: icon.into(),
            render_hint: render_hint.into(),
            sub_anchor: None,
            flag: None,
        }
    }

    fn issue(issue: &StoredIssue) -> Self {
        Self::projection(issue.title.clone(), issue.state.clone(), "issue", "issue")
    }
}

pub trait ReferenceCardResolver: Send + Sync {
    /// Resolve one viewport at a time. Every requested reference receives a card. Unsupported
    /// artifact types retain their already-visible canonical reference, while missing, denied, and
    /// temporarily unavailable artifacts owned by a known projector collapse to one safe tombstone.
    fn resolve(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard>;
}

trait ReferenceCardProjector: Send + Sync {
    fn project(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard>;
}

#[derive(Clone, Default)]
pub struct DurableReferenceCardResolver {
    projectors: Vec<Arc<dyn ReferenceCardProjector>>,
}

impl DurableReferenceCardResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_issues(mut self, issues: DurableIssueReadApi) -> Self {
        self.projectors
            .push(Arc::new(IssueReferenceCardProjector { issues }));
        self
    }

    pub fn with_knowledge(mut self, knowledge: DurableKnowledgeReadApi) -> Self {
        self.projectors
            .push(Arc::new(KnowledgeReferenceCardProjector { knowledge }));
        self
    }

    pub fn with_git(mut self, git: Arc<DurableGitBackend>) -> Self {
        self.projectors
            .push(Arc::new(GitReferenceCardProjector { git }));
        self
    }

    pub fn with_ci(mut self, ci: DurableCiReadApi) -> Self {
        self.projectors
            .push(Arc::new(CiReferenceCardProjector { ci }));
        self
    }

    pub fn with_chat(mut self, chat: DurableChatReferenceApi) -> Self {
        self.projectors
            .push(Arc::new(ChatReferenceCardProjector { chat }));
        self
    }

    pub fn with_agent_threads(mut self, threads: DurableAgentThreadReferenceApi) -> Self {
        self.projectors
            .push(Arc::new(AgentThreadReferenceCardProjector { threads }));
        self
    }
}

impl ReferenceCardResolver for DurableReferenceCardResolver {
    fn resolve(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard> {
        let mut cards = references
            .iter()
            .cloned()
            .map(|reference| (reference, ReferenceCard::Reference))
            .collect::<HashMap<_, _>>();
        for projector in &self.projectors {
            cards.extend(projector.project(viewer, references));
        }
        cards
    }
}

struct IssueReferenceCardProjector {
    issues: DurableIssueReadApi,
}

impl ReferenceCardProjector for IssueReferenceCardProjector {
    fn project(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard> {
        let issue_references = root_references(viewer, references, "issue", "issue");
        if issue_references.is_empty() {
            return HashMap::new();
        }
        let mut cards = claimed_tombstones(&issue_references);

        let keys = issue_references.keys().cloned().collect::<Vec<_>>();
        let Ok(visible_issues) = self.issues.view_keys(viewer, &keys) else {
            return cards;
        };
        for issue in visible_issues {
            let Some(references) = issue_references.get(&issue.key) else {
                continue;
            };
            let card = ReferenceCard::issue(&issue);
            for reference in references {
                cards.insert(reference.clone(), card.clone());
            }
        }
        cards
    }
}

struct KnowledgeReferenceCardProjector {
    knowledge: DurableKnowledgeReadApi,
}

impl ReferenceCardProjector for KnowledgeReferenceCardProjector {
    fn project(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard> {
        let page_references = root_references(viewer, references, "knowledge", "page");
        if page_references.is_empty() {
            return HashMap::new();
        }
        let mut cards = claimed_tombstones(&page_references);
        let page_ids = page_references.keys().cloned().collect::<Vec<_>>();
        let Ok(visible_pages) = self.knowledge.project_pages(viewer, &page_ids) else {
            return cards;
        };
        for page in visible_pages {
            let (Some(title), Some(references)) = (page.title, page_references.get(&page.page_id))
            else {
                continue;
            };
            let card = ReferenceCard::projection(title, "active", "knowledge", "knowledge_page");
            for reference in references {
                cards.insert(reference.clone(), card.clone());
            }
        }
        cards
    }
}

struct GitReferenceCardProjector {
    git: Arc<DurableGitBackend>,
}

struct CiReferenceCardProjector {
    ci: DurableCiReadApi,
}

struct ChatReferenceCardProjector {
    chat: DurableChatReferenceApi,
}

struct AgentThreadReferenceCardProjector {
    threads: DurableAgentThreadReferenceApi,
}

impl ReferenceCardProjector for AgentThreadReferenceCardProjector {
    fn project(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard> {
        let thread_roots = root_references(viewer, references, "agent", "thread");
        if thread_roots.is_empty() {
            return HashMap::new();
        }
        let mut cards = claimed_tombstones(&thread_roots);
        let canonical_references = thread_roots
            .iter()
            .filter(|(thread_id, _)| canonical_uuid(thread_id))
            .map(|(thread_id, references)| (thread_id.clone(), references.clone()))
            .collect::<BTreeMap<_, _>>();
        let thread_ids = canonical_references
            .keys()
            .filter_map(|thread_id| Uuid::parse_str(thread_id).ok())
            .collect::<Vec<_>>();
        let Ok(visible_threads) = self.threads.project_threads(viewer, &thread_ids) else {
            return cards;
        };

        for thread in visible_threads {
            let Some(references) = canonical_references.get(&thread.thread_id) else {
                continue;
            };
            let card = ReferenceCard::projection(
                thread.name,
                thread.state.token(),
                "agent",
                "agent_thread",
            );
            for reference in references {
                cards.insert(reference.clone(), card.clone());
            }
        }
        cards
    }
}

impl ReferenceCardProjector for ChatReferenceCardProjector {
    fn project(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard> {
        let conversation_roots = root_references(viewer, references, "chat", "channel");
        if conversation_roots.is_empty() {
            return HashMap::new();
        }
        let mut cards = claimed_tombstones(&conversation_roots);
        let canonical_references = conversation_roots
            .iter()
            .filter(|(conversation_id, _)| myelin_chat::is_canonical_ulid(conversation_id))
            .map(|(conversation_id, references)| (conversation_id.clone(), references.clone()))
            .collect::<BTreeMap<_, _>>();
        let conversation_ids = canonical_references.keys().cloned().collect::<Vec<_>>();
        let Ok(visible_conversations) = self.chat.project_conversations(viewer, &conversation_ids)
        else {
            return cards;
        };

        for conversation in visible_conversations {
            let (Some(title), Some(references)) = (
                conversation.topic,
                canonical_references.get(&conversation.id.conversation_id),
            ) else {
                continue;
            };
            let card = ReferenceCard::projection(title, "active", "chat", "chat_conversation");
            for reference in references {
                cards.insert(reference.clone(), card.clone());
            }
        }
        cards
    }
}

impl ReferenceCardProjector for CiReferenceCardProjector {
    fn project(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard> {
        let run_roots = root_references(viewer, references, "ci", "run");
        if run_roots.is_empty() {
            return HashMap::new();
        }
        let mut cards = claimed_tombstones(&run_roots);
        let run_references = canonical_ci_run_references(&run_roots);
        let run_ids = run_references.keys().cloned().collect::<Vec<_>>();
        let Ok(visible_runs) = self.ci.project_run_summaries(viewer, &run_ids) else {
            return cards;
        };

        for run in visible_runs {
            let (Some(repository), Some(references)) = (
                repo_slug_from_ref(viewer.tenant.as_str(), &run.repo_ref),
                run_references.get(&run.run_id),
            ) else {
                continue;
            };
            let card =
                ReferenceCard::projection(format!("{repository} CI"), run.state, "ci", "ci_run");
            for reference in references {
                cards.insert(reference.clone(), card.clone());
            }
        }
        cards
    }
}

impl ReferenceCardProjector for GitReferenceCardProjector {
    fn project(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard> {
        let repository_references = root_references(viewer, references, "git", "repo");
        let pull_request_roots = root_references(viewer, references, "git", "pr");
        let commit_roots = root_references(viewer, references, "git", "commit");
        let ref_roots = root_references(viewer, references, "git", "ref");
        if repository_references.is_empty()
            && pull_request_roots.is_empty()
            && commit_roots.is_empty()
            && ref_roots.is_empty()
        {
            return HashMap::new();
        }
        let mut cards = claimed_tombstones(&repository_references);
        cards.extend(claimed_tombstones(&pull_request_roots));
        cards.extend(claimed_tombstones(&commit_roots));
        cards.extend(claimed_tombstones(&ref_roots));

        let repositories = repository_references
            .keys()
            .filter(|slug| myelin_git::coordinate::RepositorySlug::parse(slug).is_ok())
            .cloned()
            .collect::<Vec<_>>();
        let pull_request_references = canonical_pull_request_references(&pull_request_roots);
        let pull_requests = pull_request_references.keys().cloned().collect::<Vec<_>>();
        let commit_references = canonical_commit_references(&commit_roots);
        let commits = commit_references.keys().cloned().collect::<Vec<_>>();
        let ref_references = canonical_git_ref_references(&ref_roots);
        let refs = ref_references.keys().cloned().collect::<Vec<_>>();
        let request = crate::git_durable::GitReferenceCardRequest {
            repositories: &repositories,
            pull_requests: &pull_requests,
            commits: &commits,
            refs: &refs,
        };
        let Ok(visible) = self.git.visible_reference_cards(viewer, request) else {
            return cards;
        };

        for repository in visible.repositories {
            let Some(references) = repository_references.get(&repository) else {
                continue;
            };
            let card = ReferenceCard::projection(repository, "active", "git", "git_repository");
            for reference in references {
                cards.insert(reference.clone(), card.clone());
            }
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
            for reference in references {
                cards.insert(reference.clone(), card.clone());
            }
        }
        for commit in visible.commits {
            let coordinate = (commit.repository, commit.oid);
            let Some(references) = commit_references.get(&coordinate) else {
                continue;
            };
            let card = ReferenceCard::projection(commit.title, "committed", "commit", "git_commit");
            for reference in references {
                cards.insert(reference.clone(), card.clone());
            }
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
            for reference in references {
                cards.insert(reference.clone(), card.clone());
            }
        }
        cards
    }
}

fn canonical_pull_request_references(
    roots: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<(String, u64), Vec<String>> {
    roots
        .iter()
        .filter_map(|(id, references)| {
            let (repository, number) = id.rsplit_once(':')?;
            myelin_git::coordinate::RepositorySlug::parse(repository).ok()?;
            let number = myelin_git::coordinate::parse_positive_decimal(number)?;
            i64::try_from(number).ok()?;
            Some(((repository.to_owned(), number), references.clone()))
        })
        .collect()
}

fn canonical_ci_run_references(
    roots: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Vec<String>> {
    roots
        .iter()
        .filter(|(run_id, _)| canonical_uuid(run_id))
        .map(|(run_id, references)| (run_id.clone(), references.clone()))
        .collect()
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

fn root_references(
    viewer: &Principal,
    references: &[String],
    subsystem: &str,
    artifact_type: &str,
) -> BTreeMap<String, Vec<String>> {
    let mut owned_references = BTreeMap::<String, Vec<String>>::new();
    for reference in references {
        let Ok(parsed) = myelin_refs::parse_scoped(reference) else {
            continue;
        };
        if parsed.tenant == viewer.tenant
            && parsed.subsystem == subsystem
            && parsed.type_ == artifact_type
            && parsed.sub.is_none()
        {
            owned_references
                .entry(parsed.id)
                .or_default()
                .push(reference.clone());
        }
    }
    owned_references
}

fn claimed_tombstones(
    owned_references: &BTreeMap<String, Vec<String>>,
) -> HashMap<String, ReferenceCard> {
    owned_references
        .values()
        .flatten()
        .cloned()
        .map(|reference| (reference, ReferenceCard::Tombstone))
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
    fn only_current_tenant_issue_roots_reach_the_issue_owner() {
        let references = [
            "myelin://acme/issue/issue/ENG-41".into(),
            "myelin://acme/issue/issue/ENG-41#field-state".into(),
            "myelin://other/issue/issue/ENG-42".into(),
            "myelin://acme/knowledge/page/runbook".into(),
        ];

        assert_eq!(
            root_references(&viewer(), &references, "issue", "issue"),
            BTreeMap::from([(
                "ENG-41".into(),
                vec!["myelin://acme/issue/issue/ENG-41".into()]
            )])
        );
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
    fn ci_run_roots_accept_only_canonical_lowercase_uuids() {
        let canonical = "3d782b25-2fb0-4f44-aa7c-3cb434e5ead7";
        let roots = BTreeMap::from([
            (
                canonical.into(),
                vec![format!("myelin://acme/ci/run/{canonical}")],
            ),
            (
                "3D782B25-2FB0-4F44-AA7C-3CB434E5EAD7".into(),
                vec!["myelin://acme/ci/run/3D782B25-2FB0-4F44-AA7C-3CB434E5EAD7".into()],
            ),
        ]);

        assert_eq!(
            canonical_ci_run_references(&roots),
            BTreeMap::from([(
                canonical.into(),
                vec![format!("myelin://acme/ci/run/{canonical}")]
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
    fn a_tombstone_serializes_without_a_leaky_reason_or_title() {
        assert_eq!(
            serde_json::to_value(ReferenceCard::Tombstone).unwrap(),
            serde_json::json!({ "kind": "tombstone" })
        );
    }
}
