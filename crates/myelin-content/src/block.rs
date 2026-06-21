//! The frozen v1 `Block` taxonomy (architecture
//! `04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md`
//! §2.1; contract-index 13.1, frozen X-2/OQ-B).
//!
//! Knowledge **leads and freezes** this canonical block set — it is the complete v1
//! taxonomy. Chat and Issues declare strict *subsets* (neither adds a node type, X-2).
//! `db_view` and `sync_block` are Knowledge-only in v1. The shared crate defines the
//! shapes; Knowledge stores them.
//!
//! ## `sync_block` engine FLOOR (Δ3, §2.4)
//! `sync_block` is present as a node TYPE here so the taxonomy is complete, but its v1
//! **engine** is a read-projection floor — it renders like `embed` (resolve `source`
//! via Refs, permission-filtered, tombstone-on-loss), NOT editable-in-place multi-home.
//! That engine lands in **KN-P12** (P-243-band); the editable-in-place multi-home
//! follow-on is post-M5 (KQ-6, designed against the CRDT). See §2.4.

use crate::inline::Inline;
use myelin_events::ArtifactRef;
use myelin_query::ViewSpec;
use serde::{Deserialize, Serialize};

/// The frozen v1 block taxonomy (§2.1). Byte-for-byte the architecture's set:
/// paragraph / heading / bullet_list / ordered_list / task_list / blockquote /
/// code_block / callout / table / divider / image / embed / db_view / toggle /
/// sync_block. Adding or removing a variant is a whole-workspace contract PR (X-2), not
/// a local change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    /// `paragraph { inline }`.
    Paragraph { inline: Inline },
    /// `heading { level: 1..6, inline }`.
    Heading { level: HeadingLevel, inline: Inline },
    /// `bullet_list { items: [list_item] }`.
    BulletList { items: Vec<ListItem> },
    /// `ordered_list { items: [list_item], start: u32 }`.
    OrderedList { items: Vec<ListItem>, start: u32 },
    /// `task_list { items: [task_item{ checked, inline }] }`.
    TaskList { items: Vec<TaskItem> },
    /// `blockquote { blocks: [Block] }`.
    Blockquote { blocks: Vec<Block> },
    /// `code_block { lang: Option<String>, text: String }` — `text` is **raw**, NOT
    /// markdown-parsed (§2.1).
    CodeBlock { lang: Option<String>, text: String },
    /// `callout { tone, blocks: [Block] }`.
    Callout { tone: CalloutTone, blocks: Vec<Block> },
    /// `table { columns: [col], rows: [[cell{ blocks }]] }`.
    Table { columns: Vec<Column>, rows: Vec<Vec<Cell>> },
    /// `divider`.
    Divider,
    /// `image { blob: ArtifactRef, alt: String, caption: Option<inline> }`.
    Image {
        blob: ArtifactRef,
        alt: String,
        caption: Option<Inline>,
    },
    /// `embed { ref: ArtifactRef, display }` — a structured (load-bearing) node; the
    /// `ref` produces `refs.edge.created` (5.4). `ref` is a reserved word so the field
    /// is `reference` on the wire (serde-renamed to `ref`).
    Embed {
        #[serde(rename = "ref")]
        reference: ArtifactRef,
        display: EmbedDisplay,
    },
    /// `db_view { db: ArtifactRef, view: ViewSpec }` — **Knowledge-only** in v1; a
    /// `myelin-query` view. `view` is the **frozen [`ViewSpec`]** (contract 13.3, X-3),
    /// landed by **KN-P02 (P-235)** — the KN-P01 `ViewHandle` floor is now resolved: the
    /// `db_view` carries the real structured view-model (kind/filter:`QueryAst`/group_by/
    /// sort/visible/order_field), not an opaque ref.
    DbView { db: ArtifactRef, view: ViewSpec },
    /// `toggle { summary: inline, blocks: [Block] }`.
    Toggle { summary: Inline, blocks: Vec<Block> },
    /// `sync_block { source: ArtifactRef }` — **Knowledge-only**; transclusion. The node
    /// type is frozen here; its engine is a read-projection FLOOR (§2.4) shipped in
    /// **KN-P12**, editable-in-place follow-on post-M5 (KQ-6).
    SyncBlock { source: ArtifactRef },
}

/// `heading.level`, constrained to 1..=6 (§2.1). The newtype makes the range part of the
/// frozen shape rather than a runtime check buried in the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadingLevel(u8);

impl HeadingLevel {
    /// Construct a heading level, clamped error-free into 1..=6. Returns `None` outside
    /// the frozen range (the taxonomy admits exactly six heading levels).
    pub fn new(level: u8) -> Option<Self> {
        (1..=6).contains(&level).then_some(HeadingLevel(level))
    }
    /// The 1..=6 level.
    pub fn get(self) -> u8 {
        self.0
    }
}

/// A `list_item` — a sequence of blocks (lists nest; §2.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListItem {
    pub blocks: Vec<Block>,
}

/// A `task_item { checked: bool, inline }` (§2.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskItem {
    pub checked: bool,
    pub inline: Inline,
}

/// `callout.tone` — the frozen five tones (§2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalloutTone {
    Info,
    Warn,
    Success,
    Danger,
    Note,
}

/// `embed.display` — the frozen three render modes (§2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbedDisplay {
    Inline,
    Card,
    Preview,
}

/// A table column (§2.1, `table.columns: [col]`). The header inline + any frozen
/// per-column render hints live here; v1 carries the header label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub header: Inline,
}

/// A table cell — a sequence of blocks (§2.1, `cell{ blocks }`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub blocks: Vec<Block>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inline::parse_inline;
    use myelin_query::{FieldId, ViewSpec};

    #[test]
    fn heading_level_range_is_frozen() {
        assert!(HeadingLevel::new(0).is_none());
        assert_eq!(HeadingLevel::new(1).unwrap().get(), 1);
        assert_eq!(HeadingLevel::new(6).unwrap().get(), 6);
        assert!(HeadingLevel::new(7).is_none());
    }

    #[test]
    fn code_block_text_is_raw_not_parsed() {
        // code_block.text must stay verbatim — `**bold**` inside code is literal text,
        // never a parsed mark (§2.1).
        let b = Block::CodeBlock {
            lang: Some("rust".into()),
            text: "let x = **not bold**;".into(),
        };
        let json = serde_json::to_string(&b).unwrap();
        let back: Block = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
        match back {
            Block::CodeBlock { text, .. } => assert!(text.contains("**not bold**")),
            _ => panic!("expected code_block"),
        }
    }

    #[test]
    fn all_fifteen_variants_serde_roundtrip() {
        let blocks = vec![
            Block::Paragraph { inline: parse_inline("hi", &[]) },
            Block::Heading { level: HeadingLevel::new(2).unwrap(), inline: parse_inline("**T**", &[]) },
            Block::BulletList { items: vec![ListItem { blocks: vec![Block::Divider] }] },
            Block::OrderedList { items: vec![], start: 3 },
            Block::TaskList { items: vec![TaskItem { checked: true, inline: parse_inline("done", &[]) }] },
            Block::Blockquote { blocks: vec![Block::Divider] },
            Block::CodeBlock { lang: None, text: "x".into() },
            Block::Callout { tone: CalloutTone::Warn, blocks: vec![] },
            Block::Table {
                columns: vec![Column { header: parse_inline("c", &[]) }],
                rows: vec![vec![Cell { blocks: vec![] }]],
            },
            Block::Divider,
            Block::Image { blob: ArtifactRef("myelin://t/blob/1".into()), alt: "a".into(), caption: None },
            Block::Embed { reference: ArtifactRef("myelin://t/issue/1".into()), display: EmbedDisplay::Card },
            Block::DbView { db: ArtifactRef("myelin://t/db/1".into()), view: ViewSpec::table(FieldId::new("order_key")) },
            Block::Toggle { summary: parse_inline("more", &[]), blocks: vec![] },
            Block::SyncBlock { source: ArtifactRef("myelin://t/block/9".into()) },
        ];
        assert_eq!(blocks.len(), 15, "the frozen v1 taxonomy is exactly 15 variants");
        for b in &blocks {
            let json = serde_json::to_string(b).unwrap();
            let back: Block = serde_json::from_str(&json).unwrap();
            assert_eq!(*b, back);
        }
    }
}
