use std::collections::{BTreeMap, HashMap};

use myelin_identity::Principal;
use myelin_issues::StoredIssue;
use serde::Serialize;

use crate::DurableIssueReadApi;

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
    fn issue(issue: &StoredIssue) -> Self {
        Self::Projection {
            title: issue.title.clone(),
            state: issue.state.clone(),
            icon: "issue".into(),
            render_hint: "issue".into(),
            sub_anchor: None,
            flag: None,
        }
    }
}

pub trait ReferenceCardResolver: Send + Sync {
    /// Resolve one viewport at a time. Every requested reference receives a card. Unsupported
    /// artifact types retain their already-visible canonical reference, while missing, denied, and
    /// temporarily unavailable artifacts owned by a known projector collapse to one safe tombstone.
    fn resolve(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard>;
}

#[derive(Clone)]
pub struct DurableReferenceCardResolver {
    issues: DurableIssueReadApi,
}

impl DurableReferenceCardResolver {
    pub fn new(issues: DurableIssueReadApi) -> Self {
        Self { issues }
    }
}

impl ReferenceCardResolver for DurableReferenceCardResolver {
    fn resolve(&self, viewer: &Principal, references: &[String]) -> HashMap<String, ReferenceCard> {
        let mut cards = references
            .iter()
            .cloned()
            .map(|reference| (reference, ReferenceCard::Reference))
            .collect::<HashMap<_, _>>();
        let issue_references = issue_references(viewer, references);
        if issue_references.is_empty() {
            return cards;
        }
        for references in issue_references.values() {
            for reference in references {
                cards.insert(reference.clone(), ReferenceCard::Tombstone);
            }
        }

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

fn issue_references(viewer: &Principal, references: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut issue_references = BTreeMap::<String, Vec<String>>::new();
    for reference in references {
        let Ok(parsed) = myelin_refs::parse_scoped(reference) else {
            continue;
        };
        if parsed.tenant == viewer.tenant
            && parsed.subsystem == "issue"
            && parsed.type_ == "issue"
            && parsed.sub.is_none()
        {
            issue_references
                .entry(parsed.id)
                .or_default()
                .push(reference.clone());
        }
    }
    issue_references
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
            issue_references(&viewer(), &references),
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
