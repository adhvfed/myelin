//! The frozen KN-D2 round-trip corpus
//! (`05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! KN-D2: `render(parse(md)) === md`, 100% round-trip, 0 corpus regressions).
//!
//! The corpus is a directory of `*.md` fixtures under `crates/myelin-content/corpus/`,
//! each holding ONE markdown-subset inline string. The structured nodes are encoded with
//! the literal `U+FFFC` placeholder ([`crate::inline::OBJ`]); the gate parses each
//! fixture (with a synthetic node array sized to its `U+FFFC` count) and asserts
//! `serialize_inline(parse_inline(md, nodes)) == md`. The corpus-pass-rate signal = 100%
//! is the dated green artifact (CI). The corpus deliberately exercises the three
//! structured nodes anchored in bold / lists / tables, code blocks, and IME/paste edge
//! cases (§KN-P01 DELIVERABLE).
//!
//! The corpus is **embedded at compile time** ([`include_str!`] via [`CORPUS`]) so the
//! gate runs identically native and on `wasm32-unknown-unknown` (no filesystem on the
//! WASM target) — the round-trip runs against the ONE compiled render path, not a second
//! renderer.

use crate::inline::{parse_inline, serialize_inline, InlineNode, OBJ};
use myelin_events::ArtifactRef;

/// One frozen corpus fixture: a name (for diagnostics) + its markdown-subset string.
pub struct Fixture {
    pub name: &'static str,
    pub md: &'static str,
}

macro_rules! corpus {
    ($($name:literal => $file:literal),* $(,)?) => {
        /// The frozen corpus, embedded at compile time so the gate is filesystem-free
        /// (runs on `wasm32-unknown-unknown`).
        pub const CORPUS: &[Fixture] = &[
            $( Fixture { name: $name, md: include_str!(concat!("../corpus/", $file)) } ),*
        ];
    };
}

corpus! {
    "plain"              => "plain.md",
    "bold"               => "bold.md",
    "italic"             => "italic.md",
    "code"               => "code.md",
    "strike"             => "strike.md",
    "link"               => "link.md",
    "nested-marks"       => "nested_marks.md",
    "bold-in-list"       => "bold_in_list.md",
    "code-verbatim"      => "code_verbatim.md",
    "escaped-delimiters" => "escaped_delimiters.md",
    "mention-in-bold"    => "mention_in_bold.md",
    "ref-in-list"        => "ref_in_list.md",
    "embed-in-table"     => "embed_in_table.md",
    "three-nodes"        => "three_nodes.md",
    "ime-paste"          => "ime_paste.md",
    "empty"              => "empty.md",
    "adjacent-marks"     => "adjacent_marks.md",
    "link-with-marks"    => "link_with_marks.md",
}

/// Build a synthetic structured-node array sized to the fixture's `U+FFFC` count. The
/// node payloads are placeholders — the round-trip gate asserts the STRING, and the node
/// array is preserved positionally (the i-th `U+FFFC` ⇒ `nodes[i]`); the payload values
/// do not affect the serialized string. Real node payloads come from the editor; here we
/// only need the count to be correct so the positional binding holds.
pub fn synthetic_nodes_for(md: &str) -> Vec<InlineNode> {
    md.chars()
        .filter(|&c| c == OBJ)
        .map(|_| InlineNode::ArtifactRefNode(ArtifactRef("myelin://corpus/node".into())))
        .collect()
}

/// Run the KN-D2 round-trip over a single fixture. Returns `Ok(())` on a byte-identical
/// round-trip, `Err((got, want))` otherwise.
pub fn roundtrip(md: &str) -> Result<(), (String, String)> {
    let nodes = synthetic_nodes_for(md);
    let got = serialize_inline(&parse_inline(md, &nodes));
    if got == md {
        Ok(())
    } else {
        Err((got, md.to_string()))
    }
}

/// The KN-D2 corpus pass-rate (the telemetry signal that must read 100%). Returns
/// `(passed, total)`.
pub fn corpus_pass_rate() -> (usize, usize) {
    let passed = CORPUS.iter().filter(|f| roundtrip(f.md).is_ok()).count();
    (passed, CORPUS.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// KN-D2 — `render(parse(md)) === md` over the WHOLE frozen corpus: 100% round-trip,
    /// 0 regressions. This is the dated green artifact (the corpus-pass-rate = 100%).
    #[test]
    fn kn_d2_corpus_roundtrips_100_percent() {
        let mut failures = Vec::new();
        for f in CORPUS {
            if let Err((got, want)) = roundtrip(f.md) {
                failures.push(format!("  [{}] got {got:?} want {want:?}", f.name));
            }
        }
        assert!(
            failures.is_empty(),
            "KN-D2 corpus regressions ({}/{} passed):\n{}",
            CORPUS.len() - failures.len(),
            CORPUS.len(),
            failures.join("\n")
        );
        let (passed, total) = corpus_pass_rate();
        assert_eq!(passed, total, "corpus-pass-rate must be 100%");
        assert!(
            total >= 18,
            "corpus must not be shrunk below its frozen size"
        );
    }
}
