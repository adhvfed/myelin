#![forbid(unsafe_code)]

pub mod block;
pub mod corpus;
pub mod editor;
pub mod events;
pub mod inline;
pub mod rebac_fragment;

pub use block::{Block, CalloutTone, Cell, Column, EmbedDisplay, HeadingLevel, ListItem, TaskItem};
pub use editor::{
    canonicalize, caret_count, dom_to_offset, offset_to_dom, segments, split_at, BlockSplit,
    CaretMap, DomPosition, Segment, SegmentKind,
};
pub use inline::{parse_inline, serialize_inline, Inline, InlineNode, Mark, Span, OBJ};

pub mod wasm {
    use crate::inline::{parse_inline, serialize_inline, Inline, InlineNode};

    pub fn render_parse(md: &str, nodes: &[InlineNode]) -> Inline {
        parse_inline(md, nodes)
    }

    pub fn render_serialize(inline: &Inline) -> String {
        serialize_inline(inline)
    }
}
