use crate::inline::{parse_inline, serialize_inline, InlineNode, OBJ};

pub mod offset;
pub mod surgery;

pub use offset::{
    caret_count, dom_to_offset, offset_to_dom, segments, CaretMap, DomPosition, Segment,
    SegmentKind,
};
pub use surgery::{normalize_paste, split_at, BlockSplit};

pub fn canonicalize(md: &str, nodes: &[InlineNode]) -> (String, Vec<InlineNode>) {
    let inline = parse_inline(md, nodes);
    (serialize_inline(&inline), inline.nodes)
}

pub(crate) fn obj_count(md: &str) -> usize {
    md.chars().filter(|&c| c == OBJ).count()
}

pub(crate) fn split_nodes(
    nodes: &[InlineNode],
    obj_split: usize,
) -> (Vec<InlineNode>, Vec<InlineNode>) {
    let k = obj_split.min(nodes.len());
    (nodes[..k].to_vec(), nodes[k..].to_vec())
}
