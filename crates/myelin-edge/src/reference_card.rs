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

mod git;

use git::GitReferenceCardProjector;

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

    fn projection_at(
        title: String,
        state: impl Into<String>,
        icon: impl Into<String>,
        render_hint: impl Into<String>,
        sub_anchor: Option<String>,
    ) -> Self {
        let mut card = Self::projection(title, state, icon, render_hint);
        if let Self::Projection {
            sub_anchor: anchor, ..
        } = &mut card
        {
            *anchor = sub_anchor;
        }
        card
    }

    fn projection_at_with_flag(
        title: String,
        state: impl Into<String>,
        icon: impl Into<String>,
        render_hint: impl Into<String>,
        sub_anchor: Option<String>,
        flag: Option<String>,
    ) -> Self {
        let mut card = Self::projection_at(title, state, icon, render_hint, sub_anchor);
        if let Self::Projection {
            flag: card_flag, ..
        } = &mut card
        {
            *card_flag = flag;
        }
        card
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
            .push(Arc::new(GitReferenceCardProjector::new(git)));
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

fn canonical_ci_run_references(
    roots: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Vec<String>> {
    roots
        .iter()
        .filter(|(run_id, _)| canonical_uuid(run_id))
        .map(|(run_id, references)| (run_id.clone(), references.clone()))
        .collect()
}

pub(super) fn root_references(
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

pub(super) fn claimed_tombstones(
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
    fn a_tombstone_serializes_without_a_leaky_reason_or_title() {
        assert_eq!(
            serde_json::to_value(ReferenceCard::Tombstone).unwrap(),
            serde_json::json!({ "kind": "tombstone" })
        );
    }
}
