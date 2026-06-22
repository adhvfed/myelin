//! The DOM-surgery primitive — primitive #3 (architecture
//! `02-internals-and-algorithms.md` §8.2.3; design-system block-editor §2 rule 5 + §4
//! block ops; EI-05 §2). This is the module the named top Knowledge risk lives in
//! (browser variance: Enter / IME / paste).
//!
//! Two operations are their own designed, unit-tested module because **"Enter just inserts
//! a newline" is the #1 'this isn't a real editor' tell** (EI-05 §2):
//!
//! 1. **Enter-splits-a-block** ([`split_at`]) — splitting a block at a caret CHAR offset
//!    (the offset-model coordinate, [`super::offset`]) produces TWO blocks: the left
//!    keeps `md[..offset]`, the right takes `md[offset..]`, and the positional structured
//!    `inline_nodes` array is split at the U+FFFC boundary so each block carries exactly
//!    its own nodes. Both halves are re-serialized through the ONE frozen render path so
//!    the result is always canonical (a split inside a `**bold**` run leaves a dangling
//!    `**` only as the literal characters the user typed — the editor reconciles, it does
//!    not invent structure).
//! 2. **Caret-placement-after-split** — the caret lands at **offset 0 of the new (right)
//!    block** (`BlockSplit::caret`). This is the load-bearing correctness bar the design
//!    calls out: pressing Enter puts you at the start of the new block, every time.
//!
//! Plus the paste/IME normalisation seam ([`normalize_paste`]): pasted / IME-committed
//! text is **re-parsed + re-serialized through the SAME serializer** before insertion — it
//! is never injected raw into the document model (EI-05 §2: *normalise on serialise*;
//! design-system §7: paste-from-Word is normalised through the shared WASM sanitiser).

use crate::editor::{obj_count, split_nodes};
use crate::inline::{parse_inline, serialize_inline, InlineNode};

/// The result of an Enter-split: the two resulting blocks (each a canonical serialized
/// markdown string + its own positional structured-node array) and the **caret position**
/// the editor must place after the split — `caret == 0` of the RIGHT block (the start of
/// the new block).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockSplit {
    /// The left block: the canonical serialized `md[..offset]` + the nodes before the split.
    pub left: String,
    pub left_nodes: Vec<InlineNode>,
    /// The right (new) block: the canonical serialized `md[offset..]` + the nodes after.
    pub right: String,
    pub right_nodes: Vec<InlineNode>,
    /// The caret position after the split: **char offset 0 of the right block** (the
    /// caret-placement-after-split bar — the caret is always at the START of the new
    /// block). Carried explicitly so a consumer cannot mis-place it.
    pub caret: usize,
}

/// Split a serialized markdown line at the caret CHAR offset `offset` (the offset-model
/// coordinate). Produces the left + right blocks and the caret-at-start-of-new-block. The
/// `offset` is clamped to `0..=char_len`; a split AT a U+FFFC boundary keeps the node in
/// the right block (the chip moves down with the new line). Both halves are re-serialized
/// through the frozen render path so they are canonical.
///
/// **Caret invariant:** `caret == 0` (start of the right block) for every split — this is
/// the green artifact the caret-placement gate counts.
pub fn split_at(md: &str, nodes: &[InlineNode], offset: usize) -> BlockSplit {
    let chars: Vec<char> = md.chars().collect();
    let cut = offset.min(chars.len());
    let left_str: String = chars[..cut].iter().collect();
    let right_str: String = chars[cut..].iter().collect();
    // Split the positional node array at the count of U+FFFC chars in the left half.
    let obj_left = obj_count(&left_str);
    let (left_nodes_in, right_nodes_in) = split_nodes(nodes, obj_left);
    // Re-serialize each half through the ONE render path so both blocks are canonical
    // (the editor reconciles the DOM to the model; it never trusts a raw fragment).
    let left = serialize_inline(&parse_inline(&left_str, &left_nodes_in));
    let right = serialize_inline(&parse_inline(&right_str, &right_nodes_in));
    BlockSplit {
        left,
        left_nodes: left_nodes_in,
        right,
        right_nodes: right_nodes_in,
        caret: 0, // start of the NEW (right) block — the #1 "real editor" bar
    }
}

/// Normalise pasted / IME-committed inline content through the SAME frozen serializer
/// before it enters the document model (EI-05 §2: *normalise on serialise*; never inject
/// raw). Returns the canonical serialized string + its (preserved) positional node array.
/// This is the identity on already-canonical input (the KN-D2 fixed point) and the seam
/// paste-from-Word / a CJK IME composition-commit funnels through — there is no second
/// sanitiser.
pub fn normalize_paste(raw: &str, nodes: &[InlineNode]) -> (String, Vec<InlineNode>) {
    let inline = parse_inline(raw, nodes);
    (serialize_inline(&inline), inline.nodes)
}

/// Insert `text` (a plain IME composition commit — no structured nodes) at char `offset`
/// into a serialized line, returning the canonicalised line + the caret offset AFTER the
/// inserted text. The inserted text is escaped-on-serialize through the render path so a
/// literally-typed `*` stays literal until the user completes a delimiter pair. Used for
/// the IME composition-end event (text lands at the caret).
pub fn insert_text(
    md: &str,
    nodes: &[InlineNode],
    offset: usize,
    text: &str,
) -> (String, Vec<InlineNode>, usize) {
    let chars: Vec<char> = md.chars().collect();
    let at = offset.min(chars.len());
    let mut joined: String = chars[..at].iter().collect();
    joined.push_str(text);
    joined.extend(chars[at..].iter());
    let (canon, canon_nodes) = normalize_paste(&joined, nodes);
    // caret after the inserted text, measured in the canonical string by re-deriving the
    // left half's canonical length (escapes can lengthen it).
    let left_raw: String = {
        let mut s: String = chars[..at].iter().collect();
        s.push_str(text);
        s
    };
    let obj_left = obj_count(&left_raw);
    let (left_nodes, _) = split_nodes(nodes, obj_left);
    let left_canon = serialize_inline(&parse_inline(&left_raw, &left_nodes));
    let caret = left_canon.chars().count();
    (canon, canon_nodes, caret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::offset::{caret_count, dom_to_offset, offset_to_dom};
    use crate::inline::{InlineNode, OBJ};
    use myelin_events::ArtifactRef;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn aref(i: usize) -> InlineNode {
        InlineNode::ArtifactRefNode(ArtifactRef(format!("myelin://acme/k/{i}")))
    }
    fn mention(name: &str) -> InlineNode {
        InlineNode::Mention(Principal::stub(
            PrincipalId(name.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        ))
    }

    /// THE caret-placement gate: Enter-split ALWAYS places the caret at offset 0 of the new
    /// (right) block. This is the green artifact (the caret-placement counter).
    #[test]
    fn enter_split_caret_is_always_start_of_new_block() {
        let md = "hello world";
        let mut caret_at_zero = 0usize;
        let total = md.chars().count() + 1;
        for off in 0..=md.chars().count() {
            let s = split_at(md, &[], off);
            assert_eq!(
                s.caret, 0,
                "caret must be at the START of the new block at split {off}"
            );
            caret_at_zero += 1;
            // plain text (no delimiters) reconstructs exactly: left ++ right == md
            assert_eq!(
                format!("{}{}", s.left, s.right),
                md,
                "split {off} lost/gained chars"
            );
        }
        assert_eq!(
            caret_at_zero, total,
            "every split placed the caret at start"
        );
    }

    /// A split mid-line preserves all chars across the two halves and is canonical on both.
    #[test]
    fn split_preserves_content_and_is_canonical() {
        let md = "the quick brown fox";
        for off in 0..=md.chars().count() {
            let s = split_at(md, &[], off);
            assert_eq!(format!("{}{}", s.left, s.right), md);
            // each half is a fixed point (canonical)
            assert_eq!(normalize_paste(&s.left, &s.left_nodes).0, s.left);
            assert_eq!(normalize_paste(&s.right, &s.right_nodes).0, s.right);
        }
    }

    /// Splitting a line with structured nodes routes each node to the correct half by its
    /// U+FFFC position, and the caret-after offset 0 of the right block lands BEFORE the
    /// nodes that moved down.
    #[test]
    fn split_routes_structured_nodes_to_correct_half() {
        let nodes = vec![mention("alice"), aref(1), aref(2)];
        let md = format!("hi {OBJ} mid {OBJ} end {OBJ}"); // 3 OBJ
                                                          // split right after the first OBJ
        let first_obj = md.chars().position(|c| c == OBJ).unwrap();
        let s = split_at(&md, &nodes, first_obj + 1);
        assert_eq!(s.left_nodes, vec![mention("alice")]);
        assert_eq!(s.right_nodes, vec![aref(1), aref(2)]);
        // node counts match the OBJ counts in each half
        assert_eq!(
            s.left.chars().filter(|&c| c == OBJ).count(),
            s.left_nodes.len()
        );
        assert_eq!(
            s.right.chars().filter(|&c| c == OBJ).count(),
            s.right_nodes.len()
        );
        assert_eq!(s.caret, 0);
    }

    /// Split AT a U+FFFC boundary keeps the node in the RIGHT block (the chip moves down).
    #[test]
    fn split_at_node_boundary_keeps_node_right() {
        let nodes = vec![aref(0)];
        let md = format!("ab{OBJ}cd");
        let obj_pos = md.chars().position(|c| c == OBJ).unwrap();
        let s = split_at(&md, &nodes, obj_pos); // cut exactly at the OBJ
        assert_eq!(s.left_nodes, vec![]);
        assert_eq!(s.right_nodes, vec![aref(0)]);
        assert_eq!(s.left, "ab");
        assert_eq!(s.right, format!("{OBJ}cd"));
    }

    /// Split at the extremes: offset 0 → empty left, whole line right; offset len → whole
    /// line left, empty right. Both place the caret at start of the (new) right block.
    #[test]
    fn split_at_extremes() {
        let md = "abc";
        let s0 = split_at(md, &[], 0);
        assert_eq!(
            (s0.left.as_str(), s0.right.as_str(), s0.caret),
            ("", "abc", 0)
        );
        let sn = split_at(md, &[], md.chars().count());
        assert_eq!(
            (sn.left.as_str(), sn.right.as_str(), sn.caret),
            ("abc", "", 0)
        );
    }

    /// After a split, the caret (offset 0 of the right block) is a VALID offset-model
    /// position and bridges to/from the DOM with 0 off-by-one — the two primitives compose.
    #[test]
    fn split_caret_is_a_valid_offset_position() {
        let nodes = vec![aref(0)];
        let md = format!("x {OBJ} y");
        let s = split_at(&md, &nodes, 3);
        // s.caret (0) is in range on the right block and round-trips through the DOM bridge
        assert!(s.caret < caret_count(&s.right));
        let dom = offset_to_dom(&s.right, s.caret);
        assert_eq!(dom_to_offset(&s.right, dom), s.caret);
    }

    /// Paste normalisation is the identity on canonical input and re-serializes raw paste
    /// through the ONE render path (no second sanitiser). A non-canonical escape collapses
    /// to the canonical form.
    #[test]
    fn paste_normalises_through_the_one_render_path() {
        // already canonical → identity (fixed point)
        let (c1, _) = normalize_paste("**bold** and `code`", &[]);
        assert_eq!(c1, "**bold** and `code`");
        // raw paste of a bare reserved char canonicalises (a lone `[` becomes `\[`)
        let (c2, _) = normalize_paste("[not a link", &[]);
        assert_eq!(c2, r"\[not a link");
        // and the canonical form is a fixed point
        assert_eq!(normalize_paste(&c2, &[]).0, c2);
    }

    /// An IME composition-commit inserts plain text at the caret and returns the caret
    /// AFTER it (the CJK / accented-input path). Char offsets, never byte offsets.
    #[test]
    fn ime_commit_inserts_text_at_caret() {
        let md = "ab cd";
        // commit "日本" between "ab " and "cd" (offset 3)
        let (out, _nodes, caret) = insert_text(md, &[], 3, "日本");
        assert_eq!(out, "ab 日本cd");
        assert_eq!(caret, 5); // 'a','b',' ','日','本' → 5 chars before the caret
                              // the caret is a valid offset-model position on the new line
        assert!(caret < caret_count(&out));
        assert_eq!(dom_to_offset(&out, offset_to_dom(&out, caret)), caret);
    }

    /// Inserting a reserved char escapes-on-serialize (a typed literal `*` is `\*`), and the
    /// caret lands after the escaped form (length-aware, no off-by-one).
    #[test]
    fn ime_commit_escapes_reserved_char() {
        let md = "ax";
        let (out, _n, caret) = insert_text(md, &[], 1, "*");
        assert_eq!(out, r"a\*x");
        // 'a','\\','*' = 3 chars before the caret in the canonical string
        assert_eq!(caret, 3);
        assert_eq!(normalize_paste(&out, &[]).0, out);
    }
}
