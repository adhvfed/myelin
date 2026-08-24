use std::collections::{BTreeMap, HashMap};

use myelin_identity::Principal;
use myelin_issues::StoredIssue;
use serde::Serialize;
use std::sync::Arc;

use crate::{DurableIssueReadApi, DurableKnowledgeReadApi};

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
    fn a_tombstone_serializes_without_a_leaky_reason_or_title() {
        assert_eq!(
            serde_json::to_value(ReferenceCard::Tombstone).unwrap(),
            serde_json::json!({ "kind": "tombstone" })
        );
    }
}
