use myelin_content::Block;

use crate::indexer::{IndexSpec, SearchProjection};

pub const CHAT_SUBSYSTEM: &str = "chat";

pub const MESSAGE_TYPE: &str = "message";

pub const MESSAGE_ACL_OBJECT_TYPE: &str = "channel";

pub use crate::kn_projection::FACET_ARTIFACT_REF;
pub use crate::kn_projection::FACET_EMBED;
pub use crate::kn_projection::FACET_MENTION;

pub fn message_index_spec() -> IndexSpec {
    let struct_fields = crate::kn_projection::kn_page_index_spec().struct_fields;
    IndexSpec::new(CHAT_SUBSYSTEM, MESSAGE_TYPE, struct_fields)
        .with_acl_object_type(MESSAGE_ACL_OBJECT_TYPE)
}

pub fn message_index_specs() -> Vec<IndexSpec> {
    vec![message_index_spec()]
}

pub fn register_message_index_specs() -> Vec<IndexSpec> {
    let specs = message_index_specs();
    let _accepted = crate::indexer::IncrementalIndexer::new(
        specs.clone(),
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
    );
    specs
}

struct NullProjectFetcher;

impl crate::indexer::ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<SearchProjection, crate::indexer::ProjectFetchError> {
        Err(crate::indexer::ProjectFetchError::Gone)
    }
}

pub fn message_search_projection(body: &[Block], lang: Option<&str>) -> SearchProjection {
    crate::kn_projection::page_search_projection(body, lang)
}

pub fn message_doc_ref(tenant: &str, message_id: &str) -> String {
    format!("myelin://{tenant}/chat/{MESSAGE_TYPE}/{message_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::IncrementalIndexer;
    use myelin_content::{parse_inline, Block, HeadingLevel, InlineNode};
    use myelin_events::ArtifactRef;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_query::{FieldType, FieldValue};
    use myelin_tenancy::TenantId;

    fn mention(id: &str) -> InlineNode {
        InlineNode::Mention(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        ))
    }

    #[test]
    fn message_spec_is_the_consumed_6_3_shape() {
        let s = message_index_spec();
        assert_eq!(s.subsystem, "chat");
        assert_eq!(s.type_, "message");
        assert_eq!(
            s.acl_object_type, "channel",
            "a message's reachability is its parent channel's `read` (message.view = parent_channel->read)"
        );
        assert!(
            !s.semantic,
            "Chat is multilingual full-text + reference facets in v1, not vector-embedded"
        );
        assert_eq!(
            s.struct_fields.len(),
            3,
            "exactly the three reference facets"
        );
        for facet in [FACET_MENTION, FACET_ARTIFACT_REF, FACET_EMBED] {
            assert_eq!(
                s.struct_fields.get(facet),
                Some(&FieldType::Relation),
                "`{facet}` is a dependable reference facet (Relation)"
            );
        }
    }

    #[test]
    fn acl_object_is_the_parent_channel_not_the_message() {
        let s = message_index_spec();
        assert_eq!(s.acl_object_type, "channel");
        assert_ne!(
            s.acl_object_type, s.type_,
            "the ACL anchor is the parent channel, NOT the per-message doc (no per-message ACL object)"
        );
    }

    #[test]
    fn cross_subsystem_facets_are_byte_identical_to_kn() {
        let chat = message_index_spec();
        let kn = crate::kn_projection::kn_page_index_spec();
        assert_eq!(
            chat.struct_fields, kn.struct_fields,
            "Chat's reference facets are byte-identical to KN's (X-2 - one cross-producer facet shape)"
        );
        assert_eq!(FACET_MENTION, crate::kn_projection::FACET_MENTION);
        assert_eq!(FACET_ARTIFACT_REF, crate::kn_projection::FACET_ARTIFACT_REF);
        assert_eq!(FACET_EMBED, crate::kn_projection::FACET_EMBED);
    }

    #[test]
    fn message_body_is_not_a_struct_facet() {
        let s = message_index_spec();
        for absent in ["body", "text", "message", "content", "markdown"] {
            assert!(
                !s.struct_fields.contains_key(absent),
                "`{absent}` is full-text projection body, not a structured facet"
            );
        }
    }

    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_message_index_specs();
        assert_eq!(
            accepted,
            message_index_specs(),
            "Search accepts the declared Chat spec verbatim"
        );
        let _ix = IncrementalIndexer::new(
            message_index_specs(),
            std::sync::Arc::new(NullProjectFetcher),
            std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
        );
    }

    #[test]
    fn message_projection_extracts_body_and_structured_facets() {
        let referenced = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let body = vec![
            Block::Heading {
                level: HeadingLevel::new(2).unwrap(),
                inline: parse_inline("standup notes", &[]),
            },
            Block::Paragraph {
                inline: parse_inline(
                    &format!(
                        "blocked on {} - ping {}",
                        myelin_content::OBJ,
                        myelin_content::OBJ
                    ),
                    &[
                        InlineNode::ArtifactRefNode(referenced.clone()),
                        mention("alice"),
                    ],
                ),
            },
        ];
        let p = message_search_projection(&body, Some("en"));

        assert!(p.text.contains("standup notes"));
        assert!(p.text.contains("blocked on"));
        assert_eq!(p.lang.as_deref(), Some("en"));

        assert_eq!(
            p.fields.get(FACET_ARTIFACT_REF),
            Some(&FieldValue::Relation(referenced.0.clone()))
        );
        assert_eq!(
            p.fields.get(FACET_MENTION),
            Some(&FieldValue::Relation("alice".to_string()))
        );
    }

    #[test]
    fn message_with_no_nodes_has_no_reference_facets() {
        let body = vec![Block::Paragraph {
            inline: parse_inline("der Scheduler ist blockiert", &[]),
        }];
        let p = message_search_projection(&body, Some("de"));
        assert!(
            p.fields.is_empty(),
            "no structured nodes ⇒ no reference facets"
        );
        assert!(p.text.contains("Scheduler"));
        assert_eq!(
            p.lang.as_deref(),
            Some("de"),
            "the multilingual lang is carried"
        );
    }

    #[test]
    fn doc_ref_is_the_chat_message_ref() {
        assert_eq!(
            message_doc_ref("acme", "m-7"),
            "myelin://acme/chat/message/m-7"
        );
    }
}
