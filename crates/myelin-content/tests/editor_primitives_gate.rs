//! KN-P08 (P-298, M3) — the editor-primitives STANDALONE gates (CI), the dated green
//! artifacts the prompt's GATE/DRILLS section names:
//!
//!  1. **KN-D2 standalone leg** — `serialize_inline(parse_inline(md)) === md` 100% over the
//!     frozen corpus, 0 regressions, run on the SERIALIZER PRIMITIVE (the offset model +
//!     DOM-surgery build over this same one render path; this re-asserts it as a primitive
//!     leg, before the integrated KN-P09 re-run). `corpus_pass_rate() == (n, n)`.
//!  2. **The offset / DOM-surgery property gate** — a caret round-trips
//!     DOM-position ↔ char-offset across EVERY structured node (0 off-by-one), and
//!     Enter-split places the caret at the START of the new block (the caret-placement
//!     counter is the green artifact).
//!
//! These run NATIVELY against the identical single source the WASM editor compiles
//! (`build-wasm.sh`) — there is no second renderer (contract 13.1 WASM target). The
//! integrated editor + the browser-drive KN-D2 re-run is the IMMEDIATE follow-on KN-P09.

use myelin_content::corpus::{corpus_pass_rate, CORPUS};
use myelin_content::editor::offset::SegmentKind;
use myelin_content::editor::{canonicalize, dom_to_offset, offset_to_dom, segments, split_at};
use myelin_content::inline::{InlineNode, OBJ};
use myelin_events::ArtifactRef;

/// GATE 1 — the standalone KN-D2 leg on the serializer primitive: 100% round-trip, 0
/// regressions over the frozen corpus. The corpus-pass-rate = 100% is the dated green.
#[test]
fn kn_d2_standalone_leg_serializer_primitive_100_percent() {
    let (passed, total) = corpus_pass_rate();
    assert_eq!(
        passed, total,
        "KN-D2 standalone leg: corpus-pass-rate must be 100% ({passed}/{total})"
    );
    assert!(total >= 18, "the frozen corpus must not be shrunk to pass");
}

/// A structured-node array sized to the fixture's U+FFFC count (the positional binding).
fn nodes_for(md: &str) -> Vec<InlineNode> {
    md.chars()
        .filter(|&c| c == OBJ)
        .enumerate()
        .map(|(i, _)| InlineNode::ArtifactRefNode(ArtifactRef(format!("myelin://corpus/n/{i}"))))
        .collect()
}

/// GATE 2a — the OFFSET property gate over the WHOLE frozen corpus: for every fixture and
/// every caret position, `dom_to_offset(offset_to_dom(off)) == off` (0 off-by-one),
/// INCLUDING across every structured node. Asserts the off-by-one counter is exactly 0.
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
        // count the structured nodes this fixture exercises (each is one caret position)
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
    // the corpus DOES exercise structured nodes (the gate is not vacuous)
    assert!(
        structured_nodes_crossed >= 4,
        "the offset gate must cross structured nodes (crossed {structured_nodes_crossed})"
    );
}

/// GATE 2b — the CARET-PLACEMENT counter: Enter-split places the caret at the START of the
/// new block at EVERY split position over the corpus. The counter (caret-at-zero == total
/// splits) is the green artifact.
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
            // each half is CANONICAL (a fixed point through the one render path). Note:
            // splitting INSIDE a delimiter pair (e.g. mid-`**bold**`) deliberately breaks
            // the run and each half re-canonicalises independently — so `left ++ right`
            // need NOT equal `md` mid-delimiter (that is correct editor behaviour, not a
            // loss). The fixed-point + node-routing invariants are the real bars.
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
            // each half carries exactly its own structured nodes (node count == OBJ count) —
            // no node lost or duplicated across the split.
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
            // total structured nodes conserved across the split (none lost).
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
