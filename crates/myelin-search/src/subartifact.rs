use std::collections::BTreeMap;

use myelin_content::Block;
use myelin_query::{FieldType, FieldValue, OrderKey};
use myelin_refs::{sub_kind, Sub, SubKind};
use myelin_tenancy::ArtifactRef;

use crate::git_code_projection::{
    git_blob_search_projection, GitBlobProjectionInput, FACET_BLOB_OID, FACET_LANGUAGE, FACET_PATH,
};
use crate::indexer::SearchProjection;
use crate::kn_projection::page_search_projection;

pub const FACET_LINE_START: &str = "line_start";
pub const FACET_LINE_END: &str = "line_end";
pub const FACET_ANCHOR_STATE: &str = "anchor_state";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubGrain {
    Root,
    Block(String),
    Heading(String),
    Row(String),
    Field(String),
    LineRange { minted_start: u64, minted_end: u64 },
    M4Producer(SubKind),
}

impl SubGrain {
    pub fn classify(ref_: &ArtifactRef) -> SubGrain {
        match sub_kind(ref_) {
            None => SubGrain::Root,
            Some(Sub::Block(id)) => SubGrain::Block(id),
            Some(Sub::Heading(id)) => SubGrain::Heading(id),
            Some(Sub::Row(id)) => SubGrain::Row(id),
            Some(Sub::Field(id)) => SubGrain::Field(id),
            Some(Sub::LineRange { start, end }) => SubGrain::LineRange {
                minted_start: start,
                minted_end: end,
            },
            Some(other) => SubGrain::M4Producer(other.kind()),
        }
    }

    pub fn sub_kind(&self) -> Option<SubKind> {
        match self {
            SubGrain::Root => None,
            SubGrain::Block(_) => Some(SubKind::Block),
            SubGrain::Heading(_) => Some(SubKind::Heading),
            SubGrain::Row(_) => Some(SubKind::Row),
            SubGrain::Field(_) => Some(SubKind::Field),
            SubGrain::LineRange { .. } => Some(SubKind::LineRange),
            SubGrain::M4Producer(k) => Some(*k),
        }
    }

    pub fn is_m3_exercised(&self) -> bool {
        !matches!(self, SubGrain::M4Producer(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentAnchoredSpan {
    pub path: String,
    pub language: String,
    pub blob_oid: String,
    pub resolved_start: u64,
    pub resolved_end: u64,
    pub span_text: String,
    pub anchor_state: AnchorState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorState {
    Exact,
    Rebased,
    Partial,
}

impl AnchorState {
    pub const fn label(self) -> &'static str {
        match self {
            AnchorState::Exact => "exact",
            AnchorState::Rebased => "rebased",
            AnchorState::Partial => "partial",
        }
    }
}

pub fn block_subdoc_projection(block: &Block, lang: Option<&str>) -> SearchProjection {
    page_search_projection(std::slice::from_ref(block), lang)
}

pub fn db_row_subdoc_projection(
    fields: &BTreeMap<String, FieldValue>,
    full_text: &str,
    order_key: Option<OrderKey>,
) -> SearchProjection {
    let mut out: BTreeMap<String, FieldValue> = fields.clone();
    if let Some(ok) = order_key {
        out.insert(
            crate::engine::ORDER_KEY_FIELD.to_string(),
            FieldValue::OrderKey(ok),
        );
    }
    SearchProjection {
        text: full_text.to_string(),
        fields: out,
        lang: None,
    }
}

pub fn db_field_subdoc_projection(
    field_name: &str,
    value: FieldValue,
    rendered_text: &str,
) -> SearchProjection {
    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    fields.insert(field_name.to_string(), value);
    SearchProjection {
        text: rendered_text.to_string(),
        fields,
        lang: None,
    }
}

pub fn line_range_subdoc_projection(span: &ContentAnchoredSpan) -> SearchProjection {
    let mut projection = git_blob_search_projection(&GitBlobProjectionInput {
        path: span.path.clone(),
        language: span.language.clone(),
        text: span.span_text.clone(),
        literals: Vec::new(),
        commit_message: String::new(),
        blob_oid: span.blob_oid.clone(),
    });

    projection.fields.insert(
        FACET_LINE_START.to_string(),
        FieldValue::Text(span.resolved_start.to_string()),
    );
    projection.fields.insert(
        FACET_LINE_END.to_string(),
        FieldValue::Text(span.resolved_end.to_string()),
    );
    projection.fields.insert(
        FACET_ANCHOR_STATE.to_string(),
        FieldValue::Text(span.anchor_state.label().to_string()),
    );
    projection
}

pub fn line_range_subdoc_facets() -> BTreeMap<String, FieldType> {
    let mut f: BTreeMap<String, FieldType> = BTreeMap::new();
    f.insert(FACET_PATH.to_string(), FieldType::Text);
    f.insert(FACET_LANGUAGE.to_string(), FieldType::Text);
    f.insert(FACET_BLOB_OID.to_string(), FieldType::Text);
    f.insert(FACET_LINE_START.to_string(), FieldType::Text);
    f.insert(FACET_LINE_END.to_string(), FieldType::Text);
    f.insert(FACET_ANCHOR_STATE.to_string(), FieldType::Text);
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::{parse_inline, HeadingLevel, InlineNode};
    use myelin_events::ArtifactRef as EvArtifactRef;

    fn ref_(s: &str) -> ArtifactRef {
        ArtifactRef(s.to_string())
    }

    #[test]
    fn classifies_each_m3_sub_grain_through_the_frozen_grammar() {
        assert_eq!(
            SubGrain::classify(&ref_("myelin://acme/knowledge/page/42")),
            SubGrain::Root
        );
        assert_eq!(
            SubGrain::classify(&ref_("myelin://acme/knowledge/page/42#b9")),
            SubGrain::Block("9".into())
        );
        assert_eq!(
            SubGrain::classify(&ref_("myelin://acme/knowledge/page/42#hintro")),
            SubGrain::Heading("intro".into())
        );
        assert_eq!(
            SubGrain::classify(&ref_("myelin://acme/knowledge/row/tasks:r7#row-r7")),
            SubGrain::Row("r7".into())
        );
        assert_eq!(
            SubGrain::classify(&ref_("myelin://acme/knowledge/row/tasks:r7#field-priority")),
            SubGrain::Field("priority".into())
        );
        assert_eq!(
            SubGrain::classify(&ref_("myelin://acme/git/blob/repo:main:src/x.rs#L42-L88")),
            SubGrain::LineRange {
                minted_start: 42,
                minted_end: 88
            }
        );
    }

    #[test]
    fn m3_grains_are_exercised_and_m4_kinds_are_named_floor() {
        for (r, kind) in [
            ("myelin://acme/knowledge/page/1#b2", SubKind::Block),
            ("myelin://acme/knowledge/page/1#hx", SubKind::Heading),
            ("myelin://acme/knowledge/row/d:r#row-r", SubKind::Row),
            ("myelin://acme/knowledge/row/d:r#field-f", SubKind::Field),
            (
                "myelin://acme/git/blob/repo:main:x.rs#L1-L9",
                SubKind::LineRange,
            ),
        ] {
            let g = SubGrain::classify(&ref_(r));
            assert_eq!(g.sub_kind(), Some(kind), "{r} reports its frozen SubKind");
            assert!(
                g.is_m3_exercised(),
                "{r} is an M3-exercised grain (Git + KN)"
            );
        }

        let chat = SubGrain::classify(&ref_("myelin://acme/chat/channel/c#message-m1"));
        assert_eq!(chat, SubGrain::M4Producer(SubKind::Message));
        assert!(
            !chat.is_m3_exercised(),
            "a Chat message sub-anchor is the M4 floor (named)"
        );
        let ci = SubGrain::classify(&ref_("myelin://acme/ci/run/r#step-3"));
        assert_eq!(ci, SubGrain::M4Producer(SubKind::Step));
        assert!(
            !ci.is_m3_exercised(),
            "a CI step sub-anchor is the M4 floor (named)"
        );
    }

    #[test]
    fn block_subdoc_projects_one_block_at_block_grain() {
        let referenced = EvArtifactRef("myelin://acme/issues/issue/ENG-7".into());
        let block = Block::Paragraph {
            inline: parse_inline(
                &format!("the deadlock fix references {}", myelin_content::OBJ),
                &[InlineNode::ArtifactRefNode(referenced.clone())],
            ),
        };
        let p = block_subdoc_projection(&block, Some("en"));
        assert!(
            p.text.contains("deadlock fix"),
            "the block's prose is the searchable body"
        );
        assert_eq!(p.lang.as_deref(), Some("en"));
        assert_eq!(
            p.fields.get(crate::kn_projection::FACET_ARTIFACT_REF),
            Some(&FieldValue::Relation(referenced.0.clone()))
        );
    }

    #[test]
    fn heading_subdoc_projects_at_block_grain() {
        let block = Block::Heading {
            level: HeadingLevel::new(2).unwrap(),
            inline: parse_inline("Scheduler internals", &[]),
        };
        let p = block_subdoc_projection(&block, Some("en"));
        assert!(p.text.contains("Scheduler internals"));
    }

    #[test]
    fn db_row_subdoc_projects_the_row_grain() {
        let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
        fields.insert("priority".into(), FieldValue::Select("P0".into()));
        fields.insert(
            "owner".into(),
            FieldValue::Principal("u-1-pseudonym".into()),
        );
        let ok = OrderKey::parse("hzzzzz").expect("a base-62 LexoRank key");
        let p = db_row_subdoc_projection(&fields, "the row about a P0 incident", Some(ok.clone()));
        assert_eq!(
            p.fields.get("priority"),
            Some(&FieldValue::Select("P0".into()))
        );
        assert!(p.fields.contains_key("owner"));
        assert_eq!(
            p.fields.get(crate::engine::ORDER_KEY_FIELD),
            Some(&FieldValue::OrderKey(ok)),
            "the columnar sort key is carried (13.3)"
        );
        assert!(
            p.text.contains("P0 incident"),
            "the row's full-text is searchable"
        );
    }

    #[test]
    fn db_field_subdoc_projects_the_field_grain() {
        let p =
            db_field_subdoc_projection("priority", FieldValue::Select("P0".into()), "priority: P0");
        assert_eq!(p.fields.len(), 1, "exactly the one resolved field");
        assert_eq!(
            p.fields.get("priority"),
            Some(&FieldValue::Select("P0".into()))
        );
        assert!(p.text.contains("P0"));
    }

    fn exact_span() -> ContentAnchoredSpan {
        ContentAnchoredSpan {
            path: "src/scheduler/deadlock.rs".into(),
            language: "rust".into(),
            blob_oid: "oid-v1".into(),
            resolved_start: 42,
            resolved_end: 45,
            span_text: "fn detectDeadlock(graph: &WaitForGraph) -> bool {\n    \
                        graph.has_cycle()\n}"
                .into(),
            anchor_state: AnchorState::Exact,
        }
    }

    #[test]
    fn line_range_subdoc_projects_the_resolved_span_content_anchored() {
        let p = line_range_subdoc_projection(&exact_span());
        let toks: std::collections::BTreeSet<&str> = p.text.split(' ').collect();
        assert!(toks.contains("detect"), "the span is code-tokenized");
        assert!(toks.contains("deadlock"));
        assert!(
            toks.contains("detectdeadlock"),
            "whole identifier kept (exact-identifier hit)"
        );
        assert!(toks.contains("->"), "the operator survives at span grain");
        assert_eq!(p.lang.as_deref(), Some("code"));
        assert_eq!(
            p.fields.get(FACET_LINE_START),
            Some(&FieldValue::Text("42".into()))
        );
        assert_eq!(
            p.fields.get(FACET_LINE_END),
            Some(&FieldValue::Text("45".into()))
        );
        assert_eq!(
            p.fields.get(FACET_ANCHOR_STATE),
            Some(&FieldValue::Text("exact".into()))
        );
        assert_eq!(
            p.fields.get(FACET_PATH),
            Some(&FieldValue::Text("src/scheduler/deadlock.rs".into()))
        );
    }

    #[test]
    fn force_push_rebase_re_derives_the_span_never_a_stale_line() {
        let before = line_range_subdoc_projection(&exact_span());
        assert_eq!(
            before.fields.get(FACET_LINE_START),
            Some(&FieldValue::Text("42".into()))
        );

        let after_span = ContentAnchoredSpan {
            blob_oid: "oid-v2-after-force-push".into(),
            resolved_start: 60,
            resolved_end: 63,
            anchor_state: AnchorState::Rebased,
            ..exact_span()
        };
        let after = line_range_subdoc_projection(&after_span);

        assert_eq!(
            after.fields.get(FACET_LINE_START),
            Some(&FieldValue::Text("60".into())),
            "the span re-derives to the shifted position (content-anchored, not positional)"
        );
        assert_eq!(
            after.fields.get(FACET_LINE_END),
            Some(&FieldValue::Text("63".into()))
        );
        assert_eq!(
            after.fields.get(FACET_ANCHOR_STATE),
            Some(&FieldValue::Text("rebased".into())),
            "the hit renders the `moved` flag from the CURRENT resolve"
        );
        assert_eq!(
            after.fields.get(FACET_BLOB_OID),
            Some(&FieldValue::Text("oid-v2-after-force-push".into()))
        );
        let toks: std::collections::BTreeSet<&str> = after.text.split(' ').collect();
        assert!(
            toks.contains("detectdeadlock"),
            "the span content is still searchable post-rebase"
        );
    }

    #[test]
    fn partial_span_re_derives_to_the_surviving_sub_range() {
        let partial = ContentAnchoredSpan {
            resolved_start: 42,
            resolved_end: 43,
            anchor_state: AnchorState::Partial,
            span_text: "fn detectDeadlock(graph: &WaitForGraph) -> bool {".into(),
            ..exact_span()
        };
        let p = line_range_subdoc_projection(&partial);
        assert_eq!(
            p.fields.get(FACET_LINE_END),
            Some(&FieldValue::Text("43".into()))
        );
        assert_eq!(
            p.fields.get(FACET_ANCHOR_STATE),
            Some(&FieldValue::Text("partial".into()))
        );
    }

    #[test]
    fn line_range_facet_union_is_blob_plus_re_derived_line_facets() {
        let f = line_range_subdoc_facets();
        for facet in [
            FACET_PATH,
            FACET_LANGUAGE,
            FACET_BLOB_OID,
            FACET_LINE_START,
            FACET_LINE_END,
            FACET_ANCHOR_STATE,
        ] {
            assert_eq!(
                f.get(facet),
                Some(&FieldType::Text),
                "`{facet}` is a typed columnar facet"
            );
        }
        assert_eq!(
            f.len(),
            6,
            "exactly the blob facets + the three re-derived line-range facets"
        );
    }
}
