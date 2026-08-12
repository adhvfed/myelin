use crate::inline::Inline;
use myelin_events::ArtifactRef;
use myelin_query::ViewSpec;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Paragraph {
        inline: Inline,
    },
    Heading {
        level: HeadingLevel,
        inline: Inline,
    },
    BulletList {
        items: Vec<ListItem>,
    },
    OrderedList {
        items: Vec<ListItem>,
        start: u32,
    },
    TaskList {
        items: Vec<TaskItem>,
    },
    Blockquote {
        blocks: Vec<Block>,
    },
    CodeBlock {
        lang: Option<String>,
        text: String,
    },
    Callout {
        tone: CalloutTone,
        blocks: Vec<Block>,
    },
    Table {
        columns: Vec<Column>,
        rows: Vec<Vec<Cell>>,
    },
    Divider,
    Image {
        blob: ArtifactRef,
        alt: String,
        caption: Option<Inline>,
    },
    Embed {
        #[serde(rename = "ref")]
        reference: ArtifactRef,
        display: EmbedDisplay,
    },
    DbView {
        db: ArtifactRef,
        view: ViewSpec,
    },
    Toggle {
        summary: Inline,
        blocks: Vec<Block>,
    },
    SyncBlock {
        source: ArtifactRef,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadingLevel(u8);

impl HeadingLevel {
    pub fn new(level: u8) -> Option<Self> {
        (1..=6).contains(&level).then_some(HeadingLevel(level))
    }
    pub fn get(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListItem {
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskItem {
    pub checked: bool,
    pub inline: Inline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalloutTone {
    Info,
    Warn,
    Success,
    Danger,
    Note,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbedDisplay {
    Inline,
    Card,
    Preview,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub header: Inline,
}

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
            Block::Paragraph {
                inline: parse_inline("hi", &[]),
            },
            Block::Heading {
                level: HeadingLevel::new(2).unwrap(),
                inline: parse_inline("**T**", &[]),
            },
            Block::BulletList {
                items: vec![ListItem {
                    blocks: vec![Block::Divider],
                }],
            },
            Block::OrderedList {
                items: vec![],
                start: 3,
            },
            Block::TaskList {
                items: vec![TaskItem {
                    checked: true,
                    inline: parse_inline("done", &[]),
                }],
            },
            Block::Blockquote {
                blocks: vec![Block::Divider],
            },
            Block::CodeBlock {
                lang: None,
                text: "x".into(),
            },
            Block::Callout {
                tone: CalloutTone::Warn,
                blocks: vec![],
            },
            Block::Table {
                columns: vec![Column {
                    header: parse_inline("c", &[]),
                }],
                rows: vec![vec![Cell { blocks: vec![] }]],
            },
            Block::Divider,
            Block::Image {
                blob: ArtifactRef("myelin://t/blob/1".into()),
                alt: "a".into(),
                caption: None,
            },
            Block::Embed {
                reference: ArtifactRef("myelin://t/issue/1".into()),
                display: EmbedDisplay::Card,
            },
            Block::DbView {
                db: ArtifactRef("myelin://t/db/1".into()),
                view: ViewSpec::table(FieldId::new("order_key")),
            },
            Block::Toggle {
                summary: parse_inline("more", &[]),
                blocks: vec![],
            },
            Block::SyncBlock {
                source: ArtifactRef("myelin://t/block/9".into()),
            },
        ];
        assert_eq!(
            blocks.len(),
            15,
            "the frozen v1 taxonomy is exactly 15 variants"
        );
        for b in &blocks {
            let json = serde_json::to_string(b).unwrap();
            let back: Block = serde_json::from_str(&json).unwrap();
            assert_eq!(*b, back);
        }
    }
}
