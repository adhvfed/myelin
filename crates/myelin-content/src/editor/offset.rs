//! The offset model — primitive #2 (architecture
//! `02-internals-and-algorithms.md` §8.2.2; design-system block-editor §2 rule 4).
//!
//! **The caret is a character offset into the SERIALIZED markdown.** The controlled
//! `contenteditable` owns this document model and reconciles the DOM to it — it never
//! reads the document FROM the DOM (the ProseMirror-class lesson: contenteditable ignores
//! model state and Chrome/Firefox diverge on caret behaviour). To reconcile, we need a
//! lossless bridge between the model coordinate (a **char offset**, `0..=len_chars`) and a
//! DOM coordinate (a **`DomPosition`** = which rendered segment + the char offset within
//! it).
//!
//! ## The segment grid
//! A serialized line renders in the DOM as an ordered sequence of [`Segment`]s. A
//! [`SegmentKind::Text`] segment is a run of caret-addressable characters; a
//! [`SegmentKind::Node`] segment is a single structured node ([`crate::inline::OBJ`]) —
//! rendered as a non-editable `<ReferenceChip>` island that occupies **exactly one caret
//! position** (the caret passes AROUND it; `Tab` exits — design-system §7). Every char of
//! the serialized string maps to exactly one caret-addressable position, so the bridge is
//! a total bijection on `0..=char_len` with **0 off-by-one** (the green gate artifact).
//!
//! ## Why segment over the SERIALIZED string (not the inline AST runs)
//! The caret coordinate is the markdown string the user's keystrokes edit. Mark delimiters
//! (`**`, `*`, `` ` ``, `~~`, `[`/`]`/`(`/`)`) ARE characters in that string and ARE caret
//! stops while the user is mid-typing a `**bold**` (rule 4: "show bold while the `**` is
//! still being typed"). Only the three structured nodes collapse to one position — they
//! are the islands. So the segment grid is: maximal runs of NON-`OBJ` chars (`Text`)
//! interleaved with single-`OBJ` `Node` segments.

use crate::inline::OBJ;

/// What a rendered segment is in the controlled `contenteditable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    /// A run of caret-addressable characters (prose + visible mark delimiters). Each char
    /// is its own caret stop.
    Text,
    /// A single structured node (one [`OBJ`] in the serialized string) rendered as a
    /// non-editable chip island. **Exactly one caret position** — the caret sits before or
    /// after the chip, never inside it.
    Node,
}

/// One rendered segment with its half-open char range `[start, end)` into the serialized
/// markdown string (char indices, NOT byte indices — the caret is a CHAR offset). For a
/// [`SegmentKind::Node`] segment `end == start + 1` (one `OBJ`); `node_index` is its index
/// into the positional `inline_nodes` array (the i-th `OBJ` ⇒ `nodes[i]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub kind: SegmentKind,
    /// Char offset of the segment's first char into the serialized string.
    pub start: usize,
    /// Char offset one-past the segment's last char.
    pub end: usize,
    /// For a `Node` segment, its positional index into `inline_nodes`; `None` for `Text`.
    pub node_index: Option<usize>,
}

impl Segment {
    /// The segment's length in caret-addressable characters (`Node` is always 1).
    pub fn len(&self) -> usize {
        self.end - self.start
    }
    /// A zero-width segment never occurs in a well-formed grid (both kinds are ≥ 1 char).
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// A DOM caret coordinate: which rendered [`Segment`] (by index into [`segments`]) and the
/// char offset WITHIN it. `offset_in_segment == 0` is the position before the segment's
/// first char; `offset_in_segment == segment.len()` is the position after its last char.
/// For a `Node` segment the only interior offsets are `0` (before the chip) and `1` (after
/// the chip) — the caret never lands strictly inside an island.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DomPosition {
    pub segment: usize,
    pub offset_in_segment: usize,
}

/// Build the ordered segment grid for a serialized markdown line: maximal runs of
/// non-[`OBJ`] characters become [`SegmentKind::Text`] segments; each `OBJ` becomes its
/// own single-char [`SegmentKind::Node`] segment carrying its positional `node_index`. The
/// concatenated ranges tile `0..char_len` with no gaps or overlaps (the bijection
/// invariant the offset gate asserts).
pub fn segments(md: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut node_index = 0usize;
    let mut run_start: Option<usize> = None;
    let mut idx = 0usize; // CHAR index (the caret coordinate), not byte
    for c in md.chars() {
        if c == OBJ {
            // close any open text run
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

/// The total number of caret positions on a serialized line = `char_len + 1` (a position
/// before each char and one after the last). A structured node contributes exactly ONE
/// char (one `OBJ`), hence exactly one caret-step to pass it — the offset-gate invariant.
pub fn caret_count(md: &str) -> usize {
    md.chars().count() + 1
}

/// Bridge a model char offset (`0..=char_len`) to its DOM [`DomPosition`]. The mapping is
/// total and exact (0 off-by-one): an offset that falls AT a segment boundary binds to the
/// END of the left segment for all but offset 0 / the empty line, so a caret sitting
/// "after the chip" is `DomPosition{ segment: chip, offset_in_segment: 1 }` and not a
/// phantom zero-width position on the next segment. Returns the canonical position.
pub fn offset_to_dom(md: &str, offset: usize) -> DomPosition {
    let segs = segments(md);
    let clamped = offset.min(md.chars().count());
    // Empty line (or offset 0 with a leading segment): the position before everything.
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
    // Find the segment whose half-open range CONTAINS offset-1's char, i.e. the segment we
    // are "inside or at the end of". We bind a boundary offset to the LEFT segment's end so
    // the caret-after-a-chip is offset_in_segment == 1 of the chip, the canonical form.
    // Because `clamped` is already clamped to `char_len` and the grid tiles `0..char_len`
    // with no gaps, the LAST segment has `end == char_len >= clamped`, so the loop ALWAYS
    // returns — there is no fallthrough (no dead defensive branch to drift, EI-01 §7).
    for (i, seg) in segs.iter().enumerate() {
        if clamped <= seg.end {
            return DomPosition {
                segment: i,
                offset_in_segment: clamped - seg.start,
            };
        }
    }
    // Unreachable for a tiled grid + clamped offset (asserted by the offset gate over the
    // corpus). `unreachable!` documents the invariant without a behaviour-equivalent
    // arithmetic branch a mutant could survive on.
    unreachable!(
        "offset {clamped} is within a tiled grid of len {}",
        md.chars().count()
    )
}

/// Bridge a DOM [`DomPosition`] back to a model char offset. Inverse of [`offset_to_dom`]
/// for every canonical position (the round-trip the offset gate asserts across every
/// structured node, 0 off-by-one). A `DomPosition` past a segment's end clamps to its end
/// (browser caret normalisation), and an out-of-range segment clamps to the line end.
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

/// A precomputed caret map for a serialized line: the segment grid + the line's char
/// length. Lets the editor reconcile many caret events without re-walking the string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaretMap {
    pub segments: Vec<Segment>,
    pub char_len: usize,
}

impl CaretMap {
    /// Build the caret map for a serialized line.
    pub fn new(md: &str) -> Self {
        CaretMap {
            segments: segments(md),
            char_len: md.chars().count(),
        }
    }
    /// Number of caret positions (`char_len + 1`).
    pub fn caret_count(&self) -> usize {
        self.char_len + 1
    }
    /// Is `offset` a valid caret position on this line (`0..=char_len`)?
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

    /// The segment grid tiles `0..char_len` with NO gaps or overlaps (the bijection the
    /// bridge depends on).
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

    /// A structured node is EXACTLY ONE caret position (one OBJ ⇒ one Node segment of len 1).
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
        // positional binding: the i-th OBJ ⇒ node_index i
        assert_eq!(nodes[0].node_index, Some(0));
        assert_eq!(nodes[1].node_index, Some(1));
    }

    /// THE offset gate: offset ↔ DOM-position round-trips with 0 off-by-one across EVERY
    /// caret position, including every structured node, on every shape.
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

    /// Crossing a chip is exactly one caret step (the chip is one position): the offset
    /// immediately before a node and immediately after differ by exactly 1, and the
    /// "after" DOM-position is offset_in_segment == 1 of the SAME node segment (canonical),
    /// not a zero-width position on the next segment.
    #[test]
    fn caret_steps_over_a_chip_by_exactly_one() {
        let md = format!("x{OBJ}y");
        let segs = segments(&md);
        let node_seg = segs
            .iter()
            .position(|s| s.kind == SegmentKind::Node)
            .unwrap();
        let node_start = segs[node_seg].start; // char offset of the OBJ
        let before = offset_to_dom(&md, node_start);
        let after = offset_to_dom(&md, node_start + 1);
        assert_eq!(dom_to_offset(&md, after) - dom_to_offset(&md, before), 1);
        // canonical "after the chip" form
        assert_eq!(
            after,
            DomPosition {
                segment: node_seg,
                offset_in_segment: 1
            }
        );
    }

    /// The end-of-line offset (== char_len) maps to the END of the last segment and back.
    #[test]
    fn end_of_line_offset_is_exact() {
        let md = format!("end {OBJ}");
        let len = md.chars().count();
        let dom = offset_to_dom(&md, len);
        assert_eq!(dom_to_offset(&md, dom), len);
        // it is the last segment (the trailing node), offset 1 (after the chip)
        let segs = segments(&md);
        assert_eq!(dom.segment, segs.len() - 1);
        assert_eq!(dom.offset_in_segment, segs[segs.len() - 1].len());
    }

    /// A multi-byte (non-ASCII / IME) char is ONE caret position — the caret is a CHAR
    /// offset, never a byte offset (the CJK/accented-input correctness obligation, G2).
    #[test]
    fn multibyte_char_is_one_caret_position() {
        let md = "café 日本"; // 'é' is 2 bytes, '日'/'本' 3 bytes each — each is ONE caret step
        assert_eq!(caret_count(md), md.chars().count() + 1);
        let len = md.chars().count();
        for off in 0..=len {
            assert_eq!(
                dom_to_offset(md, offset_to_dom(md, off)),
                off,
                "byte/char confusion in {md:?}"
            );
        }
        // sanity: char_len (7) is strictly less than the byte length
        assert!(md.len() > len);
    }

    /// Pin `Segment::len`/`is_empty` exactly (kills the is_empty mutants): every segment in
    /// a well-formed grid is non-empty (len ≥ 1); a `Node` is len 1.
    #[test]
    fn segment_len_and_is_empty_are_exact() {
        let md = format!("ab{OBJ}c");
        for s in segments(&md) {
            assert_ne!(s.len(), 0, "well-formed segment is non-empty");
            assert!(!s.is_empty(), "well-formed segment must not report empty");
        }
        // a synthetic zero-width segment DOES report empty (pins the == in is_empty)
        let zero = Segment {
            kind: SegmentKind::Text,
            start: 3,
            end: 3,
            node_index: None,
        };
        assert!(zero.is_empty());
        assert_eq!(zero.len(), 0);
        // a non-zero segment whose start != 0 reports NOT empty (pins is_empty's == AND
        // that len is end-start, not a constant)
        let nonzero = Segment {
            kind: SegmentKind::Text,
            start: 3,
            end: 5,
            node_index: None,
        };
        assert!(!nonzero.is_empty());
        assert_eq!(nonzero.len(), 2);
    }

    /// Pin the `clamped - seg.start` arithmetic in `offset_to_dom` for a segment that does
    /// NOT start at 0 (so `-`/`+`/`/` mutants diverge): a caret inside the SECOND text
    /// segment (after a leading chip) must yield offset_in_segment relative to that
    /// segment's start, not the absolute offset.
    #[test]
    fn offset_to_dom_is_relative_to_segment_start() {
        // segment 0 = Node (start 0,len 1); segment 1 = Text "xyz" (start 1,len 3)
        let md = format!("{OBJ}xyz");
        // absolute offset 3 = the 'y'..'z' boundary → segment 1, offset_in_segment 2
        let dom = offset_to_dom(&md, 3);
        assert_eq!(
            dom,
            DomPosition {
                segment: 1,
                offset_in_segment: 2
            }
        );
        // if `-` were `+`: 3 + 1 = 4 (out of the len-3 segment); if `/`: 3/1 = 3 (the end,
        // not offset 2). Both are caught by this exact expectation.
        assert_eq!(dom_to_offset(&md, dom), 3);
        // a deeper case: leading "ab" text then a chip then "cdef"; offset inside "cdef"
        let md2 = format!("ab{OBJ}cdef");
        let dom2 = offset_to_dom(&md2, 5); // 'd' boundary in seg "cdef" (start 3)
        assert_eq!(
            dom2,
            DomPosition {
                segment: 2,
                offset_in_segment: 2
            }
        );
        assert_eq!(dom_to_offset(&md2, dom2), 5);
    }

    /// CaretMap agrees with the free functions and validates offsets.
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

    /// The grid + bridge survive a REAL structured-node line built from the canonical
    /// serializer (a mention + a ref), proving the model coordinate is the same serialized
    /// string the offset model walks.
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
        // canonicalize is the identity on this already-canonical line
        let (canon, canon_nodes) = canonicalize(&md, &nodes);
        assert_eq!(canon, md);
        assert_eq!(canon_nodes, nodes);
        let len = canon.chars().count();
        for off in 0..=len {
            assert_eq!(dom_to_offset(&canon, offset_to_dom(&canon, off)), off);
        }
        // two node segments, bound positionally
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
