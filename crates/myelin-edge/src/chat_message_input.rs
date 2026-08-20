use myelin_content::{InlineNode, OBJ};
use myelin_identity::{Principal, PrincipalId};
use serde::Deserialize;

use crate::EdgeError;

const MAX_MESSAGE_BYTES: usize = 32 * 1024;
const MAX_STRUCTURED_NODES: usize = 32;
const MAX_ARTIFACT_REF_BYTES: usize = 1024;
const MAX_PRINCIPAL_ID_BYTES: usize = 255;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageInput {
    pub content: String,
    #[serde(default)]
    references: Vec<String>,
    #[serde(default)]
    nodes: Vec<NodeInput>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum NodeInput {
    ArtifactRef {
        #[serde(rename = "ref")]
        reference: String,
    },
    Mention {
        principal_id: String,
    },
}

impl MessageInput {
    pub(crate) fn references(content: &str, references: &[String]) -> Self {
        Self {
            content: content.to_string(),
            references: references.to_vec(),
            nodes: Vec::new(),
        }
    }

    pub(crate) fn validate_content(&self) -> Result<(), EdgeError> {
        if self.content.len() > MAX_MESSAGE_BYTES || self.content.trim().is_empty() {
            return Err(EdgeError::BadRequest(
                "Chat message must contain 1-32768 UTF-8 bytes".into(),
            ));
        }
        if self.content.chars().any(|character| {
            character == '\0' || (character.is_control() && character != '\n' && character != '\t')
        }) {
            return Err(EdgeError::BadRequest(
                "Chat message contains an unsupported control character".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn resolve_nodes(
        &self,
        author: &Principal,
        mut resolve_mention: impl FnMut(&PrincipalId) -> Result<Principal, EdgeError>,
    ) -> Result<Vec<InlineNode>, EdgeError> {
        if !self.references.is_empty() && !self.nodes.is_empty() {
            return Err(EdgeError::BadRequest(
                "Chat message must use either nodes or the legacy references field, not both"
                    .into(),
            ));
        }

        let node_count = if self.nodes.is_empty() {
            self.references.len()
        } else {
            self.nodes.len()
        };
        if node_count > MAX_STRUCTURED_NODES {
            return Err(EdgeError::BadRequest(format!(
                "Chat message may contain at most {MAX_STRUCTURED_NODES} structured nodes"
            )));
        }
        let placeholders = self
            .content
            .chars()
            .filter(|character| *character == OBJ)
            .count();
        if placeholders != node_count {
            return Err(EdgeError::BadRequest(
                "Chat content must contain one U+FFFC placeholder for each structured node".into(),
            ));
        }

        if self.nodes.is_empty() {
            return self
                .references
                .iter()
                .map(|reference| artifact_node(author, reference))
                .collect();
        }

        self.nodes
            .iter()
            .map(|node| match node {
                NodeInput::ArtifactRef { reference } => artifact_node(author, reference),
                NodeInput::Mention { principal_id } => {
                    validate_principal_id(principal_id)?;
                    resolve_mention(&PrincipalId(principal_id.clone())).map(InlineNode::Mention)
                }
            })
            .collect()
    }
}

fn artifact_node(author: &Principal, reference: &str) -> Result<InlineNode, EdgeError> {
    if reference.len() > MAX_ARTIFACT_REF_BYTES {
        return Err(EdgeError::BadRequest(format!(
            "Chat ArtifactRef exceeds {MAX_ARTIFACT_REF_BYTES} bytes"
        )));
    }
    let parsed = myelin_refs::parse_scoped(reference)
        .map_err(|error| EdgeError::BadRequest(format!("invalid Chat ArtifactRef: {error}")))?;
    if parsed.tenant != author.tenant {
        return Err(EdgeError::BadRequest(
            "Chat cannot store a cross-tenant ArtifactRef".into(),
        ));
    }
    Ok(InlineNode::ArtifactRefNode(parsed.artifact_ref))
}

fn validate_principal_id(principal_id: &str) -> Result<(), EdgeError> {
    if principal_id.trim() == principal_id
        && !principal_id.is_empty()
        && principal_id.len() <= MAX_PRINCIPAL_ID_BYTES
        && !principal_id.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "Chat mention principal_id must be 1-255 clean UTF-8 bytes".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalKind, PrincipalStatus};
    use myelin_tenancy::TenantId;

    fn principal(tenant: &str, id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )
    }

    #[test]
    fn ordered_nodes_preserve_the_message_authors_intent() {
        let body: MessageInput = serde_json::from_value(serde_json::json!({
            "content": "Ask \u{FFFC} about \u{FFFC}",
            "nodes": [
                { "kind": "mention", "principal_id": "reviewer" },
                { "kind": "artifact_ref", "ref": "myelin://acme/issue/issue/ENG-41" }
            ]
        }))
        .unwrap();

        let nodes = body
            .resolve_nodes(&principal("acme", "author"), |id| {
                Ok(principal("acme", &id.0))
            })
            .unwrap();

        assert!(matches!(
            &nodes[0],
            InlineNode::Mention(mentioned) if mentioned.principal_id.0 == "reviewer"
        ));
        assert!(matches!(&nodes[1], InlineNode::ArtifactRefNode(_)));
    }

    #[test]
    fn legacy_references_remain_a_small_compatible_subset() {
        let body = MessageInput::references(
            "Tracking \u{FFFC}",
            &["myelin://acme/issue/issue/ENG-41".into()],
        );
        assert_eq!(
            body.resolve_nodes(&principal("acme", "author"), |_| unreachable!())
                .unwrap(),
            vec![InlineNode::ArtifactRefNode(myelin_refs::ArtifactRef(
                "myelin://acme/issue/issue/ENG-41".into()
            ))]
        );
    }

    #[test]
    fn malformed_or_ambiguous_structure_is_refused_before_storage() {
        let author = principal("acme", "author");
        let resolve = |_: &PrincipalId| Ok(principal("acme", "reviewer"));

        for value in [
            serde_json::json!({
                "content": "No placeholder",
                "nodes": [{ "kind": "mention", "principal_id": "reviewer" }]
            }),
            serde_json::json!({
                "content": "\u{FFFC}",
                "references": ["myelin://acme/issue/issue/ENG-41"],
                "nodes": [{ "kind": "mention", "principal_id": "reviewer" }]
            }),
            serde_json::json!({
                "content": "\u{FFFC}",
                "nodes": [{ "kind": "mention", "principal_id": " bad" }]
            }),
        ] {
            let input: MessageInput = serde_json::from_value(value).unwrap();
            assert!(input.resolve_nodes(&author, resolve).is_err());
        }
    }

    #[test]
    fn mention_resolution_decides_the_authoritative_identity_state() {
        let body: MessageInput = serde_json::from_value(serde_json::json!({
            "content": "\u{FFFC}",
            "nodes": [{ "kind": "mention", "principal_id": "reviewer" }]
        }))
        .unwrap();
        let error = body
            .resolve_nodes(&principal("acme", "author"), |_| {
                let mut suspended = principal("acme", "reviewer");
                suspended.status = PrincipalStatus::Suspended;
                Err(EdgeError::BadRequest(format!(
                    "{} is unavailable",
                    suspended.principal_id.0
                )))
            })
            .unwrap_err();
        assert!(matches!(error, EdgeError::BadRequest(_)));
    }
}
