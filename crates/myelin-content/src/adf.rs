use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdfNode {
    Paragraph,
    Heading,
    Blockquote,
    CodeBlock,
    Rule,
    BulletList,
    OrderedList,
    Table,
    MediaSingle,
    TaskList,
    TaskItem,
    Panel,
    Mention,
    InlineCard,
    BlockCard,
    Emoji,
    Status,
    Date,
    MediaGroup,
    Expand,
    NestedExpand,
    Extension,
    BodiedExtension,
    LayoutSection,
    LayoutColumn,
}

impl AdfNode {
    pub fn wire_id(self) -> &'static str {
        match self {
            AdfNode::Paragraph => "paragraph",
            AdfNode::Heading => "heading",
            AdfNode::Blockquote => "blockquote",
            AdfNode::CodeBlock => "codeBlock",
            AdfNode::Rule => "rule",
            AdfNode::BulletList => "bulletList",
            AdfNode::OrderedList => "orderedList",
            AdfNode::Table => "table",
            AdfNode::MediaSingle => "mediaSingle",
            AdfNode::TaskList => "taskList",
            AdfNode::TaskItem => "taskItem",
            AdfNode::Panel => "panel",
            AdfNode::Mention => "mention",
            AdfNode::InlineCard => "inlineCard",
            AdfNode::BlockCard => "blockCard",
            AdfNode::Emoji => "emoji",
            AdfNode::Status => "status",
            AdfNode::Date => "date",
            AdfNode::MediaGroup => "mediaGroup",
            AdfNode::Expand => "expand",
            AdfNode::NestedExpand => "nestedExpand",
            AdfNode::Extension => "extension",
            AdfNode::BodiedExtension => "bodiedExtension",
            AdfNode::LayoutSection => "layoutSection",
            AdfNode::LayoutColumn => "layoutColumn",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdfTarget {
    Paragraph,
    Heading,
    Blockquote,
    CodeBlock,
    Divider,
    BulletList,
    OrderedList,
    Table,
    Image,
    TaskList,
    TaskItem,
    Callout,
    Toggle,
    Mention,
    ArtifactRef,
    PlainText,
    Link,
    InlineCode,
    UnicodeGlyph,
    FlattenedBlocks,
    ImageWithAttachments,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Loss {
    None,
    Lossy {
        what: Cow<'static, str>,
    },
    Conditional {
        condition: Cow<'static, str>,
        what: Cow<'static, str>,
        degraded_to: AdfTarget,
    },
}

impl Loss {
    pub fn is_potentially_lossy(&self) -> bool {
        !matches!(self, Loss::None)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdfMapping {
    pub node: AdfNode,
    pub target: AdfTarget,
    pub loss: Loss,
}

pub const MAP: &[AdfMapping] = &[
    AdfMapping { node: AdfNode::Paragraph, target: AdfTarget::Paragraph, loss: Loss::None },
    AdfMapping { node: AdfNode::Heading, target: AdfTarget::Heading, loss: Loss::None },
    AdfMapping { node: AdfNode::Blockquote, target: AdfTarget::Blockquote, loss: Loss::None },
    AdfMapping { node: AdfNode::CodeBlock, target: AdfTarget::CodeBlock, loss: Loss::None },
    AdfMapping { node: AdfNode::Rule, target: AdfTarget::Divider, loss: Loss::None },
    AdfMapping { node: AdfNode::BulletList, target: AdfTarget::BulletList, loss: Loss::None },
    AdfMapping { node: AdfNode::OrderedList, target: AdfTarget::OrderedList, loss: Loss::None },
    AdfMapping { node: AdfNode::Table, target: AdfTarget::Table, loss: Loss::None },
    AdfMapping {
        node: AdfNode::MediaSingle,
        target: AdfTarget::Image,
        loss: Loss::None,
    },
    AdfMapping { node: AdfNode::TaskList, target: AdfTarget::TaskList, loss: Loss::None },
    AdfMapping { node: AdfNode::TaskItem, target: AdfTarget::TaskItem, loss: Loss::None },
    AdfMapping {
        node: AdfNode::Panel,
        target: AdfTarget::Callout,
        loss: Loss::None,
    },
    AdfMapping {
        node: AdfNode::Mention,
        target: AdfTarget::Mention,
        loss: Loss::Conditional {
            condition: Cow::Borrowed("the principal resolves in-tenant"),
            what: Cow::Borrowed("an unresolved external mention degrades to a plain-text @name run"),
            degraded_to: AdfTarget::PlainText,
        },
    },
    AdfMapping {
        node: AdfNode::InlineCard,
        target: AdfTarget::ArtifactRef,
        loss: Loss::Conditional {
            condition: Cow::Borrowed("the URL resolves to a Myelin artifact"),
            what: Cow::Borrowed("an external URL stays a [text](url) link, not a typed ref"),
            degraded_to: AdfTarget::Link,
        },
    },
    AdfMapping {
        node: AdfNode::BlockCard,
        target: AdfTarget::ArtifactRef,
        loss: Loss::Conditional {
            condition: Cow::Borrowed("the URL resolves to a Myelin artifact"),
            what: Cow::Borrowed("an external URL stays a [text](url) link, not a typed ref"),
            degraded_to: AdfTarget::Link,
        },
    },
    AdfMapping {
        node: AdfNode::Emoji,
        target: AdfTarget::UnicodeGlyph,
        loss: Loss::Conditional {
            condition: Cow::Borrowed("the emoji is a standard unicode glyph"),
            what: Cow::Borrowed("a custom emoji degrades to a :shortcode: text run"),
            degraded_to: AdfTarget::PlainText,
        },
    },
    AdfMapping {
        node: AdfNode::Status,
        target: AdfTarget::InlineCode,
        loss: Loss::Lossy { what: Cow::Borrowed("loses the lozenge colour/styling (kept as a code run with the label)") },
    },
    AdfMapping {
        node: AdfNode::Date,
        target: AdfTarget::PlainText,
        loss: Loss::Lossy { what: Cow::Borrowed("loses the interactive date chip (kept as an ISO-date text run)") },
    },
    AdfMapping {
        node: AdfNode::MediaGroup,
        target: AdfTarget::ImageWithAttachments,
        loss: Loss::None,
    },
    AdfMapping { node: AdfNode::Expand, target: AdfTarget::Toggle, loss: Loss::None },
    AdfMapping { node: AdfNode::NestedExpand, target: AdfTarget::Toggle, loss: Loss::None },
    AdfMapping {
        node: AdfNode::Extension,
        target: AdfTarget::Callout,
        loss: Loss::Lossy {
            what: Cow::Borrowed("macros are not executed; kept as a callout(note) with the body + an [unsupported macro: name] marker"),
        },
    },
    AdfMapping {
        node: AdfNode::BodiedExtension,
        target: AdfTarget::Callout,
        loss: Loss::Lossy {
            what: Cow::Borrowed("macros are not executed; kept as a callout(note) with the body + an [unsupported macro: name] marker"),
        },
    },
    AdfMapping {
        node: AdfNode::LayoutSection,
        target: AdfTarget::FlattenedBlocks,
        loss: Loss::Lossy { what: Cow::Borrowed("multi-column layout is flattened to sequential blocks") },
    },
    AdfMapping {
        node: AdfNode::LayoutColumn,
        target: AdfTarget::FlattenedBlocks,
        loss: Loss::Lossy { what: Cow::Borrowed("multi-column layout is flattened to sequential blocks") },
    },
];

pub fn mapping_for(node: AdfNode) -> &'static AdfMapping {
    MAP.iter()
        .find(|m| m.node == node)
        .expect("every AdfNode has exactly one frozen mapping row (the closed table)")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LossyConversion {
    pub node: AdfNode,
    pub degraded_to: AdfTarget,
    pub what: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub conversions: Vec<LossyConversion>,
}

impl ImportReport {
    pub fn new() -> ImportReport {
        ImportReport::default()
    }

    pub fn record(&mut self, node: AdfNode, degraded_to: AdfTarget, what: impl Into<String>) {
        self.conversions.push(LossyConversion {
            node,
            degraded_to,
            what: what.into(),
        });
    }

    pub fn is_lossless(&self) -> bool {
        self.conversions.is_empty()
    }

    pub fn loss_count(&self) -> usize {
        self.conversions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_covers_every_node_exactly_once() {
        let all = [
            AdfNode::Paragraph,
            AdfNode::Heading,
            AdfNode::Blockquote,
            AdfNode::CodeBlock,
            AdfNode::Rule,
            AdfNode::BulletList,
            AdfNode::OrderedList,
            AdfNode::Table,
            AdfNode::MediaSingle,
            AdfNode::TaskList,
            AdfNode::TaskItem,
            AdfNode::Panel,
            AdfNode::Mention,
            AdfNode::InlineCard,
            AdfNode::BlockCard,
            AdfNode::Emoji,
            AdfNode::Status,
            AdfNode::Date,
            AdfNode::MediaGroup,
            AdfNode::Expand,
            AdfNode::NestedExpand,
            AdfNode::Extension,
            AdfNode::BodiedExtension,
            AdfNode::LayoutSection,
            AdfNode::LayoutColumn,
        ];
        assert_eq!(MAP.len(), all.len(), "the map has exactly one row per node");
        for node in all {
            let count = MAP.iter().filter(|m| m.node == node).count();
            assert_eq!(count, 1, "{} appears exactly once", node.wire_id());
        }
    }

    #[test]
    fn lossless_rows_match_the_frozen_set() {
        let lossless: Vec<&str> = MAP
            .iter()
            .filter(|m| matches!(m.loss, Loss::None))
            .map(|m| m.node.wire_id())
            .collect();
        assert_eq!(
            lossless,
            [
                "paragraph",
                "heading",
                "blockquote",
                "codeBlock",
                "rule",
                "bulletList",
                "orderedList",
                "table",
                "mediaSingle",
                "taskList",
                "taskItem",
                "panel",
                "mediaGroup",
                "expand",
                "nestedExpand",
            ],
            "the frozen lossless set (X-2 'none' column)"
        );
    }

    #[test]
    fn conditional_rows_name_condition_and_degraded_target() {
        for node in [
            AdfNode::Mention,
            AdfNode::InlineCard,
            AdfNode::BlockCard,
            AdfNode::Emoji,
        ] {
            let m = mapping_for(node);
            match &m.loss {
                Loss::Conditional {
                    condition,
                    what,
                    degraded_to,
                } => {
                    assert!(
                        !condition.is_empty(),
                        "{} names its lossless condition",
                        node.wire_id()
                    );
                    assert!(!what.is_empty(), "{} names what is lost", node.wire_id());
                    let _ = degraded_to;
                }
                other => panic!("{} should be conditional, got {other:?}", node.wire_id()),
            }
        }
    }

    #[test]
    fn unconditional_lossy_rows_name_the_loss() {
        for node in [
            AdfNode::Status,
            AdfNode::Date,
            AdfNode::Extension,
            AdfNode::BodiedExtension,
            AdfNode::LayoutSection,
            AdfNode::LayoutColumn,
        ] {
            let m = mapping_for(node);
            match &m.loss {
                Loss::Lossy { what } => {
                    assert!(!what.is_empty(), "{} names the loss", node.wire_id())
                }
                other => panic!(
                    "{} should be unconditionally lossy, got {other:?}",
                    node.wire_id()
                ),
            }
        }
    }

    #[test]
    fn import_report_records_lossy_conversions() {
        let mut report = ImportReport::new();
        assert!(report.is_lossless(), "a fresh report is lossless");

        let mention = mapping_for(AdfNode::Mention);
        if let Loss::Conditional {
            what, degraded_to, ..
        } = &mention.loss
        {
            report.record(AdfNode::Mention, *degraded_to, what.to_string());
        }
        let status = mapping_for(AdfNode::Status);
        if let Loss::Lossy { what } = &status.loss {
            report.record(AdfNode::Status, status.target, what.to_string());
        }

        assert_eq!(report.loss_count(), 2);
        assert!(!report.is_lossless());
        assert_eq!(report.conversions[0].node, AdfNode::Mention);
        assert_eq!(report.conversions[0].degraded_to, AdfTarget::PlainText);
        assert_eq!(report.conversions[1].node, AdfNode::Status);
    }

    #[test]
    fn mapping_round_trips_stably() {
        for m in MAP {
            let json = serde_json::to_string(m).unwrap();
            let back: AdfMapping = serde_json::from_str(&json).unwrap();
            assert_eq!(*m, back);
        }
    }
}
