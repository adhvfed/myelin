//! The frozen **ADF → `myelin-content` lossy-node map** (contract 13.2, X-2/CR-9).
//!
//! **Owning architecture/contract:** `contract-index.md` row 13.2 ("ADF → `myelin-content`
//! lossy-map (frozen) — the Issues import conversion table; lossy nodes named + recorded in the
//! import report") and the reconciliation `00-reconciliation-decisions.md` X-2 ("ADF →
//! `myelin-content` lossy-node map (frozen — Issues import, CR-9)").
//!
//! ## What this is (the freeze; Issues consumes it at import)
//! Atlassian Document Format (ADF — Jira/Confluence) is the dominant import source. This module
//! freezes **the conversion table**: for every ADF node, which `myelin-content` [`crate::Block`] /
//! [`crate::InlineNode`] it maps to, and **whether that conversion loses information** — and if so,
//! exactly what is lost. **Knowledge ships the table**; **Issues consumes it at import time** (the
//! actual byte-level ADF JSON parser is the Issues import prompt's job — this is the frozen *map*
//! the import builds against, the X-2 anti-drift anchor so Issues' import assumption is bounded to
//! exactly this map, no looser).
//!
//! ## Named, never silent (EI-04 §4)
//! Every lossy conversion is recorded — the [`AdfMapping::loss`] field names the loss, and an
//! [`ImportReport`] accumulates each [`LossyConversion`] so the floor is **named in the import
//! report**, not silently dropped. A `[unsupported macro: name]` marker is emitted in-content for
//! the genuinely-unmappable nodes (macros), so the loss is visible to the reader, not hidden.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// **The frozen ADF node kinds the import map covers** (X-2 table, in table order). A closed set:
/// adding a kind is a whole-workspace contract PR (the map is the frozen import contract). The
/// `wire_id` is the ADF `type` token (the JSON discriminant the Issues parser keys on).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdfNode {
    /// `paragraph` — direct equivalent.
    Paragraph,
    /// `heading` — direct equivalent.
    Heading,
    /// `blockquote` — direct equivalent.
    Blockquote,
    /// `codeBlock` — direct equivalent (`text` raw, not markdown-parsed).
    CodeBlock,
    /// `rule` (horizontal rule) — direct equivalent to `divider`.
    Rule,
    /// `bulletList` — direct equivalent.
    BulletList,
    /// `orderedList` — direct equivalent.
    OrderedList,
    /// `table` — direct equivalent.
    Table,
    /// `mediaSingle` (a single image) — direct equivalent to `image`.
    MediaSingle,
    /// `taskList` — direct equivalent.
    TaskList,
    /// `taskItem` — direct equivalent.
    TaskItem,
    /// `panel` — maps to `callout` (tone mapped, lossless).
    Panel,
    /// `mention` — `mention(Principal)` IF the principal resolves in-tenant; else a plain-text
    /// `@name` run (**lossy** when unresolved).
    Mention,
    /// `inlineCard` (a URL card) — `artifact_ref` IF the URL resolves to a Myelin artifact; else a
    /// `[text](url)` link (**lossy** when external).
    InlineCard,
    /// `blockCard` (a URL card) — same mapping/loss as `inlineCard`.
    BlockCard,
    /// `emoji` — the unicode glyph; a custom (named) emoji degrades to `:shortcode:` text
    /// (**lossy** only for custom emoji).
    Emoji,
    /// `status` (a Jira lozenge) — an inline `code` run with the label (**lossy**: loses
    /// colour/lozenge styling).
    Status,
    /// `date` — plain text (the ISO date) (**lossy**: loses the interactive date chip).
    Date,
    /// `mediaGroup` (attachments) — image blocks + an attachments list (lossless).
    MediaGroup,
    /// `expand` — maps to `toggle` (lossless).
    Expand,
    /// `nestedExpand` — maps to `toggle` (lossless).
    NestedExpand,
    /// `extension` (a macro) — a `callout(note)` carrying the macro text + an
    /// `[unsupported macro: name]` marker (**lossy by design**: macros are not executed).
    Extension,
    /// `bodiedExtension` (a macro) — same mapping/loss as `extension`.
    BodiedExtension,
    /// `layoutSection` — flattened to sequential blocks (**lossy**: loses multi-column layout).
    LayoutSection,
    /// `layoutColumn` — flattened to sequential blocks (**lossy**: loses multi-column layout).
    LayoutColumn,
}

impl AdfNode {
    /// The ADF `type` JSON token the Issues parser keys on (the wire discriminant).
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

/// **The `myelin-content` target a mapped ADF node lands in** — the named block/inline node, or a
/// degraded text/link form. This is the *frozen target identity*, not the constructed node (the
/// Issues parser constructs the real [`crate::Block`]/[`crate::InlineNode`]; this names which one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdfTarget {
    /// `paragraph` block.
    Paragraph,
    /// `heading` block.
    Heading,
    /// `blockquote` block.
    Blockquote,
    /// `code_block` block.
    CodeBlock,
    /// `divider` block.
    Divider,
    /// `bullet_list` block.
    BulletList,
    /// `ordered_list` block.
    OrderedList,
    /// `table` block.
    Table,
    /// `image` block.
    Image,
    /// `task_list` block.
    TaskList,
    /// `task_item` (within a `task_list`).
    TaskItem,
    /// `callout` block.
    Callout,
    /// `toggle` block.
    Toggle,
    /// `mention` inline structured node.
    Mention,
    /// `artifact_ref` inline structured node.
    ArtifactRef,
    /// A degraded plain-text run (a `@name` mention or an ISO date or a custom-emoji shortcode that
    /// did not survive as a structured node).
    PlainText,
    /// A degraded `[text](url)` markdown link (an external card that did not resolve to an
    /// artifact).
    Link,
    /// An inline `code` run (a Jira status lozenge mapped to its label as code).
    InlineCode,
    /// A unicode glyph in the markdown-subset string (a standard emoji).
    UnicodeGlyph,
    /// Sequential blocks (a flattened multi-column layout — the layout is gone).
    FlattenedBlocks,
    /// Image blocks + an attachments list.
    ImageWithAttachments,
}

/// **The loss class of a mapping** (X-2: "Loss" column, named, never silent).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Loss {
    /// No information lost — a direct, lossless equivalent.
    None,
    /// Lossy *unconditionally* — every instance loses the named information.
    Lossy {
        /// The human-readable description of exactly what is lost (recorded in the import report).
        /// `Cow` so the frozen [`MAP`] const holds `&'static str` while a deserialized value owns it.
        what: Cow<'static, str>,
    },
    /// Lossy *conditionally* — lossless when `condition` holds, lossy (losing `what`) otherwise. The
    /// import parser evaluates `condition` per node and records the loss in the report only when it
    /// actually degraded (e.g. a `mention` is lossless when the principal resolves in-tenant).
    Conditional {
        /// The condition under which the conversion is lossless (e.g. "the principal resolves
        /// in-tenant", "the URL resolves to a Myelin artifact").
        condition: Cow<'static, str>,
        /// What is lost when the condition does NOT hold.
        what: Cow<'static, str>,
        /// The degraded target used when the condition does NOT hold (so the report is precise).
        degraded_to: AdfTarget,
    },
}

impl Loss {
    /// `true` iff this mapping can EVER lose information (unconditionally, or in the degraded
    /// branch). The import report's "any-loss" summary uses it.
    pub fn is_potentially_lossy(&self) -> bool {
        !matches!(self, Loss::None)
    }
}

/// **One frozen row of the ADF → `myelin-content` map** (an ADF node, its content target, and the
/// loss class). The closed table (`MAP`) is the frozen contract Issues' import builds against.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdfMapping {
    /// The ADF source node.
    pub node: AdfNode,
    /// The `myelin-content` target the node maps to (lossless target; see `loss` for the degraded
    /// branch when conditional).
    pub target: AdfTarget,
    /// The loss class — named, so a lossy conversion is recorded, never silent.
    pub loss: Loss,
}

/// **The frozen ADF → `myelin-content` conversion table** (contract 13.2, X-2, in table order).
/// THIS is the freeze: a consumer (Issues' import) reconciles to this exact map; a row change is a
/// whole-workspace contract PR. Knowledge ships it; Issues consumes it.
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
        // tone mapped info/note/success/warning/error → info/note/success/warn/danger — lossless.
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

/// Look up the frozen mapping for an ADF node (every [`AdfNode`] is in the map by construction).
pub fn mapping_for(node: AdfNode) -> &'static AdfMapping {
    MAP.iter()
        .find(|m| m.node == node)
        .expect("every AdfNode has exactly one frozen mapping row (the closed table)")
}

/// **One recorded lossy conversion** in an [`ImportReport`] — the X-2 "recorded in the import
/// report" obligation made concrete. Carries the source node, what was lost, and the degraded
/// target so the floor is *named*, surfacing to the importing user (EI-04 §4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LossyConversion {
    /// The ADF node that lost information.
    pub node: AdfNode,
    /// The `myelin-content` target it degraded to.
    pub degraded_to: AdfTarget,
    /// The human-readable description of what was lost.
    pub what: String,
}

/// **The per-import report** (X-2: "a per-import Knowledge doc"). It accumulates every
/// [`LossyConversion`] an import made so the loss is *named, not silent* — the importing user sees
/// exactly which nodes degraded and how. An import with no lossy nodes produces an empty report.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    /// Every recorded lossy conversion, in encounter order.
    pub conversions: Vec<LossyConversion>,
}

impl ImportReport {
    /// A fresh, empty report.
    pub fn new() -> ImportReport {
        ImportReport::default()
    }

    /// Record one lossy conversion (the import parser calls this for each node it actually
    /// degraded). The frozen [`mapping_for`] table tells the parser *whether* a node degraded; this
    /// records *that it did*.
    pub fn record(&mut self, node: AdfNode, degraded_to: AdfTarget, what: impl Into<String>) {
        self.conversions.push(LossyConversion { node, degraded_to, what: what.into() });
    }

    /// `true` iff the import was fully lossless (no recorded conversions).
    pub fn is_lossless(&self) -> bool {
        self.conversions.is_empty()
    }

    /// The number of recorded lossy conversions.
    pub fn loss_count(&self) -> usize {
        self.conversions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frozen map covers every `AdfNode` exactly once (a closed, complete table — no node is
    /// silently un-mapped, none duplicated).
    #[test]
    fn map_covers_every_node_exactly_once() {
        let all = [
            AdfNode::Paragraph, AdfNode::Heading, AdfNode::Blockquote, AdfNode::CodeBlock,
            AdfNode::Rule, AdfNode::BulletList, AdfNode::OrderedList, AdfNode::Table,
            AdfNode::MediaSingle, AdfNode::TaskList, AdfNode::TaskItem, AdfNode::Panel,
            AdfNode::Mention, AdfNode::InlineCard, AdfNode::BlockCard, AdfNode::Emoji,
            AdfNode::Status, AdfNode::Date, AdfNode::MediaGroup, AdfNode::Expand,
            AdfNode::NestedExpand, AdfNode::Extension, AdfNode::BodiedExtension,
            AdfNode::LayoutSection, AdfNode::LayoutColumn,
        ];
        assert_eq!(MAP.len(), all.len(), "the map has exactly one row per node");
        for node in all {
            let count = MAP.iter().filter(|m| m.node == node).count();
            assert_eq!(count, 1, "{} appears exactly once", node.wire_id());
        }
    }

    /// The lossless rows are exactly the X-2 "direct equivalent / none" set (a frozen regression
    /// anchor — adding loss to a lossless node, or vice versa, is a contract change this catches).
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
                "paragraph", "heading", "blockquote", "codeBlock", "rule", "bulletList",
                "orderedList", "table", "mediaSingle", "taskList", "taskItem", "panel",
                "mediaGroup", "expand", "nestedExpand",
            ],
            "the frozen lossless set (X-2 'none' column)"
        );
    }

    /// Each conditionally-lossy node names its condition + degraded target (the X-2 conditional
    /// rows: mention / inlineCard / blockCard / emoji).
    #[test]
    fn conditional_rows_name_condition_and_degraded_target() {
        for node in [AdfNode::Mention, AdfNode::InlineCard, AdfNode::BlockCard, AdfNode::Emoji] {
            let m = mapping_for(node);
            match &m.loss {
                Loss::Conditional { condition, what, degraded_to } => {
                    assert!(!condition.is_empty(), "{} names its lossless condition", node.wire_id());
                    assert!(!what.is_empty(), "{} names what is lost", node.wire_id());
                    let _ = degraded_to;
                }
                other => panic!("{} should be conditional, got {other:?}", node.wire_id()),
            }
        }
    }

    /// The unconditionally-lossy nodes (status/date/extension/bodiedExtension/layout*) are flagged
    /// lossy with a named loss.
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
                Loss::Lossy { what } => assert!(!what.is_empty(), "{} names the loss", node.wire_id()),
                other => panic!("{} should be unconditionally lossy, got {other:?}", node.wire_id()),
            }
        }
    }

    /// **The import report records each lossy conversion (named, not silent)** — the X-2
    /// "recorded in the import report" obligation. A lossless import produces an empty report.
    #[test]
    fn import_report_records_lossy_conversions() {
        let mut report = ImportReport::new();
        assert!(report.is_lossless(), "a fresh report is lossless");

        // Simulate an import that degraded an external mention + a Jira status lozenge.
        let mention = mapping_for(AdfNode::Mention);
        if let Loss::Conditional { what, degraded_to, .. } = &mention.loss {
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

    /// The map serializes/deserializes stably (the wire contract Issues' import builds against).
    #[test]
    fn mapping_round_trips_stably() {
        for m in MAP {
            let json = serde_json::to_string(m).unwrap();
            let back: AdfMapping = serde_json::from_str(&json).unwrap();
            assert_eq!(*m, back);
        }
    }
}
