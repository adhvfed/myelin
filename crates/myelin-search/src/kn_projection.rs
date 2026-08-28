use std::collections::BTreeMap;

use myelin_content::{Block, Inline, InlineNode};
use myelin_query::{FieldType, FieldValue};

use crate::indexer::{IndexSpec, SearchProjection};

pub const KN_SUBSYSTEM: &str = "knowledge";

pub const KN_PAGE_TYPE: &str = "page";

pub const KN_ROW_TYPE: &str = "row";

pub const FACET_MENTION: &str = "mention";
pub const FACET_ARTIFACT_REF: &str = "artifact_ref";
pub const FACET_EMBED: &str = "embed";

pub fn kn_page_index_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    struct_fields.insert(FACET_MENTION.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_ARTIFACT_REF.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_EMBED.to_string(), FieldType::Relation);
    IndexSpec::new(KN_SUBSYSTEM, KN_PAGE_TYPE, struct_fields).semantic()
}

pub fn kn_row_index_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    struct_fields.insert("priority".to_string(), FieldType::Select);
    struct_fields.insert("owner".to_string(), FieldType::Principal);
    struct_fields.insert("due".to_string(), FieldType::Date);
    struct_fields.insert(
        crate::engine::ORDER_KEY_FIELD.to_string(),
        FieldType::OrderKey,
    );
    IndexSpec::new(KN_SUBSYSTEM, KN_ROW_TYPE, struct_fields)
}

pub fn kn_index_specs() -> Vec<IndexSpec> {
    vec![kn_page_index_spec(), kn_row_index_spec()]
}

pub fn page_search_projection(blocks: &[Block], lang: Option<&str>) -> SearchProjection {
    let mut text = String::new();
    let mut mentions: Vec<String> = Vec::new();
    let mut artifact_refs: Vec<String> = Vec::new();
    let mut embeds: Vec<String> = Vec::new();

    for block in blocks {
        collect_block(
            block,
            &mut text,
            &mut mentions,
            &mut artifact_refs,
            &mut embeds,
        );
    }

    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    if let Some(m) = mentions.first() {
        fields.insert(FACET_MENTION.to_string(), FieldValue::Relation(m.clone()));
    }
    if let Some(a) = artifact_refs.first() {
        fields.insert(
            FACET_ARTIFACT_REF.to_string(),
            FieldValue::Relation(a.clone()),
        );
    }
    if let Some(e) = embeds.first() {
        fields.insert(FACET_EMBED.to_string(), FieldValue::Relation(e.clone()));
    }

    SearchProjection {
        text,
        fields,
        lang: lang.map(|s| s.to_string()),
    }
}

fn collect_block(
    block: &Block,
    text: &mut String,
    mentions: &mut Vec<String>,
    artifact_refs: &mut Vec<String>,
    embeds: &mut Vec<String>,
) {
    match block {
        Block::Paragraph { inline } | Block::Heading { inline, .. } => {
            collect_inline(inline, text, mentions, artifact_refs, embeds);
        }
        Block::BulletList { items } | Block::OrderedList { items, .. } => {
            for item in items {
                for b in &item.blocks {
                    collect_block(b, text, mentions, artifact_refs, embeds);
                }
            }
        }
        Block::TaskList { items } => {
            for item in items {
                collect_inline(&item.inline, text, mentions, artifact_refs, embeds);
            }
        }
        Block::Blockquote { blocks } | Block::Callout { blocks, .. } => {
            for b in blocks {
                collect_block(b, text, mentions, artifact_refs, embeds);
            }
        }
        Block::CodeBlock { text: code, .. } => {
            push_text(text, code);
        }
        Block::Table { columns, rows } => {
            for col in columns {
                collect_inline(&col.header, text, mentions, artifact_refs, embeds);
            }
            for row in rows {
                for cell in row {
                    for b in &cell.blocks {
                        collect_block(b, text, mentions, artifact_refs, embeds);
                    }
                }
            }
        }
        Block::Toggle { summary, blocks } => {
            collect_inline(summary, text, mentions, artifact_refs, embeds);
            for b in blocks {
                collect_block(b, text, mentions, artifact_refs, embeds);
            }
        }
        Block::Image { alt, caption, .. } => {
            push_text(text, alt);
            if let Some(c) = caption {
                collect_inline(c, text, mentions, artifact_refs, embeds);
            }
        }
        Block::Embed { reference, .. } => {
            embeds.push(reference.0.clone());
        }
        Block::DbView { .. } | Block::SyncBlock { .. } | Block::Divider => {}
    }
}

fn collect_inline(
    inline: &Inline,
    text: &mut String,
    mentions: &mut Vec<String>,
    artifact_refs: &mut Vec<String>,
    embeds: &mut Vec<String>,
) {
    for span in &inline.spans {
        if let myelin_content::Span::Text { text: run, .. } = span {
            push_text(text, run);
        }
    }
    for node in inline.structured_nodes() {
        match node {
            InlineNode::Mention(principal) => mentions.push(principal.principal_id.0.clone()),
            InlineNode::ArtifactRefNode(r) => artifact_refs.push(r.0.clone()),
            InlineNode::Embed(r) => embeds.push(r.0.clone()),
        }
    }
}

fn push_text(text: &mut String, run: &str) {
    if !text.is_empty() && !run.is_empty() {
        text.push(' ');
    }
    text.push_str(run);
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::{parse_inline, Block, HeadingLevel};
    use myelin_events::ArtifactRef;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn mention(id: &str) -> InlineNode {
        InlineNode::Mention(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        ))
    }

    #[test]
    fn page_spec_is_kn_owned_6_3_shape() {
        let s = kn_page_index_spec();
        assert_eq!(s.subsystem, "knowledge");
        assert_eq!(s.type_, "page");
        assert_eq!(
            s.acl_object_type, "page",
            "a page's reachability is the page-tree's"
        );
        assert!(
            s.semantic,
            "a page is semantically indexed (vector-in-v1, §4.5)"
        );
        assert_eq!(s.struct_fields.len(), 3);
        for facet in [FACET_MENTION, FACET_ARTIFACT_REF, FACET_EMBED] {
            assert_eq!(
                s.struct_fields.get(facet),
                Some(&FieldType::Relation),
                "`{facet}` is a dependable reference facet (Relation)"
            );
        }
    }

    #[test]
    fn row_spec_is_the_gin_scan_facet_shape() {
        let s = kn_row_index_spec();
        assert_eq!(s.subsystem, "knowledge");
        assert_eq!(s.type_, "row");
        assert!(
            !s.semantic,
            "a db row is a structured record, not vector-embedded prose"
        );
        assert_eq!(s.struct_fields.get("priority"), Some(&FieldType::Select));
        assert_eq!(s.struct_fields.get("owner"), Some(&FieldType::Principal));
        assert_eq!(s.struct_fields.get("due"), Some(&FieldType::Date));
        assert_eq!(
            s.struct_fields.get(crate::engine::ORDER_KEY_FIELD),
            Some(&FieldType::OrderKey)
        );
        assert!(!s.struct_fields.contains_key("rollup"));
        assert!(!s.struct_fields.contains_key("formula"));
    }

    #[test]
    fn page_projection_extracts_text_and_structured_facets() {
        let referenced = ArtifactRef("myelin://acme/issues/issue/ENG-1".into());
        let embedded = ArtifactRef("myelin://acme/knowledge/page/99".into());
        let blocks = vec![
            Block::Heading {
                level: HeadingLevel::new(1).unwrap(),
                inline: parse_inline("Design Notes", &[]),
            },
            Block::Paragraph {
                inline: parse_inline(
                    &format!(
                        "see {} and ping {}",
                        myelin_content::OBJ,
                        myelin_content::OBJ
                    ),
                    &[
                        InlineNode::ArtifactRefNode(referenced.clone()),
                        mention("alice"),
                    ],
                ),
            },
            Block::CodeBlock {
                lang: Some("rust".into()),
                text: "let x = scheduler_deadlock();".into(),
            },
            Block::Embed {
                reference: embedded.clone(),
                display: myelin_content::EmbedDisplay::Card,
            },
        ];
        let p = page_search_projection(&blocks, Some("en"));

        assert!(p.text.contains("Design Notes"));
        assert!(
            p.text.contains("scheduler_deadlock"),
            "raw code body is indexed (X-2)"
        );
        assert_eq!(p.lang.as_deref(), Some("en"));

        assert_eq!(
            p.fields.get(FACET_ARTIFACT_REF),
            Some(&FieldValue::Relation(referenced.0.clone()))
        );
        assert_eq!(
            p.fields.get(FACET_MENTION),
            Some(&FieldValue::Relation("alice".to_string()))
        );
        assert_eq!(
            p.fields.get(FACET_EMBED),
            Some(&FieldValue::Relation(embedded.0.clone())),
            "the structured embed block is a dependable embed facet"
        );
    }

    #[test]
    fn page_with_no_nodes_has_no_reference_facets() {
        let blocks = vec![Block::Paragraph {
            inline: parse_inline("plain prose only", &[]),
        }];
        let p = page_search_projection(&blocks, None);
        assert!(
            p.fields.is_empty(),
            "no structured nodes ⇒ no reference facets"
        );
        assert!(p.text.contains("plain prose"));
        assert!(
            p.lang.is_none(),
            "no source-declared lang ⇒ the indexer detects it (§4.7)"
        );
    }
}
