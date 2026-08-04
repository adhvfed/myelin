use crate::events;
use myelin_content::{
    parse_inline, serialize_inline, wasm, Block, Cell, Column, Inline, InlineNode, ListItem,
};
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventId, EventType, OutboxTx, PiiKeyRef,
    Result as BusResult, Visibility,
};

pub const ISSUES_EXCLUDED_BLOCKS: [&str; 3] = ["db_view", "sync_block", "toggle"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubsetError {
    pub excluded: &'static str,
}

impl std::fmt::Display for SubsetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "block `{}` is Knowledge-only and not in the Issues content subset (X-2) - rejected, not dropped",
            self.excluded
        )
    }
}

impl std::error::Error for SubsetError {}

pub fn is_issue_block(block: &Block) -> bool {
    !matches!(
        block,
        Block::DbView { .. } | Block::SyncBlock { .. } | Block::Toggle { .. }
    )
}

fn excluded_name(block: &Block) -> Option<&'static str> {
    match block {
        Block::DbView { .. } => Some("db_view"),
        Block::SyncBlock { .. } => Some("sync_block"),
        Block::Toggle { .. } => Some("toggle"),
        _ => None,
    }
}

pub fn validate_subtree(blocks: &[Block]) -> Result<(), SubsetError> {
    for block in blocks {
        if let Some(excluded) = excluded_name(block) {
            return Err(SubsetError { excluded });
        }
        match block {
            Block::Blockquote { blocks } | Block::Callout { blocks, .. } => {
                validate_subtree(blocks)?;
            }
            Block::BulletList { items } | Block::OrderedList { items, .. } => {
                for ListItem { blocks } in items {
                    validate_subtree(blocks)?;
                }
            }
            Block::Table { rows, columns } => {
                let _ = columns;
                for row in rows {
                    for Cell { blocks } in row {
                        validate_subtree(blocks)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn inline_runs(blocks: &[Block]) -> Vec<&Inline> {
    let mut out = Vec::new();
    collect_inlines(blocks, &mut out);
    out
}

fn collect_inlines<'a>(blocks: &'a [Block], out: &mut Vec<&'a Inline>) {
    for block in blocks {
        match block {
            Block::Paragraph { inline } | Block::Heading { inline, .. } => out.push(inline),
            Block::TaskList { items } => {
                for item in items {
                    out.push(&item.inline);
                }
            }
            Block::Blockquote { blocks } | Block::Callout { blocks, .. } => {
                collect_inlines(blocks, out)
            }
            Block::BulletList { items } | Block::OrderedList { items, .. } => {
                for ListItem { blocks } in items {
                    collect_inlines(blocks, out);
                }
            }
            Block::Table { columns, rows } => {
                for Column { header } in columns {
                    out.push(header);
                }
                for row in rows {
                    for Cell { blocks } in row {
                        collect_inlines(blocks, out);
                    }
                }
            }
            Block::Image {
                caption: Some(caption),
                ..
            } => out.push(caption),
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentKind {
    Body,
    Comment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueContent {
    pub kind: ContentKind,
    pub blocks: Vec<Block>,
    pub version: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CasConflict {
    pub expected: u64,
    pub actual: u64,
}

impl std::fmt::Display for CasConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "single-author CAS conflict: edit expected version {} but the content is at {}",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for CasConflict {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentError {
    Subset(SubsetError),
    Cas(CasConflict),
}

impl std::fmt::Display for ContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentError::Subset(e) => write!(f, "{e}"),
            ContentError::Cas(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ContentError {}

impl From<SubsetError> for ContentError {
    fn from(e: SubsetError) -> Self {
        ContentError::Subset(e)
    }
}

impl IssueContent {
    pub fn new(kind: ContentKind, blocks: Vec<Block>) -> Result<IssueContent, SubsetError> {
        validate_subtree(&blocks)?;
        Ok(IssueContent {
            kind,
            blocks,
            version: 0,
        })
    }

    pub fn empty(kind: ContentKind) -> IssueContent {
        IssueContent {
            kind,
            blocks: Vec::new(),
            version: 0,
        }
    }

    pub fn round_trips(&self) -> bool {
        inline_runs(&self.blocks).into_iter().all(roundtrips_inline)
    }

    pub fn cas_edit(
        &mut self,
        expected_version: u64,
        blocks: Vec<Block>,
    ) -> Result<u64, ContentError> {
        validate_subtree(&blocks)?;
        if expected_version != self.version {
            return Err(ContentError::Cas(CasConflict {
                expected: expected_version,
                actual: self.version,
            }));
        }
        self.blocks = blocks;
        self.version += 1;
        Ok(self.version)
    }

    pub fn structured_nodes(&self) -> Vec<&InlineNode> {
        inline_runs(&self.blocks)
            .into_iter()
            .flat_map(|inline| inline.structured_nodes().iter())
            .collect()
    }

    pub fn edit_event_token(&self) -> &'static str {
        match self.kind {
            ContentKind::Body => events::ISSUE_UPDATED,
            ContentKind::Comment if self.version == 0 => events::COMMENT_CREATED,
            ContentKind::Comment => events::COMMENT_UPDATED,
        }
    }
}

fn roundtrips_inline(inline: &Inline) -> bool {
    let md = serialize_inline(inline);
    let reparsed = wasm::render_parse(&md, &inline.nodes);
    wasm::render_serialize(&reparsed) == md
}

pub fn roundtrips_md(md: &str, nodes: &[InlineNode]) -> bool {
    let parsed = wasm::render_parse(md, nodes);
    wasm::render_serialize(&parsed) == md
}

pub fn paragraph_body(md: &str, nodes: &[InlineNode]) -> IssueContent {
    let inline = parse_inline(md, nodes);
    IssueContent {
        kind: ContentKind::Body,
        blocks: vec![Block::Paragraph { inline }],
        version: 0,
    }
}

fn content_event_draft(
    token: &str,
    issue_ref: &ArtifactRef,
    content_ref: &ArtifactRef,
    aggregate: &AggregateKey,
    new_version: u64,
    pii_key_ref: Option<PiiKeyRef>,
) -> EventDraft {
    let contains_pii = pii_key_ref.is_some();
    EventDraft {
        type_: EventType(token.into()),
        subject: content_ref.clone(),
        aggregate: aggregate.clone(),
        payload: serde_json::json!({
            "issue": issue_ref.0,
            "content": content_ref.0,
            "version": new_version,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: contains_pii,
        pii_key_ref,
    }
}

pub fn emit_content_event(
    tx: &mut dyn OutboxTx,
    issue_ref: &ArtifactRef,
    content_ref: &ArtifactRef,
    aggregate: &AggregateKey,
    content: &IssueContent,
    pii_key_ref: Option<PiiKeyRef>,
    cause: Option<&myelin_events::EventEnvelope>,
) -> BusResult<EventId> {
    let draft = content_event_draft(
        content.edit_event_token(),
        issue_ref,
        content_ref,
        aggregate,
        content.version,
        pii_key_ref,
    );
    tx.emit(draft, cause)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::{CalloutTone, HeadingLevel, TaskItem};
    use myelin_events::{
        Actor, ArtifactRef, CausedBy, EmitContextBase, MonotonicMinter, OutboxStore, Region,
        TenantId, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::sync::Arc;

    fn para(md: &str, nodes: Vec<InlineNode>) -> Block {
        Block::Paragraph {
            inline: parse_inline(md, &nodes),
        }
    }

    fn alice() -> InlineNode {
        InlineNode::Mention(Principal::stub(
            PrincipalId("p-opaque-alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        ))
    }

    #[test]
    fn admitted_issues_subset_is_valid() {
        let blocks = vec![
            para("a **rich** body", vec![]),
            Block::Heading {
                level: HeadingLevel::new(2).unwrap(),
                inline: parse_inline("**Heading**", &[]),
            },
            Block::BulletList {
                items: vec![ListItem {
                    blocks: vec![para("item", vec![])],
                }],
            },
            Block::TaskList {
                items: vec![TaskItem {
                    checked: true,
                    inline: parse_inline("done", &[]),
                }],
            },
            Block::Blockquote {
                blocks: vec![para("quoted", vec![])],
            },
            Block::CodeBlock {
                lang: Some("rust".into()),
                text: "let x = **not bold**;".into(),
            },
            Block::Callout {
                tone: CalloutTone::Warn,
                blocks: vec![para("note", vec![])],
            },
            Block::Table {
                columns: vec![Column {
                    header: parse_inline("col", &[]),
                }],
                rows: vec![vec![Cell {
                    blocks: vec![para("cell", vec![])],
                }]],
            },
            Block::Divider,
            Block::Image {
                blob: ArtifactRef("myelin://acme/blob/1".into()),
                alt: "a".into(),
                caption: Some(parse_inline("*cap*", &[])),
            },
        ];
        assert!(validate_subtree(&blocks).is_ok(), "the subset is admitted");
        let content = IssueContent::new(ContentKind::Body, blocks).expect("in-subset");
        assert!(
            content.round_trips(),
            "render(parse(md)) === md over the subtree"
        );
    }

    #[test]
    fn knowledge_only_blocks_are_rejected() {
        use myelin_query::{FieldId, ViewSpec};
        let excluded = [
            Block::DbView {
                db: ArtifactRef("myelin://acme/db/1".into()),
                view: ViewSpec::table(FieldId::new("order_key")),
            },
            Block::SyncBlock {
                source: ArtifactRef("myelin://acme/block/9".into()),
            },
            Block::Toggle {
                summary: parse_inline("more", &[]),
                blocks: vec![],
            },
        ];
        for (block, name) in excluded.into_iter().zip(ISSUES_EXCLUDED_BLOCKS) {
            assert!(!is_issue_block(&block), "{name} is out of subset");
            let err = validate_subtree(std::slice::from_ref(&block)).unwrap_err();
            assert_eq!(err.excluded, name, "the error names the excluded variant");
            assert!(IssueContent::new(ContentKind::Body, vec![block]).is_err());
        }
    }

    #[test]
    fn nested_knowledge_only_block_is_rejected() {
        let smuggled = Block::Blockquote {
            blocks: vec![
                para("ok", vec![]),
                Block::SyncBlock {
                    source: ArtifactRef("myelin://acme/block/9".into()),
                },
            ],
        };
        let err = validate_subtree(&[smuggled]).unwrap_err();
        assert_eq!(err.excluded, "sync_block");
    }

    #[test]
    fn nested_in_list_and_table_is_rejected() {
        let in_list = Block::BulletList {
            items: vec![ListItem {
                blocks: vec![Block::Toggle {
                    summary: parse_inline("x", &[]),
                    blocks: vec![],
                }],
            }],
        };
        assert_eq!(validate_subtree(&[in_list]).unwrap_err().excluded, "toggle");

        let in_table = Block::Table {
            columns: vec![Column {
                header: parse_inline("c", &[]),
            }],
            rows: vec![vec![Cell {
                blocks: vec![Block::DbView {
                    db: ArtifactRef("myelin://acme/db/1".into()),
                    view: myelin_query::ViewSpec::table(myelin_query::FieldId::new("k")),
                }],
            }]],
        };
        assert_eq!(
            validate_subtree(&[in_table]).unwrap_err().excluded,
            "db_view"
        );
    }

    #[test]
    fn body_round_trips_byte_identical_via_wasm_path() {
        use myelin_content::OBJ;
        let md = format!("**bold** and *italic* with `code` and a {OBJ} mention");
        let nodes = vec![alice()];
        assert!(
            roundtrips_md(&md, &nodes),
            "render(parse(md)) === md via the WASM path"
        );
        let content = paragraph_body(&md, &nodes);
        assert!(content.round_trips());
    }

    #[test]
    fn empty_content_round_trips() {
        assert!(IssueContent::empty(ContentKind::Body).round_trips());
        assert!(IssueContent::empty(ContentKind::Comment).round_trips());
    }

    #[test]
    fn round_trips_md_is_false_on_non_canonical_body() {
        assert!(
            !roundtrips_md("a*b", &[]),
            "a non-canonical source body must NOT round-trip byte-exact"
        );
        assert!(roundtrips_md(r"a\*b", &[]));
    }

    #[test]
    fn issue_content_round_trips_is_ast_idempotent() {
        let from_non_canonical = paragraph_body("a*b", &[]);
        assert!(
            from_non_canonical.round_trips(),
            "the stored (canonical) AST is always a fixed point"
        );
        if let Block::Paragraph { inline } = &from_non_canonical.blocks[0] {
            assert_eq!(serialize_inline(inline), r"a\*b");
        } else {
            panic!("expected a paragraph body");
        }
    }

    #[test]
    fn wasm_path_is_identical_to_native_parse() {
        let md = "**a** `b` ~~c~~ [t](u)";
        let native = serialize_inline(&parse_inline(md, &[]));
        let via_wasm = wasm::render_serialize(&wasm::render_parse(md, &[]));
        assert_eq!(native, via_wasm, "one renderer, native === wasm path");
        assert_eq!(native, md, "and it round-trips");
    }

    #[test]
    fn cas_edit_rejects_stale_version() {
        let mut content = paragraph_body("v0", &[]);
        assert_eq!(content.version, 0);
        assert_eq!(content.cas_edit(0, vec![para("v1", vec![])]).unwrap(), 1);
        let err = content
            .cas_edit(0, vec![para("v2-stale", vec![])])
            .unwrap_err();
        assert_eq!(
            err,
            ContentError::Cas(CasConflict {
                expected: 0,
                actual: 1
            })
        );
        assert_eq!(content.version, 1, "a rejected CAS edit does not bump");
        assert_eq!(
            content.blocks,
            vec![para("v1", vec![])],
            "a rejected CAS edit does not mutate the content"
        );
    }

    #[test]
    fn out_of_subset_edit_is_rejected_before_cas() {
        let mut content = paragraph_body("v0", &[]);
        let err = content
            .cas_edit(
                0,
                vec![Block::SyncBlock {
                    source: ArtifactRef("myelin://acme/block/9".into()),
                }],
            )
            .unwrap_err();
        assert!(matches!(err, ContentError::Subset(_)));
        assert_eq!(
            content.version, 0,
            "a rejected edit never bumps the version"
        );
    }

    #[test]
    fn cas_conflict_display_names_the_versions() {
        let msg = CasConflict {
            expected: 0,
            actual: 3,
        }
        .to_string();
        assert!(msg.contains('0') && msg.contains('3'), "names both: {msg}");
        assert!(msg.to_lowercase().contains("cas"));
    }

    #[test]
    fn structured_nodes_walks_the_whole_subtree() {
        use myelin_content::OBJ;
        let page = ArtifactRef("myelin://acme/knowledge/page/7".into());
        let content = IssueContent::new(
            ContentKind::Body,
            vec![
                para(&format!("hi {OBJ}"), vec![alice()]),
                Block::BulletList {
                    items: vec![ListItem {
                        blocks: vec![para(
                            &format!("see {OBJ}"),
                            vec![InlineNode::Embed(page.clone())],
                        )],
                    }],
                },
            ],
        )
        .unwrap();
        let nodes = content.structured_nodes();
        assert_eq!(nodes.len(), 2, "both structured nodes are reached");
        assert!(matches!(nodes[0], InlineNode::Mention(_)));
        assert!(matches!(nodes[1], InlineNode::Embed(_)));
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-23T10:00:00Z".into()),
            recorded_at: Timestamp("2026-06-23T10:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    #[test]
    fn body_edit_co_commits_issue_updated() {
        let store = OutboxStore::new();
        let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(MonotonicMinter::new());
        let issue = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let content_ref = ArtifactRef("myelin://acme/issue/issue/ENG-1#b-desc".into());
        let aggregate = AggregateKey("issue:7:ENG-1".into());

        let mut content = paragraph_body("initial", &[]);
        content
            .cas_edit(0, vec![para("edited body", vec![])])
            .unwrap();
        assert_eq!(content.version, 1);

        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("issue ENG-1 body edited");
        let eid = emit_content_event(
            &mut tx,
            &issue,
            &content_ref,
            &aggregate,
            &content,
            None,
            None,
        )
        .unwrap();
        tx.commit().unwrap();

        assert_eq!(store.outbox_depth(), 1, "one content event co-committed");
        let row = store.row(&eid).expect("the committed row is present");
        assert_eq!(row.envelope.type_.0, events::ISSUE_UPDATED);
        assert_eq!(row.seq, 0);
        assert_eq!(row.aggregate, aggregate);
        assert_eq!(row.envelope.payload["issue"], issue.0);
        assert_eq!(row.envelope.payload["version"], 1);
        assert!(!row.envelope.contains_personal_data || row.envelope.pii_key_ref.is_some());
    }

    #[test]
    fn comment_create_then_update_tokens() {
        let mut comment =
            IssueContent::new(ContentKind::Comment, vec![para("first", vec![])]).unwrap();
        assert_eq!(comment.version, 0);
        assert_eq!(comment.edit_event_token(), events::COMMENT_CREATED);
        comment.cas_edit(0, vec![para("edited", vec![])]).unwrap();
        assert_eq!(comment.version, 1);
        assert_eq!(comment.edit_event_token(), events::COMMENT_UPDATED);
    }

    #[test]
    fn pii_body_event_carries_key_ref_not_body() {
        let store = OutboxStore::new();
        let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(MonotonicMinter::new());
        let issue = ArtifactRef("myelin://acme/issue/issue/ENG-2".into());
        let content_ref = ArtifactRef("myelin://acme/issue/issue/ENG-2#b-desc".into());
        let aggregate = AggregateKey("issue:7:ENG-2".into());
        let content = paragraph_body("contains alice's email", &[]);

        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("issue ENG-2 body edited (PII)");
        let key = PiiKeyRef("kms://acme/3/subject:psn-x".into());
        let eid = emit_content_event(
            &mut tx,
            &issue,
            &content_ref,
            &aggregate,
            &content,
            Some(key.clone()),
            None,
        )
        .unwrap();
        tx.commit().unwrap();

        let row = store.row(&eid).unwrap();
        assert!(
            row.envelope.contains_personal_data,
            "PII body flags the event"
        );
        assert_eq!(row.envelope.pii_key_ref, Some(key));
        let payload = serde_json::to_string(&row.envelope.payload).unwrap();
        assert!(
            !payload.contains("alice"),
            "references-not-payloads: no inline body on the wire"
        );
    }
}
