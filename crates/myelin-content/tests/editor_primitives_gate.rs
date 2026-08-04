use myelin_content::corpus::{corpus_pass_rate, CORPUS};
use myelin_content::editor::offset::SegmentKind;
use myelin_content::editor::{canonicalize, dom_to_offset, offset_to_dom, segments, split_at};
use myelin_content::inline::{InlineNode, OBJ};
use myelin_events::ArtifactRef;

#[test]
fn kn_d2_standalone_leg_serializer_primitive_100_percent() {
    let (passed, total) = corpus_pass_rate();
    assert_eq!(
        passed, total,
        "KN-D2 standalone leg: corpus-pass-rate must be 100% ({passed}/{total})"
    );
    assert!(total >= 18, "the frozen corpus must not be shrunk to pass");
}

fn nodes_for(md: &str) -> Vec<InlineNode> {
    md.chars()
        .filter(|&c| c == OBJ)
        .enumerate()
        .map(|(i, _)| InlineNode::ArtifactRefNode(ArtifactRef(format!("myelin://corpus/n/{i}"))))
        .collect()
}

#[test]
fn offset_dom_bridge_zero_off_by_one_over_corpus() {
    let mut off_by_one = 0usize;
    let mut structured_nodes_crossed = 0usize;
    for f in CORPUS {
        let md = f.md;
        let len = md.chars().count();
        for off in 0..=len {
            let back = dom_to_offset(md, offset_to_dom(md, off));
            if back != off {
                off_by_one += 1;
            }
        }
        for s in segments(md) {
            if s.kind == SegmentKind::Node {
                structured_nodes_crossed += 1;
                assert_eq!(
                    s.len(),
                    1,
                    "[{}] a structured node must be exactly one caret position",
                    f.name
                );
            }
        }
    }
    assert_eq!(
        off_by_one, 0,
        "offset/DOM bridge off-by-one count must be 0 (was {off_by_one})"
    );
    assert!(
        structured_nodes_crossed >= 4,
        "the offset gate must cross structured nodes (crossed {structured_nodes_crossed})"
    );
}

#[test]
fn enter_split_caret_placement_counter_is_green() {
    let mut caret_at_start = 0usize;
    let mut total_splits = 0usize;
    for f in CORPUS {
        let md = f.md;
        let nodes = nodes_for(md);
        for off in 0..=md.chars().count() {
            let s = split_at(md, &nodes, off);
            total_splits += 1;
            if s.caret == 0 {
                caret_at_start += 1;
            }
            assert_eq!(
                canonicalize(&s.left, &s.left_nodes).0,
                s.left,
                "[{}] split {off} left not canonical",
                f.name
            );
            assert_eq!(
                canonicalize(&s.right, &s.right_nodes).0,
                s.right,
                "[{}] split {off} right not canonical",
                f.name
            );
            assert_eq!(
                s.left.chars().filter(|&c| c == OBJ).count(),
                s.left_nodes.len(),
                "[{}] split {off} left node mismatch",
                f.name
            );
            assert_eq!(
                s.right.chars().filter(|&c| c == OBJ).count(),
                s.right_nodes.len(),
                "[{}] split {off} right node mismatch",
                f.name
            );
            assert_eq!(
                s.left_nodes.len() + s.right_nodes.len(),
                nodes.len(),
                "[{}] split {off} dropped a structured node",
                f.name
            );
        }
    }
    assert_eq!(caret_at_start, total_splits, "caret-placement: every split must land at the start of the new block ({caret_at_start}/{total_splits})");
    assert!(total_splits > 0, "the caret-placement gate must run splits");
}
