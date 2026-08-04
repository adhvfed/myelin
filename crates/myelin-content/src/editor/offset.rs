use crate::inline::OBJ;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    Text,
    Node,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub kind: SegmentKind,
    pub start: usize,
    pub end: usize,
    pub node_index: Option<usize>,
}

impl Segment {
    pub fn len(&self) -> usize {
        self.end - self.start
    }
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DomPosition {
    pub segment: usize,
    pub offset_in_segment: usize,
}

pub fn segments(md: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut node_index = 0usize;
    let mut run_start: Option<usize> = None;
    let mut idx = 0usize;
    for c in md.chars() {
        if c == OBJ {
            if let Some(s) = run_start.take() {
                out.push(Segment {
                    kind: SegmentKind::Text,
                    start: s,
                    end: idx,
                    node_index: None,
                });
            }
            out.push(Segment {
                kind: SegmentKind::Node,
                start: idx,
                end: idx + 1,
                node_index: Some(node_index),
            });
            node_index += 1;
        } else if run_start.is_none() {
            run_start = Some(idx);
        }
        idx += 1;
    }
    if let Some(s) = run_start.take() {
        out.push(Segment {
            kind: SegmentKind::Text,
            start: s,
            end: idx,
            node_index: None,
        });
    }
    out
}

pub fn caret_count(md: &str) -> usize {
    md.chars().count() + 1
}

pub fn offset_to_dom(md: &str, offset: usize) -> DomPosition {
    let segs = segments(md);
    let clamped = offset.min(md.chars().count());
    if segs.is_empty() {
        return DomPosition {
            segment: 0,
            offset_in_segment: 0,
        };
    }
    if clamped == 0 {
        return DomPosition {
            segment: 0,
            offset_in_segment: 0,
        };
    }
    for (i, seg) in segs.iter().enumerate() {
        if clamped <= seg.end {
            return DomPosition {
                segment: i,
                offset_in_segment: clamped - seg.start,
            };
        }
    }
    unreachable!(
        "offset {clamped} is within a tiled grid of len {}",
        md.chars().count()
    )
}

pub fn dom_to_offset(md: &str, pos: DomPosition) -> usize {
    let segs = segments(md);
    let len = md.chars().count();
    if segs.is_empty() {
        return 0;
    }
    if pos.segment >= segs.len() {
        return len;
    }
    let seg = segs[pos.segment];
    (seg.start + pos.offset_in_segment.min(seg.len())).min(len)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaretMap {
    pub segments: Vec<Segment>,
    pub char_len: usize,
}

impl CaretMap {
    pub fn new(md: &str) -> Self {
        CaretMap {
            segments: segments(md),
            char_len: md.chars().count(),
        }
    }
    pub fn caret_count(&self) -> usize {
        self.char_len + 1
    }
    pub fn is_valid_offset(&self, offset: usize) -> bool {
        offset <= self.char_len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::canonicalize;
    use crate::inline::{InlineNode, OBJ};
    use myelin_events::ArtifactRef;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn node(i: usize) -> InlineNode {
        InlineNode::ArtifactRefNode(ArtifactRef(format!("myelin://acme/k/{i}")))
    }

    fn assert_tiles(md: &str) {
        let segs = segments(md);
        let mut cursor = 0usize;
        for s in &segs {
            assert_eq!(s.start, cursor, "gap/overlap before segment in {md:?}");
            assert!(s.end > s.start, "zero-width segment in {md:?}");
            cursor = s.end;
        }
        assert_eq!(
            cursor,
            md.chars().count(),
            "grid does not cover the line {md:?}"
        );
    }

    #[test]
    fn grid_tiles_every_corpus_shape() {
        for md in [
            "",
            "hello",
            "**bold**",
            "a `c` b",
            "[t](u)",
            &format!("{OBJ}"),
            &format!("hi {OBJ} there"),
            &format!("{OBJ}{OBJ}{OBJ}"),
            &format!("**{OBJ}** and {OBJ}"),
        ] {
            assert_tiles(md);
        }
    }

    #[test]
    fn structured_node_is_one_caret_position() {
        let md = format!("a{OBJ}b{OBJ}");
        let segs = segments(&md);
        let nodes: Vec<_> = segs
            .iter()
            .filter(|s| s.kind == SegmentKind::Node)
            .collect();
        assert_eq!(nodes.len(), 2);
        for n in &nodes {
            assert_eq!(
                n.len(),
                1,
                "a structured node must be exactly one caret position"
            );
        }
        assert_eq!(nodes[0].node_index, Some(0));
        assert_eq!(nodes[1].node_index, Some(1));
    }

    #[test]
    fn offset_dom_roundtrips_zero_off_by_one() {
        let shapes = [
            String::new(),
            "plain text".to_string(),
            "**bold** *i* `c` ~~s~~".to_string(),
            "[link text](https://x.test/p)".to_string(),
            format!("{OBJ}"),
            format!("before {OBJ} after"),
            format!("{OBJ}{OBJ}{OBJ}"),
            format!("**{OBJ}** mid {OBJ} end {OBJ}"),
            format!("a{OBJ}{OBJ}b{OBJ}c"),
        ];
        for md in &shapes {
            let len = md.chars().count();
            for off in 0..=len {
                let dom = offset_to_dom(md, off);
                let back = dom_to_offset(md, dom);
                assert_eq!(
                    back,
                    off,
                    "off-by-{} at offset {off} in {md:?} (dom {dom:?})",
                    back as i64 - off as i64
                );
            }
        }
    }

    #[test]
    fn caret_steps_over_a_chip_by_exactly_one() {
        let md = format!("x{OBJ}y");
        let segs = segments(&md);
        let node_seg = segs
            .iter()
            .position(|s| s.kind == SegmentKind::Node)
            .unwrap();
        let node_start = segs[node_seg].start;
        let before = offset_to_dom(&md, node_start);
        let after = offset_to_dom(&md, node_start + 1);
        assert_eq!(dom_to_offset(&md, after) - dom_to_offset(&md, before), 1);
        assert_eq!(
            after,
            DomPosition {
                segment: node_seg,
                offset_in_segment: 1
            }
        );
    }

    #[test]
    fn end_of_line_offset_is_exact() {
        let md = format!("end {OBJ}");
        let len = md.chars().count();
        let dom = offset_to_dom(&md, len);
        assert_eq!(dom_to_offset(&md, dom), len);
        let segs = segments(&md);
        assert_eq!(dom.segment, segs.len() - 1);
        assert_eq!(dom.offset_in_segment, segs[segs.len() - 1].len());
    }

    #[test]
    fn multibyte_char_is_one_caret_position() {
        let md = "café 日本";
        assert_eq!(caret_count(md), md.chars().count() + 1);
        let len = md.chars().count();
        for off in 0..=len {
            assert_eq!(
                dom_to_offset(md, offset_to_dom(md, off)),
                off,
                "byte/char confusion in {md:?}"
            );
        }
        assert!(md.len() > len);
    }

    #[test]
    fn segment_len_and_is_empty_are_exact() {
        let md = format!("ab{OBJ}c");
        for s in segments(&md) {
            assert_ne!(s.len(), 0, "well-formed segment is non-empty");
            assert!(!s.is_empty(), "well-formed segment must not report empty");
        }
        let zero = Segment {
            kind: SegmentKind::Text,
            start: 3,
            end: 3,
            node_index: None,
        };
        assert!(zero.is_empty());
        assert_eq!(zero.len(), 0);
        let nonzero = Segment {
            kind: SegmentKind::Text,
            start: 3,
            end: 5,
            node_index: None,
        };
        assert!(!nonzero.is_empty());
        assert_eq!(nonzero.len(), 2);
    }

    #[test]
    fn offset_to_dom_is_relative_to_segment_start() {
        let md = format!("{OBJ}xyz");
        let dom = offset_to_dom(&md, 3);
        assert_eq!(
            dom,
            DomPosition {
                segment: 1,
                offset_in_segment: 2
            }
        );
        assert_eq!(dom_to_offset(&md, dom), 3);
        let md2 = format!("ab{OBJ}cdef");
        let dom2 = offset_to_dom(&md2, 5);
        assert_eq!(
            dom2,
            DomPosition {
                segment: 2,
                offset_in_segment: 2
            }
        );
        assert_eq!(dom_to_offset(&md2, dom2), 5);
    }

    #[test]
    fn caret_map_is_consistent() {
        let md = format!("a {OBJ} b");
        let map = CaretMap::new(&md);
        assert_eq!(map.caret_count(), caret_count(&md));
        assert_eq!(map.segments, segments(&md));
        assert!(map.is_valid_offset(0));
        assert!(map.is_valid_offset(map.char_len));
        assert!(!map.is_valid_offset(map.char_len + 1));
    }

    #[test]
    fn bridge_over_canonical_serialized_line() {
        let nodes = vec![
            InlineNode::Mention(Principal::stub(
                PrincipalId("alice".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            node(7),
        ];
        let md = format!("hi {OBJ} see {OBJ}");
        let (canon, canon_nodes) = canonicalize(&md, &nodes);
        assert_eq!(canon, md);
        assert_eq!(canon_nodes, nodes);
        let len = canon.chars().count();
        for off in 0..=len {
            assert_eq!(dom_to_offset(&canon, offset_to_dom(&canon, off)), off);
        }
        let node_segs: Vec<_> = segments(&canon)
            .into_iter()
            .filter(|s| s.kind == SegmentKind::Node)
            .collect();
        assert_eq!(
            node_segs.iter().map(|s| s.node_index).collect::<Vec<_>>(),
            vec![Some(0), Some(1)]
        );
    }
}
