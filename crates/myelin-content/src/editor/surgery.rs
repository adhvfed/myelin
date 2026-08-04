use crate::editor::{obj_count, split_nodes};
use crate::inline::{parse_inline, serialize_inline, InlineNode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockSplit {
    pub left: String,
    pub left_nodes: Vec<InlineNode>,
    pub right: String,
    pub right_nodes: Vec<InlineNode>,
    pub caret: usize,
}

pub fn split_at(md: &str, nodes: &[InlineNode], offset: usize) -> BlockSplit {
    let chars: Vec<char> = md.chars().collect();
    let cut = offset.min(chars.len());
    let left_str: String = chars[..cut].iter().collect();
    let right_str: String = chars[cut..].iter().collect();
    let obj_left = obj_count(&left_str);
    let (left_nodes_in, right_nodes_in) = split_nodes(nodes, obj_left);
    let left = serialize_inline(&parse_inline(&left_str, &left_nodes_in));
    let right = serialize_inline(&parse_inline(&right_str, &right_nodes_in));
    BlockSplit {
        left,
        left_nodes: left_nodes_in,
        right,
        right_nodes: right_nodes_in,
        caret: 0,
    }
}

pub fn normalize_paste(raw: &str, nodes: &[InlineNode]) -> (String, Vec<InlineNode>) {
    let inline = parse_inline(raw, nodes);
    (serialize_inline(&inline), inline.nodes)
}

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

    #[test]
    fn split_preserves_content_and_is_canonical() {
        let md = "the quick brown fox";
        for off in 0..=md.chars().count() {
            let s = split_at(md, &[], off);
            assert_eq!(format!("{}{}", s.left, s.right), md);
            assert_eq!(normalize_paste(&s.left, &s.left_nodes).0, s.left);
            assert_eq!(normalize_paste(&s.right, &s.right_nodes).0, s.right);
        }
    }

    #[test]
    fn split_routes_structured_nodes_to_correct_half() {
        let nodes = vec![mention("alice"), aref(1), aref(2)];
        let md = format!("hi {OBJ} mid {OBJ} end {OBJ}");
        let first_obj = md.chars().position(|c| c == OBJ).unwrap();
        let s = split_at(&md, &nodes, first_obj + 1);
        assert_eq!(s.left_nodes, vec![mention("alice")]);
        assert_eq!(s.right_nodes, vec![aref(1), aref(2)]);
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

    #[test]
    fn split_at_node_boundary_keeps_node_right() {
        let nodes = vec![aref(0)];
        let md = format!("ab{OBJ}cd");
        let obj_pos = md.chars().position(|c| c == OBJ).unwrap();
        let s = split_at(&md, &nodes, obj_pos);
        assert_eq!(s.left_nodes, vec![]);
        assert_eq!(s.right_nodes, vec![aref(0)]);
        assert_eq!(s.left, "ab");
        assert_eq!(s.right, format!("{OBJ}cd"));
    }

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

    #[test]
    fn split_caret_is_a_valid_offset_position() {
        let nodes = vec![aref(0)];
        let md = format!("x {OBJ} y");
        let s = split_at(&md, &nodes, 3);
        assert!(s.caret < caret_count(&s.right));
        let dom = offset_to_dom(&s.right, s.caret);
        assert_eq!(dom_to_offset(&s.right, dom), s.caret);
    }

    #[test]
    fn paste_normalises_through_the_one_render_path() {
        let (c1, _) = normalize_paste("**bold** and `code`", &[]);
        assert_eq!(c1, "**bold** and `code`");
        let (c2, _) = normalize_paste("[not a link", &[]);
        assert_eq!(c2, r"\[not a link");
        assert_eq!(normalize_paste(&c2, &[]).0, c2);
    }

    #[test]
    fn ime_commit_inserts_text_at_caret() {
        let md = "ab cd";
        let (out, _nodes, caret) = insert_text(md, &[], 3, "日本");
        assert_eq!(out, "ab 日本cd");
        assert_eq!(caret, 5);
        assert!(caret < caret_count(&out));
        assert_eq!(dom_to_offset(&out, offset_to_dom(&out, caret)), caret);
    }

    #[test]
    fn ime_commit_escapes_reserved_char() {
        let md = "ax";
        let (out, _n, caret) = insert_text(md, &[], 1, "*");
        assert_eq!(out, r"a\*x");
        assert_eq!(caret, 3);
        assert_eq!(normalize_paste(&out, &[]).0, out);
    }
}
